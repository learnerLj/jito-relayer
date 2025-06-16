# Jito Protos Package

The `jito-protos` package serves as the central protocol buffer definition library for Jito's MEV infrastructure. It defines all gRPC services, message formats, and communication protocols used across the entire Jito relayer ecosystem.

## Overview

This package provides:

- **gRPC Service Definitions** for authentication, transaction relaying, and MEV operations
- **Protocol Buffer Schemas** for efficient data serialization
- **Type-Safe Communication** between Jito components
- **JSON-RPC HTTP Interface** for web-based integration
- **Cross-Language Compatibility** through protobuf standards

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Client Apps   │────│   jito-protos    │────│  Jito Services  │
│ (Searchers,etc) │    │ (This Package)   │    │ (Relayer, BE)   │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │
                              ├─ gRPC Services
                              ├─ Protocol Buffers
                              ├─ JSON-RPC HTTP
                              └─ Type Conversions
```

## Build System

The package uses **tonic-build** for automatic protobuf compilation during the Rust build process.

### Build Configuration (`build.rs`)

```rust
use tonic_build::configure;

fn main() {
    configure()
        .compile(
            &[
                "protos/auth.proto",
                "protos/block.proto", 
                "protos/block_engine.proto",
                "protos/bundle.proto",
                "protos/packet.proto",
                "protos/relayer.proto",
                "protos/searcher.proto",
                "protos/shared.proto",
            ],
            &["protos"],
        )
        .unwrap();
}
```

### Generated Code Structure

Each `.proto` file generates:
- **Client stubs** for gRPC service consumption
- **Server traits** for service implementation  
- **Message types** with serialization/deserialization
- **Type conversions** for Rust interoperability

## Protocol Buffer Schemas

### 1. **Authentication** (`auth.proto`)

JWT-based authentication system with challenge-response mechanism.

**Key Components:**
- **Roles**: RELAYER, SEARCHER, VALIDATOR, SHREDSTREAM_SUBSCRIBER
- **Security**: Ed25519 cryptographic signatures with PEM keys
- **Token Lifecycle**: Challenge → Signing → Token Issuance → Refresh

**Core Messages:**
```protobuf
message GenerateAuthChallengeRequest {
  Role role = 1;
  string pubkey = 2;  // PEM encoded public key
}

message GenerateAuthTokensRequest {
  string challenge = 1;
  string client_pubkey = 2;  // PEM encoded
  string signed_challenge = 3;  // base64 encoded signature
}
```

**Authentication Flow:**
```
Client → Challenge Request → Auth Service
Client ← Challenge Response ← Auth Service  
Client → Signed Challenge → Auth Service
Client ← JWT Tokens ← Auth Service
```

### 2. **Packet Processing** (`packet.proto`)

Network packet representation for TPU transaction forwarding.

**Key Types:**
```protobuf
message Packet {
  bytes data = 1;          // Transaction data
  Meta meta = 2;           // Network metadata
}

message PacketBatch {
  repeated Packet packets = 1;
  google.protobuf.Timestamp received_at = 2;
}

message Meta {
  uint32 size = 1;         // Packet size
  string addr = 2;         // Source address
  uint32 port = 3;         // Source port
  PacketFlags flags = 4;   // Processing flags
  uint64 sender_stake = 5; // Validator stake
}
```

**Packet Flags:**
- `DISCARD`: Drop packet
- `FORWARDED`: Already forwarded
- `REPAIR`: Repair protocol packet
- `SIMPLE_VOTE_TX`: Vote transaction
- `TRACER_PACKET`: Debug tracing

### 3. **Bundle Operations** (`bundle.proto`)

MEV bundle representation and result tracking.

**Core Types:**
```protobuf
message Bundle {
  Header header = 1;
  repeated bytes transactions = 2;  // Max 5 transactions
}

message BundleResult {
  string bundle_id = 1;
  repeated bytes transactions = 2;
  uint64 slot = 3;
  BundleResultType result = 4;
  CommitmentLevel commitment_level = 5;
}
```

**Result Types:**
- `BUNDLE_RESULT_ACCEPTED`: Bundle accepted for processing
- `BUNDLE_RESULT_REJECTED`: Bundle rejected (various reasons)
- `BUNDLE_RESULT_PROCESSED`: Bundle processed on-chain
- `BUNDLE_RESULT_FINALIZED`: Bundle finalized

### 4. **Block Engine** (`block_engine.proto`)

MEV block engine integration with advanced filtering capabilities.

**Key Features:**
- **AOI/POI Filtering**: Accounts and Programs of Interest
- **Expiring Packets**: Censorship-resistant packet batches
- **Bidirectional Streaming**: Real-time communication
- **Regional Support**: Multi-region block engine connectivity

**Core Messages:**
```protobuf
message AccountsOfInterestUpdate {
  repeated string accounts = 1;  // Base58 encoded pubkeys
}

