# RESP3 Redis Clone: Instructions & Milestones

This document is the execution plan for building a Redis-compatible server that is fully RESP3 compliant, backwards compatible with RESP2, and production-grade enough to run `redis-benchmark` against it.

## Goals (Definition of Done)
- Full RESP3 protocol support, including capability negotiation and all RESP3 data types.
- Backwards compatibility with RESP2 clients.
- Robust, deterministic unit tests for all protocol handling, command semantics, and error cases.
- Concurrency strategy that can scale on multi-core CPUs (multithreading where appropriate).
- SIMD usage where it meaningfully improves performance (parsing/encoding/hot paths).
- `redis-benchmark` runs successfully and results are documented in the README.
- README contains complete, simple instructions for running the server, real-world usage, and benchmarking.

## Milestones

### Milestone 0: Project Foundations
- Confirm target language/runtime and tooling (Rust assumed by current repo).
- Establish crate layout:
  - `src/main.rs` (entry)
  - `src/server/mod.rs`
  - `src/protocol/resp2.rs`, `src/protocol/resp3.rs`
  - `src/command/` (command dispatch + handlers)
  - `src/storage/` (data structures)
  - `src/metrics/` (optional)
- Add `tests/` for integration tests.
- Add CI scripts (optional) for `cargo test` and `cargo fmt`.

### Milestone 1: Networking + Basic Event Loop
- Implement TCP listener and per-connection handling.
- Decide concurrency model (see notes below):
  - Multithreaded acceptor + worker pool, or
  - Thread-per-connection for early correctness, then evolve.
- Read/write framing buffer strategy (ring buffer or VecDeque).
- Graceful shutdown + timeouts for idle connections.

### Milestone 2: RESP2 Support (Compatibility Base)
- Implement full RESP2 parser/encoder:
  - Simple strings, errors, integers, bulk strings, arrays.
- Add parser tests including fuzz-style inputs, invalid frames, partial reads.
- Ensure interleaved read/write works with partial frames.

### Milestone 3: RESP3 Core Support
- Implement RESP3 data types:
  - Simple strings, errors, integers, bulk strings, arrays
  - Nulls, doubles, booleans
  - Maps, sets
  - Pushes, attributes, verbatim strings, big numbers
- Implement `HELLO 3` negotiation and capability tracking per connection.
- Add compatibility behavior: if client is RESP2, only return RESP2 data types.
- Verify RESP3 correctness via unit tests and golden frames.

### Milestone 4: Command Surface (Minimal Redis Compatibility)
- Implement a core set of commands sufficient for benchmarking and real use:
  - `PING`, `ECHO`, `HELLO`, `QUIT`
  - `GET`, `SET`, `DEL`, `EXISTS`, `INCR`, `DECR`
  - `MGET`, `MSET`
  - `EXPIRE`, `TTL`
  - Optional: `INFO`, `CONFIG GET`, `CLIENT SETNAME`
- Decide key-value data structures and eviction strategy (if any).
- Command tests for RESP2 and RESP3 reply formats.

### Milestone 5: Storage Engine + Data Model
- Start with in-memory map with sharding for concurrency.
- Use `RwLock`/`Mutex` or sharded lock strategy.
- If multithreading is used, ensure no data races and predictable behavior under contention.
- Consider time-based expiry via wheel or background cleanup.

### Milestone 6: Multithreading Strategy
- Choose a performance-safe model:
  - Single-threaded IO + multi-threaded command execution, or
  - Thread pool with sharded state, or
  - One-thread-per-core with socket affinity (advanced)
- Provide configuration to toggle threading mode for testing.
- Add tests for concurrency safety (e.g., `SET`/`GET` correctness under parallel access).

### Milestone 7: SIMD Opportunities
- Identify hotspots:
  - RESP parsing (e.g., scanning for CRLF, counting byte types)
  - Encoding output buffers
- Evaluate using `std::arch` or crates like `memchr` or `simdutf8`.
- Add benchmarks and tests to prove correctness with SIMD paths.
- Ensure fallback scalar path exists for unsupported architectures.

### Milestone 8: Benchmarking + `redis-benchmark`
- Add a documented benchmark mode in README.
- Validate `redis-benchmark` works against the server for:
  - `GET`, `SET`, `INCR`, `MGET`, `MSET`.
- Capture example command lines and a sample results block.
- Ensure running benchmark doesn't crash the server.

### Milestone 9: Hardening + Robust Testing
- Add property-based or fuzz tests for protocol parsing.
- Add regression tests for malformed frames and RESP3 negotiation.
- Stress tests for multi-client concurrency.
- Confirm memory safety and correctness with `cargo test` + `cargo clippy`.

### Milestone 10: Documentation and Definition of Done Review
- Update README with full run instructions, usage, and benchmark examples.
- Provide a compatibility matrix of RESP2 vs RESP3.
- Confirm all tests pass.

## Implementation Notes

### RESP3 Compatibility & Fallbacks
- Each connection tracks its protocol version and capabilities.
- RESP3 clients can use attributes and pushes; RESP2 clients must not receive them.
- The server should be strict about invalid frames to avoid protocol confusion.

### Multithreading
- Ensure deterministic behavior and avoid locking the entire map for hot paths.
- Consider a sharded `HashMap` by key hash to distribute locks.

### SIMD
- Prioritize correctness and clean fallbacks.
- Only enable SIMD when the target architecture supports it.

### Unit Testing Strategy
- Parser round-trip tests for each RESP2/RESP3 type.
- Edge cases: partial reads, nulls, empty collections, large payloads.
- Concurrency tests with multiple clients.

## Acceptance Checklist
- [ ] RESP3 and RESP2 fully supported.
- [ ] `HELLO 3` negotiation works; `HELLO 2` or default falls back to RESP2.
- [ ] `redis-benchmark` runs without error.
- [ ] Server handles concurrent clients.
- [ ] Unit tests cover parser and command semantics.
- [ ] README updated with run + usage + benchmark instructions.

