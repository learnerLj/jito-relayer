use std::{
    collections::HashSet,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    ops::Range,
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use agave_validator::admin_rpc_service::StakedNodesOverrides;
use clap::Parser;
use crossbeam_channel::tick;
use dashmap::DashMap;
use env_logger::Env;
use jito_block_engine::block_engine::{BlockEngineConfig, BlockEngineRelayerHandler};
use jito_core::{
    graceful_panic,
    tpu::{Tpu, TpuSockets},
};
use jito_protos::{
    auth::auth_service_server::AuthServiceServer, relayer::relayer_server::RelayerServer,
};
use jito_relayer::{
    auth_interceptor::AuthInterceptor,
    auth_service::{AuthServiceImpl, ValidatorAuther},
    health_manager::HealthManager,
    relayer::RelayerImpl,
    schedule_cache::{LeaderScheduleCacheUpdater, LeaderScheduleUpdatingHandle},
};
use jito_relayer_web::{start_relayer_web_server, RelayerState};
use jito_rpc::load_balancer::LoadBalancer;
use jito_transaction_relayer::forwarder::start_forward_and_delay_thread;
use jwt::{AlgorithmType, PKeyWithDigest};
use log::{debug, error, info, warn};
use openssl::{hash::MessageDigest, pkey::PKey};
use solana_metrics::{datapoint_error, datapoint_info};
use solana_net_utils::multi_bind_in_range;
use solana_program::address_lookup_table::{state::AddressLookupTable, AddressLookupTableAccount};
use solana_sdk::{
    pubkey::Pubkey,
    signature::{read_keypair_file, Signer},
};
use tikv_jemallocator::Jemalloc;
use tokio::{runtime::Builder, signal, sync::mpsc::channel};
use tonic::transport::Server;

// no-op change to test ci

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

/// Command-line arguments for the Jito Transaction Relayer.
/// The relayer acts as a high-performance TPU (Transaction Processing Unit) proxy
/// that forwards transactions to Solana validators while integrating with the
/// Jito Block Engine for MEV (Maximum Extractable Value) bundle processing.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// DEPRECATED: Legacy UDP TPU port (Transaction Processing Unit)
    /// UDP-based TPU was replaced with QUIC for better performance and reliability.
    /// This field remains for backward compatibility but is no longer used.
    #[deprecated(since = "0.1.8", note = "UDP TPU disabled")]
    #[arg(long, env, default_value_t = 0)]
    tpu_port: u16,

    /// DEPRECATED: Legacy UDP TPU forward port
    /// Used for forwarding transactions to next leader. Replaced by QUIC-based forwarding.
    /// This field remains for backward compatibility but is no longer used.
    #[deprecated(since = "0.1.8", note = "UDP TPU_FWD disabled")]
    #[arg(long, env, default_value_t = 0)]
    tpu_fwd_port: u16,

    /// QUIC-based TPU port for receiving transactions from clients.
    /// QUIC provides better performance than UDP with built-in congestion control,
    /// multiplexing, and connection reliability. The relayer binds to a port range:
    /// [tpu_quic_port, tpu_quic_port + num_tpu_quic_servers)
    ///
    /// IMPORTANT: Open firewall ports for the entire range.
    /// IMPORTANT: Avoid overlap with TPU forward ports.
    ///
    /// Note: Returns (port - 6) to validators to maintain compatibility with legacy UDP TPU numbering.
    #[arg(long, env, default_value_t = 11_228)]
    tpu_quic_port: u16,

    /// Number of concurrent QUIC TPU servers to spawn for load distribution.
    /// Each server handles incoming transaction packets on its own port.
    /// More servers can improve throughput under high load but consume more resources.
    #[arg(long, env, default_value_t = 1)]
    num_tpu_quic_servers: u16,

    /// QUIC-based TPU forward port for leader-to-leader transaction forwarding.
    /// When the current relayer is not the leader, it forwards transactions to
    /// the current leader's TPU forward port. This enables efficient transaction
    /// propagation across the validator network.
    ///
    /// Port range: [tpu_quic_fwd_port, tpu_quic_fwd_port + num_tpu_fwd_quic_servers)
    ///
    /// IMPORTANT: Set at least (num_tpu_fwd_quic_servers + 6) higher than regular TPU ports
    /// to avoid port conflicts. Open firewall ports for the entire range.
    ///
    /// Note: Returns (port - 6) to validators for UDP compatibility.
    #[arg(long, env, default_value_t = 11_229)]
    tpu_quic_fwd_port: u16,

    /// Number of concurrent QUIC TPU forward servers for leader-to-leader forwarding.
    /// Multiple servers enable parallel processing of forwarded transactions
    /// and improve resilience under high transaction volume.
    #[arg(long, env, default_value_t = 1)]
    num_tpu_fwd_quic_servers: u16,

    /// IP address for the gRPC server that exposes relayer services.
    /// The gRPC server provides authentication endpoints and relayer configuration APIs.
    /// Default 0.0.0.0 binds to all interfaces, allowing external connections.
    /// Use 127.0.0.1 to restrict to localhost only for security.
    #[arg(long, env, default_value_t = IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)))]
    grpc_bind_ip: IpAddr,

    /// Port for the gRPC server that exposes relayer services.
    /// Validators connect to this port for authentication and to receive
    /// TPU configuration information (ports, IP addresses, etc.).
    #[arg(long, env, default_value_t = 11_226)]
    grpc_bind_port: u16,

    /// List of Solana RPC server HTTP URLs for blockchain queries (space-separated).
    /// These servers provide access to blockchain state, account data, and transaction submission.
    /// The LoadBalancer automatically routes requests to the server with the highest slot
    /// (most up-to-date blockchain state) for optimal MEV performance.
    ///
    /// IMPORTANT: Must match the order of websocket_servers (paired by position).
    /// Example: "http://rpc1.com:8899 http://rpc2.com:8899"
    #[arg(
        long,
        env,
        value_delimiter = ' ',
        default_value = "http://127.0.0.1:8899"
    )]
    rpc_servers: Vec<String>,

    /// List of Solana WebSocket server URLs for real-time slot updates (space-separated).
    /// WebSocket connections provide live blockchain slot notifications used for:
    /// - Determining which RPC server has the most current state
    /// - Health monitoring and system coordination
    /// - Leader schedule updates for optimal transaction forwarding
    ///
    /// IMPORTANT: Must match the order of rpc_servers (paired by position).
    /// Example: "ws://rpc1.com:8900 ws://rpc2.com:8900"
    #[arg(
        long,
        env,
        value_delimiter = ' ',
        default_value = "ws://127.0.0.1:8900"
    )]
    websocket_servers: Vec<String>,

    /// Solana network entrypoint for gossip network discovery and public IP detection.
    /// The entrypoint serves as a bootstrap node that provides:
    /// - Access to the gossip network for validator discovery
    /// - Public IP address detection when --public-ip is not specified
    /// - Network topology information and cluster configuration
    ///
    /// The relayer contacts this entrypoint to join the Solana gossip network
    /// and discover other validators, leader schedules, and network state.
    #[arg(long, env, default_value = "entrypoint.mainnet-beta.solana.com:8001")]
    entrypoint_address: String,

    /// Public IP address that validators will advertise to the network.
    /// This is the IP address where other network participants will send transactions.
    /// If not specified, the relayer will automatically discover its public IP
    /// by contacting the entrypoint address.
    ///
    /// CRITICAL: Must be the externally routable IP address, not localhost or private IPs.
    /// Validators will share this IP with the entire Solana network.
    #[arg(long, env)]
    public_ip: Option<IpAddr>,

    /// Intentional delay before forwarding packets to validators (milliseconds).
    /// This delay allows the relayer to collect and batch multiple transactions
    /// before forwarding, improving efficiency. However, it trades latency for throughput.
    /// Lower values reduce transaction confirmation time but may increase network overhead.
    #[arg(long, env, default_value_t = 200)]
    packet_delay_ms: u32,

    /// URL of the Jito Block Engine for MEV bundle processing.
    /// The Block Engine coordinates Maximum Extractable Value (MEV) operations
    /// by processing transaction bundles from searchers and coordinating with validators.
    ///
    /// If not provided, MEV functionality is disabled and the relayer operates
    /// in standard transaction forwarding mode only.
    ///
    /// See: https://jito-labs.gitbook.io/mev/searcher-resources/block-engine#connection-details
    #[arg(long, env)]
    block_engine_url: Option<String>,

    /// Override URL for the Block Engine's authentication service.
    /// The auth service handles validator authentication and authorization for MEV operations.
    /// If not specified, defaults to the same URL as --block-engine-url.
    ///
    /// Useful when the Block Engine and auth service are deployed separately
    /// or when using different endpoints for load balancing.
    #[arg(long, env)]
    block_engine_auth_service_url: Option<String>,

    /// Path to the relayer's identity keypair file.
    /// This keypair is used to:
    /// - Authenticate with the Jito Block Engine
    /// - Sign metrics and telemetry data
    /// - Establish identity for validator coordination
    ///
    /// SECURITY: Protect this file with appropriate permissions (600).
    /// The private key should be kept secure as it represents the relayer's identity.
    #[arg(long, env)]
    keypair_path: PathBuf,

    /// Whitelist of validator public keys allowed to authenticate with this relayer.
    /// Restricts access to only specified validators for enhanced security.
    /// Use comma-separated list of base58-encoded pubkeys.
    ///
    /// If not specified (null), all validators in the current leader schedule
    /// are automatically permitted to authenticate. This is the typical configuration
    /// for open relayers that serve the entire validator set.
    ///
    /// Example: "pubkey1,pubkey2,pubkey3"
    #[arg(long, env, value_delimiter = ',')]
    allowed_validators: Option<Vec<Pubkey>>,

    /// Path to PEM-encoded private key file for JWT token signing.
    /// This key is used by the authentication service to sign access tokens
    /// and refresh tokens issued to authenticated validators.
    ///
    /// SECURITY: Must be kept secure with restricted file permissions (600).
    /// Compromise of this key allows unauthorized token generation.
    #[arg(long, env)]
    signing_key_pem_path: PathBuf,

    /// Path to PEM-encoded public key file for JWT token verification.
    /// This key is used to verify the authenticity of tokens presented by validators.
    /// Multiple services can share this public key for distributed token verification.
    ///
    /// Must correspond to the private key specified in signing_key_pem_path.
    #[arg(long, env)]
    verifying_key_pem_path: PathBuf,

    /// Time-to-live for access tokens in seconds (default: 30 minutes).
    /// Access tokens are short-lived credentials that validators use for API calls.
    /// Shorter TTL improves security but requires more frequent token refresh.
    /// Longer TTL reduces refresh overhead but increases security risk if compromised.
    #[arg(long, env, default_value_t = 1_800)]
    access_token_ttl_secs: u64,

    /// Time-to-live for refresh tokens in seconds (default: 50 hours).
    /// Refresh tokens are used to obtain new access tokens without re-authentication.
    /// Much longer-lived than access tokens to reduce authentication overhead.
    /// Should be long enough to cover typical validator restart/maintenance cycles.
    #[arg(long, env, default_value_t = 180_000)]
    refresh_token_ttl_secs: u64,

    /// Time-to-live for authentication challenges in seconds (default: 30 minutes).
    /// Challenges are cryptographic puzzles sent to validators during initial auth.
    /// Must be long enough for validators to process but short enough to prevent replay attacks.
    /// Expired challenges are automatically cleaned up to prevent memory leaks.
    #[arg(long, env, default_value_t = 1_800)]
    challenge_ttl_secs: u64,

    /// Interval for cleaning up expired authentication challenges (seconds).
    /// Background task runs at this interval to remove stale challenges from memory.
    /// Should be frequent enough to prevent memory buildup but not so frequent
    /// as to impact performance. Default 3 minutes provides good balance.
    #[arg(long, env, default_value_t = 180)]
    challenge_expiration_sleep_interval_secs: u64,

    /// Slot miss threshold for marking the system as unhealthy (seconds).
    /// If no slot updates are received within this timeframe, the health
    /// manager marks the system as unhealthy, which affects metrics and
    /// potentially triggers alerts.
    ///
    /// Solana produces slots every ~400ms, so 10 seconds allows for significant
    /// network issues while avoiding false positives.
    #[arg(long, env, default_value_t = 10)]
    missing_slot_unhealthy_secs: u64,

    /// DEPRECATED: Solana cluster identifier (mainnet-beta, testnet, devnet, etc.).
    /// Originally used for metrics and regional coordination but no longer functional.
    /// Retained for backward compatibility only.
    #[arg(long, env)]
    cluster: Option<String>,

    /// DEPRECATED: Geographic region identifier (amsterdam, dallas, frankfurt, etc.).
    /// Originally used for latency optimization and regional metrics but no longer used.
    /// Retained for backward compatibility only.
    #[arg(long, env)]
    region: Option<String>,

    /// Cache TTL for "Accounts of Interest" used in MEV bundle processing (seconds).
    /// The Block Engine tracks specific accounts that are frequently accessed in MEV bundles.
    /// This cache reduces RPC load by temporarily storing account states.
    ///
    /// IMPORTANT: Must coordinate with Block Engine's full update refresh period
    /// to avoid stale data inconsistencies. Default 5 minutes balances performance and freshness.
    #[arg(long, env, default_value_t = 300)]
    aoi_cache_ttl_secs: u64,

    /// Interval for refreshing Solana address lookup tables (seconds).
    /// Address lookup tables compress transaction sizes by storing frequently used addresses.
    /// Regular refresh ensures the relayer has current lookup table data for transaction processing.
    /// Only active when enable_lookup_table_refresh is true.
    #[arg(long, env, default_value_t = 600)]
    lookup_table_refresh_secs: u64,

    /// Enable automatic refresh of address lookup table data from RPC servers.
    /// When enabled, periodically fetches all address lookup tables to keep local cache current.
    /// Improves transaction processing efficiency but increases RPC load.
    /// Recommended for high-throughput relayers handling many compressed transactions.
    #[arg(long, env, default_value_t = false)]
    enable_lookup_table_refresh: bool,

    /// List of addresses subject to OFAC sanctions (space-separated pubkeys).
    /// Transactions involving any of these addresses will be automatically dropped
    /// for regulatory compliance. This includes transactions that:
    /// - Send/receive from these addresses
    /// - Interact with programs owned by these addresses
    /// - Reference these addresses in any capacity
    ///
    /// COMPLIANCE: Operators in regulated jurisdictions should maintain this list current.
    #[arg(long, env, value_delimiter = ' ', value_parser = Pubkey::from_str)]
    ofac_addresses: Option<Vec<Pubkey>>,

    /// Bind address for the diagnostic web server.
    /// Exposes health metrics, system status, and operational information via HTTP endpoints.
    /// Used for monitoring, alerting, and operational visibility.
    /// Default binds to localhost only for security.
    #[arg(long, env, default_value_t = SocketAddr::from_str("127.0.0.1:11227").unwrap())]
    webserver_bind_addr: SocketAddr,

    /// Maximum concurrent QUIC connections from unstaked validators.
    /// Unstaked validators have lower priority and resource allocation.
    /// Lower limit prevents unstaked validators from overwhelming the relayer
    /// and ensures resources are available for staked validators.
    #[arg(long, env, default_value_t = 500)]
    max_unstaked_quic_connections: usize,

    /// Maximum concurrent QUIC connections from staked validators.
    /// Staked validators get higher priority and resource allocation.
    /// Higher limit ensures staked validators can always connect and participate
    /// in consensus without connection limits becoming a bottleneck.
    #[arg(long, env, default_value_t = 2000)]
    max_staked_quic_connections: usize,

    /// Number of transaction packets to batch together when forwarding to validators.
    /// Larger batches improve network efficiency and reduce syscall overhead
    /// but may increase latency. Smaller batches reduce latency but increase overhead.
    /// Default 4 provides good balance for typical network conditions.
    #[arg(long, env, default_value_t = 4)]
    validator_packet_batch_size: usize,

    /// Disable forwarding transactions to the mempool/gossip network.
    /// When true, transactions are only forwarded directly to current leaders
    /// and not broadcast to the wider network. This can improve performance
    /// for MEV-focused operations but may reduce transaction propagation.
    #[arg(long, env, default_value_t = false)]
    disable_mempool: bool,

    /// Forward transactions to ALL connected validators regardless of leader schedule.
    /// When true, ignores leader schedule and broadcasts to all validators.
    ///
    /// IMPORTANT: Required for Stake Weighted Quality of Service (SWQOS) functionality.
    /// Improves transaction propagation but increases network traffic significantly.
    /// Use with caution on bandwidth-limited connections.
    #[arg(long, env, default_value_t = false)]
    forward_all: bool,

    /// Path to YAML file containing custom stake overrides for network validators.
    ///
    /// BACKGROUND: In Solana, validators must "stake" SOL tokens to participate in consensus.
    /// Higher stake = more influence and higher priority for network resources.
    /// The relayer needs to know each validator's stake amount to make resource allocation decisions.
    ///
    /// This file allows manual override of stake amounts when:
    /// - RPC stake data is unreliable or stale
    /// - Testing with custom stake configurations
    /// - Adjusting priorities for specific validators
    ///
    /// Stake amounts are used for:
    /// - Maximum QUIC connections allowed from each validator
    /// - Transaction forwarding priority (higher stake = higher priority)
    /// - Vote packet processing order in consensus
    ///
    /// File format (YAML):
    /// ```yaml
    /// staked_map_id:
    ///   "validator_pubkey_1": 1000000
    ///   "validator_pubkey_2": 500000
    ///   "validator_pubkey_3": 2000000
    /// ```
    #[arg(long, env)]
    staked_nodes_overrides: Option<PathBuf>,

    /// Number of slots to look ahead when determining transaction forwarding targets.
    /// Larger values provide more time for leader schedule calculation and network
    /// coordination but may reduce responsiveness to leader changes.
    /// Default 5 slots (~2 seconds) balances predictability with responsiveness.
    #[arg(long, env, default_value_t = 5)]
    slot_lookahead: u64,
}

