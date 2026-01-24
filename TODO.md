# Next Steps
- Guard RESP frame lengths: reject bulk/array lengths < -1, cap max payload size, and add parser tests for negative/oversized lengths.
- Fix expiry semantics: treat expired keys as missing in `expire`, and add tests that `EXPIRE` on expired keys returns 0.
- Tighten command arity and null handling: error on `PING` with >1 arg, `MSET` with 0 args, and null bulk arguments; add unit tests for each.
- Expand RESP3 coverage: implement maps/sets/attributes/push frames and RESP3-specific tests per `RESP3_PLAN.md`.
- Add integration tests for basic Redis command flows using `redis-cli`/golden RESP frames.
