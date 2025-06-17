use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    thread::{sleep, Builder, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dashmap::DashMap;
use log::{error, info};
use solana_client::{pubsub_client::PubsubClient, rpc_client::RpcClient};
use solana_metrics::{datapoint_error, datapoint_info};
use solana_sdk::{
    clock::Slot,
    commitment_config::{CommitmentConfig, CommitmentLevel},
};

/// LoadBalancer provides intelligent RPC load balancing for Solana blockchain interactions.
/// Unlike traditional load balancers, this implements slot-based selection - it always routes
/// requests to the RPC server with the most current blockchain state (highest slot number).
///
/// Key features:
/// - Real-time slot tracking via WebSocket subscriptions to all configured RPC servers
/// - Automatic failover when servers become unresponsive or stale
/// - Global slot update stream for system-wide health monitoring and coordination
/// - Connection pre-warming and persistent RPC client management
pub struct LoadBalancer {
    /// Maps WebSocket URLs to their current slot numbers.
    /// Used to determine which server has the most up-to-date blockchain state.
    /// Key: WebSocket URL (e.g., "ws://127.0.0.1:8900")
    /// Value: Current slot number reported by that server
    server_to_slot: Arc<DashMap<String, Slot>>,
    
    /// Maps WebSocket URLs to their corresponding pre-warmed RPC clients.
    /// Stores clients by WebSocket URL (not HTTP URL) to enable lookup by the
    /// furthest-ahead WebSocket subscription when routing RPC requests.
    /// Key: WebSocket URL (e.g., "ws://127.0.0.1:8900") 
    /// Value: Pre-configured RPC client for the corresponding HTTP endpoint
    server_to_rpc_client: DashMap<String, Arc<RpcClient>>,
    
    /// Background threads that maintain WebSocket subscriptions for real-time slot updates.
    /// Each thread manages one WebSocket connection and continuously updates server_to_slot.
    /// These threads automatically reconnect on failures and handle connection recovery.
    subscription_threads: Vec<JoinHandle<()>>,
}

impl LoadBalancer {
    /// Maximum time to wait for slot updates before forcing WebSocket reconnection.
    /// If a WebSocket connection stops receiving slot updates for this duration,
    /// the subscription thread will disconnect and attempt to reconnect.
    const DISCONNECT_WEBSOCKET_TIMEOUT: Duration = Duration::from_secs(30);
    
    /// Timeout for individual RPC requests to prevent hanging operations.
    /// Applied to all RPC client operations including connection warming.
    const RPC_TIMEOUT: Duration = Duration::from_secs(120);
    
    /// Maximum number of slot updates that can be queued for downstream processing.
    /// This prevents memory buildup if slot consumers can't keep up with slot updates.
    pub const SLOT_QUEUE_CAPACITY: usize = 100;
    
    /// Creates a new LoadBalancer with WebSocket slot monitoring and RPC client management.
    /// 
    /// # Arguments
    /// * `servers` - Pairs of (HTTP RPC URL, WebSocket URL) for each server to monitor
    /// * `exit` - Shared flag to signal shutdown to all background threads
    /// 
    /// # Returns
    /// * `LoadBalancer` - The configured load balancer instance
    /// * `Receiver<Slot>` - Channel receiver for global slot updates (highest slots only)
    /// 
    /// The slot receiver provides a stream of blockchain slot updates that represents
    /// the highest slot seen across all monitored servers. This is used by downstream
    /// components for health monitoring and transaction timing coordination.
    pub fn new(
        servers: &[(String, String)], /* http rpc url, ws url */
        exit: &Arc<AtomicBool>,
    ) -> (LoadBalancer, Receiver<Slot>) {
        // Initialize slot tracking map with all WebSocket URLs starting at slot 0
        let server_to_slot = Arc::new(DashMap::from_iter(
            servers.iter().map(|(_, ws)| (ws.clone(), 0)),
        ));

        // Pre-warm RPC connections for all servers and store them keyed by WebSocket URL
        let server_to_rpc_client = DashMap::from_iter(servers.iter().map(|(rpc_url, ws)| {
            // Create RPC client with optimized settings for relayer operations:
            // - Processed commitment for fastest response times
            // - Extended timeout to handle network congestion
            let rpc_client = Arc::new(RpcClient::new_with_timeout_and_commitment(
                rpc_url,
                Self::RPC_TIMEOUT,
                CommitmentConfig {
                    commitment: CommitmentLevel::Processed,
                },
            ));
            
            // Warm up the connection by making an initial RPC call
            // This establishes the TCP connection and validates server accessibility
            if let Err(e) = rpc_client.get_slot() {
                error!("error warming up rpc: {rpc_url}. error: {e}");
            }
            
            // Store using WebSocket URL as key (not HTTP URL) to enable lookup
            // by the furthest-ahead WebSocket subscription when routing requests
            (ws.clone(), rpc_client)
        }));

        // Create channel for global slot updates - only highest slots are sent downstream
        // Sender tracked as health_manager-channel_stats.slot_sender_len in metrics
        let (slot_sender, slot_receiver) = crossbeam_channel::bounded(Self::SLOT_QUEUE_CAPACITY);
        
        // Start background WebSocket subscription threads for real-time slot monitoring
        let subscription_threads =
            Self::start_subscription_threads(servers, server_to_slot.clone(), slot_sender, exit);
            
        (
            LoadBalancer {
                server_to_slot,
                server_to_rpc_client,
                subscription_threads,
            },
            slot_receiver,
        )
    }

    /// Starts background threads that maintain WebSocket subscriptions for real-time slot updates.
    /// Each server gets its own dedicated thread to ensure independent monitoring and recovery.
    /// 
    /// # Arguments
    /// * `servers` - List of (HTTP RPC URL, WebSocket URL) pairs to monitor
    /// * `server_to_slot` - Shared map to update with latest slot numbers from each server
    /// * `slot_sender` - Channel to send global highest slot updates downstream
    /// * `exit` - Shared shutdown signal for graceful thread termination
    /// 
    /// # Returns
    /// Vector of thread handles for joining during shutdown
    fn start_subscription_threads(
        servers: &[(String, String)],
        server_to_slot: Arc<DashMap<String, Slot>>,
        slot_sender: Sender<Slot>,
        exit: &Arc<AtomicBool>,
    ) -> Vec<JoinHandle<()>> {
        // Track the highest slot seen across all servers to avoid sending duplicate updates
        let highest_slot = Arc::new(AtomicU64::default());

        servers
            .iter()
            .map(|(_, websocket_url)| {
                // Extract hostname/port from WebSocket URL for thread naming and logging
                let ws_url_no_token = websocket_url
                    .split('/')
                    .nth(2)
                    .unwrap_or_default()
                    .to_string();
                    
                // Clone shared resources for thread ownership
                let exit = exit.clone();
                let websocket_url = websocket_url.clone();
                let server_to_slot = server_to_slot.clone();
                let slot_sender = slot_sender.clone();
                let highest_slot = highest_slot.clone();

                // Create named thread for easier debugging and monitoring
                Builder::new()
                    .name(format!("load_balancer_subscription_thread-{ws_url_no_token}"))
                    .spawn(move || {
                        // Main reconnection loop - continues until shutdown signal
                        while !exit.load(Ordering::Relaxed) {
                            info!("running slot_subscribe() with url: {websocket_url}");
                            let mut last_slot_update = Instant::now();

                            // Attempt to establish WebSocket subscription for slot updates
                            match PubsubClient::slot_subscribe(&websocket_url) {
                                Ok((_subscription, receiver)) => {
                                    // Subscription established - enter message processing loop
                                    while !exit.load(Ordering::Relaxed) {
                                        // Non-blocking receive with short timeout to allow shutdown checks
                                        match receiver.recv_timeout(Duration::from_millis(100))
                                        {
                                            Ok(slot) => {
                                                // Successfully received slot update
                                                last_slot_update = Instant::now();

                                                // Update this server's current slot in the tracking map
                                                server_to_slot
                                                    .insert(websocket_url.clone(), slot.slot);
                                                    
                                                // Emit metrics for monitoring slot update frequency per server
                                                datapoint_info!(
                                                        "rpc_load_balancer-slot_count",
                                                        "url" => ws_url_no_token,
                                                        ("slot", slot.slot, i64)
                                                );

                                                // Global slot coordination: only send downstream if this is a new highest slot
                                                {
                                                    let old_slot = highest_slot.fetch_max(slot.slot, Ordering::Relaxed);
                                                    if slot.slot > old_slot {
                                                        // This is the new highest slot across all servers - notify downstream
                                                        if let Err(e) = slot_sender.send(slot.slot)
                                                        {
                                                            error!("error sending slot: {e}");
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                            Err(RecvTimeoutError::Timeout) => {
                                                // No slot update received within timeout - check for stale connection
                                                // RPC servers occasionally stop sending slot updates and never recover.
                                                // If enough time has passed, attempt to recover by forcing a new connection
                                                if last_slot_update.elapsed() >= Self::DISCONNECT_WEBSOCKET_TIMEOUT
                                                {
                                                    datapoint_error!(
                                                        "rpc_load_balancer-force_disconnect",
                                                        "url" => ws_url_no_token,
                                                        ("event", 1, i64)
                                                    );
                                                    break; // Exit message loop to reconnect
                                                }
                                            }
                                            Err(RecvTimeoutError::Disconnected) => {
                                                // WebSocket connection lost - attempt to reconnect
                                                info!("slot subscribe disconnected. url: {ws_url_no_token}");
                                                break; // Exit message loop to reconnect
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Failed to establish WebSocket subscription
                                    error!(
                                        "slot subscription error client: {ws_url_no_token}, error: {e:?}"
                                    );
                                }
                            }

                            // Brief pause before attempting reconnection to avoid tight retry loops
                            sleep(Duration::from_secs(1));
                        }
                    })
                    .unwrap()
            })
            .collect()
    }

    /// Returns the RPC client for the server with the highest (most current) slot.
    /// This ensures all RPC requests are routed to the server with the most up-to-date
    /// blockchain state, which is critical for MEV operations and accurate data retrieval.
    /// 
    /// # Returns
    /// Arc-wrapped RPC client for the server with the highest slot number
    pub fn rpc_client(&self) -> Arc<RpcClient> {
        let (highest_server, _) = self.get_highest_slot();

        self.server_to_rpc_client
            .get(&highest_server)
            .unwrap()
            .value()
            .to_owned()
    }

    /// Finds the server with the highest slot number among all monitored servers.
    /// This represents the server with the most current view of the blockchain state.
    /// 
    /// # Returns
    /// Tuple of (WebSocket URL, highest slot number) for the most up-to-date server
    pub fn get_highest_slot(&self) -> (String, Slot) {
        let multi = self
            .server_to_slot
            .iter()
            .max_by(|lhs, rhs| lhs.value().cmp(rhs.value()))
            .unwrap();
        let (server, slot) = multi.pair();
        (server.to_string(), *slot)
    }

    /// Gracefully shuts down all WebSocket subscription threads.
    /// Should be called during application shutdown to ensure clean thread termination.
    /// 
    /// # Returns
    /// Result indicating whether all threads joined successfully
    pub fn join(self) -> thread::Result<()> {
        for s in self.subscription_threads {
            s.join()?;
        }
        Ok(())
    }
}
