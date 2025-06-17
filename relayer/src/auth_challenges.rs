//! Authentication challenge management for the Jito relayer.
//! 
//! This module implements a secure challenge-response authentication system that:
//! - Prevents DOS attacks by limiting one challenge per IP address
//! - Uses cryptographic signatures to verify validator identity  
//! - Automatically expires old challenges to prevent memory leaks
//! - Maintains challenge priority ordering for efficient cleanup
//! 
//! The authentication flow:
//! 1. Validator requests challenge for their public key
//! 2. Server generates random challenge string tied to validator's IP/pubkey
//! 3. Validator signs the challenge with their private key
//! 4. Server verifies signature and issues JWT tokens
//! 5. Challenge is removed after successful authentication

use std::{cmp::Reverse, net::IpAddr, sync::Arc};

use chrono::{NaiveDateTime, Utc};
use keyed_priority_queue::KeyedPriorityQueue;
use tokio::sync::Mutex;

use crate::auth_interceptor::Claims;

/// A single authentication challenge issued to a validator.
/// 
/// Contains all information needed to verify a validator's identity and generate
/// JWT tokens upon successful signature verification. Each challenge is tied to
/// a specific IP address and validator public key.
#[derive(Clone)]
pub(crate) struct AuthChallenge {
    /// Random challenge string that the validator must sign with their private key.
    /// This proves they control the private key corresponding to their claimed public key.
    pub(crate) challenge: String,

    /// Claims that will be embedded in the short-lived access token.
    /// Access tokens are used for API authentication and expire quickly for security.
    pub(crate) access_claims: Claims,

    /// Claims that will be embedded in the longer-lived refresh token.
    /// Refresh tokens can generate new access tokens without re-authentication.
    pub(crate) refresh_claims: Claims,

    /// UTC timestamp when this challenge expires and should be cleaned up.
    /// Prevents indefinite memory growth from unused challenges.
    pub(crate) expires_at_utc: NaiveDateTime,
}

impl AuthChallenge {
    /// Checks if this challenge has expired and should be removed.
    /// 
    /// Expired challenges cannot be used for authentication and should be
    /// cleaned up to prevent memory leaks and DOS attacks.
    /// 
    /// # Returns
    /// `true` if the challenge has passed its expiration time
    pub(crate) fn is_expired(&self) -> bool {
        self.expires_at_utc.le(&Utc::now().naive_utc())
    }
}

// Comparison traits for AuthChallenge are based on expiration time.
// This enables efficient priority queue ordering where older (earlier expiring)
// challenges are prioritized for cleanup.

impl Eq for AuthChallenge {}

impl PartialEq<Self> for AuthChallenge {
    /// Two challenges are equal if they have the same expiration time.
    /// This is used for priority queue ordering, not logical equality.
    fn eq(&self, other: &Self) -> bool {
        self.expires_at_utc.eq(&other.expires_at_utc)
    }
}

impl PartialOrd<Self> for AuthChallenge {
    /// Challenges are ordered by expiration time for efficient cleanup.
    /// Earlier expiring challenges are considered "less than" later ones.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.expires_at_utc.cmp(&other.expires_at_utc))
    }
}

impl Ord for AuthChallenge {
    /// Total ordering based on expiration time enables priority queue usage.
    /// This allows the priority queue to efficiently find and remove expired challenges.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.expires_at_utc.cmp(&other.expires_at_utc)
    }
}

/// Thread-safe container for managing authentication challenges across multiple IP addresses.
/// 
/// Uses a priority queue keyed by IP address to:
/// - Limit one challenge per IP (DOS protection)
/// - Efficiently expire old challenges in chronological order
/// - Support concurrent access from multiple gRPC handlers
/// 
/// The `Reverse` wrapper ensures older challenges (with earlier expiration times)
/// are prioritized for cleanup, since the priority queue is normally max-heap.
#[derive(Clone, Default)]
pub(crate) struct AuthChallenges(Arc<Mutex<KeyedPriorityQueue<IpAddr, Reverse<AuthChallenge>>>>);

impl AuthChallenges {
    /// Removes all expired challenges from the queue.
    /// 
    /// This is called periodically by a background task to prevent memory leaks
    /// and maintain system performance. The priority queue ordering ensures that
    /// expired challenges are always at the front of the queue.
    /// 
    /// # Performance
    /// O(k) where k is the number of expired challenges. Since challenges are
    /// ordered by expiration time, we can stop as soon as we find a non-expired challenge.
    pub(crate) async fn remove_all_expired(&self) {
        let mut inner = self.0.lock().await;
        // Remove expired challenges from the front of the queue
        while let Some((_ip_addr, auth_challenge)) = inner.peek() {
            if auth_challenge.0.is_expired() {
                inner.pop(); // Remove expired challenge
            } else {
                break; // All remaining challenges are still valid
            }
        }
    }

    /// Adds or updates a challenge for the given IP address.
    /// 
    /// If the IP already has a challenge, it will be replaced with the new one.
    /// This enforces the "one challenge per IP" DOS protection policy.
    /// 
    /// # Arguments
    /// * `ip` - The IP address of the validator requesting authentication
    /// * `challenge` - The challenge wrapped in Reverse for priority queue ordering
    pub(crate) async fn push(&self, ip: IpAddr, challenge: Reverse<AuthChallenge>) {
        let mut inner = self.0.lock().await;
        inner.push(ip, challenge); // Replaces existing challenge for this IP
    }

    /// Returns the current number of active challenges in the system.
    /// 
    /// Used for capacity checking to prevent DOS attacks through challenge flooding.
    pub(crate) async fn len(&self) -> usize {
        let inner = self.0.lock().await;
        inner.len()
    }

    /// Retrieves the challenge for a specific IP address.
    /// 
    /// Returns None if the IP has no active challenge or if the challenge has expired.
    /// This is used during the authentication flow to validate challenge responses.
    /// 
    /// # Arguments
    /// * `ip` - The IP address to lookup
    /// 
    /// # Returns
    /// The challenge for this IP, or None if no challenge exists
    pub(crate) async fn get_priority(&self, ip: &IpAddr) -> Option<Reverse<AuthChallenge>> {
        let inner = self.0.lock().await;
        inner.get_priority(ip).cloned()
    }

    /// Removes the challenge for a specific IP address.
    /// 
    /// This is called after successful authentication to clean up the used challenge.
    /// 
    /// # Parameters
    /// 
    /// * `ip` - The IP address whose challenge should be removed
    pub(crate) async fn remove(&self, ip: &IpAddr) {
        let mut inner = self.0.lock().await;
        let _ = inner.remove(ip);
    }
}
