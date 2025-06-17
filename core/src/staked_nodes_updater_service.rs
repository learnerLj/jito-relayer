//! Service for maintaining up-to-date validator stake information.
//! 
//! This service continuously fetches validator stake data from RPC servers and updates
//! the shared stake map used for resource allocation decisions. Stake information is
//! critical for:
//! - Determining QUIC connection limits per validator
//! - Prioritizing transaction processing by validator stake
//! - Vote packet processing order in consensus
//! 
//! The service combines RPC-fetched stake data with manual overrides to provide
//! a complete and accurate view of validator stake for network operations.

use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::{self, sleep, Builder, JoinHandle},
    time::{Duration, Instant},
};

use jito_rpc::load_balancer::LoadBalancer;
use log::warn;
use solana_client::client_error;
use solana_sdk::pubkey::Pubkey;
use solana_streamer::streamer::StakedNodes;

/// How frequently to refresh validator stake information from RPC servers.
/// 5 seconds provides a good balance between keeping data current and not
/// overwhelming RPC servers with requests. Stake changes are relatively infrequent.
const PK_TO_STAKE_REFRESH_DURATION: Duration = Duration::from_secs(5);

/// Background service that maintains current validator stake information.
/// 
/// This service runs in its own thread and:
/// 1. Periodically fetches vote account data from RPC servers
/// 2. Extracts validator identity and activated stake amounts
/// 3. Combines RPC data with manual overrides
/// 4. Updates the shared StakedNodes structure used by QUIC servers
/// 
/// The stake data is used throughout the system for resource allocation:
/// - Higher stake validators get more QUIC connection slots
/// - Transaction forwarding prioritizes high-stake validators
/// - Vote processing order follows stake-weighted priorities
pub struct StakedNodesUpdaterService {
    /// Handle to the background thread updating stake information
    thread_hdl: JoinHandle<()>,
}

impl StakedNodesUpdaterService {
    /// Creates and starts a new stake updater service.
    /// 
    /// # Arguments
    /// * `exit` - Shared shutdown signal for graceful termination
    /// * `rpc_load_balancer` - Load balancer for RPC requests to fetch stake data
    /// * `shared_staked_nodes` - Shared stake map updated by this service
    /// * `staked_nodes_overrides` - Manual stake overrides for testing or special cases
    /// 
    /// # Returns
    /// A new service instance with background updating thread started
    pub fn new(
        exit: Arc<AtomicBool>,
        rpc_load_balancer: Arc<LoadBalancer>,
        shared_staked_nodes: Arc<RwLock<StakedNodes>>,
        staked_nodes_overrides: HashMap<Pubkey, u64>,
    ) -> Self {
        // Start background thread for continuous stake data updates
        let thread_hdl = Builder::new()
            .name("staked_nodes_updater_thread".to_string())
            .spawn(move || {
                let mut last_stakes = Instant::now();
                
                // Main update loop - continues until shutdown signal
                while !exit.load(Ordering::Relaxed) {
                    let mut stake_map = Arc::new(HashMap::new());
                    
                    // Attempt to refresh stake data from RPC
                    match Self::try_refresh_pk_to_stake(
                        &mut last_stakes,
                        &mut stake_map,
                        &rpc_load_balancer,
                    ) {
                        // Successfully refreshed - update shared stake map
                        Ok(true) => {
                            // Combine RPC data with manual overrides
                            let shared =
                                StakedNodes::new(stake_map, staked_nodes_overrides.clone());
                            *shared_staked_nodes.write().unwrap() = shared;
                        }
                        
                        // RPC error - log warning and retry after delay
                        Err(err) => {
                            warn!("Failed to refresh pk to stake map! Error: {:?}", err);
                            sleep(PK_TO_STAKE_REFRESH_DURATION);
                        }
                        
                        // Not time to refresh yet - continue loop
                        _ => {}
                    }
                }
            })
            .unwrap();

        Self { thread_hdl }
    }

    /// Attempts to refresh validator stake data from RPC if enough time has passed.
    /// 
    /// This function fetches current vote account information which includes:
    /// - Validator identity public keys
    /// - Activated stake amounts
    /// - Current vs delinquent validator status
    /// 
    /// Both current and delinquent validators are included since delinquent validators
    /// may still have active stake and could become current again.
    /// 
    /// # Arguments
    /// * `last_stakes` - Timestamp of last successful refresh
    /// * `pubkey_stake_map` - Output map to populate with validator -> stake mappings
    /// * `rpc_load_balancer` - RPC client for fetching vote account data
    /// 
    /// # Returns
    /// * `Ok(true)` if data was refreshed successfully
    /// * `Ok(false)` if not enough time has passed since last refresh
    /// * `Err(...)` if RPC request failed
    fn try_refresh_pk_to_stake(
        last_stakes: &mut Instant,
        pubkey_stake_map: &mut Arc<HashMap<Pubkey, u64>>,
        rpc_load_balancer: &Arc<LoadBalancer>,
    ) -> client_error::Result<bool> {
        // Check if enough time has passed since last refresh
        if last_stakes.elapsed() > PK_TO_STAKE_REFRESH_DURATION {
            // Get RPC client with highest slot (most current data)
            let client = rpc_load_balancer.rpc_client();
            
            // Fetch all vote accounts (both current and delinquent)
            let vote_accounts = client.get_vote_accounts()?;

            // Build validator identity -> stake mapping
            *pubkey_stake_map = Arc::new(
                vote_accounts
                    .current
                    .iter()
                    .chain(vote_accounts.delinquent.iter()) // Include delinquent validators
                    .filter_map(|vote_account| {
                        // Extract validator identity from vote account
                        // Some vote accounts may have invalid pubkey strings
                        Some((
                            Pubkey::from_str(&vote_account.node_pubkey).ok()?,
                            vote_account.activated_stake,
                        ))
                    })
                    .collect(),
            );

            *last_stakes = Instant::now();
            Ok(true)
        } else {
            // Not time to refresh yet - wait briefly before next check
            sleep(Duration::from_secs(1));
            Ok(false)
        }
    }

    /// Gracefully shuts down the stake updater service.
    /// 
    /// # Returns
    /// `Ok(())` if the thread shut down successfully, or the thread's panic result
    pub fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}
