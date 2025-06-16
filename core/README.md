# Core Package

The `jito-core` package provides foundational Solana-compatible transaction processing functionality without requiring full validator dependencies. It contains core Solana functionality stripped of validator-specific components like `poh_recorder` and `bank_forks`, making it ideal for building transaction relay infrastructure.

## Overview

The core package serves as the foundation for Jito's transaction relay infrastructure by providing:

- **High-performance transaction processing** with QUIC-based packet reception
- **OFAC compliance filtering** for regulatory adherence  
- **Stake-weighted prioritization** for validator traffic management
- **Packet batching and forwarding** for optimal network efficiency
- **Graceful error handling** with coordinated shutdown mechanisms

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   QUIC Clients  │────│     TPU Core     │────│  Banking Stage  │
│   (Senders)     │    │   (This Package) │    │  (Downstream)   │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │
                              ├─ OFAC Filtering
                              ├─ Stake Prioritization  
                              ├─ Packet Batching
                              └─ Signature Verification
```

## Core Components

### 1. **Transaction Processing Unit** (`tpu.rs`)

The complete TPU implementation that orchestrates the entire transaction processing pipeline.

**Key Responsibilities:**
- Manages multiple QUIC servers for transaction and forward channels
- Coordinates multi-stage packet processing pipeline
- Handles connection limits and traffic prioritization
- Provides banking-ready packet output

**Architecture Flow:**
```
QUIC Reception → Fetch Stage → Signature Verification → Banking Output
      ↓              ↓                ↓                     ↓
  Packet Input  → Batching &    → Crypto Verification → Ready for
  from Clients     Forwarding     & OFAC Filtering      Execution
```

**Key Configuration:**
```rust
pub struct TpuSockets {
    pub transactions: Vec<UdpSocket>,     // Transaction reception
    pub transaction_forwards: Vec<UdpSocket>,  // Forward channel
}

// TPU initialization with full pipeline
let tpu = Tpu::new(
    tpu_sockets,
    cluster_info,
    sigverify_disabled,
    max_unstaked_connections,
    max_staked_connections,
    staked_nodes,
    banking_packet_sender,
    // ... other params
)?;
```

### 2. **Fetch Stage** (`fetch_stage.rs`)

High-performance packet batching and forwarding component that optimizes network traffic.

**Key Features:**
- **Batch Processing**: Processes up to 1024 packets per iteration
- **Forward Handling**: Manages packet forwarding with proper marking
- **Channel Management**: Efficient inter-thread communication
- **Performance Monitoring**: Tracks channel utilization and capacity

**Core Function:**
```rust
pub fn handle_forwarded_packets(
    forward_receiver: &PacketBatchReceiver,
    tpu_receiver: &PacketBatchSender,
) -> Result<(), RecvTimeoutError> {
    // Receives packets from forward channel
    // Marks them as forwarded
    // Batches and sends to TPU processing pipeline
}
```

**Performance Characteristics:**
- 1024 packet batch size for optimal throughput
- Non-blocking channel operations for low latency
- Comprehensive metrics for monitoring

### 3. **OFAC Compliance** (`ofac.rs`)

Regulatory compliance filtering that ensures all transactions adhere to OFAC sanctions.

**Filtering Capabilities:**
- **Static Account Filtering**: Checks transaction's static account keys
- **Lookup Table Resolution**: Resolves and checks address lookup tables  
- **Comprehensive Coverage**: Handles both versioned and legacy transactions
- **Performance Optimized**: O(1) lookups using HashSet data structures

**Core API:**
```rust
// Main OFAC checking function
pub fn is_tx_ofac_related(
    transaction: &SanitizedTransaction,
    ofac_addresses: &HashSet<Pubkey>,
    address_lookup_table_cache: &AddressLookupTableCache,
) -> bool {
    // Check static keys
    // Resolve and check lookup table addresses
    // Return true if any OFAC address found
}
```

**Integration Points:**
- Used by block_engine for transaction filtering
- Integrated into TPU pipeline for compliance
- Supports address lookup table caching

### 4. **Staked Nodes Updater** (`staked_nodes_updater_service.rs`)

Maintains up-to-date validator stake information for traffic prioritization and connection management.

**Key Functions:**
- **Periodic Updates**: Refreshes stake data every 5 seconds
- **RPC Integration**: Fetches vote account data from multiple RPC endpoints
- **Stake Calculation**: Combines current and delinquent validator stakes
- **Override Support**: Allows manual stake overrides for testing

**Data Flow:**
```
RPC Endpoints → Vote Accounts → Stake Calculation → Shared StakedNodes
     ↓              ↓              ↓                    ↓
