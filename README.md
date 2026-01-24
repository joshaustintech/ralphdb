# ralphdb

A Redis-compatible server clone that targets full RESP3 support with RESP2 backwards compatibility.

## Requirements
- Rust toolchain (stable). Install via `rustup`.
- Redis CLI tools (for `redis-benchmark` and `redis-cli`).

Install examples:
```bash
# macOS (Homebrew)
brew install rustup redis

# Ubuntu/Debian
sudo apt-get update
sudo apt-get install -y curl build-essential redis-tools
curl https://sh.rustup.rs -sSf | sh
```

## Build
```bash
cargo build --release
```

## Run
```bash
# Default listen address and port
cargo run --release

# Or run the built binary directly
./target/release/ralphdb
```

### Configuration
Set these environment variables to override defaults:
- `RALPHDB_HOST` (default: `127.0.0.1`)
- `RALPHDB_PORT` (default: `6379`)
- `RALPHDB_THREADS` (default: `0` for single-thread, >0 to enable thread pool)

## Real-World Usage
```bash
# Basic usage with redis-cli
redis-cli -p 6379 PING
redis-cli -p 6379 SET hello world
redis-cli -p 6379 GET hello

# RESP3 negotiation
redis-cli -p 6379 HELLO 3
redis-cli -p 6379 MSET key1 value1 key2 value2
redis-cli -p 6379 MGET key1 key2
```

## Features
- RESP3 handshake via `HELLO 3` with RESP2 fallback when needed.
- Core command surface: `PING`, `ECHO`, `SET`, `GET`, `DEL`, `EXISTS`, `INCR`, `DECR`, `MGET`, `MSET`, `EXPIRE`, and `TTL`.
- In-memory store with optional expirations and atomic counter support using a sharded `DashMap`.
- Configurable thread pool via `RALPHDB_THREADS` for multi-core command handling.

## Benchmarks (redis-benchmark)
```bash
# Basic benchmark
redis-benchmark -p 6379 -t set,get -n 100000 -c 50

# Pipeline benchmark
redis-benchmark -p 6379 -t set,get -n 100000 -c 50 -P 16
```

## Tests
```bash
cargo test
```

## RESP3 Compatibility
- RESP2 clients work out of the box; RESP3-specific capabilities activate when `HELLO 3` is negotiated.
- The protocol module parses RESP2/RESP3 frames and encodes replies that remain readable to either protocol version.

## Project Plan
See `RESP3_PLAN.md` for instructions and milestones to reach full RESP3 compatibility.
