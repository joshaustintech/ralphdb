# Next Steps
WIGGUM_REMAINING_WORK=yes

Are there important TODO items not yet captured? Yes.

- Add integration coverage for additional RESP3-native frame types (`map`, `set`, `attribute`, `push`, `verbatim`, `bignum`) on live TCP connections.
- Document and validate `redis-benchmark` command lines/results in `README.md` against current command set.
- Add concurrency-focused integration tests (multi-client contention on shared keys) to validate behavior under parallel load.