/// Container for all QUIC socket bindings used by the TPU system.
/// Separates socket creation from socket usage for better resource management.
#[derive(Debug)]
struct Sockets {
    /// QUIC sockets for transaction processing and forwarding.
    /// Includes both regular TPU sockets and TPU forward sockets.
    tpu_sockets: TpuSockets,
}

/// Creates and binds all QUIC sockets needed for TPU operations.
///
/// This function:
/// 1. Validates port ranges don't overlap
/// 2. Binds QUIC sockets for both regular TPU and TPU forwarding
/// 3. Ensures all sockets are successfully bound before returning
///
/// # Panics
/// - If too many servers are requested (> u16::MAX)
/// - If port ranges overlap
/// - If socket binding fails
fn get_sockets(args: &Args) -> Sockets {
    // Validate server counts are within reasonable bounds
    assert!(args.num_tpu_quic_servers < u16::MAX);
    assert!(args.num_tpu_fwd_quic_servers < u16::MAX);

    // Calculate port ranges for regular TPU and TPU forwarding
    // Each server gets its own port for load distribution
    let tpu_ports = Range {
        start: args.tpu_quic_port,
        end: args
            .tpu_quic_port
            .checked_add(args.num_tpu_quic_servers)
            .unwrap(),
    };
    let tpu_fwd_ports = Range {
        start: args.tpu_quic_fwd_port,
        end: args
            .tpu_quic_fwd_port
            .checked_add(args.num_tpu_fwd_quic_servers)
            .unwrap(),
    };

    // Ensure port ranges don't overlap to prevent binding conflicts
    for tpu_port in tpu_ports.start..tpu_ports.end {
        assert!(!tpu_fwd_ports.contains(&tpu_port));
    }

    // Bind regular TPU QUIC sockets for incoming transactions
    // Each socket binds to a specific port in the calculated range
    let (tpu_p, tpu_quic_sockets): (Vec<_>, Vec<_>) = (0..args.num_tpu_quic_servers)
        .map(|i| {
            // Bind to a single port within the range for this server instance
            let (port, mut sock) = multi_bind_in_range(
                IpAddr::V4(Ipv4Addr::from([0, 0, 0, 0])), // Bind to all interfaces
                (tpu_ports.start + i, tpu_ports.start + 1 + i),
                1, // Request exactly 1 socket
            )
            .unwrap();

            (port, sock.pop().unwrap())
        })
        .unzip();

    // Bind TPU forward QUIC sockets for leader-to-leader transaction forwarding
    // Similar process but for the forward port range
    let (tpu_fwd_p, tpu_fwd_quic_sockets): (Vec<_>, Vec<_>) = (0..args.num_tpu_fwd_quic_servers)
        .map(|i| {
            // Bind to a single port within the forward range for this server instance
            let (port, mut sock) = multi_bind_in_range(
                IpAddr::V4(Ipv4Addr::from([0, 0, 0, 0])), // Bind to all interfaces
                (tpu_fwd_ports.start + i, tpu_fwd_ports.start + 1 + i),
                1, // Request exactly 1 socket
            )
            .unwrap();

            (port, sock.pop().unwrap())
        })
        .unzip();

    // Verify that we bound to exactly the ports we expected
    assert_eq!(tpu_ports.collect::<Vec<_>>(), tpu_p);
    assert_eq!(tpu_fwd_ports.collect::<Vec<_>>(), tpu_fwd_p);

    Sockets {
        tpu_sockets: TpuSockets {
            transactions_quic_sockets: tpu_quic_sockets,
            transactions_forwards_quic_sockets: tpu_fwd_quic_sockets,
        },
    }
}

