//! The `fetch_stage` handles transaction forwarding and routing within the TPU pipeline.
//! 
//! This stage is responsible for:
//! - Receiving forwarded transactions from other validators
//! - Marking packets with the FORWARDED flag to prevent infinite loops
//! - Routing forwarded packets back into the main TPU processing pipeline
//! - Monitoring channel health and performance metrics
//! 
//! The FetchStage acts as a bridge between the TPU forward receiver and the main
//! TPU processing channel, ensuring that forwarded transactions are properly
//! handled and deduplicated.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, Builder, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{RecvError, RecvTimeoutError, SendError};
use solana_metrics::{datapoint_error, datapoint_info};
use solana_perf::packet::PacketBatch;
use solana_sdk::packet::{Packet, PacketFlags};
use solana_streamer::streamer::{PacketBatchReceiver, PacketBatchSender};

/// Errors that can occur during fetch stage operation.
#[derive(Debug, thiserror::Error)]
pub enum FetchStageError {
    /// Failed to send packet batch to downstream TPU channel
    #[error("send error: {0}")]
    Send(#[from] SendError<PacketBatch>),
    
    /// Timeout while waiting for forwarded packets (normal during low traffic)
    #[error("recv timeout: {0}")]
    RecvTimeout(#[from] RecvTimeoutError),
    
    /// Channel disconnected or other receive error (indicates serious problem)
    #[error("recv error: {0}")]
    Recv(#[from] RecvError),
}

/// Result type for fetch stage operations.
pub type FetchStageResult<T> = Result<T, FetchStageError>;

/// The FetchStage handles forwarded transaction routing within the TPU pipeline.
/// 
/// This stage runs in its own thread and continuously:
/// 1. Receives forwarded transaction packets from other validators
/// 2. Marks packets with FORWARDED flag to prevent processing loops
/// 3. Routes packets to the main TPU processing pipeline
/// 4. Monitors channel performance and emits metrics
pub struct FetchStage {
    /// Handle to the background thread processing forwarded transactions
    thread_hdls: Vec<JoinHandle<()>>,
}

impl FetchStage {
    /// Creates and starts a new FetchStage for handling forwarded transactions.
    /// 
    /// # Arguments
    /// * `tpu_forwards_receiver` - Channel receiving forwarded packets from other validators
    /// * `tpu_sender` - Channel for sending packets to the main TPU processing pipeline
    /// * `exit` - Shared shutdown signal for graceful termination
    /// 
    /// # Returns
    /// A new FetchStage instance with background processing thread started
    pub fn new(
        tpu_forwards_receiver: PacketBatchReceiver,
        tpu_sender: PacketBatchSender,
        exit: Arc<AtomicBool>,
    ) -> Self {
        // Start background thread for forwarded packet processing
        let fwd_thread_hdl = Builder::new()
            .name("fetch_stage-forwarder_thread".to_string())
            .spawn(move || {
                // Metrics collection configuration
                let metrics_interval = Duration::from_secs(1);
                let mut start = Instant::now();
                let mut tpu_forwards_receiver_max_len = 0usize;
                let mut tpu_sender_max_len = 0usize;
                
                // Main processing loop - continues until shutdown signal
                while !exit.load(Ordering::Relaxed) {
                    // Process forwarded packets and handle errors
                    match Self::handle_forwarded_packets(&tpu_forwards_receiver, &tpu_sender) {
                        // Success or timeout (normal during low traffic) - continue processing
                        Ok(()) | Err(FetchStageError::RecvTimeout(RecvTimeoutError::Timeout)) => {}
                        
                        // Critical error - log and panic to trigger restart
                        Err(e) => {
                            datapoint_error!(
                                "fetch_stage-handle_forwarded_packets_error",
                                ("error", e.to_string(), String)
                            );
                            panic!("Failed to handle forwarded packets. Error: {e}")
                        }
                    };

                    // Emit metrics every second for operational monitoring
                    if start.elapsed() >= metrics_interval {
                        datapoint_info!(
                            "fetch_stage-channel_stats",
                            ("tpu_sender_len", tpu_sender_max_len, i64),
                            ("tpu_sender_capacity", tpu_sender.capacity().unwrap(), i64),
                            (
                                "tpu_forwards_receiver_len",
                                tpu_forwards_receiver_max_len,
                                i64
                            ),
                            (
                                "tpu_forwards_receiver_capacity",
                                tpu_forwards_receiver.capacity().unwrap(),
                                i64
                            ),
                        );
                        start = Instant::now();
                        tpu_forwards_receiver_max_len = 0;
                        tpu_sender_max_len = 0;
                    }
                    
                    // Track peak channel utilization for performance monitoring
                    tpu_forwards_receiver_max_len =
                        std::cmp::max(tpu_forwards_receiver_max_len, tpu_forwards_receiver.len());
                    tpu_sender_max_len = std::cmp::max(tpu_sender_max_len, tpu_sender.len());
                }
            })
            .unwrap();

        Self {
            thread_hdls: vec![fwd_thread_hdl],
        }
    }

    /// Processes forwarded packets by marking them and routing to the main TPU pipeline.
    /// 
    /// This function:
    /// 1. Receives forwarded packets from other validators
    /// 2. Marks each packet with FORWARDED flag to prevent infinite forwarding loops
    /// 3. Batches packets for efficiency (up to 1024 packets per batch)
    /// 4. Sends batches to the main TPU processing pipeline
    /// 
    /// # Arguments
    /// * `tpu_forwards_receiver` - Channel receiving forwarded packets
    /// * `tpu_sender` - Channel for sending to main TPU processing
    /// 
    /// # Returns
    /// `Ok(())` on success, or error if channel operations fail
    fn handle_forwarded_packets(
        tpu_forwards_receiver: &PacketBatchReceiver,
        tpu_sender: &PacketBatchSender,
    ) -> FetchStageResult<()> {
        // Helper function to mark packets as forwarded to prevent processing loops
        let mark_forwarded = |packet: &mut Packet| {
            packet.meta_mut().flags |= PacketFlags::FORWARDED;
        };

        // Block waiting for the first packet batch
        let mut packet_batch = tpu_forwards_receiver.recv()?;
        let mut num_packets = packet_batch.len();
        packet_batch.iter_mut().for_each(mark_forwarded);

        // Collect additional available batches without blocking for efficiency
        let mut packet_batches = vec![packet_batch];
        while let Ok(mut packet_batch) = tpu_forwards_receiver.try_recv() {
            num_packets += packet_batch.len();
            packet_batch.iter_mut().for_each(mark_forwarded);
            packet_batches.push(packet_batch);
            
            // Limit batch size to prevent memory buildup and ensure responsive processing
            // 1024 packets is a good balance between efficiency and latency
            if num_packets > 1024 {
                break;
            }
        }

        // Send all collected batches to the main TPU processing pipeline
        for packet_batch in packet_batches {
            if let Err(e) = tpu_sender.send(packet_batch) {
                return Err(FetchStageError::Send(e));
            }
        }

        Ok(())
    }

    /// Gracefully shuts down the fetch stage and waits for thread completion.
    /// 
    /// # Returns
    /// `Ok(())` if all threads shut down successfully, or the first error encountered
    pub fn join(self) -> thread::Result<()> {
        for thread_hdl in self.thread_hdls {
            thread_hdl.join()?;
        }
        Ok(())
    }
}
