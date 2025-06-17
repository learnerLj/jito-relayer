//! Health monitoring system for the Jito relayer.
//! 
//! This module provides centralized health tracking that monitors the relayer's
//! connection to the Solana network and overall operational status. The health
//! system impacts multiple components:
//! 
//! ## Health States
//! - **Healthy**: Relayer is receiving slot updates and functioning normally
//! - **Unhealthy**: Relayer has lost connection or is experiencing issues
//! 
//! ## Health-Dependent Behaviors
//! - **Authentication**: New validator authentications are rejected when unhealthy
//! - **Connections**: Existing validator connections are dropped when unhealthy
//! - **Metrics**: Health state is reported to monitoring systems
//! 
//! ## Health Determination
//! Health is based on recency of slot updates from the Solana network:
//! - Recent slot updates → Healthy (connected to network)
//! - Missing slot updates → Unhealthy (network disconnection or RPC issues)

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread,
    thread::{Builder, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{select, tick, Receiver, Sender};
use solana_metrics::datapoint_info;
use solana_sdk::clock::Slot;

/// Health status of the relayer system.
/// 
/// The numeric values (0/1) are used for metrics reporting where:
/// - 0 = Unhealthy (system issues, reject new connections)
/// - 1 = Healthy (system operational, accept connections)
#[derive(PartialEq, Eq, Copy, Clone)]
pub enum HealthState {
    /// Relayer is experiencing issues and should reject new connections
    Unhealthy = 0,
    /// Relayer is operating normally and can accept new connections
    Healthy = 1,
}

/// Manages and monitors the overall health status of the relayer.
/// 
/// The health manager runs in a background thread and continuously monitors
/// slot updates to determine if the relayer is connected to the Solana network.
/// Other components can query the health state to make operational decisions.
pub struct HealthManager {
    /// Shared health state accessible by other components
    state: Arc<RwLock<HealthState>>,
    /// Background thread handle for health monitoring
    manager_thread: JoinHandle<()>,
}

/// Implementation of health monitoring and management.
/// 
/// The health manager tracks slot updates as a proxy for network connectivity
/// and overall system health. It provides a shared health state that other
/// components can use to make decisions about accepting connections and requests.
impl HealthManager {
    /// Creates a new health manager that monitors slot updates for network connectivity.
    /// 
    /// The health manager starts in an Unhealthy state and transitions to Healthy
    /// once slot updates are being received regularly. It also forwards slot updates
    /// to other components that need them.
    /// 
    /// # Arguments
    /// * `slot_receiver` - Channel receiving slot updates from the network monitor
    /// * `slot_sender` - Channel for forwarding slots to other components
    /// * `missing_slot_unhealthy_threshold` - How long without slots before marking unhealthy
    /// * `exit` - Shutdown signal for graceful termination
    /// 
    /// # Returns
    /// A new health manager with background monitoring thread started
    pub fn new(
        slot_receiver: Receiver<Slot>,
        slot_sender: Sender<Slot>,
        missing_slot_unhealthy_threshold: Duration,
        exit: Arc<AtomicBool>,
    ) -> HealthManager {
        // Start in unhealthy state until we receive slot updates
        let health_state = Arc::new(RwLock::new(HealthState::Unhealthy));
        
        HealthManager {
            state: health_state.clone(),
            manager_thread: Builder::new()
                .name("health_manager".to_string())
                .spawn(move || {
                    let mut last_update = Instant::now();
                    let mut slot_sender_max_len = 0usize;
                    
                    // Set up periodic tasks
                    let channel_len_tick = tick(Duration::from_secs(5));  // Channel metrics every 5s
                    let check_and_metrics_tick = tick(missing_slot_unhealthy_threshold / 2);  // Health checks twice per threshold

                    // Main monitoring loop
                    while !exit.load(Ordering::Relaxed) {
                        select! {
                            // Periodic health check and metrics reporting
                            recv(check_and_metrics_tick) -> _ => {
                                // Determine health based on recency of slot updates
                                let new_health_state =
                                    match last_update.elapsed() <= missing_slot_unhealthy_threshold {
                                        true => HealthState::Healthy,   // Recent slot update = healthy
                                        false => HealthState::Unhealthy, // No recent slots = unhealthy
                                    };
                                    
                                // Update shared health state
                                *health_state.write().unwrap() = new_health_state;
                                
                                // Report health status to metrics system
                                datapoint_info!(
                                    "relayer-health-state",
                                    ("health_state", new_health_state, i64)
                                );
                            }
                            
                            // Handle incoming slot updates
                            recv(slot_receiver) -> maybe_slot => {
                                let slot = maybe_slot.expect("error receiving slot, exiting");
                                // Forward slot to other components that need it
                                slot_sender.send(slot).expect("error forwarding slot, exiting");
                                // Update timestamp to indicate we're receiving network data
                                last_update = Instant::now();
                            }
                            
                            // Periodic channel metrics reporting
                            recv(channel_len_tick) -> _ => {
                                datapoint_info!(
                                    "health_manager-channel_stats",
                                    ("slot_sender_len", slot_sender_max_len, i64),
                                    ("slot_sender_capacity", slot_sender.capacity().unwrap(), i64),
                                );
                                slot_sender_max_len = 0; // Reset for next measurement period
                            }
                        }
                        
                        // Track peak channel utilization for performance monitoring
                        slot_sender_max_len = std::cmp::max(slot_sender_max_len, slot_sender.len());
                    }
                })
                .unwrap(),
        }
    }

    /// Returns a handle to the shared health state.
    /// 
    /// Other components can use this handle to check the current health status
    /// and make decisions about accepting connections or processing requests.
    /// 
    /// # Returns
    /// Shared reference to the current health state
    pub fn handle(&self) -> Arc<RwLock<HealthState>> {
        self.state.clone()
    }

    /// Gracefully shuts down the health manager and waits for thread completion.
    /// 
    /// # Returns
    /// `Ok(())` if the thread shut down successfully, or the thread's panic result
    pub fn join(self) -> thread::Result<()> {
        self.manager_thread.join()
    }
}
