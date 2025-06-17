//! JWT-based authentication service for validator access control.
//! 
//! This service implements a secure challenge-response authentication system:
//! 
//! ## Authentication Flow
//! 1. **Challenge Generation**: Validator requests challenge for their public key
//! 2. **Identity Verification**: Validator signs challenge with their private key  
//! 3. **Token Issuance**: Server verifies signature and issues JWT access/refresh tokens
//! 4. **Token Refresh**: Long-lived refresh tokens can generate new access tokens
//! 
//! ## Security Features
//! - **DOS Protection**: One challenge per IP address prevents flooding attacks
//! - **Signature Verification**: Cryptographic proof of validator identity
//! - **Token Binding**: Tokens tied to specific IP addresses and validator pubkeys
//! - **Automatic Expiration**: Challenges and tokens expire to limit exposure
//! - **Authorization Control**: Only whitelisted validators can authenticate
//! 
//! ## Token Types
//! - **Access Tokens**: Short-lived (typically minutes), used for API authentication
//! - **Refresh Tokens**: Long-lived (typically hours/days), used to renew access tokens

use std::{
    cmp::Reverse,
    net::IpAddr,
    ops::Add,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration as StdDuration,
};

use chrono::{Duration, Utc};
use ed25519_dalek::{PublicKey, Signature, Verifier};
use jito_protos::auth::{
    auth_service_server::AuthService, GenerateAuthChallengeRequest, GenerateAuthChallengeResponse,
    GenerateAuthTokensRequest, GenerateAuthTokensResponse, RefreshAccessTokenRequest,
    RefreshAccessTokenResponse, Role, Token as PbToken,
};
use jwt::{AlgorithmType, Header, PKeyWithDigest, SignWithKey, Token, VerifyWithKey};
use log::*;
use openssl::pkey::{Private, Public};
use prost_types::Timestamp;
use rand::{distributions::Alphanumeric, Rng};
use solana_sdk::pubkey::Pubkey;
use tokio::{task::JoinHandle, time::interval};
use tonic::{Request, Response, Status};

use crate::{
    auth_challenges::{AuthChallenge, AuthChallenges},
    auth_interceptor::{Claims, DeSerClaims},
    health_manager::HealthState,
};

/// Trait for validator authorization control.
/// 
/// Implementations determine which validator public keys are allowed to authenticate.
/// This enables flexible authorization policies like whitelists, stake requirements,
/// or integration with external authorization systems.
pub trait ValidatorAuther: Send + Sync + 'static {
    /// Checks if a validator public key is authorized to use this relayer.
    /// 
    /// # Arguments
    /// * `pubkey` - The validator's public key to check
    /// 
    /// # Returns
    /// `true` if the validator is authorized, `false` otherwise
    fn is_authorized(&self, pubkey: &Pubkey) -> bool;
}

/// Implementation of the gRPC authentication service.
/// 
/// This service handles the complete authentication lifecycle from challenge generation
/// through token issuance and refresh. It maintains security through cryptographic
/// signature verification and prevents DOS attacks through rate limiting.
/// 
/// # Generic Parameters
/// * `V` - Validator authorization implementation (e.g., whitelist, stake-based)
pub struct AuthServiceImpl<V: ValidatorAuther> {
    /// Authorization policy for determining which validators can authenticate
    validator_auther: V,

    /// Background task handle for periodic challenge cleanup
    _t_hdl: JoinHandle<()>,

    /// Active authentication challenges indexed by IP address.
    /// 
    /// Uses a priority queue to efficiently expire old challenges and prevent DOS attacks:
    /// - One challenge per IP address limits attack surface
    /// - Priority ordering by expiration time enables efficient cleanup
    /// - Reverse ordering ensures oldest challenges are removed first
    auth_challenges: AuthChallenges,

    /// RSA private key for signing JWT tokens.
    /// Used to create cryptographically secure access and refresh tokens.
    signing_key: PKeyWithDigest<Private>,
    
    /// RSA public key for token verification.
    /// Shared with all services that need to validate JWT tokens.
    /// Must correspond to the signing_key for proper token validation.
    verifying_key: Arc<PKeyWithDigest<Public>>,

    /// Time-to-live for access tokens (typically short, e.g., 15 minutes).
    /// Short TTL limits exposure if tokens are compromised.
    access_token_ttl: Duration,
    
    /// Time-to-live for refresh tokens (typically longer, e.g., 24 hours).
    /// Longer TTL reduces re-authentication frequency while maintaining security.
    refresh_token_ttl: Duration,

    /// Time-to-live for authentication challenges (typically very short, e.g., 5 minutes).
    /// Short TTL prevents challenge accumulation and DOS attacks.
    challenge_ttl: Duration,

