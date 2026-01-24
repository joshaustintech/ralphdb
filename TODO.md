# Next Steps
- Decide whether RESP3 scalar argument frames (bool/double/bignum/etc.) should be accepted after `HELLO 3`; if so, coerce them into command arguments and update integration tests.
- Add integration coverage for `INFO` in both RESP2 and RESP3 modes, including unsupported-section errors.
- Implement another optional command from `RESP3_PLAN.md` (`CONFIG GET` or `CLIENT SETNAME`) with RESP2/RESP3 response coverage.
- Add a RESP2 vs RESP3 compatibility matrix to `README.md`, covering reply types and argument handling.