message ExpiringPacketBatch {
  PacketBatch batch = 1;
  google.protobuf.Timestamp expires_at = 2;
}
```

### 5. **Relayer Services** (`relayer.proto`)

TPU proxy configuration and packet subscription.

**Services:**
```protobuf
service Relayer {
  rpc GetTpuConfigs(GetTpuConfigsRequest) returns (GetTpuConfigsResponse);
  rpc SubscribePackets(SubscribePacketsRequest) returns (stream SubscribePacketsResponse);
}
```

**Configuration Response:**
```protobuf
message GetTpuConfigsResponse {
  repeated Socket tpu_socket = 1;      // TPU addresses
  repeated Socket tpu_forward_socket = 2;  // Forward addresses
}
```

### 6. **Searcher Interface** (`searcher.proto`)

Complete MEV searcher client interface.

**Comprehensive Services:**
```protobuf
service SearcherService {
  // Bundle management
  rpc SendBundle(SendBundleRequest) returns (SendBundleResponse);
  rpc SubscribeBundleResults(SubscribeBundleResultsRequest) returns (stream BundleResult);
  
  // Leader information
  rpc GetNextScheduledLeader(NextScheduledLeaderRequest) returns (NextScheduledLeaderResponse);
  rpc GetConnectedLeaders(ConnectedLeadersRequest) returns (ConnectedLeadersResponse);
  rpc GetConnectedLeadersRegioned(ConnectedLeadersRegionedRequest) returns (ConnectedLeadersRegionedResponse);
  
  // System information
  rpc GetTipAccounts(GetTipAccountsRequest) returns (GetTipAccountsResponse);
  rpc GetRegions(GetRegionsRequest) returns (GetRegionsResponse);
}
```

### 7. **Shared Types** (`shared.proto`)

Common types used across all services.

**Essential Types:**
```protobuf
message Header {
  google.protobuf.Timestamp ts = 1;
}

message Heartbeat {
  uint64 count = 1;
}

message Socket {
  string ip = 1;
  uint32 port = 2;
}
```

## gRPC Services by Category

### **Authentication Layer**
- **AuthService**: Centralized authentication for all client types
  - Challenge generation and validation
  - JWT token issuance and refresh
  - Role-based access control

### **Transaction Processing Layer**
- **Relayer**: TPU proxy services for validators
  - Socket configuration management
  - Packet subscription and forwarding
  
- **BlockEngineValidator**: Bundle and packet streaming to validators
  - Bundle subscription for block building
  - Packet streaming with priority handling
  - Fee information for block builders

### **MEV Integration Layer**  
- **BlockEngineRelayer**: Account filtering and intelligent packet forwarding
  - Accounts of Interest (AOI) subscription
  - Programs of Interest (POI) subscription
  - Expiring packet streams for censorship resistance

- **SearcherService**: Complete MEV searcher interface
  - Bundle submission and result tracking
  - Leader schedule information
  - Regional connectivity management
  - Tip account information

### **Infrastructure Layer**
- **Shredstream**: Data replication and regional distribution
  - Regional heartbeat management
  - Shred data streaming

## JSON-RPC HTTP Interface

In addition to gRPC, the system provides **JSON-RPC 2.0 HTTP endpoints** for web integration:

### Available Methods

**Bundle Operations:**
```bash
# Submit bundle (up to 5 transactions)
curl https://mainnet.block-engine.jito.wtf/api/v1/bundles -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "sendBundle",
  "params": [["<transaction1_base58>", "<transaction2_base58>"]]
}'

# Get bundle status
curl https://mainnet.block-engine.jito.wtf/api/v1/bundles -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0", 
  "id": 1,
  "method": "getBundleStatuses",
  "params": [["<bundle_id>"]]
}'

# Get tip accounts
curl https://mainnet.block-engine.jito.wtf/api/v1/bundles -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1, 
  "method": "getTipAccounts",
  "params": []
}'
```

## Integration with Other Packages

### **Direct Consumers:**
- **`block_engine/`**: Implements BlockEngineValidator and BlockEngineRelayer services
- **`relayer/`**: Implements Relayer service and authentication functionality  
- **`transaction-relayer/`**: Main binary that orchestrates all gRPC servers
- **`web/`**: HTTP server that may expose simplified endpoints

### **Type Conversion (`convert.rs`)**

Provides seamless conversion between Solana types and protobuf types:

```rust
// Convert Solana packet to protobuf packet
pub fn packet_to_proto_packet(packet: &Packet) -> proto::Packet {
    proto::Packet {
        data: packet.data(..).to_vec(),
        meta: Some(proto::Meta {
            size: packet.meta().size,
            addr: packet.meta().socket_addr().ip().to_string(),
            port: packet.meta().socket_addr().port() as u32,
            flags: packet.meta().flags().bits(),
            sender_stake: packet.meta().sender_stake(),
        }),
    }
}
```

## Usage Examples

### **Client Code Generation**

```rust
// Generated client usage
use jito_protos::searcher::searcher_service_client::SearcherServiceClient;

let mut client = SearcherServiceClient::connect("https://mainnet.block-engine.jito.wtf").await?;

// Send bundle
let response = client.send_bundle(SendBundleRequest {
    bundle: Some(Bundle {
        header: Some(Header { ts: Some(timestamp) }),
        transactions: vec![transaction_bytes],
    }),
}).await?;
```

### **Server Implementation**

```rust
use jito_protos::relayer::{Relayer, GetTpuConfigsRequest, GetTpuConfigsResponse};

#[tonic::async_trait]
impl Relayer for MyRelayerService {
    async fn get_tpu_configs(
        &self,
        request: Request<GetTpuConfigsRequest>,
    ) -> Result<Response<GetTpuConfigsResponse>, Status> {
        // Implementation
    }
}
```

## Protocol Design Principles

### **Streaming-First Architecture**
- Heavy use of server-streaming and bidirectional streaming
- Real-time data flow for low-latency MEV operations
- Heartbeat mechanisms for connection health

### **Regional Distribution**  
- Multi-region support with explicit region handling
- Regional load balancing and failover capabilities
- Location-aware service discovery

### **Censorship Resistance**
- Expiring packet batches prevent MEV censorship
- Redundant pathways for critical transactions
- Decentralized block building infrastructure

### **Security and Compliance**
- JWT-based authentication with cryptographic proofs
- Role-based access control
- OFAC compliance integration points

This package forms the communication backbone of Jito's MEV infrastructure, enabling type-safe, efficient, and feature-rich interactions between all system components.