/// Main entry point for the Jito Transaction Relayer.
///
/// The relayer operates as a high-performance TPU proxy that:
/// 1. Receives transactions from clients via QUIC
/// 2. Authenticates validators using JWT tokens
/// 3. Forwards transactions to current leaders based on slot timing
/// 4. Integrates with Jito Block Engine for MEV bundle processing
/// 5. Provides health monitoring and operational metrics
///
/// Architecture:
/// - Multi-threaded design with async gRPC services
/// - QUIC-based transaction ingestion for high throughput
/// - Real-time slot tracking for optimal forwarding decisions
/// - JWT-based authentication with challenge-response protocol
/// - Optional MEV integration via Block Engine connection
fn main() {
    // Rate limiting configuration for the diagnostic web server
    const MAX_BUFFERED_REQUESTS: usize = 10;
    const REQUESTS_PER_SECOND: u64 = 5;

    // Initialize logging with millisecond timestamps for operational debugging
    // Default log level is 'info' but can be overridden with RUST_LOG environment variable
    env_logger::Builder::from_env(Env::new().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Parse command-line arguments and environment variables
    let args: Args = Args::parse();
    info!("args: {:?}", args);

    // Issue deprecation warnings for legacy arguments
    if args.cluster.is_some() {
        warn!("--cluster arg is deprecated and may be removed in the next release.")
    }
    if args.region.is_some() {
        warn!("--region arg is deprecated and may be removed in the next release.")
    }

    // Determine the public IP address that will be advertised to the network
    let public_ip = if args.public_ip.is_some() {
        // Use explicitly provided public IP
        args.public_ip.unwrap()
    } else {
        // Auto-discover public IP by contacting the Solana entrypoint
        let entrypoint = solana_net_utils::parse_host_port(args.entrypoint_address.as_str())
            .expect("parse entrypoint");
        info!(
            "Contacting {} to determine the validator's public IP address",
            entrypoint
        );
        solana_net_utils::get_public_ip_addr(&entrypoint).expect("get public ip address")
    };

    info!("public ip: {:?}", public_ip);

    // Validate that the public IP is suitable for network operations
    assert!(
        public_ip.is_ipv4(),
        "Your public IP address needs to be IPv4 but is currently listed as {}. \
    If you are seeing this error and not passing in --public-ip, \
    please find your public ip address and pass it in on the command line",
        public_ip
    );
    assert!(
        !public_ip.is_loopback(),
        "Your public IP can't be the loopback interface"
    );

    // IPv4-only restriction for security: IPv6 addresses are cheap to generate
    // in large quantities, which could be used to overwhelm the authentication
    // challenge queue and create a denial-of-service attack vector.
    assert!(args.grpc_bind_ip.is_ipv4(), "must bind to IPv4 address");

    let sockets = get_sockets(&args);
    let tpu_quic_ports: Vec<u16> = sockets
        .tpu_sockets
        .transactions_quic_sockets
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect();
    let tpu_quic_fwd_ports: Vec<u16> = sockets
        .tpu_sockets
        .transactions_forwards_quic_sockets
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect();

    // make sure to allow your firewall to accept UDP packets on these ports
    // if you're using staked overrides, you can provide one of these addresses
    // to --rpc-send-transaction-tpu-peer
    for port in &tpu_quic_ports {
        info!(
            "TPU quic socket is listening at: {}:{}",
            public_ip.to_string(),
            port
        );
    }
    for port in &tpu_quic_fwd_ports {
        info!(
            "TPU forward quic socket is listening at: {}:{}",
            public_ip.to_string(),
            port
        );
    }

    let keypair =
        Arc::new(read_keypair_file(args.keypair_path).expect("keypair file does not exist"));
    solana_metrics::set_host_id(format!(
        "{}_{}",
        hostname::get().unwrap().to_str().unwrap(), // hostname should follow RFC1123
        keypair.pubkey()
    ));
    info!("Relayer started with pubkey: {}", keypair.pubkey());

    let major: String = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
    let minor: String = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
    let patch: String = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();

    datapoint_info!(
        "relayer-info",
        ("mempool_enabled", !args.disable_mempool, bool),
        ("version", format!("{}.{}.{}", major, minor, patch), String),
    );

    let exit = graceful_panic(None);

    assert_eq!(
        args.rpc_servers.len(),
        args.websocket_servers.len(),
        "number of rpc servers must match number of websocket servers"
    );

    let servers: Vec<(String, String)> = args
        .rpc_servers
        .into_iter()
        .zip(args.websocket_servers)
        .collect();

    let ofac_addresses: HashSet<Pubkey> = args
        .ofac_addresses
        .map(|a| a.into_iter().collect())
        .unwrap_or_default();
    info!("ofac addresses: {:?}", ofac_addresses);

    let (rpc_load_balancer, slot_receiver) = LoadBalancer::new(&servers, &exit);
    let rpc_load_balancer = Arc::new(rpc_load_balancer);

    // Lookup table refresher
    let address_lookup_table_cache: Arc<DashMap<Pubkey, AddressLookupTableAccount>> =
        Arc::new(DashMap::new());
    let lookup_table_refresher = if args.enable_lookup_table_refresh {
        Some(start_lookup_table_refresher(
            &rpc_load_balancer,
            &address_lookup_table_cache,
            Duration::from_secs(args.lookup_table_refresh_secs),
            &exit,
        ))
    } else {
        None
    };

    // Load validator stake overrides from YAML file if provided
    // This allows manual control over validator resource allocation priorities
    let staked_nodes_overrides = match args.staked_nodes_overrides {
        None => StakedNodesOverrides::default(),
        Some(p) => {
            let file = fs::File::open(&p).expect(&format!(
                "Failed to open staked nodes overrides file: {:?}",
                &p
            ));
            serde_yaml::from_reader(file).expect(&format!(
                "Failed to read staked nodes overrides file: {:?}",
                &p,
            ))
        }
    };
    let (tpu, verified_receiver) = Tpu::new(
        sockets.tpu_sockets,
        &exit,
        &keypair,
        &rpc_load_balancer,
        args.max_unstaked_quic_connections,
        args.max_staked_quic_connections,
        staked_nodes_overrides.staked_map_id,
    );

    let leader_cache = LeaderScheduleCacheUpdater::new(&rpc_load_balancer, &exit);

    // receiver tracked as relayer_metrics.delay_packet_receiver_len
    let (delay_packet_sender, delay_packet_receiver) =
        crossbeam_channel::bounded(Tpu::TPU_QUEUE_CAPACITY);

    // NOTE: make sure the channel here isn't too big because it will get backed up
    // with packets when the block engine isn't connected
    // tracked as forwarder_metrics.block_engine_sender_len
    let (block_engine_sender, block_engine_receiver) =
        channel(jito_transaction_relayer::forwarder::BLOCK_ENGINE_FORWARDER_QUEUE_CAPACITY);

    let forward_and_delay_threads = start_forward_and_delay_thread(
        verified_receiver,
        delay_packet_sender,
        args.packet_delay_ms,
        block_engine_sender,
        1,
        args.disable_mempool,
        &exit,
    );

    let is_connected_to_block_engine = Arc::new(AtomicBool::new(false));
    let block_engine_config = if !args.disable_mempool && args.block_engine_url.is_some() {
        let block_engine_url = args.block_engine_url.unwrap();
        let auth_service_url = args
            .block_engine_auth_service_url
            .unwrap_or(block_engine_url.clone());
        Some(BlockEngineConfig {
            block_engine_url,
            auth_service_url,
        })
    } else {
        None
    };
    let block_engine_forwarder = BlockEngineRelayerHandler::new(
        block_engine_config,
        block_engine_receiver,
        keypair,
        exit.clone(),
        args.aoi_cache_ttl_secs,
        address_lookup_table_cache.clone(),
        &is_connected_to_block_engine,
        ofac_addresses.clone(),
    );

    // receiver tracked as relayer_metrics.slot_receiver_len
    // downstream channel gets data that was duplicated by HealthManager
    let (downstream_slot_sender, downstream_slot_receiver) =
        crossbeam_channel::bounded(LoadBalancer::SLOT_QUEUE_CAPACITY);
    let health_manager = HealthManager::new(
        slot_receiver,
        downstream_slot_sender,
        Duration::from_secs(args.missing_slot_unhealthy_secs),
        exit.clone(),
    );

    let server_addr = SocketAddr::new(args.grpc_bind_ip, args.grpc_bind_port);
    let relayer_svc = RelayerImpl::new(
        downstream_slot_receiver,
        delay_packet_receiver,
        leader_cache.handle(),
        public_ip,
        tpu_quic_ports,
        tpu_quic_fwd_ports,
        health_manager.handle(),
        exit.clone(),
        ofac_addresses,
        address_lookup_table_cache,
        args.validator_packet_batch_size,
        args.forward_all,
        args.slot_lookahead,
    );

    let priv_key = fs::read(&args.signing_key_pem_path).unwrap_or_else(|_| {
        panic!(
            "Failed to read signing key file: {:?}",
            &args.verifying_key_pem_path
        )
    });
    let signing_key = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: PKey::private_key_from_pem(&priv_key).unwrap(),
    };

    let key = fs::read(&args.verifying_key_pem_path).unwrap_or_else(|_| {
        panic!(
            "Failed to read verifying key file: {:?}",
            &args.verifying_key_pem_path
        )
    });
    let verifying_key = Arc::new(PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: PKey::public_key_from_pem(&key).unwrap(),
    });

    let validator_store = match args.allowed_validators {
        Some(pubkeys) => ValidatorStore::UserDefined(HashSet::from_iter(pubkeys)),
        None => ValidatorStore::LeaderSchedule(leader_cache.handle()),
    };

    let relayer_state = Arc::new(RelayerState::new(
        health_manager.handle(),
        &is_connected_to_block_engine,
        relayer_svc.handle(),
    ));

    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    rt.spawn({
        let relayer_state = relayer_state.clone();
        start_relayer_web_server(
            relayer_state,
            args.webserver_bind_addr,
            MAX_BUFFERED_REQUESTS,
            REQUESTS_PER_SECOND,
        )
    });

    rt.block_on(async {
        let auth_svc = AuthServiceImpl::new(
            ValidatorAutherImpl {
                store: validator_store,
            },
            signing_key,
            verifying_key.clone(),
            Duration::from_secs(args.access_token_ttl_secs),
            Duration::from_secs(args.refresh_token_ttl_secs),
            Duration::from_secs(args.challenge_ttl_secs),
            Duration::from_secs(args.challenge_expiration_sleep_interval_secs),
            &exit,
            health_manager.handle(),
        );

        info!("starting relayer at: {:?}", server_addr);
        Server::builder()
            .add_service(RelayerServer::with_interceptor(
                relayer_svc,
                AuthInterceptor::new(verifying_key.clone(), AlgorithmType::Rs256),
            ))
            .add_service(AuthServiceServer::new(auth_svc))
            .serve_with_shutdown(server_addr, shutdown_signal(exit.clone()))
            .await
            .expect("serve relayer");
    });

    exit.store(true, Ordering::Relaxed);

    tpu.join().unwrap();
    health_manager.join().unwrap();
    leader_cache.join().unwrap();
    for t in forward_and_delay_threads {
        t.join().unwrap();
    }
    if let Some(lookup_table_refresher) = lookup_table_refresher {
        lookup_table_refresher.join().unwrap();
    }
    block_engine_forwarder.join();
}

