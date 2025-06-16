# Relayer Package

The `relayer` package implements the core TPU (Transaction Processing Unit) proxy functionality for Jito's MEV infrastructure. It provides secure, authenticated transaction forwarding between clients and Solana validators with comprehensive health monitoring and performance optimization.

## Overview

The relayer serves as the central hub for transaction forwarding by providing:

- **Secure Authentication** with JWT-based challenge-response mechanisms
- **Intelligent Packet Routing** based on leader schedules and health status
- **OFAC Compliance** filtering for regulatory adherence
- **Health Monitoring** with automatic degradation and recovery
- **Performance Optimization** through caching and connection management
- **Comprehensive Metrics** for monitoring and observability

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Client Apps   │────│    Relayer       │────│   Validators    │
│  (Searchers)    │    │ (This Package)   │    │  (TPU Proxy)    │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │
                              ├─ Authentication
                              ├─ Health Monitoring
                              ├─ Leader Schedule
                              ├─ OFAC Filtering
                              └─ Metrics Collection
```

## Core Components

### 1. **Authentication System** (`auth_*.rs`)

Implements a comprehensive JWT-based authentication system with DoS protection.

#### **Challenge Management** (`auth_challenges.rs`)

Manages authentication challenges with automatic expiration and DoS mitigation.

```rust
pub struct AuthChallenge {
    challenge: String,           // Random alphanumeric challenge
    claims: Claims,              // JWT claims (IP, pubkey, exp)
    expires_at: Instant,         // Challenge expiration time
}

pub struct AuthChallenges {
    challenges: KeyedPriorityQueue<IpAddr, AuthChallenge>,  // One challenge per IP
}
```

**Key Features:**
- **DoS Protection**: Maximum one active challenge per IP address
- **Automatic Expiration**: Configurable challenge TTL with cleanup
- **Memory Management**: Priority queue automatically removes expired challenges

#### **JWT Interceptor** (`auth_interceptor.rs`)

gRPC interceptor that validates JWT tokens on every request.

```rust
pub struct Claims {
    client_ip: String,           // Client IP address
    client_pubkey: String,       // Client's public key
    exp: usize,                  // Token expiration timestamp
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        // Extract Bearer token from Authorization header
        // Verify JWT signature using RS256
        // Inject validated pubkey into request extensions
    }
}
```

**Security Features:**
- **Bearer Token Extraction**: Parses Authorization headers
- **JWT Signature Verification**: RS256 algorithm validation
- **Claims Validation**: Checks expiration and client IP
- **Request Enhancement**: Injects validated pubkey for downstream use

#### **Authentication Service** (`auth_service.rs`)

Complete authentication service implementing the AuthService gRPC interface.

**Authentication Flow:**
```
1. Challenge Request    → Generate random challenge per IP
2. Challenge Response   ← Return challenge string
3. Token Request        → Verify Ed25519 signature of challenge
4. Token Response       ← Issue access + refresh JWT tokens
5. Token Refresh        → Use refresh token to get new access token
6. Refreshed Response   ← New access token
```

**Core Methods:**
```rust
// Generate authentication challenge
async fn generate_auth_challenge(
    &self,
    request: Request<GenerateAuthChallengeRequest>,
) -> Result<Response<GenerateAuthChallengeResponse>, Status>

// Issue JWT tokens after signature verification
async fn generate_auth_tokens(
    &self,
    request: Request<GenerateAuthTokensRequest>,
) -> Result<Response<GenerateAuthTokensResponse>, Status>

// Refresh access token using refresh token
async fn refresh_access_token(
    &self,
    request: Request<RefreshAccessTokenRequest>,
) -> Result<Response<RefreshAccessTokenResponse>, Status>
```

**Security Implementation:**
- **Ed25519 Signature Verification**: Validates client signatures against challenges
- **Health Checks**: Prevents authentication when relayer is unhealthy
- **Validator Authorization**: Integrates with ValidatorAuther trait for pubkey validation
- **Token Lifecycle Management**: Separate TTLs for access vs refresh tokens

### 2. **Health Management** (`health_manager.rs`)

Monitors system health based on Solana slot updates and manages system state.

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HealthState {
    Unhealthy,
    Healthy,
}

pub struct HealthManager {
    state: Arc<AtomicU8>,                    // Atomic health state
    last_slot_update: Arc<AtomicU64>,        // Last seen slot
    missing_slot_unhealthy_secs: u64,        // Unhealthy threshold
}
```

