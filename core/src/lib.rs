//! Core functionality for the Jito Transaction Relayer.
//! 
//! This crate provides the foundational components for high-performance transaction
//! processing and routing in the Jito relayer system:
//! 
//! - **TPU (Transaction Processing Unit)**: Multi-stage pipeline for transaction ingestion,
//!   signature verification, and forwarding with QUIC-based networking
//! - **Fetch Stage**: Handles transaction forwarding between validators with loop prevention
//! - **Staked Nodes Updater**: Maintains real-time validator stake information for
//!   resource allocation and prioritization
//! - **OFAC Compliance**: Filters transactions involving sanctioned addresses
//! - **Graceful Shutdown**: Coordinated shutdown system for multi-threaded operations
//! 
//! The core crate is designed to be validator-agnostic and provides clean abstractions
//! for transaction processing that can be used independently of Jito-specific features.

use std::{
    panic,
    panic::PanicInfo,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use log::*;

// Internal modules
mod fetch_stage;
mod staked_nodes_updater_service;

// Public modules
pub mod ofac;
pub mod tpu;

/// Sets up a graceful panic handler that coordinates shutdown across all threads.
/// 
/// When a panic occurs in any thread, this handler:
/// 1. Logs the panic information with full details
/// 2. Executes an optional custom callback for cleanup
/// 3. Sets the exit flag to signal all threads to shut down
/// 4. Waits 5 seconds for graceful thread termination
/// 5. Forces process exit if threads don't respond
/// 
/// This "fail-fast" approach ensures that partial failures don't leave the system
/// in an inconsistent state, which is critical for financial applications.
/// 
/// # Arguments
/// * `callback` - Optional function to execute during panic for custom cleanup
/// 
/// # Returns
/// * `Arc<AtomicBool>` - Shared exit flag that threads should monitor for shutdown
/// 
/// # Example
/// ```rust
/// let exit = graceful_panic(None);
/// 
/// // In worker threads:
/// while !exit.load(Ordering::Relaxed) {
///     // Do work...
/// }
/// ```
pub fn graceful_panic(callback: Option<fn(&PanicInfo)>) -> Arc<AtomicBool> {
    let exit = Arc::new(AtomicBool::new(false));
    
    // Replace the default panic handler with our coordinated shutdown handler
    let panic_hook = panic::take_hook();
    {
        let exit = exit.clone();
        panic::set_hook(Box::new(move |panic_info| {
            // Log panic details for debugging and alerting
            error!("process panicked: {}", panic_info);
            
            // Execute custom cleanup callback if provided
            if let Some(f) = callback {
                f(panic_info);
            }
            
            // Signal all threads to begin graceful shutdown
            exit.store(true, Ordering::Relaxed);
            
            // Give threads time to clean up resources and shut down gracefully
            // This prevents data corruption and ensures proper resource cleanup
            std::thread::sleep(Duration::from_secs(5));
            
            // Print panic backtrace using the default handler (exit code 101)
            panic_hook(panic_info);

            // Force exit if threads are unresponsive (prevents hanging processes)
            process::exit(1);
        }));
    }
    
    exit
}