pub async fn shutdown_signal(exit: Arc<AtomicBool>) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    exit.store(true, Ordering::Relaxed);
    warn!("signal received, starting graceful shutdown");
}

enum ValidatorStore {
    LeaderSchedule(LeaderScheduleUpdatingHandle),
    UserDefined(HashSet<Pubkey>),
}

struct ValidatorAutherImpl {
    store: ValidatorStore,
}

impl ValidatorAuther for ValidatorAutherImpl {
    fn is_authorized(&self, pubkey: &Pubkey) -> bool {
        match &self.store {
            ValidatorStore::LeaderSchedule(cache) => cache.is_scheduled_validator(pubkey),
            ValidatorStore::UserDefined(pubkeys) => pubkeys.contains(pubkey),
        }
    }
}

fn start_lookup_table_refresher(
    rpc_load_balancer: &Arc<LoadBalancer>,
    lookup_table: &Arc<DashMap<Pubkey, AddressLookupTableAccount>>,
    refresh_duration: Duration,
    exit: &Arc<AtomicBool>,
) -> JoinHandle<()> {
    let rpc_load_balancer = rpc_load_balancer.clone();
    let exit = exit.clone();
    let lookup_table = lookup_table.clone();

    thread::Builder::new()
        .name("lookup_table_refresher".to_string())
        .spawn(move || {
            // seed lookup table
            if let Err(e) = refresh_address_lookup_table(&rpc_load_balancer, &lookup_table) {
                error!("error refreshing address lookup table: {e:?}");
            }

            let tick_receiver = tick(Duration::from_secs(1));
            let mut last_refresh = Instant::now();

            while !exit.load(Ordering::Relaxed) {
                let _ = tick_receiver.recv();
                if last_refresh.elapsed() < refresh_duration {
                    continue;
                }

                let now = Instant::now();
                let refresh_result =
                    refresh_address_lookup_table(&rpc_load_balancer, &lookup_table);
                let updated_elapsed = now.elapsed().as_micros();
                match refresh_result {
                    Ok(_) => {
                        datapoint_info!(
                            "lookup_table_refresher-ok",
                            ("count", 1, i64),
                            ("lookup_table_size", lookup_table.len(), i64),
                            ("updated_elapsed_us", updated_elapsed, i64),
                        );
                    }
                    Err(e) => {
                        datapoint_error!(
                            "lookup_table_refresher-error",
                            ("count", 1, i64),
                            ("lookup_table_size", lookup_table.len(), i64),
                            ("updated_elapsed_us", updated_elapsed, i64),
                            ("error", e.to_string(), String),
                        );
                    }
                }
                last_refresh = Instant::now();
            }
        })
        .unwrap()
}

