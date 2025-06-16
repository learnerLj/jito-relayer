# RPC Package

The `jito-rpc` package provides intelligent RPC load balancing for Solana blockchain interactions. It manages multiple RPC endpoints with real-time slot tracking, ensuring optimal performance and high availability for the Jito relayer infrastructure.

## Overview

The RPC package serves as a critical infrastructure component by providing:

- **Smart Load Balancing** based on real-time blockchain state (slot height)
- **High Availability** through multiple redundant RPC endpoints
- **Real-time Slot Tracking** via WebSocket subscriptions
- **Automatic Failover** with health monitoring and reconnection
- **Performance Optimization** through connection reuse and concurrent operations

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│  Jito Services  │────│   RPC LoadBalancer│────│  Solana RPC     │
│ (Relayer, etc.) │    │  (This Package)   │    │   Endpoints     │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │                        │
                              ├─ Slot-based Selection  ├─ HTTP RPC
                              ├─ Health Monitoring     ├─ WebSocket Slots
                              ├─ Automatic Failover    └─ Multiple Servers
                              └─ Connection Management
```

## Core Components

### 1. **RPC Load Balancer** (`load_balancer.rs`)

The main component that implements intelligent load balancing based on blockchain state.

#### **Smart Selection Algorithm**

Unlike traditional load balancers, the RPC LoadBalancer uses **slot-based selection**:

```rust
pub fn rpc_client(&self) -> Arc<RpcClient> {
    let (highest_server, _) = self.get_highest_slot();
    self.server_to_rpc_client
        .get(&highest_server)
        .unwrap()
        .value()
        .to_owned()
}
```

**Selection Strategy:**
- **Highest Slot Priority**: Always routes to the RPC server with the most current blockchain state
- **Real-time Updates**: Continuously tracks slot progression across all servers
- **Atomic Operations**: Thread-safe slot tracking using `AtomicU64` and `fetch_max`

#### **Connection Management**

**Initialization Process:**
```rust
pub async fn new_with_servers(
    servers: &[String],
    websocket_servers: &[String],
    exit: Arc<AtomicBool>,
) -> Result<(Self, Receiver<Slot>), RpcError> {
    // Pre-warm RPC connections with health checks
    // Establish WebSocket subscriptions for slot tracking  
    // Start monitoring threads for each server
    // Return LoadBalancer instance and slot update receiver
}
```

**Key Features:**
- **Connection Warming**: Pre-establishes RPC connections during startup
- **Health Validation**: Tests each server with `get_slot()` calls during initialization
- **Timeout Configuration**: 120-second RPC timeout for balance between reliability and responsiveness
- **Thread Management**: Separate monitoring thread per RPC server

#### **Real-time Slot Tracking**

Each RPC server gets a dedicated WebSocket subscription thread:

```rust
pub fn slot_subscribe(websocket_url: &str) -> Result<(), RpcError> {
    match PubsubClient::slot_subscribe(&websocket_url) {
        Ok((_subscription, receiver)) => {
            while !exit.load(Ordering::Relaxed) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(slot) => {
                        // Update server's current slot
                        server_to_slot.insert(websocket_url.clone(), slot.slot);
                        
                        // Track global highest slot
                        let old_slot = highest_slot.fetch_max(slot.slot, Ordering::Relaxed);
                        if slot.slot > old_slot {
                            // Broadcast new highest slot to consumers
                            slot_sender.send(slot.slot)?;
                        }
                    }
                }
            }
        }
    }
}
```

**Slot Tracking Benefits:**
- **Optimized Routing**: Ensures requests go to servers with latest blockchain state
- **Low Latency**: Real-time updates minimize stale data access
- **Global Coordination**: Provides system-wide slot awareness for all components

### 2. **Failover and Health Management**

#### **Automatic Reconnection**

The system implements sophisticated health monitoring with automatic recovery:

```rust
// Disconnect timeout detection
if last_slot_update.elapsed() >= Self::DISCONNECT_WEBSOCKET_TIMEOUT {
    datapoint_error!(
        "rpc_load_balancer-force_disconnect",
        ("websocket_url", websocket_url, String),
        ("slot", latest_slot, i64)
    );
    break; // Forces reconnection by exiting subscription loop
}
```

**Health Monitoring Features:**
- **WebSocket Timeout**: 30-second disconnect timeout for unresponsive connections
- **Forced Reconnection**: Automatic reconnection for stalled WebSocket subscriptions
- **Graceful Recovery**: 1-second delay between reconnection attempts
- **Error Isolation**: Individual server failures don't affect overall system

#### **Error Handling Strategy**

**Error Categories:**
- **Network Errors**: Connection failures, timeouts, DNS issues
- **Protocol Errors**: Invalid responses, subscription failures
- **Service Errors**: RPC method failures, rate limiting

**Recovery Mechanisms:**
- **Non-blocking Operations**: Errors don't stop other server monitoring
- **Detailed Logging**: Comprehensive error context for debugging
- **Metrics Emission**: Error rates and patterns for operational monitoring
- **Thread Isolation**: Each server runs in isolated thread to prevent cascading failures

### 3. **Performance Optimization**

#### **Concurrency Design**

**Thread-Safe Data Structures:**
```rust
pub struct LoadBalancer {
    server_to_slot: Arc<DashMap<String, Slot>>,           // WebSocket URL → Current Slot
    server_to_rpc_client: Arc<DashMap<String, Arc<RpcClient>>>, // WebSocket URL → RPC Client
    highest_slot: Arc<AtomicU64>,                         // Global highest slot
    exit: Arc<AtomicBool>,                                // Shutdown coordination
}
```

**Concurrency Benefits:**
- **Lock-free Access**: `DashMap` and `AtomicU64` eliminate contention
- **Parallel Operations**: Concurrent slot monitoring across all servers
- **Shared State**: Efficient `Arc` reference counting for shared access
- **Non-blocking Updates**: Real-time slot updates without blocking operations

#### **Memory and Network Optimization**

**Memory Management:**
- **Bounded Channels**: 100-slot capacity prevents memory bloat during high activity
- **Connection Reuse**: Persistent RPC connections eliminate connection overhead
- **Efficient Data Structures**: `DashMap` optimized for concurrent read/write operations

**Network Optimization:**
- **Connection Warming**: Pre-established connections eliminate cold start delays
- **Timeout Tuning**: Balanced timeouts (120s RPC, 30s WebSocket) for optimal performance
- **Parallel Subscriptions**: Simultaneous WebSocket connections maximize data freshness

## Integration Points

### **With Other Jito Components:**

#### **`transaction-relayer` (Main Binary):**
```rust
// Initialize load balancer for main application
let (rpc_load_balancer, slot_receiver) = RpcLoadBalancer::new_with_servers(
    &args.rpc_servers,
    &args.websocket_servers,
    exit.clone(),
).await?;

