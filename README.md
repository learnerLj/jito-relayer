# Jito Relayer

⚡ **Low Latency Transaction Send**

Jito provides Solana MEV users with superior transaction execution through fast landing, MEV protection, and revert protection, available for both single transactions and multiple transactions (bundles) via gRPC and JSON-RPC services, ensuring optimal performance in the highly competitive Solana ecosystem.

## What is Jito Relayer?

Jito Relayer acts as a transaction processing unit (TPU) proxy for Solana validators, serving as a critical component in Jito's MEV infrastructure. It forwards transactions to validators while integrating with the block engine for MEV bundle processing.

## 🌐 How does the system work?

1. **Validators** run a modified Agave validator client called Jito-Solana that enables higher value capture for them and their stakers
2. **The validator** then connects to the Jito Block-Engine and Jito-Relayer
3. **The Block-Engine** submits profitable bundles to the validator
4. **The Relayer** acts as a proxy to filter and verify transactions for validators
5. **Searchers, dApps, Telegram bots** and others connect to the Block-Engine and submit transactions & bundles
6. **Submissions** can be over gRPC or JSON-RPC
7. **Bundles** have tips bids associated with them; these bids are then redistributed to the validators and their stakers

## 💼 What are bundles?

- **Bundles** are groups of transactions (max 5) bundled together
- **Sequential Execution**: The transactions are executed sequentially and atomically meaning all-or-nothing
- **MEV Strategies**: Bundles enable complex MEV strategies like atomic arbitrage
- **Competition**: Bundles compete against other bundles on tips to the validator

## 🔄 How do Bundles work?

1. Traders submit bundle to block engines
2. Block engines simulate bundles to determine the most profitable combinations
3. Winning bundles are sent to validators to include in blocks
4. Validators execute bundles atomically and collect tips
5. MEV rewards from bundles are distributed to validators and stakers

## ⚖️ What is the auction?

- **Priority Auction**: Bundles submitted by traders are put through a priority auction
- **Scarcity**: An auction is needed since opportunities and blockspace are scarce
- **Optimization**: The auction creates a stream of bundles that maximizes tips in a block
- **Parallelism**: Parallelism in locking patterns is leveraged where possible to allow for local state auctions
- **Timing**: Parallel auctions are run at 200ms ticks
- **Bundle Selection**: Jito submits the highest paying combination of bundles to the validator up to some CU limit

# Building
```shell
# pull submodules to get protobuffers required to connect to Block Engine and validator
$ git submodule update --init --recursive
# build from source
$ cargo build --release

run --bin jito-transaction-relayer -- \
    --keypair-path keypair.json \
    --signing-key-pem-path signing_key.pem \
    --verifying-key-pem-path verifying_key.pem \
    --rpc-servers https://api.mainnet-beta.solana.com \
    --websocket-servers wss://api.mainnet-beta.solana.com
```

# Releases

## Making a release

We opt to use cargo workspaces for making releases.
First, install cargo workspaces by running: `cargo install cargo-workspaces`.
Next, check out the master branch of the jito-relayer repo and 
ensure you're on the latest commit.
In the master branch, run the following command and follow the instructions:
```shell
$ ./release
```
This will bump all the versions of the packages in your repo, 
push to master and tag a new commit.

