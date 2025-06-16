# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Jito Relayer is a Rust-based TPU (Transaction Processing Unit) proxy for Solana validators, part of Jito's MEV infrastructure. It forwards transactions to validators while integrating with the block engine for MEV bundle processing.

## Development Commands

### Build and Setup
```bash
# Initialize submodules (required for protobuf definitions)
git submodule update --init --recursive

# On macOS, set OpenSSL environment variables
export OPENSSL_DIR=$(brew --prefix openssl)
export PKG_CONFIG_PATH="$OPENSSL_DIR/lib/pkgconfig:$PKG_CONFIG_PATH"

# Build release version
cargo build --release

# Build specific crate
cargo build -p transaction-relayer
```

### Testing
```bash
# Run all tests with logging
RUST_LOG=info cargo test

# Test specific crate
cargo test -p relayer
```

### Code Quality
```bash
# Run clippy (uses nightly toolchain)
cargo +nightly-2024-09-05-x86_64-unknown-linux-gnu clippy --all-targets

# Check for unused dependencies
cargo +nightly-2024-09-05-x86_64-unknown-linux-gnu udeps --locked

# Format code (follows custom rustfmt.toml)
cargo fmt
```

## Architecture

### Workspace Structure
- **`block_engine/`** - MEV block engine integration
- **`core/`** - Core Solana functionality without validator dependencies
- **`jito-protos/`** - gRPC protocol buffer definitions
- **`relayer/`** - Authentication, health management, and TPU proxy logic
- **`rpc/`** - RPC load balancing
- **`transaction-relayer/`** - Main binary and gRPC server orchestration
- **`web/`** - Web server component

### Key Components
- **TPU Proxy**: QUIC-based high-performance packet forwarding to Solana validators
- **Authentication Service**: JWT-based auth with challenge-response using PEM keys
- **Schedule Cache**: Leader schedule caching for optimal validator targeting
- **Block Engine Integration**: MEV bundle processing and OFAC compliance filtering

### gRPC Services
- Auth service for authentication endpoints
- Relayer service for TPU configuration and packet subscription
- Block engine service for MEV bundle processing
- JSON RPC HTTP endpoints alongside gRPC

## Important Notes

- Uses Solana v2.1.16 dependencies
- Requires nightly Rust toolchain (2024-09-05) for clippy and udeps
- Uses tikv-jemallocator for optimized memory allocation
- Docker builds use multi-stage process with Rust 1.64 slim
- Protocol buffers require submodule initialization before building (`git submodule update --init --recursive`)