Multiple RPC    Current &      Lamport-based      Thread-safe
Load Balanced   Delinquent     Stake Weights      Shared State
   Sources      Validators
```

**Configuration:**
```rust
// Refresh stake information
pub fn try_refresh_pk_to_stake(
    cluster_rpc: &RpcLoadBalancer,
    pubkey_to_stake: &mut Arc<RwLock<HashMap<Pubkey, u64>>>,
    staked_nodes_overrides: &HashMap<Pubkey, u64>,
) -> Result<(), RpcError> {
    // Fetch current and delinquent vote accounts
    // Calculate combined stake weights  
    // Apply manual overrides
    // Update shared stake map
}
```

### 5. **Graceful Panic Handling** (`lib.rs`)

Robust error handling system that ensures coordinated shutdown across all threads.

**Features:**
- **Custom Panic Handler**: Intercepts process panics
- **Graceful Shutdown**: 5-second grace period for thread cleanup
- **Coordinated Exit**: Allows other threads to shut down properly
- **Force Termination**: Ensures process doesn't hang

```rust
pub fn graceful_panic() {
    std::panic::set_hook(Box::new(|_| {
        GRACEFUL_EXIT.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(5));
        std::process::exit(1);
    }));
}
```

## Integration with Other Packages

### **With `jito-relayer`:**
- Provides TPU infrastructure for packet processing
- Supplies OFAC filtering capabilities
- Offers stake-weighted prioritization

### **With `jito-block-engine`:**
- OFAC filtering used for transaction compliance
- Packet batching for efficient forwarding

### **With `jito-rpc`:**
- RPC load balancing for stake information updates
- Multi-endpoint resilience for critical data

### **With `transaction-relayer`:**
- Core TPU implementation for main binary
- Provides complete packet processing pipeline

## Performance Characteristics

### **High Throughput:**
- Batch processing up to 1024 packets per iteration
- Multiple QUIC servers for parallel packet reception
- Optimized channel operations for minimal latency

### **Connection Management:**
- Configurable limits for staked vs unstaked connections
- IP-based connection limiting (8 connections per IP)
- Stake-weighted traffic prioritization

### **Memory Efficiency:**
- `Arc<RwLock<>>` for efficient shared state
- Packet batching reduces memory allocation overhead
- Address lookup table caching

### **Latency Optimization:**
- 5ms default QUIC coalescing timeout
- Non-blocking channel operations
- Streamlined packet processing pipeline

## Configuration Constants

```rust
// Performance tuning
const TPU_QUEUE_CAPACITY: usize = 10_000;
const DEFAULT_TPU_COALESCE_MS: u64 = 5;
const MAX_QUIC_CONNECTIONS_PER_IP: usize = 8;

// Service intervals  
const PK_TO_STAKE_REFRESH_DURATION: Duration = Duration::from_secs(5);

// Batch sizes
const MAX_PACKETS_PER_BATCH: usize = 1024;
```

## Usage Example

```rust
use jito_core::{Tpu, graceful_panic, is_tx_ofac_related};

// Set up graceful panic handling
graceful_panic();

// Create TPU with full pipeline
let tpu = Tpu::new(
    tpu_sockets,
    cluster_info,
    false, // sigverify enabled
    500,   // max unstaked connections
    2000,  // max staked connections
    staked_nodes,
    banking_packet_sender,
    vote_signature_sender,
    gossip_subscriber_sender,
    log_messages_bytes_limit,
    staked_nodes_updater_service,
    1, // num quic endpoints
    tpu_coalesce_ms,
)?;

// Use OFAC filtering
let is_sanctioned = is_tx_ofac_related(
    &transaction,
    &ofac_addresses,
    &address_lookup_table_cache,
);
```

## Monitoring and Metrics

The core package provides extensive metrics integration:

- **Packet Processing**: Throughput, latency, batch sizes
- **Connection Management**: Active connections, connection limits
- **OFAC Filtering**: Filtered transaction counts
- **Stake Updates**: Refresh frequency, RPC latency
- **Error Rates**: Failed operations, retry counts

This package forms the robust foundation for Jito's high-performance, compliant transaction relay infrastructure, providing all the essential components needed for enterprise-grade transaction processing without validator dependencies.