## Running a release
There are two options for running the relayer from releases:
- Download the most recent release on the [releases](https://github.com/jito-foundation/jito-relayer/releases) page.
- (Not recommended for production): One can download and run Docker containers from the Docker [registry](https://hub.docker.com/r/jitolabs/jito-transaction-relayer).

# API Documentation

## Block Engine Endpoints

You can send JSON-RPC requests to any Block Engine using the following URLs. To route to a specific region, specify the desired region:

| Location | Block Engine URL | Shred Receiver | Relayer URL | NTP Server |
|----------|-----------------|----------------|-------------|------------|
| 🌍 🌎 🌏 **Mainnet** | https://mainnet.block-engine.jito.wtf | - | - | - |
| 🇳🇱 **Amsterdam** | https://amsterdam.mainnet.block-engine.jito.wtf | 74.118.140.240:1002 | http://amsterdam.mainnet.relayer.jito.wtf:8100 | ntp.amsterdam.jito.wtf |
| 🇩🇪 **Frankfurt** | https://frankfurt.mainnet.block-engine.jito.wtf | 64.130.50.14:1002 | http://frankfurt.mainnet.relayer.jito.wtf:8100 | ntp.frankfurt.jito.wtf |
| 🇬🇧 **London** | https://london.mainnet.block-engine.jito.wtf | 142.91.127.175:1002 | http://london.mainnet.relayer.jito.wtf:8100 | ntp.london.jito.wtf |
| 🇺🇸 **New York** | https://ny.mainnet.block-engine.jito.wtf | 141.98.216.96:1002 | http://ny.mainnet.relayer.jito.wtf:8100 | ntp.dallas.jito.wtf |
| 🇺🇸 **Salt Lake City** | https://slc.mainnet.block-engine.jito.wtf | 64.130.53.8:1002 | http://slc.mainnet.relayer.jito.wtf:8100 | ntp.slc.jito.wtf |
| 🇸🇬 **Singapore** | https://singapore.mainnet.block-engine.jito.wtf | 202.8.11.224:1002 | http://singapore.mainnet.relayer.jito.wtf:8100 | ntp.singapore.jito.wtf |
| 🇯🇵 **Tokyo** | https://tokyo.mainnet.block-engine.jito.wtf | 202.8.9.160:1002 | http://tokyo.mainnet.relayer.jito.wtf:8100 | ntp.tokyo.jito.wtf |
| 🌍 🌎 🌏 **Testnet** | https://testnet.block-engine.jito.wtf | - | - | - |
| 🇺🇸 **Dallas** | https://dallas.testnet.block-engine.jito.wtf | 141.98.218.12:1002 | http://dallas.testnet.relayer.jito.wtf:8100 | ntp.dallas.jito.wtf |
| 🇺🇸 **New York** | https://ny.testnet.block-engine.jito.wtf | 64.130.35.224:1002 | http://ny.testnet.relayer.jito.wtf:8100 | ntp.dallas.jito.wtf |

## 📨 Transactions (/api/v1/transactions)

For single transaction-related methods, use the URL path `/api/v1/transactions`

### sendTransaction

This method forwards transactions directly to validators through the local relayer proxy, providing MEV protection and enhanced transaction processing.

**Key Features:**
- Direct proxy to validator TPU endpoints
- MEV protection and revert protection
- Bypasses standard RPC mempool for faster execution
- Automatic `skip_preflight=true` setting

**Important Notes:**
- Minimum tip requirement: 1000 lamports
- Recommended 70/30 split: Priority fee (70%) + Jito tip (30%)
- Enable revert protection with `bundleOnly=true` query parameter
- Transactions are forwarded to current slot leader

**Local Relayer Integration:**
The local relayer acts as a proxy between clients and validators:
1. Client sends transaction to local relayer (port 11228)
2. Relayer authenticates and processes the transaction
3. Relayer forwards to appropriate validator via QUIC
4. Validator processes transaction with MEV protection

**Request Parameters:**
- `params` (string, required): Signed transaction as base64 (recommended) or base58 (deprecated)
- `encoding` (string, optional): Transaction encoding format. Default: base58

**Example Request to Local Relayer:**
```bash
# First, ensure your local relayer is running on port 11226
curl http://localhost:11226/api/v1/transactions -X POST -H "Content-Type: application/json" -d '{
  "id": 1,
  "jsonrpc": "2.0",
  "method": "sendTransaction",
  "params": [
    "AVXo5X7UNzpuOmYzkZ+fqHDGiRLTSMlWlUCcZKzEV5CIKlrdvZa3/2GrJJfPrXgZqJbYDaGiOnP99tI/sRJfiwwBAAEDRQ/n5E5CLbMbHanUG3+iVvBAWZu0WFM6NoB5xfybQ7kNwwgfIhv6odn2qTUu/gOisDtaeCW1qlwW/gx3ccr/4wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAvsInicc+E3IZzLqeA+iM5cn9kSaeFzOuClz1Z2kZQy0BAgIAAQwCAAAAAPIFKgEAAAA=",
    {
      "encoding": "base64"
    }
  ]
}'
```

**Alternative: Direct Block Engine (Production):**
```bash
# For production, use hosted block engine
curl https://mainnet.block-engine.jito.wtf/api/v1/transactions -X POST -H "Content-Type: application/json" -d '{
  "id": 1,
  "jsonrpc": "2.0",
  "method": "sendTransaction",
  "params": [
    "AVXo5X7UNzpuOmYzkZ+fqHDGiRLTSMlWlUCcZKzEV5CIKlrdvZa3/2GrJJfPrXgZqJbYDaGiOnP99tI/sRJfiwwBAAEDRQ/n5E5CLbMbHanUG3+iVvBAWZu0WFM6NoB5xfybQ7kNwwgfIhv6odn2qTUu/gOisDtaeCW1qlwW/gx3ccr/4wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAvsInicc+E3IZzLqeA+iM5cn9kSaeFzOuClz1Z2kZQy0BAgIAAQwCAAAAAPIFKgEAAAA=",
    {
      "encoding": "base64"
    }
  ]
}'
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "result": "2id3YC2jK9G5Wo2phDx4gJVAew8DcY5NAojnVuao8rkxwPYPe8cSwE5GzhEgJA2y8fVjDEo6iR6ykBvDxrTQrtpb",
  "id": 1
}
```

**Authentication (if required):**
```bash
# Include authentication header for private relayers
curl http://localhost:11226/api/v1/transactions -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_JWT_TOKEN" \
  -d '{ ... }'
```

## 💼 Bundles (/api/v1)

Bundles are a list of up to 5 transactions that execute sequentially and atomically, ensuring an all-or-nothing outcome.

### sendBundle

Submits a bundled list of signed transactions to the cluster for processing. The transactions are atomically processed in order, meaning if any transaction fails, the entire bundle is rejected.

**Requirements:**
- Maximum of 5 transactions per bundle
- A tip is necessary for the bundle to be considered
- Use `getTipAccounts` to retrieve tip accounts

**Example Request (base64):**
```bash
curl https://mainnet.block-engine.jito.wtf:443/api/v1/bundles -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "sendBundle",
  "params": [
    [
      "AT2AqtlokikUWgGNnSX5xrmdvBjSaiIPxvFz6zc5Abn5Z0CPFW5GO+Y3rXceLnqLgQFnGw0yTk3NtJdFNsbrwwQBAAIEsXPDJ9cMVbpFQYClVM7PGLh8JOfCD6E2vz5VNmBCF+p4Uhyxec67hYm1VqLV7JTSSYaC/fm7KvWtZOSRzEFT2gAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABUpTWpkpIQZNJOhxYNo4fHw1td28kruB5B+oQEEFRI1i3Wzl2VfewCI8oYXParnP78725sKFzYheTEn8v865YQIDABhqaXRvIGJ1bmRsZSAwOiBqaXRvIHRlc3QCAgABDAIAAACghgEAAAAAAA==",
      "AS6fOZuGDsmyYdd+RC0fiFUgNe1BYTOYT+1hkRXHAeroC8R60h3g34EPF5Ys8sGzVBMP9MDSTVgy1/SSTqpCtA4BAAIEsXPDJ9cMVbpFQYClVM7PGLh8JOfCD6E2vz5VNmBCF+p4Uhyxec67hYm1VqLV7JTSSYaC/fm7KvWtZOSRzEFT2gAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABUpTWpkpIQZNJOhxYNo4fHw1td28kruB5B+oQEEFRI1i3Wzl2VfewCI8oYXParnP78725sKFzYheTEn8v865YQIDABhqaXRvIGJ1bmRsZSAxOiBqaXRvIHRlc3QCAgABDAIAAACghgEAAAAAAA=="
    ],
    {
      "encoding": "base64"
    }
  ]
}'
```

### getBundleStatuses

Returns the status of submitted bundle(s). If a bundle_id is not found or has not landed, it returns null.

**Example Request:**
```bash
curl https://mainnet.block-engine.jito.wtf/api/v1/getBundleStatuses -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "getBundleStatuses",
  "params": [
    [
      "892b79ed49138bfb3aa5441f0df6e06ef34f9ee8f3976c15b323605bae0cf51d"
    ]
  ]
}'
```

### getTipAccounts

Retrieves the tip accounts designated for tip payments for bundles.

**Example Request:**
```bash
curl https://mainnet.block-engine.jito.wtf/api/v1/getTipAccounts -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "getTipAccounts",
  "params": []
}'
```

**Response:**
```json
{    
  "jsonrpc": "2.0",
  "result": [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT"
  ],
  "id": 1
}
```

## 💸 Tips and Pricing

### Tip Amounts

**For sendTransaction:**
- Use a 70/30 split between priority fee and jito tip
- Example: Priority Fee (70%): 0.7 SOL + Jito Tip (30%): 0.3 SOL = Total: 1.0 SOL

**For sendBundle:**
- Only the Jito tip matters
- Minimum tip: 1000 lamports

### Get Tip Information

**REST API endpoint:**
```bash
curl https://bundles.jito.wtf/api/v1/bundles/tip_floor
```

**WebSocket:**
```bash
wscat -c wss://bundles.jito.wtf/api/v1/bundles/tip_stream
```

## 🛡️ Sandwich Mitigation

Add any valid Solana public key that starts with `jitodontfront` to any instruction to prevent sandwich attacks:
- Example: `jitodontfront111111111111111111111111111111`
- Account doesn't need to exist on-chain but must be a valid pubkey
- Mark the account as read-only for optimal performance
- Works with both `sendBundle` and `sendTransaction` endpoints

## 📊 Rate Limits

- **Default**: 1 request per second per IP per region
- **Exceeding limits**: 429 rate limit error
- **Authentication**: Contact Jito for higher rate limits

## 🔧 Getting Started

### SDKs Available:
- **Python**: [Jito Py JSON-RPC](https://github.com/jito-foundation/jito-py)
- **JavaScript/TypeScript**: [Jito JS JSON-RPC](https://github.com/jito-foundation/jito-js)
- **Rust**: [Jito Rust JSON-RPC](https://github.com/jito-foundation/jito-rust)
- **Go**: [Jito Go JSON-RPC](https://github.com/jito-foundation/jito-go)

# Running a Relayer
See https://jito-foundation.gitbook.io/mev/jito-relayer/running-a-relayer for setup and usage instructions.

## Quick Start Guide

### 1. Generate Required Keys
```bash
# Generate Solana keypair
solana-keygen new --no-bip39-passphrase --outfile keypair.json

# Generate Ed25519 signing key
openssl genpkey -algorithm Ed25519 -out signing_key.pem

# Extract public key for verification
openssl pkey -in signing_key.pem -pubout -out verifying_key.pem
```

### 2. Run the Relayer
```bash
# For local development (requires local Solana validator)
cargo run --bin jito-transaction-relayer -- \
    --keypair-path keypair.json \
    --signing-key-pem-path signing_key.pem \
    --verifying-key-pem-path verifying_key.pem

# For mainnet connection
cargo run --bin jito-transaction-relayer -- \
    --keypair-path keypair.json \
    --signing-key-pem-path signing_key.pem \
    --verifying-key-pem-path verifying_key.pem \
    --rpc-servers https://api.mainnet-beta.solana.com \
    --websocket-servers wss://api.mainnet-beta.sola
    na.com
```

### 3. Port Configuration
The relayer uses the following ports by default:
- **11226** - gRPC API server (authentication, client connections)
- **11228** - TPU QUIC socket (receives transactions from clients)
- **11229** - TPU Forward QUIC socket (forwards transactions to validators)
- **11227** - Web server (HTTP endpoints)

### 4. Filter Logs
To reduce verbose metrics logging:
```bash
cargo run --bin jito-transaction-relayer -- [args] 2>&1 | grep -v "metrics"
```

### 5. Troubleshooting
Common connection errors when running locally:
- `Connection refused (127.0.0.1:8899)` - No local Solana RPC server running
- `Connection refused (127.0.0.1:8900)` - No local Solana WebSocket server running
- `Couldn't Get Epoch Info from RPC` - Unable to fetch validator schedule

Solution: Either start a local Solana validator (`solana-test-validator`) or use mainnet RPC endpoints.

# Knowledge Base

## Understanding Preflight in Solana Transactions

**Preflight** is Solana's transaction validation mechanism that simulates and validates transactions before actual submission to the network.

### What is Preflight?

Preflight is a pre-execution validation process that:

1. **Simulates the transaction** against current blockchain state
2. **Checks for errors** like insufficient funds, invalid instructions, or program failures
3. **Estimates compute units** required for execution
4. **Validates account permissions** and ownership
5. **Returns detailed simulation results** including logs and state changes

### Standard Solana RPC Preflight Process

```bash
# Standard RPC call with preflight (default behavior)
curl -X POST https://api.mainnet-beta.solana.com -H "Content-Type: application/json" -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "sendTransaction",
  "params": [
    "<base64_transaction>",
    {
      "skipPreflight": false,  # Default: runs simulation first
      "encoding": "base64"
    }
  ]
}'
```

**Standard flow with preflight:**
1. RPC node receives transaction
2. **Simulation runs** against current state
3. If simulation fails → transaction is rejected
4. If simulation succeeds → transaction is submitted to TPU
5. Returns simulation results or errors

### Why Jito Skips Preflight

Jito automatically sets `skipPreflight=true` for several critical reasons:

#### **1. Performance Optimization**
```
With Preflight:    Client → RPC → Simulate → Validate → TPU → Leader
Without Preflight: Client → RPC → TPU → Leader (Direct)
```
- **Reduced Latency**: Eliminates simulation step that can take 100-500ms
- **Faster Execution**: Direct submission to validator TPU endpoints
- **MEV Advantage**: Critical for time-sensitive MEV opportunities

#### **2. MEV-Specific Requirements**
- **Bundle Atomicity**: MEV bundles may fail individually but succeed as a group
- **State Dependencies**: Transactions in bundles depend on each other's execution
- **Simulation Inaccuracy**: Preflight simulation doesn't account for bundle context

#### **3. Fresh State Access**
- **Current Leader Targeting**: Sends directly to current slot leader
- **Reduced Congestion**: Avoids RPC node bottlenecks
- **Real-time Execution**: No waiting in RPC node queues

### Preflight vs. Skip Preflight Comparison

| Aspect | With Preflight | Skip Preflight (Jito) |
|--------|----------------|------------------------|
| **Latency** | Higher (simulation delay) | Lower (direct submission) |
| **Error Detection** | Early validation | Runtime detection |
| **MEV Suitability** | Poor (state assumptions) | Optimal (real execution) |
| **Bundle Support** | Limited (individual tx) | Full (atomic bundles) |
| **Throughput** | Lower (simulation overhead) | Higher (direct path) |

### Trade-offs of Skipping Preflight

#### **Benefits:**
- **Faster transaction submission** for time-sensitive MEV
- **Bundle compatibility** with atomic execution
- **Direct validator communication** bypassing RPC bottlenecks
- **Reduced computational overhead** on transaction processing

#### **Considerations:**
- **No early error detection** - invalid transactions fail at execution
- **Potential fee loss** if transactions fail after submission
- **Higher responsibility** on client to validate transactions
- **Less detailed error reporting** compared to simulation

### Example: MEV Bundle Context

```rust
// MEV Bundle with interdependent transactions
Bundle {
  tx1: "Borrow 100 SOL from lending protocol",
  tx2: "Swap SOL→USDC on DEX A", 
  tx3: "Swap USDC→SOL on DEX B",
  tx4: "Repay 100 SOL + profit to lending protocol"
}
```

**With Preflight:**
- `tx1` simulation: ✅ Pass (can borrow)
- `tx2` simulation: ❌ Fail (insufficient balance, doesn't see tx1 effect)
- Bundle rejected despite being valid as a group

**Without Preflight (Jito):**
- All transactions submitted as atomic bundle
- Execute sequentially with state changes
- Bundle succeeds with profitable arbitrage

### Jito's Implementation

Jito hardcodes `skipPreflight=true` because:

1. **MEV Infrastructure**: Designed for MEV use cases where preflight is counterproductive
2. **Performance Critical**: Every millisecond matters in MEV extraction
3. **Bundle-First Design**: Architecture assumes atomic bundle execution
4. **Direct TPU Access**: Bypasses traditional RPC limitations

This design choice makes Jito optimal for MEV searchers and other performance-critical applications while requiring users to handle transaction validation on the client side.


