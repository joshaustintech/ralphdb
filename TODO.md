# Next Steps
- Decide on a RESP3-safe `CLIENT LIST` reply shape (attributes must precede a reply); either return a map/array response and optionally send a push, or add protocol support for attribute + reply frames.
- Add integration coverage for `CLIENT ID` (RESP2 + RESP3) and `CLIENT LIST` in RESP2 mode to confirm encodings and null handling stay consistent.
- Extend `CONFIG GET` tests for non-matching patterns (empty array) and exact key lookup (e.g., `server.name`) in both RESP2 and RESP3 modes.
- Document the exact `CONFIG GET` key list in `README.md` (with a stable ordering) so users know what `redis-benchmark` consumes.
