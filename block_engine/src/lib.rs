//! # Block Engine Module
//!
//! This module provides integration with Jito's MEV (Maximum Extracted Value) block engine
//! infrastructure. It handles the communication between the relayer and the block engine,
//! including authentication, packet filtering, and forwarding of transactions.
//!
//! The block engine is responsible for:
//! - Processing MEV bundles and transactions
//! - Maintaining accounts and programs of interest (AOI/POI) for filtering
//! - OFAC compliance filtering to prevent sanctioned addresses from transacting
//! - Coordinating with validators for optimal block construction
//!
//! ## Main Components
//!
//! - [`block_engine`]: Core block engine client and packet forwarding logic
//! - [`block_engine_stats`]: Performance metrics and statistics collection

pub mod block_engine;
pub mod block_engine_stats;
