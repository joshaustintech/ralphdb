# Next Steps
- Run `cargo test` and `cargo clippy -- -D warnings` to validate the RESP3 attribute response path changes.
- Add a unit test to ensure `encode_response` drops attributes for RESP2 sessions and only emits them for RESP3.
- Decide whether RESP3 `CLIENT LIST` should expand its metadata (addr, flags, etc.) beyond `id`, `name`, and `protocol`, and document any additions in `README.md`.
- Refresh the `README.md` benchmark numbers with fresh `redis-benchmark` runs once the above stabilizes.