**Health Monitoring Logic:**
```rust
impl HealthManager {
    // Start background health monitoring
    pub fn start_tracking_health(&self, slot_receiver: Receiver<Slot>) {
        // Update last_slot_update on every received slot
        // Mark unhealthy if slots stop arriving within threshold
        // Provide metrics on health state changes
    }
    
    // Check if system is currently healthy
    pub fn is_healthy(&self) -> bool {
        // Returns current health state
    }
}
```

**Impact on Operations:**
- **Authentication**: Blocks new auth challenges when unhealthy
- **Packet Forwarding**: Drops all connections when unhealthy
- **Metrics**: Reports health state for monitoring

### 3. **Leader Schedule Cache** (`schedule_cache.rs`)

Maintains up-to-date validator leader schedules for optimal packet routing.

```rust
pub struct LeaderScheduleCacheUpdater {
    rpc_client: Arc<RpcLoadBalancer>,
    schedule: Arc<RwLock<HashMap<u64, Pubkey>>>,  // Slot -> Leader mapping
}
```

**Schedule Management:**
```rust
impl LeaderScheduleCacheUpdater {
    // Background task updating schedule every 10 seconds
    pub async fn start_update_leader_schedule_cache(&self) {
        // Fetch epoch info and leader schedule from RPC
        // Build slot-to-leader mapping for current epoch
        // Update shared schedule cache atomically
    }
    
    // Get current slot leader
    pub fn get_current_leader(&self, slot: Slot) -> Option<Pubkey> {
        // Lookup leader for given slot
    }
}
```

**Optimization Benefits:**
- **Efficient Routing**: Direct packet forwarding to current slot leaders
- **Reduced Latency**: Local cache avoids RPC calls on hot path
- **Future Planning**: Supports routing for upcoming slots

### 4. **Core Relayer Implementation** (`relayer.rs`)

The main packet forwarding engine with comprehensive connection and packet management.

#### **Packet Forwarding Logic**

```rust
impl RelayerService {
    // Main packet subscription handler
    async fn subscribe_packets(
        &self,
        request: Request<SubscribePacketsRequest>,
    ) -> Result<Response<Self::SubscribePacketsStream>, Status> {
        // Validate authentication and health
        // Establish packet streaming connection
        // Start packet forwarding loop
    }
    
    // Core packet forwarding function
    fn forward_packets_to_leaders(
        &self,
        packet_batch: &PacketBatch,
        validator_pubkey: &Pubkey,
    ) -> usize {
        // Filter OFAC-related transactions
        // Route packets based on leader schedule
        // Update forwarding metrics
    }
}
```

#### **Connection Management**

**Subscription Lifecycle:**
```
1. Client Authentication  → Validate JWT token
2. Health Check          → Verify system health
3. Connection Setup      → Create packet stream
4. Packet Forwarding     → Route packets to leaders
5. Metrics Collection    → Track performance stats
6. Connection Cleanup    → Remove on disconnect
```

**Connection Limits:**
- **Health-Based**: Immediately drop all connections when unhealthy
- **Authentication**: Only authenticated clients can subscribe
- **Resource Management**: Track active connections and packets

#### **OFAC Compliance Integration**

```rust
// Filter out OFAC-related transactions
fn filter_ofac_packets(
    &self,
    packet_batch: &PacketBatch,
) -> Vec<Packet> {
    packet_batch.packets
        .iter()
        .filter(|packet| {
            !is_tx_ofac_related(
                &packet.transaction,
                &self.ofac_addresses,
                &self.address_lookup_table_cache,
            )
        })
        .cloned()
        .collect()
}
```

#### **Comprehensive Metrics**

The relayer provides extensive metrics for monitoring:

```rust
pub struct RelayerStats {
    // Connection metrics
    pub num_added_connections: AtomicU64,
    pub num_removed_connections: AtomicU64,
    pub num_current_connections: AtomicU64,
    
    // Packet metrics
    pub packet_latencies_us: Histogram,
    pub num_relayer_packets_forwarded: AtomicU64,
    
    // Performance metrics
    pub crossbeam_subscription_receiver_processing_us: Histogram,
    pub slot_receiver_len: AtomicUsize,
    pub packet_subscriptions_total_queued: AtomicU64,
    
    // Health metrics
    pub highest_slot: AtomicU64,
    pub num_heartbeats: AtomicU64,
}
```

## Integration Points

### **With Other Packages:**

#### **`jito-core`:**
- **OFAC Filtering**: Uses `is_tx_ofac_related()` for transaction compliance
- **TPU Integration**: Leverages core TPU infrastructure

#### **`jito-protos`:**
- **gRPC Services**: Implements AuthService and Relayer service definitions
- **Message Types**: Uses protobuf types for all communication

#### **`jito-rpc`:**
- **RPC Load Balancing**: Uses RpcLoadBalancer for Solana network queries
- **Schedule Updates**: Fetches leader schedules via RPC

#### **`transaction-relayer`:**
- **Main Binary Integration**: Provides gRPC servers for the main application
- **Health Manager**: Shared health state across application

### **External Dependencies:**

- **Solana Network**: RPC connections for leader schedules and epoch info
- **JWT Infrastructure**: RSA keys for token signing and verification
- **Ed25519 Cryptography**: Client signature verification

## Configuration

### **Authentication Configuration:**
```rust
pub struct AuthServiceConfig {
    pub challenge_ttl_secs: u64,           // Challenge expiration time
    pub access_token_ttl_secs: u64,        // Access token lifetime
    pub refresh_token_ttl_secs: u64,       // Refresh token lifetime
}
```

### **Health Configuration:**
```rust
pub struct HealthConfig {
    pub missing_slot_unhealthy_secs: u64,  // Health threshold
    pub challenge_expiration_sleep_interval_secs: u64,  // Cleanup interval
}
```

### **Performance Tuning:**
```rust
// Channel capacities
const PACKET_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 100;
const SLOT_RECEIVER_CHANNEL_CAPACITY: usize = 100;
const DELAY_PACKET_RECEIVER_CHANNEL_CAPACITY: usize = 10_000;

// Update intervals
const LEADER_SCHEDULE_UPDATE_INTERVAL_SECS: u64 = 10;
const CHALLENGE_CLEANUP_INTERVAL_SECS: u64 = 180;
```

## Usage Example

```rust
use jito_relayer::{RelayerService, AuthService, HealthManager};

// Create health manager
let health_manager = HealthManager::new(missing_slot_unhealthy_secs);

// Create authentication service
let auth_service = AuthService::new(
    auth_service_config,
    health_manager.clone(),
    validator_auther,
    signing_key,
);

// Create relayer service
let relayer_service = RelayerService::new(
    health_manager,
    schedule_cache,
    ofac_addresses,
    packet_sender,
    stats,
);

// Start gRPC servers
let auth_server = AuthServiceServer::new(auth_service);
let relayer_server = RelayerServer::new(relayer_service);
```

## Security Considerations

### **Authentication Security:**
- **Challenge Uniqueness**: Random 9-character alphanumeric challenges
- **Signature Verification**: Ed25519 cryptographic validation
- **Token Security**: RS256 JWT signing with proper claims validation
- **DoS Protection**: One challenge per IP with automatic expiration

### **Network Security:**
- **Health-Based Access Control**: All connections dropped when unhealthy
- **OFAC Compliance**: Automatic filtering of sanctioned transactions
- **Connection Limits**: Resource-based connection management

### **Operational Security:**
- **Comprehensive Logging**: Detailed error and operation logging
- **Metrics Collection**: Performance and security metrics
- **Graceful Degradation**: Health-aware operation modes

This relayer package provides the secure, high-performance foundation for Jito's transaction forwarding infrastructure, ensuring reliable and compliant packet delivery to Solana validators.