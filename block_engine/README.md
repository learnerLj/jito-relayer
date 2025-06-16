# Block Engine Package

The `block_engine` package provides intelligent transaction filtering and forwarding capabilities for Jito's MEV infrastructure. It acts as a selective proxy that connects to Jito's Block Engine service to enable efficient MEV extraction while maintaining OFAC compliance.

## Overview

The Block Engine client serves as a critical component in the MEV pipeline by:
- **Filtering transactions** based on Accounts of Interest (AOI) and Programs of Interest (POI)
- **Forwarding relevant packets** to the Block Engine for bundle processing
- **Maintaining authentication** with JWT-based challenge-response protocol
- **Enforcing compliance** through OFAC sanctions filtering
- **Providing real-time communication** with heartbeats and stream management

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Relayer       │────│  Block Engine    │────│  Block Engine   │
│   (TPU Proxy)   │    │  Client (This)   │    │  Service        │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │
                              ├─ AOI/POI Filtering
                              ├─ OFAC Compliance
                              ├─ JWT Authentication
                              └─ Metrics Collection
```

## Core Components

### 1. **BlockEngineRelayerHandler** (`block_engine.rs`)

The main handler that manages the entire Block Engine connection lifecycle.

**Key Responsibilities:**
- Establishes authenticated connections to Block Engine and Auth services
- Manages bi-directional packet streams with automatic reconnection
- Implements intelligent transaction filtering logic
- Handles JWT token refresh and authentication challenges

**Core Methods:**
```rust
// Authenticate with the Block Engine auth service
pub async fn auth(&self) -> Result<(String, String), BlockEngineError>

// Start the main event loop with packet processing
pub async fn start_event_loop(&self, packet_receiver: Receiver<BlockEnginePackets>)

// Filter transactions based on AOI/POI and OFAC compliance
fn filter_packets(&self, packet_batch: &PacketBatch) -> Vec<Packet>
```

### 2. **BlockEngineStats** (`block_engine_stats.rs`)

Comprehensive metrics collection for monitoring Block Engine performance.

**Metrics Categories:**
- **Authentication**: Token refresh attempts, auth failures, timing
- **Heartbeats**: Sent/received counts, timing measurements
- **AOI/POI Updates**: Account and program interest list updates
- **Packet Processing**: Forwarded/dropped counts, processing latency
- **Connection Health**: Stream status, reconnection attempts

**Key Metrics:**
```rust
pub struct BlockEngineStats {
    pub auth_refresh_attempts: AtomicU64,
    pub heartbeats_sent: AtomicU64,
    pub heartbeats_received: AtomicU64,
    pub aoi_updates_received: AtomicU64,
    pub poi_updates_received: AtomicU64,
    pub packets_forwarded: AtomicU64,
    pub packets_dropped: AtomicU64,
    pub refresh_auth_time_us: AtomicU64,
    // ... and many more
}
```

### 3. **Configuration** (`BlockEngineConfig`)

Configuration structure for Block Engine connectivity:

```rust
pub struct BlockEngineConfig {
    pub block_engine_url: String,        // Block Engine gRPC endpoint
    pub block_engine_auth_service_url: String,  // Auth service endpoint
}
```

## Packet Filtering Logic

The Block Engine implements sophisticated filtering to reduce bandwidth and focus on MEV-relevant transactions:

### 1. **Accounts of Interest (AOI) Filtering**
- Maintains a dynamic list of accounts that are relevant for MEV extraction
- Filters transactions that read from or write to these accounts
- Prioritizes transactions with write locks on AOI accounts

### 2. **Programs of Interest (POI) Filtering**
- Tracks specific program IDs that are MEV-relevant (e.g., DEXs, lending protocols)
- Forwards transactions that interact with these programs
- Supports dynamic updates to the POI list

### 3. **Address Lookup Table Resolution**
- Resolves Solana Address Lookup Tables to get full account lists
- Applies AOI/POI filtering to both static and dynamic addresses
- Handles lookup table caching for performance

### 4. **OFAC Compliance**
- Integrates with `jito-core` OFAC filtering
- Automatically drops transactions involving sanctioned addresses
- Ensures regulatory compliance for all forwarded transactions

## Authentication Flow

The Block Engine uses a robust JWT-based authentication system:

```
1. Challenge Request    ┌─────────────┐
   ────────────────────▶│ Auth Service │
                        └─────────────┘
2. Challenge Response      │
   ◀────────────────────────┤
   (signed with keypair)    │
                           │
3. Token Exchange          │
   ────────────────────────▶
                           │
4. JWT Tokens             │
   ◀──────────────────────┤
   (access + refresh)      │
                           │
5. Authenticated Requests  │
   ────────────────────────▶
```

**Authentication Features:**
- Ed25519 keypair-based challenge signing
- Automatic token refresh before expiration
- Graceful handling of auth failures with retry logic
- Separate access and refresh tokens

## Usage Example

```rust
use jito_block_engine::{BlockEngineRelayerHandler, BlockEngineConfig};

// Configure Block Engine connection
let config = BlockEngineConfig {
    block_engine_url: "https://amsterdam.mainnet.block-engine.jito.wtf".to_string(),
    block_engine_auth_service_url: "https://amsterdam.mainnet.auth.jito.wtf".to_string(),
};

// Create handler with keypair for authentication
let handler = BlockEngineRelayerHandler::new(
    config,
    keypair,
    Some(ofac_addresses),
).await?;

// Start the event loop to begin packet processing
let (packet_sender, packet_receiver) = crossbeam_channel::unbounded();
tokio::spawn(async move {
    handler.start_event_loop(packet_receiver).await;
});

// Send packets for filtering and forwarding
packet_sender.send(block_engine_packets)?;
```

## Integration Points

### With Other Packages:
- **`jito-core`**: OFAC compliance checking
- **`jito-protos`**: gRPC protocol definitions
- **`relayer`**: Receives filtered packets from TPU proxy
- **`transaction-relayer`**: Main binary orchestrates the Block Engine client

### External Services:
- **Block Engine Service**: Receives filtered transactions for MEV bundle processing
- **Auth Service**: Provides JWT-based authentication
- **Address Lookup Tables**: Solana on-chain data for transaction resolution

## Performance Characteristics

- **Low-latency filtering**: Microsecond-level packet processing
- **Intelligent caching**: Address lookup table caching for performance
- **Automatic reconnection**: Robust handling of network failures
- **Concurrent processing**: Async streams for high-throughput packet handling
- **Memory efficient**: Streaming processing without large memory buffers

## Monitoring and Observability

The Block Engine provides extensive metrics for monitoring:

```rust
// Access metrics through the stats instance
let stats = handler.stats();
println!("Packets forwarded: {}", stats.packets_forwarded.load(Ordering::Relaxed));
println!("Auth refresh time: {}μs", stats.refresh_auth_time_us.load(Ordering::Relaxed));
```

**Key Performance Indicators:**
- Packet forwarding rate and success ratio
- Authentication refresh frequency and timing
- AOI/POI update frequency
- Connection health and reconnection events

This package is essential for Jito's censorship-resistant MEV infrastructure, providing intelligent filtering while maintaining high-performance transaction forwarding to block builders.