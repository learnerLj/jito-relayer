# Web Package

The `jito-relayer-web` package provides HTTP monitoring and diagnostic endpoints for the Jito Relayer system. It offers REST APIs that expose health status and operational state, enabling external monitoring systems, load balancers, and administrators to assess the relayer's operational health.

## Overview

The web package serves as the operational visibility layer by providing:

- **Health Check Endpoints** for load balancer integration
- **Detailed Status APIs** for monitoring dashboards  
- **Rate Limiting Protection** against endpoint abuse
- **Real-time State Monitoring** of all critical components
- **JSON-formatted Responses** for programmatic consumption

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│  Monitoring     │────│   Web Server     │────│   Relayer       │
│  Systems        │    │ (This Package)   │    │   Core State    │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                              │                        │
                              ├─ /health              ├─ Slot Health
                              ├─ /status              ├─ Block Engine
                              ├─ Rate Limiting        └─ Validator Connections
                              └─ Error Handling
```

## HTTP Endpoints

### 1. **Root Endpoint** (`/`)

Simple service identification endpoint.

**Request:**
```bash
curl http://localhost:11227/
```

**Response:**
```
jito relayer
```

**Use Cases:**
- Basic connectivity testing
- Service identification
- Load balancer basic health checks

### 2. **Health Endpoint** (`/health`)

Binary health status for automated systems.

**Request:**
```bash
curl http://localhost:11227/health
```

**Response:**
```
ok          # When system is healthy
unhealthy   # When system has issues
```

**Health Logic:**
```rust
async fn get_health(Extension(state): Extension<Arc<RelayerState>>) -> String {
    let slots_healthy = *state.slot_health.read().unwrap() == HealthState::Healthy;
    let is_connected_to_block_engine = state.is_connected_to_block_engine.load(Ordering::Relaxed);
    
    if slots_healthy && is_connected_to_block_engine {
        "ok".to_string()
    } else {
        "unhealthy".to_string()
    }
}
```

**Health Criteria:**
- **Slot Health**: Relayer is receiving slot updates from Solana network
- **Block Engine Connection**: Active connection to Jito's MEV infrastructure

**Use Cases:**
- Load balancer health checks
- Kubernetes liveness probes
- Automated monitoring alerts

### 3. **Status Endpoint** (`/status`)

Detailed operational status in JSON format.

**Request:**
```bash
curl http://localhost:11227/status
```

**Response:**
```json
{
  "slots_healthy": true,
  "is_connected_to_block_engine": false,
  "validators_connected": [
    "7Np41oeYqPefeNQEHSv1UDhYrehxin3NStELsSKCT4K2",
    "GdnSyH3YtwcxFvQrVVJMm1JhTS4QVX7MFsX56uJLUfiZ"
  ]
}
```

**Response Fields:**
- **`slots_healthy`** (boolean): Whether slot updates are being received properly
- **`is_connected_to_block_engine`** (boolean): Block Engine connectivity status  
- **`validators_connected`** (array): List of connected validator public keys

**Use Cases:**
- Monitoring dashboard integration
- Detailed health assessment
- Operational debugging
- Metrics collection

## Implementation Details

### **State Management**

The web server operates on shared state through the `RelayerState` structure:

```rust
pub struct RelayerState {
    pub slot_health: Arc<RwLock<HealthState>>,              // Slot health tracking
    pub is_connected_to_block_engine: Arc<AtomicBool>,      // Block Engine connectivity
    pub relayer_handle: Arc<dyn RelayerHandleTrait>,        // Validator connections
}
```

**Thread Safety Features:**
- **`Arc<RwLock<HealthState>>`**: Multiple readers, single writer for slot health
- **`Arc<AtomicBool>`**: Lock-free atomic access for block engine status
- **`Arc<dyn RelayerHandleTrait>`**: Shared access to validator connection data

### **Health Check Logic**

#### **Slot Health Monitoring**
- **Purpose**: Ensures the relayer can properly route transactions to current leaders
- **Implementation**: Tracks whether Solana slot updates are being received
- **Thread Safety**: Uses `RwLock` for concurrent read access across HTTP handlers
- **Update Source**: Updated by the main relayer's slot monitoring system

#### **Block Engine Connectivity**
- **Purpose**: Monitors connection to Jito's MEV infrastructure
- **Implementation**: Atomic boolean with relaxed memory ordering for performance
- **Performance**: Lock-free reads suitable for high-frequency health checks
- **Business Logic**: Essential for MEV bundle processing capabilities

#### **Validator Connection Tracking**
- **Purpose**: Provides visibility into active TPU proxy connections
- **Data Source**: Real-time data from `RelayerHandle.connected_validators()`
- **Format**: Returns validator public keys as base58-encoded strings

### **Rate Limiting and Protection**

The web server implements comprehensive rate limiting using Tower middleware:

```rust
.layer(
    ServiceBuilder::new()
        .layer(HandleErrorLayer::new(|err: BoxError| async move {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Unhandled error: {}", err))
        }))
        .layer(BufferLayer::new(max_buffered_requests))
        .layer(RateLimitLayer::new(requests_per_second, Duration::from_secs(1)))
)
```

**Protection Features:**
- **Request Rate Limiting**: Configurable requests per second (default: 5 RPS)
- **Request Buffering**: Handles traffic spikes with configurable buffer (default: 10 requests)
- **Error Handling**: Graceful responses for rate limit exceeded scenarios
- **Time Window**: 1-second sliding window for rate calculation

**Configuration:**
```rust
pub async fn start_relayer_web_server(
    state: Arc<RelayerState>,
    addr: SocketAddr,
    max_buffered_requests: usize,  // Default: 10
    requests_per_second: u64,      // Default: 5
) -> Result<(), Box<dyn std::error::Error>>
```

### **Axum Web Framework Integration**

#### **Router Configuration**
```rust
fn build_relayer_router(
    state: Arc<RelayerState>,
    max_buffered_requests: usize,
    requests_per_second: u64,
) -> Router {
    Router::new()
        .route("/", get(homepage))
        .route("/health", get(get_health))
        .route("/status", get(get_status))
        .layer(Extension(state))
        .layer(/* rate limiting middleware */)
}
```

**Framework Benefits:**
- **Async-First Design**: All handlers are async for optimal performance
- **Type Safety**: Compile-time guarantees for route handlers and extractors
- **Middleware Stack**: Composable middleware for cross-cutting concerns
- **Zero-Copy State Sharing**: Efficient state access through `Extension` layer

#### **Server Startup**
```rust
let server = axum::Server::bind(&addr)
    .serve(app.into_make_service())
    .await;
