# Next Steps
- Serialize tests that mutate `RALPHDB_IDLE_TIMEOUT_SECS` (unit + integration) with a global mutex or a serial helper to avoid env-var races.
- Make `idle_timeout_env_closes_connection` resilient by polling for EOF/close until a deadline and tolerating `TimedOut` reads.
- In `Server::handle_connection`, only treat `TimedOut`/`WouldBlock` as idle; handle `UnexpectedEof` as a normal client disconnect and avoid the idle log.
- Stop setting `set_read_timeout` on the writer socket clone; keep read timeout on the reader and (optionally) keep write timeout on the writer.
- Re-run `cargo test` and `cargo clippy -- -D warnings` after updating idle-timeout behavior/tests.
