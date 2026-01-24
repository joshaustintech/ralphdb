# Next Steps
- Run `cargo test` and `cargo clippy -- -D warnings` after the refactor to confirm no regressions.
- Add focused unit tests for `matches_pattern` covering `*suffix` and `prefix*` cases to lock in behavior.
- Add a protocol test that asserts RESP3 attributes are encoded before the response frame.
- Re-run `cargo fmt` if any rustfmt warnings appear in CI.