    /// Shared health state - authentication is disabled when relayer is unhealthy
    health_state: Arc<RwLock<HealthState>>,
}

/// Maximum number of concurrent authentication challenges allowed.
/// 
/// This limit prevents DOS attacks through challenge flooding. With one challenge
/// per IP address, an attacker would need 100,000 unique IP addresses to exhaust
/// this capacity, making such attacks prohibitively expensive.
const AUTH_CHALLENGES_CAPACITY: usize = 100_000;

impl<V: ValidatorAuther> AuthServiceImpl<V> {
    /// Creates a new authentication service with the specified configuration.
    /// 
    /// # Arguments
    /// * `validator_auther` - Authorization policy for validator access control
    /// * `signing_key` - RSA private key for signing JWT tokens
    /// * `verifying_key` - RSA public key for token verification (shared with other services)
    /// * `access_token_ttl` - Lifetime for access tokens (short-lived)
    /// * `refresh_token_ttl` - Lifetime for refresh tokens (longer-lived)
    /// * `challenge_ttl` - Lifetime for authentication challenges (very short)
    /// * `challenge_expiration_sleep_interval` - How often to clean up expired challenges
    /// * `exit` - Shutdown signal for graceful termination
    /// * `health_state` - Shared health status (auth disabled when unhealthy)
    /// 
    /// # Returns
    /// A new authentication service ready to handle gRPC requests
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        validator_auther: V,
        signing_key: PKeyWithDigest<Private>,
        verifying_key: Arc<PKeyWithDigest<Public>>,
        access_token_ttl: StdDuration,
        refresh_token_ttl: StdDuration,
        challenge_ttl: StdDuration,
        challenge_expiration_sleep_interval: StdDuration,
        exit: &Arc<AtomicBool>,
        health_state: Arc<RwLock<HealthState>>,
    ) -> Self {
        // Initialize empty challenge storage
        let auth_challenges = AuthChallenges::default();
        
        // Start background task to periodically clean up expired challenges
        let _t_hdl = Self::start_challenge_expiration_task(
            auth_challenges.clone(),
            challenge_expiration_sleep_interval,
            exit,
        );

        Self {
            auth_challenges,
            validator_auther,
            signing_key,
            verifying_key,
            _t_hdl,
            // Convert standard durations to chrono durations for timestamp arithmetic
            access_token_ttl: Duration::from_std(access_token_ttl).unwrap(),
            refresh_token_ttl: Duration::from_std(refresh_token_ttl).unwrap(),
            challenge_ttl: Duration::from_std(challenge_ttl).unwrap(),
            health_state,
        }
    }

    /// Starts a background task to periodically clean up expired authentication challenges.
    /// 
    /// This prevents memory leaks and DOS attacks by removing challenges that have passed
    /// their expiration time. The task runs until the service is shut down.
    /// 
    /// # Arguments
    /// * `auth_challenges` - Shared challenge storage to clean up
    /// * `sleep_interval` - How frequently to run cleanup (e.g., every 30 seconds)
    /// * `exit` - Shutdown signal to stop the background task
    /// 
    /// # Returns
    /// Task handle for the background cleanup task
    fn start_challenge_expiration_task(
        auth_challenges: AuthChallenges,
        sleep_interval: StdDuration,
        exit: &Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let exit = exit.clone();
        tokio::task::spawn(async move {
            let mut interval = interval(sleep_interval);
            
            // Continue cleanup until shutdown signal
            while !exit.load(Ordering::Relaxed) {
                let _ = interval.tick().await; // Wait for next cleanup interval
                auth_challenges.remove_all_expired().await; // Clean up expired challenges
            }
        })
    }

    /// Extracts the client's IP address from a gRPC request.
    /// 
    /// The IP address is used for DOS protection (one challenge per IP) and token binding.
    /// 
    /// # Security Note
    /// If this service is behind a proxy, the remote_addr will be the proxy's IP,
    /// not the actual client IP. This could weaken DOS protection since all requests
    /// would appear to come from the proxy IP. Consider using X-Forwarded-For headers
    /// in proxy deployments.
    /// 
    /// # Arguments
    /// * `req` - The gRPC request containing connection information
    /// 
    /// # Returns
    /// The client's IP address, or internal error if unavailable
    fn client_ip<T>(req: &Request<T>) -> Result<IpAddr, Status> {
        Ok(req
            .remote_addr()
            .ok_or_else(|| Status::internal("request is missing IP address"))?
            .ip())
    }

    /// Generates a cryptographically random challenge string.
    /// 
    /// The challenge is a 9-character alphanumeric string that validators must sign
    /// to prove control of their private key. The randomness prevents replay attacks
    /// and ensures each authentication session is unique.
    /// 
    /// # Returns
    /// A random 9-character alphanumeric challenge string
    fn generate_challenge_token() -> String {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)  // Use cryptographically secure random generator
            .take(9)                     // 9 characters provides sufficient uniqueness
            .map(char::from)             // Convert bytes to characters
            .collect()                   // Assemble into string
    }

    /// Checks relayer health and prevents authentication when unhealthy.
    /// 
    /// When the relayer is unhealthy (e.g., disconnected from Solana network),
    /// new authentications are rejected to prevent validators from connecting
    /// to a non-functional service. Existing connections are also dropped.
    /// 
    /// # Arguments
    /// * `health_state` - Shared health status of the relayer
    /// 
    /// # Returns
    /// `Ok(())` if healthy, or gRPC internal error if unhealthy
    fn check_health(health_state: &Arc<RwLock<HealthState>>) -> Result<(), Status> {
        if *health_state.read().unwrap() != HealthState::Healthy {
            Err(Status::internal("relayer is unhealthy"))
        } else {
            Ok(())
        }
    }
}

