# Repository Guidelines

## Project Structure & Module Organization
- `src/` holds Rust source code; entry point is `src/main.rs`.
- `Cargo.toml` and `Cargo.lock` define dependencies and build configuration.
- `RESP3_PLAN.md` contains the implementation milestones for full RESP3 support.
- `README.md` documents usage and benchmarking.
- If added, place integration tests in `tests/` and protocol/engine modules under `src/` (e.g., `src/protocol/`, `src/server/`).

## Build, Test, and Development Commands
- `cargo build --release` builds an optimized binary.
- `cargo run --release` runs the server locally.
- `cargo test` runs unit and integration tests.
- `cargo fmt` formats code using rustfmt.
- `cargo clippy -- -D warnings` runs lints and treats warnings as errors.

## Coding Style & Naming Conventions
- Use standard Rust formatting (rustfmt). Indentation is 4 spaces.
- Prefer `snake_case` for functions/variables, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- Keep modules focused and named by responsibility (e.g., `protocol`, `storage`).
- Add brief comments only when logic is non-obvious.

## Testing Guidelines
- Use Rust’s built-in test framework (`#[test]`).
- Place unit tests in the same module file and integration tests in `tests/`.
- Name tests descriptively by behavior, e.g., `resp3_parses_push_frame`.
- Target thorough protocol coverage (RESP2/RESP3 parsing, edge cases, and negotiation).
- Test using the redis client CLI and redis-benchmark for issues

## Commit & Pull Request Guidelines
- Git history shows only an “Initial commit,” so no established message convention.
- Use concise, imperative commit messages (e.g., “Add RESP3 map parser”).
- PRs should include a short summary, testing notes (`cargo test` output or rationale), and links to relevant issues or plans (e.g., `RESP3_PLAN.md`).

## Security & Configuration Tips
- Avoid binding to public interfaces by default; use `127.0.0.1` for local development.
- Prefer environment variables for configuration (see `README.md`).
