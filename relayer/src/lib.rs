//! Jito Relayer Core Components
//! 
//! This crate provides the core relayer functionality for the Jito MEV infrastructure,
//! including authentication, health monitoring, and transaction packet routing.
//! 
//! ## Main Components
//! 
//! ### Authentication System
//! - **auth_service**: JWT-based challenge-response authentication for validators
//! - **auth_interceptor**: gRPC middleware for validating JWT tokens
//! - **auth_challenges**: DOS-resistant challenge management with expiration
//! 
//! ### Health & Monitoring
//! - **health_manager**: Tracks relayer connectivity and operational status
//! - **schedule_cache**: Maintains current Solana leader schedule for packet routing
//! 
//! ### Core Relayer
//! - **relayer**: Main packet forwarding service with OFAC filtering and metrics
//! 
//! ## Architecture
//! 
//! The relayer acts as a high-performance proxy between validators and the Solana network:
//! 1. Validators authenticate using cryptographic challenges
//! 2. Authenticated validators subscribe to packet streams
//! 3. Packets are filtered for OFAC compliance and routed to appropriate validators
//! 4. Health monitoring ensures only operational relayers accept connections
//! 
//! ## Security Features
//! 
//! - JWT token authentication with IP binding
//! - DOS protection through rate limiting and capacity controls
//! - OFAC sanctions filtering for regulatory compliance
//! - Health-based connection management

mod auth_challenges;
pub mod auth_interceptor;
pub mod auth_service;
pub mod health_manager;
pub mod relayer;
pub mod schedule_cache;