// Share load balancer across services
let relayer_service = RelayerService::new(rpc_load_balancer.clone());
let auth_service = AuthService::new(rpc_load_balancer.clone());
```

#### **`relayer/schedule_cache.rs`:**
```rust
// Use load balancer for leader schedule updates
pub fn try_refresh_pk_to_stake(
    cluster_rpc: &RpcLoadBalancer,  // Uses this package
    pubkey_to_stake: &mut HashMap<Pubkey, u64>,
) -> Result<(), RpcError> {
    let rpc_client = cluster_rpc.rpc_client();
    // Fetch vote accounts from optimal RPC server
}
```

#### **`core/staked_nodes_updater_service.rs`:**
- Refreshes validator stake information using optimal RPC client
- Critical for MEV bundle routing and connection prioritization

### **External Dependencies:**

#### **Solana Client Libraries:**
- **`solana-client`**: Core RPC and PubSub client functionality
- **`solana-pubsub-client`**: WebSocket subscription management
- **`solana-sdk`**: Solana types and commitment levels

#### **Concurrency Libraries:**
- **`crossbeam-channel`**: High-performance bounded channels for slot updates
- **`dashmap`**: Concurrent hash maps for thread-safe server state
- **`tokio`**: Async runtime for network operations

## Configuration

### **Server Configuration:**
```rust
// HTTP RPC endpoints for blockchain queries
let rpc_servers = vec![
    "https://api.mainnet-beta.solana.com".to_string(),
    "https://solana-api.projectserum.com".to_string(),
];

