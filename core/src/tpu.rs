//! The `tpu` module implements the Transaction Processing Unit (TPU), which is the
//! core transaction ingestion and processing pipeline for the Jito relayer.
//! 
//! The TPU consists of multiple stages that work together to:
//! 1. Accept incoming QUIC connections from validators and clients
//! 2. Receive transaction packets over those connections
//! 3. Verify transaction signatures for authenticity
//! 4. Forward validated transactions to the banking stage
//! 
//! This implementation is optimized for high throughput with:
//! - QUIC-based network transport for better performance than UDP
//! - Multi-stage pipeline for parallel processing
//! - Stake-based connection limits and prioritization
//! - Separate handling of regular transactions vs forwarded transactions

use std::{
    collections::HashMap,
    net::UdpSocket,
    sync::{atomic::AtomicBool, Arc, RwLock},
    thread,
    thread::JoinHandle,
    time::Duration,
};

use crossbeam_channel::Receiver;
use jito_rpc::load_balancer::LoadBalancer;
use solana_core::{
    banking_trace::{BankingPacketBatch, BankingTracer},
    sigverify::TransactionSigVerifier,
    sigverify_stage::SigVerifyStage,
    tpu::MAX_QUIC_CONNECTIONS_PER_PEER,
};
use solana_sdk::{pubkey::Pubkey, signature::Keypair};
use solana_streamer::{
    nonblocking::quic::{DEFAULT_MAX_STREAMS_PER_MS, DEFAULT_WAIT_FOR_CHUNK_TIMEOUT},
    quic::spawn_server,
    streamer::StakedNodes,
};

use crate::{fetch_stage::FetchStage, staked_nodes_updater_service::StakedNodesUpdaterService};

/// Default packet coalescing timeout in milliseconds.
/// Packets are batched together for this duration before processing to improve efficiency.
/// Shorter values reduce latency but may decrease throughput.
pub const DEFAULT_TPU_COALESCE_MS: u64 = 5;

/// Maximum QUIC connections allowed per IP address.
/// Allows multiple connections to handle NAT scenarios and connection overlap
/// during reconnection events. Prevents resource exhaustion from single IPs.
pub const MAX_QUIC_CONNECTIONS_PER_IP: usize = 8;

/// Rate limit for new connections per IP address per minute.
/// Prevents connection spam attacks while allowing legitimate reconnection patterns.
pub const MAX_CONNECTIONS_PER_IPADDR_PER_MIN: u64 = 64;

/// Container for UDP sockets that will be converted to QUIC servers.
/// Although these are UdpSocket types, they are used as the foundation for QUIC connections
/// which provide reliable, ordered packet delivery with built-in flow control.
#[derive(Debug)]
pub struct TpuSockets {
    /// Sockets for receiving transactions from clients and other validators.
    /// Each socket runs its own QUIC server to distribute load across multiple threads.
    pub transactions_quic_sockets: Vec<UdpSocket>,
    
    /// Sockets for receiving forwarded transactions from other validators.
    /// These handle transaction propagation when the current validator is not the leader.
    /// Separate from regular transaction sockets to allow different resource allocation.
    pub transactions_forwards_quic_sockets: Vec<UdpSocket>,
}

/// The main Transaction Processing Unit that orchestrates transaction ingestion and validation.
/// 
/// The TPU implements a multi-stage pipeline:
/// 1. **QUIC Servers**: Accept connections and receive packet streams
/// 2. **Fetch Stage**: Routes forwarded transactions and handles deduplication  
/// 3. **SigVerify Stage**: Validates transaction signatures in parallel
/// 4. **Staked Nodes Updater**: Maintains validator stake information for prioritization
/// 
/// This design maximizes throughput by parallelizing operations across stages and connections.
pub struct Tpu {
    /// Handles transaction forwarding and routing between validators
    fetch_stage: FetchStage,
    
    /// Maintains up-to-date validator stake information for connection prioritization
    staked_nodes_updater_service: StakedNodesUpdaterService,
    
    /// Verifies transaction signatures for authenticity before banking stage
    sigverify_stage: SigVerifyStage,
    
    /// Background threads running QUIC servers for transaction ingestion
    thread_handles: Vec<JoinHandle<()>>,
}

impl Tpu {
    /// Maximum number of packets that can be queued between TPU stages.
    /// This prevents memory buildup when downstream stages can't keep up with packet ingestion.
    /// Higher values allow for more buffering but use more memory.
    pub const TPU_QUEUE_CAPACITY: usize = 10_000;