#[tonic::async_trait]
impl<V: ValidatorAuther> AuthService for AuthServiceImpl<V> {
    async fn generate_auth_challenge(
        &self,
        req: Request<GenerateAuthChallengeRequest>,
    ) -> Result<Response<GenerateAuthChallengeResponse>, Status> {
        Self::check_health(&self.health_state)?;
        let auth_challenges = &self.auth_challenges;

        if auth_challenges.len().await >= AUTH_CHALLENGES_CAPACITY {
            return Err(Status::resource_exhausted("System overloaded."));
        }

        let client_ip = Self::client_ip(&req)?;
        if let Some(auth_challenge) = auth_challenges.get_priority(&client_ip).await {
            if !auth_challenge.0.is_expired() {
                return Ok(Response::new(GenerateAuthChallengeResponse {
                    challenge: auth_challenge.0.challenge,
                }));
            }
        }

        let inner_req = req.into_inner();

        if inner_req.role != Role::Validator as i32 {
            return Err(Status::invalid_argument("Role must be validator."));
        }

        if inner_req.pubkey.len() != solana_sdk::pubkey::PUBKEY_BYTES {
            return Err(Status::invalid_argument(
                "Pubkey must be 32 bytes in length",
            ));
        }

        let pubkey = Pubkey::try_from(inner_req.pubkey)
            .map_err(|_| Status::invalid_argument("Invalid pubkey supplied."))?;

        if !self.validator_auther.is_authorized(&pubkey) {
            return Err(Status::permission_denied(
                "The supplied pubkey is not authorized to generate a challenge.",
            ));
        }

        let challenge = Self::generate_challenge_token();
        auth_challenges
            .push(
                client_ip,
                Reverse(AuthChallenge {
                    challenge: challenge.clone(),
                    access_claims: Claims {
                        client_ip,
                        client_pubkey: pubkey,
                        expires_at_utc: Utc::now().add(self.access_token_ttl).naive_utc(),
                    },
                    refresh_claims: Claims {
                        client_ip,
                        client_pubkey: pubkey,
                        expires_at_utc: Utc::now().add(self.refresh_token_ttl).naive_utc(),
                    },
                    expires_at_utc: Utc::now().add(self.challenge_ttl).naive_utc(),
                }),
            )
            .await;

        Ok(Response::new(GenerateAuthChallengeResponse { challenge }))
    }

