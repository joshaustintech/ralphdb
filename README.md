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
redis-cli -p 6379 CONFIG GET server.*

# RESP3 negotiation
redis-cli -p 6379 HELLO 3
redis-cli -p 6379 MSET key1 value1 key2 value2
redis-cli -p 6379 MGET key1 key2
redis-cli -p 6379 INFO
```

## Features
- RESP3 handshake via `HELLO 3` with RESP2 fallback when needed.
- Core command surface: `PING`, `ECHO`, `SET`, `GET`, `DEL`, `EXISTS`, `INCR`, `DECR`, `MGET`, `MSET`, `EXPIRE`, and `TTL`.
- In-memory store with optional expirations and atomic counter support using a sharded `DashMap`.
- RESP3 parsing now understands maps, sets, attributes, push frames, verbatim strings, and big numbers and enforces payload/collection caps to avoid DoS injections.
- Configurable thread pool via `RALPHDB_THREADS` for multi-core command handling.
- Optional command: `INFO [section]` returns server metadata text (currently `server`/`default`) in both RESP2/RESP3 modes.
- `CONFIG GET <pattern>` exposes a small documented key set (server name, version, bind, port, threads) and understands simple wildcard patterns such as `server.*`.
- `CLIENT SETNAME` stores a per-connection name and `CLIENT GETNAME` returns that value (null if unset); both subcommands work for RESP2 and RESP3 sessions. `CLIENT ID` returns the numeric connection identifier while `CLIENT LIST` emits RESP3 attributes/push frames describing the caller so metadata is observable in one frame.
- RESP3 scalar arguments (booleans, doubles, big numbers, and verbatim strings) are coerced into byte arrays after `HELLO 3`, so existing commands accept them without special handling.

### INFO command
`INFO` returns a bulk-string that lists the server version, role, mode, and the stable `id` emitted via `HELLO 3`. Only the `server`/`default` sections are recognized; any other section yields `ERR unsupported INFO section '<name>'`, so clients always see deterministic text.

### Protocol Hardening
- Bulk strings are capped at 32 MiB and collections at 1 million entries to keep frame parsing predictable and resilient.
- Null bulk arguments are rejected and `PING`/`MSET` enforce their Redis-compatible arity expectations.
- Expired keys are treated as missing so repeated `EXPIRE` commands return `0` and `TTL` immediately reflects removal.

## Benchmarks (redis-benchmark)

Run two representative workloads against a locally running server:

```bash
redis-benchmark -p 6379 -t set,get -n 200 -c 5
redis-benchmark -p 6379 -t set,get -n 200 -c 5 -P 8
```

Sample throughput summaries from the runs above:

- `redis-benchmark -p 6379 -t set,get -n 200 -c 5`  
  SET: 20 000 requests per second (avg latency 0.203 ms)  
  GET: 25 000 requests per second (avg latency 0.172 ms)
- `redis-benchmark -p 6379 -t set,get -n 200 -c 5 -P 8`  
  SET: 99 999.99 requests per second (avg latency 0.231 ms)  
  GET: 66 666.66 requests per second (avg latency 0.249 ms)

`redis-benchmark` now sees a working `CONFIG GET` reply so the benchmark completes without the warning about missing configuration.

## Tests
```bash
cargo test
```
Tests cover the refreshed protocol parser (frame limits, RESP3 types) plus stricter command/expiry semantics (`PING`/`MSET` arity, `EXPIRE` on evicted keys, null argument rejection).

## RESP3 Compatibility
- RESP2 clients work out of the box; RESP3-specific capabilities activate when `HELLO 3` is negotiated.
- The protocol module parses RESP2/RESP3 frames and encodes replies that remain readable to either protocol version.
### HELLO 3 Metadata
`HELLO 3` replies with a RESP3 map describing the server state. The following keys are guaranteed:

| Key | Meaning |
| --- | ------- |
| `server` | The server name (`ralphdb`). |
| `version` | The current package version (from `CARGO_PKG_VERSION`). |
| `proto` | The negotiated protocol (`3`). |
| `id` | A stable server identifier (the crate name, so it does not change per connection). |
| `mode` | Always `standalone` in this build. |
| `role` | Always `primary`. |
| `modules` | Empty array for future module support. |

Clients can rely on `id` staying the same across connections and restarts (it mirrors the crate name), while other fields convey runtime metadata.

### RESP2 vs RESP3 Compatibility Matrix

| Behavior | RESP2 (default) | RESP3 (after `HELLO 3`) |
| --- | --- | --- |
| Scalar arguments | Only bulk strings and integers are accepted; RESP3 scalars (boolean, double, big number, verbatim) are rejected. | Scalars are coerced into byte strings (`true`/`false`, decimal doubles, textual big numbers, verbatim payloads) so existing commands keep working without special casts. |
| Null replies | Null values are encoded as bulk string nil (`$-1`). | Nulls emit the RESP3 literal (`_`) once the session is upgraded, but existing commands still return the same semantics. |
| Protocol metadata | `HELLO` is optional and defaults to RESP2; no capability negotiation happens. | `HELLO 3` upgrades the connection and replies with a stable metadata map that exposes `server`, version, `proto`, `id`, `mode`, `role`, and an empty `modules` array. |
| Client naming | `CLIENT SETNAME` works and `GETNAME` returns a null bulk string when unset. | Same behavior, but `GETNAME` replies with `_` once RESP3 is active (so clients can tell which mode they are in without parsing the value). |

## Project Plan
See `RESP3_PLAN.md` for instructions and milestones to reach full RESP3 compatibility.