// WebSocket endpoints for real-time slot tracking
let websocket_servers = vec![
    "wss://api.mainnet-beta.solana.com".to_string(),
    "wss://solana-api.projectserum.com".to_string(),
];
```

### **Performance Tuning:**
```rust
// Timeout configurations
const RPC_TIMEOUT: Duration = Duration::from_secs(120);
const DISCONNECT_WEBSOCKET_TIMEOUT: Duration = Duration::from_secs(30);

// Channel capacity for slot updates
const SLOT_CHANNEL_CAPACITY: usize = 100;

// Commitment level for optimal performance
const COMMITMENT_LEVEL: CommitmentLevel = CommitmentLevel::Processed;
```

## Usage Example

```rust
use jito_rpc::LoadBalancer;
use std::sync::{Arc, atomic::AtomicBool};

// Initialize with multiple RPC endpoints
let rpc_servers = vec![
    "https://api.mainnet-beta.solana.com".to_string(),
    "https://rpc.ankr.com/solana".to_string(),
];

let websocket_servers = vec![
    "wss://api.mainnet-beta.solana.com".to_string(),
    "wss://rpc.ankr.com/solana/ws".to_string(),
];

let exit = Arc::new(AtomicBool::new(false));

// Create load balancer with automatic failover
let (load_balancer, slot_receiver) = LoadBalancer::new_with_servers(
    &rpc_servers,
    &websocket_servers,
    exit.clone(),
).await?;

// Get optimal RPC client (highest slot)
let rpc_client = load_balancer.rpc_client();

// Use for blockchain queries
let slot = rpc_client.get_slot().await?;
let epoch_info = rpc_client.get_epoch_info().await?;

// Receive real-time slot updates
tokio::spawn(async move {
    while let Ok(new_slot) = slot_receiver.recv() {
        println!("New highest slot: {}", new_slot);
    }
});
```

## Monitoring and Metrics

The RPC package provides comprehensive metrics for operational monitoring:

### **Performance Metrics:**
- **Slot Update Frequency**: Rate of slot updates per server
- **Server Selection**: Which server is currently selected (highest slot)
- **Connection Health**: Active/inactive server status

### **Error Metrics:**
- **Disconnection Events**: WebSocket disconnection frequency
- **Reconnection Attempts**: Automatic recovery statistics
- **RPC Failures**: Failed blockchain query rates

### **Operational Metrics:**
- **Queue Lengths**: Slot update channel utilization
- **Response Times**: RPC query latency per server
- **Availability**: Uptime percentage per server

## Best Practices

### **Server Configuration:**
- **Geographic Distribution**: Use RPC servers in different regions for resilience
- **Provider Diversity**: Mix different RPC providers to avoid single points of failure
- **Capacity Planning**: Ensure sufficient bandwidth for WebSocket subscriptions

### **Monitoring:**
- **Health Checks**: Monitor metrics for early detection of server issues
- **Alerting**: Set up alerts for connection failures and high error rates
- **Performance Tracking**: Monitor slot update latency and server selection patterns

This RPC package provides the robust, high-performance foundation for blockchain connectivity in Jito's MEV infrastructure, ensuring optimal routing and maximum availability through intelligent load balancing and comprehensive failover mechanisms.