```

**Production Features:**
- **Graceful Shutdown**: Supports coordinated shutdown with main application
- **High Concurrency**: Tokio-based async runtime handles many concurrent requests
- **Resource Efficiency**: Minimal memory footprint and CPU usage

## Integration Patterns

### **With Main Application**

The web server integrates seamlessly with the main relayer application:

```rust
// In transaction-relayer/src/main.rs
let relayer_state = Arc::new(RelayerState {
    slot_health: health_manager.health_state(),
    is_connected_to_block_engine: block_engine_handler.connection_status(),
    relayer_handle: relayer_service.handle(),
});

// Start web server alongside other services
let web_server_task = tokio::spawn(async move {
    jito_relayer_web::start_relayer_web_server(
        relayer_state,
        webserver_bind_addr,
        10, // max buffered requests
        5,  // requests per second
    ).await
});
```

### **With Monitoring Systems**

#### **Load Balancer Integration**
```bash
# HAProxy health check configuration
backend jito_relayers
    option httpchk GET /health
    server relayer1 10.0.1.10:11227 check
    server relayer2 10.0.1.11:11227 check
```

#### **Prometheus/Grafana Integration**
```yaml
# Prometheus scrape config
scrape_configs:
  - job_name: 'jito-relayer'
    static_configs:
      - targets: ['relayer1:11227', 'relayer2:11227']
    metrics_path: '/status'
    scrape_interval: 30s
```

#### **Kubernetes Health Probes**
```yaml
# Kubernetes deployment health probes
livenessProbe:
  httpGet:
    path: /health
    port: 11227
  initialDelaySeconds: 30
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /health
    port: 11227
  initialDelaySeconds: 5
  periodSeconds: 5
```

## Monitoring and Alerting

### **Health Check Monitoring**
```bash
# Simple health monitoring script
#!/bin/bash
HEALTH=$(curl -s http://localhost:11227/health)
if [ "$HEALTH" != "ok" ]; then
    echo "ALERT: Jito Relayer is unhealthy"
    # Send alert to monitoring system
fi
```

### **Detailed Status Monitoring**
```bash
# Extract specific metrics from status endpoint
curl -s http://localhost:11227/status | jq '.is_connected_to_block_engine'
curl -s http://localhost:11227/status | jq '.validators_connected | length'
```

### **Operational Dashboards**
The JSON status endpoint provides structured data suitable for:
- **Grafana Dashboards**: Real-time health visualization
- **DataDog/NewRelic**: Metrics collection and alerting
- **Custom Monitoring**: Programmatic status assessment

## Configuration Examples

### **Development Setup**
```rust
// Relaxed rate limiting for development
jito_relayer_web::start_relayer_web_server(
    relayer_state,
    "127.0.0.1:11227".parse()?,
    100, // Higher buffer for testing
    100, // Higher rate limit for development
).await
```

### **Production Setup**
```rust
// Conservative rate limiting for production
jito_relayer_web::start_relayer_web_server(
    relayer_state,
    "0.0.0.0:11227".parse()?,
    10,  // Conservative buffer
    5,   // Conservative rate limit
).await
```

## Error Handling

### **Rate Limiting Responses**
When rate limits are exceeded:
```
HTTP/1.1 429 Too Many Requests
Content-Type: text/plain

Rate limit exceeded
```

### **Internal Errors**
For unexpected errors:
```
HTTP/1.1 500 Internal Server Error
Content-Type: text/plain

Unhandled error: <error description>
```

### **Service Unavailable**
When the relayer is shutting down:
```
HTTP/1.1 503 Service Unavailable
Content-Type: text/plain

Service shutting down
```

This web package provides essential operational visibility for the Jito Relayer, enabling robust monitoring, alerting, and load balancing capabilities required for production deployment in high-availability environments.