    async fn generate_auth_tokens(
        &self,
        req: Request<GenerateAuthTokensRequest>,
    ) -> Result<Response<GenerateAuthTokensResponse>, Status> {
        Self::check_health(&self.health_state)?;
        let auth_challenges = &self.auth_challenges;

        let client_ip = Self::client_ip(&req)?;
        let inner_req = req.into_inner();

        let client_pubkey = PublicKey::from_bytes(&inner_req.client_pubkey).map_err(|e| {
            warn!("Failed to create pubkey from string: {}", e);
            Status::invalid_argument("Invalid pubkey supplied.")
        })?;
        let solana_pubkey = Pubkey::try_from(client_pubkey.to_bytes())
            .map_err(|_| Status::invalid_argument("Invalid pubkey supplied."))?;

        let auth_challenge = if let Some(challenge) = auth_challenges.get_priority(&client_ip).await
        {
            Ok(challenge)
        } else {
            Err(Status::permission_denied(
                "Must invoke the GenerateAuthChallenge method before calling any method.",
            ))
        }?;

        // check the client passed in public key against the originally requested public key
        if auth_challenge.0.access_claims.client_pubkey != solana_pubkey {
            return Err(Status::permission_denied(
                "The pubkey provided does not match the pubkey that generated the challenge.",
            ));
        }

        // Prepended with the pubkey to invalidate any tx this server could maliciously send.
        let expected_challenge = format!("{}-{}", solana_pubkey, auth_challenge.0.challenge);
        if expected_challenge != inner_req.challenge {
            return Err(Status::invalid_argument(format!(
                "The provided challenge does not match the expected challenge: {expected_challenge}"
            )));
        }

        if inner_req.signed_challenge.len() != solana_sdk::signature::SIGNATURE_BYTES {
            return Err(Status::invalid_argument("Signature must be 64 bytes."));
        }
        let signed_challenge = {
            let sig_bytes =
                &<[u8; 64]>::try_from(&inner_req.signed_challenge[..]).map_err(|e| {
                    error!("Invalid signature 1: {}", e);
                    Status::invalid_argument("Invalid signature.")
                })?;

            Signature::from_bytes(sig_bytes).map_err(|e| {
                error!("Invalid signature 2: {}", e);
                Status::invalid_argument("Invalid signature.")
            })?
        };

        client_pubkey
            .verify(inner_req.challenge.as_bytes(), &signed_challenge)
            .map_err(|e| {
                warn!("Challenge verification failed: {}", e);
                Status::invalid_argument("Failed challenge verification. Did you sign with the supplied pubkey's corresponding private key?")
            })?;

        let access_token = {
            let header = Header {
                algorithm: AlgorithmType::Rs256,
                ..Default::default()
            };
            let claims: DeSerClaims = auth_challenge.0.access_claims.into();
            Token::new(header, claims)
                .sign_with_key(&self.signing_key)
                .map_err(|e| {
                    error!("Error signing access_token claims: {}", e);
                    Status::internal("Error signing access_token.")
                })
        }?
        .as_str()
        .to_string();

        let refresh_token = {
            let header = Header {
                algorithm: AlgorithmType::Rs256,
                ..Default::default()
            };
            let claims: DeSerClaims = auth_challenge.0.refresh_claims.into();
            Token::new(header, claims)
                .sign_with_key(&self.signing_key)
                .map_err(|e| {
                    error!("Error signing refresh_token claims: {}", e);
                    Status::internal("Error signing refresh_token.")
                })
        }?
        .as_str()
        .to_string();

        let access_expiry = auth_challenge.0.access_claims.expires_at_utc;
        let refresh_expiry = auth_challenge.0.refresh_claims.expires_at_utc;

        auth_challenges.remove(&client_ip).await;

        Ok(Response::new(GenerateAuthTokensResponse {
            access_token: Some(PbToken {
                value: access_token,
                expires_at_utc: Some(Timestamp {
                    seconds: access_expiry.and_utc().timestamp(),
                    nanos: 0,
                }),
            }),
            refresh_token: Some(PbToken {
                value: refresh_token,
                expires_at_utc: Some(Timestamp {
                    seconds: refresh_expiry.and_utc().timestamp(),
                    nanos: 0,
                }),
            }),
        }))
    }

    async fn refresh_access_token(
        &self,
        req: Request<RefreshAccessTokenRequest>,
    ) -> Result<Response<RefreshAccessTokenResponse>, Status> {
        Self::check_health(&self.health_state)?;

        let inner_req = req.into_inner();

        let refresh_token: &str = inner_req.refresh_token.as_str();
        let refresh_claims: DeSerClaims = refresh_token
            .verify_with_key(self.verifying_key.as_ref())
            .map_err(|e| {
                warn!("refresh_token failed to verify: {}", e);
                Status::permission_denied("Invalid refresh_token supplied")
            })?;
        let refresh_claims: Claims = (&refresh_claims).into();

        if refresh_claims.is_expired() {
            return Err(Status::permission_denied("Client refresh_token has expired, please generate a new auth challenge to obtain a set of new access tokens."));
        }

        let expires_at_utc = Utc::now().add(self.access_token_ttl).naive_utc();
        let access_claims: DeSerClaims = Claims {
            client_ip: refresh_claims.client_ip,
            client_pubkey: refresh_claims.client_pubkey,
            expires_at_utc,
        }
        .into();
        let access_token = {
            let header = Header {
                algorithm: AlgorithmType::Rs256,
                ..Default::default()
            };
            Token::new(header, access_claims)
                .sign_with_key(&self.signing_key)
                .map_err(|e| {
                    error!("Error signing access_token claims: {}", e);
                    Status::internal("Error signing access_token.")
                })
        }?
        .as_str()
        .to_string();

        Ok(Response::new(RefreshAccessTokenResponse {
            access_token: Some(PbToken {
                value: access_token,
                expires_at_utc: Some(Timestamp {
                    seconds: expires_at_utc.and_utc().timestamp(),
                    nanos: 0,
                }),
            }),
        }))
    }
}