    /// Creates and starts a new TPU with all required stages and QUIC servers.
    /// 
    /// # Arguments
    /// * `sockets` - Pre-bound UDP sockets for QUIC server creation
    /// * `exit` - Shared shutdown signal for graceful termination
    /// * `keypair` - Identity keypair for QUIC connection authentication
    /// * `rpc_load_balancer` - RPC client for fetching validator stake information
    /// * `max_unstaked_quic_connections` - Connection limit for validators without stake
    /// * `max_staked_quic_connections` - Connection limit for staked validators
    /// * `staked_nodes_overrides` - Manual stake overrides for testing/special cases
    /// 
    /// # Returns
    /// * `Tpu` - The running TPU instance with all stages active
    /// * `Receiver<BankingPacketBatch>` - Channel for receiving verified transaction batches
    pub fn new(
        sockets: TpuSockets,
        exit: &Arc<AtomicBool>,
        keypair: &Keypair,
        rpc_load_balancer: &Arc<LoadBalancer>,
        max_unstaked_quic_connections: usize,
        max_staked_quic_connections: usize,
        staked_nodes_overrides: HashMap<Pubkey, u64>,
    ) -> (Self, Receiver<BankingPacketBatch>) {
        let TpuSockets {
            transactions_quic_sockets,
            transactions_forwards_quic_sockets,
        } = sockets;

        // Initialize stake-based connection management
        // This tracks validator stake amounts to prioritize high-stake validators for resource allocation
        let staked_nodes = Arc::new(RwLock::new(StakedNodes::default()));
        let staked_nodes_updater_service = StakedNodesUpdaterService::new(
            exit.clone(),
            rpc_load_balancer.clone(),
            staked_nodes.clone(),
            staked_nodes_overrides,
        );

        // Create channels for inter-stage communication
        // Regular TPU channel: receives packets directly from clients/validators
        // Sender tracked as fetch_stage-channel_stats.tpu_sender_len in metrics
        let (tpu_sender, tpu_receiver) = crossbeam_channel::bounded(Tpu::TPU_QUEUE_CAPACITY);

        // TPU forwards channel: receives packets that need to be forwarded to current leader
        // Receiver tracked as fetch_stage-channel_stats.tpu_forwards_receiver_len in metrics
        let (tpu_forwards_sender, tpu_forwards_receiver) =
            crossbeam_channel::bounded(Tpu::TPU_QUEUE_CAPACITY);

        // Start QUIC servers for regular transaction ingestion
        // Each socket gets its own server thread for load distribution
        let mut quic_tasks = transactions_quic_sockets
            .into_iter()
            .map(|sock| {
                spawn_server(
                    "quic_streamer_tpu",           // Thread name for debugging
                    "quic_streamer_tpu",           // Metrics label
                    sock,                          // Pre-bound UDP socket
                    keypair,                       // For QUIC connection authentication
                    tpu_sender.clone(),            // Where to send received packets
                    exit.clone(),                  // Shutdown signal
                    MAX_QUIC_CONNECTIONS_PER_PEER, // Solana's per-peer connection limit
                    staked_nodes.clone(),          // Validator stake info for prioritization
                    max_staked_quic_connections,   // Connection limit for staked validators
                    max_unstaked_quic_connections, // Connection limit for unstaked validators  
                    DEFAULT_MAX_STREAMS_PER_MS,    // Stream creation rate limit
                    MAX_CONNECTIONS_PER_IPADDR_PER_MIN, // New connection rate limit per IP
                    DEFAULT_WAIT_FOR_CHUNK_TIMEOUT,     // Timeout for incomplete packets
                    Duration::from_millis(DEFAULT_TPU_COALESCE_MS), // Packet batching timeout
                )
                .unwrap()
                .thread
            })
            .collect::<Vec<_>>();

        // Start QUIC servers for transaction forwarding between validators
        // These handle leader-to-leader transaction propagation
        quic_tasks.extend(
            transactions_forwards_quic_sockets
                .into_iter()
                .map(|sock| {
                    spawn_server(
                        "quic_streamer_tpu_forwards",   // Thread name for debugging  
                        "quic_streamer_tpu_forwards",   // Metrics label
                        sock,                           // Pre-bound UDP socket
                        keypair,                        // For QUIC connection authentication
                        tpu_forwards_sender.clone(),    // Where to send forwarded packets
                        exit.clone(),                   // Shutdown signal
                        MAX_QUIC_CONNECTIONS_PER_PEER,  // Solana's per-peer connection limit
                        staked_nodes.clone(),           // Validator stake info for prioritization
                        max_staked_quic_connections.saturating_add(max_unstaked_quic_connections), // Total connection pool
                        0, // SECURITY: Prevent unstaked nodes from forwarding transactions
                        DEFAULT_MAX_STREAMS_PER_MS,     // Stream creation rate limit
                        MAX_CONNECTIONS_PER_IPADDR_PER_MIN, // New connection rate limit per IP
                        DEFAULT_WAIT_FOR_CHUNK_TIMEOUT,     // Timeout for incomplete packets
                        Duration::from_millis(DEFAULT_TPU_COALESCE_MS), // Packet batching timeout
                    )
                    .unwrap()
                    .thread
                })
                .collect::<Vec<_>>(),
        );

        // Initialize the fetch stage for transaction routing and deduplication
        // Routes forwarded transactions back into the main TPU pipeline
        let fetch_stage = FetchStage::new(tpu_forwards_receiver, tpu_sender, exit.clone());

        // Create banking packet channel for verified transactions
        // BankingTracer is disabled for performance - no transaction tracing in production
        let (banking_packet_sender, banking_packet_receiver) =
            BankingTracer::new_disabled().create_channel_non_vote();
            
        // Initialize signature verification stage
        // This stage validates transaction signatures in parallel before banking
        let sigverify_stage = SigVerifyStage::new(
            tpu_receiver,                                    // Input: raw packets from QUIC servers
            TransactionSigVerifier::new(banking_packet_sender), // Output: verified packets to banking
            "tpu-verifier",                                  // Thread name for debugging
            "tpu-verifier",                                  // Metrics label
        );

        (
            Tpu {
                fetch_stage,
                staked_nodes_updater_service,
                sigverify_stage,
                thread_handles: quic_tasks,
            },
            banking_packet_receiver, // Caller receives verified transaction batches
        )
    }

    /// Gracefully shuts down all TPU stages and waits for threads to complete.
    /// This ensures clean resource cleanup and proper thread termination.
    /// 
    /// # Returns
    /// `Ok(())` if all threads shut down successfully, or the first error encountered.
    pub fn join(self) -> thread::Result<()> {
        // Wait for each stage to complete in dependency order
        self.fetch_stage.join()?;                    // Transaction routing stage
        self.staked_nodes_updater_service.join()?;   // Stake information updater
        self.sigverify_stage.join()?;                // Signature verification stage
        
        // Wait for all QUIC server threads to complete
        for t in self.thread_handles {
            t.join()?
        }
        Ok(())
    }
}