fn refresh_address_lookup_table(
    rpc_load_balancer: &Arc<LoadBalancer>,
    lookup_table: &DashMap<Pubkey, AddressLookupTableAccount>,
) -> solana_client::client_error::Result<()> {
    let rpc_client = rpc_load_balancer.rpc_client();

    let address_lookup_table =
        Pubkey::from_str("AddressLookupTab1e1111111111111111111111111").unwrap();
    let start = Instant::now();
    let accounts = rpc_client.get_program_accounts(&address_lookup_table)?;
    info!(
        "Fetched {} lookup tables from RPC in {:?}",
        accounts.len(),
        start.elapsed()
    );

    let mut new_pubkeys = HashSet::new();
    for (pubkey, account_data) in accounts {
        match AddressLookupTable::deserialize(&account_data.data) {
            Err(e) => {
                error!("error deserializing AddressLookupTable pubkey: {pubkey}, error: {e}");
            }
            Ok(table) => {
                debug!("lookup table loaded pubkey: {pubkey:?}, table: {table:?}");
                new_pubkeys.insert(pubkey);
                lookup_table.insert(
                    pubkey,
                    AddressLookupTableAccount {
                        key: pubkey,
                        addresses: table.addresses.to_vec(),
                    },
                );
            }
        }
    }

    // remove all the closed lookup tables
    lookup_table.retain(|pubkey, _| new_pubkeys.contains(pubkey));

    Ok(())
}
