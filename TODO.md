# Next Steps
- Update `HELLO 3` to return a RESP3 map payload (per spec) and adjust tests to assert key/value contents for both RESP2 and RESP3 negotiation.
- Add protocol tests for malformed verbatim frames (bad length, missing colon, missing CRLF) and oversized collection lengths to validate error handling.
- Add integration coverage for RESP2 defaults vs RESP3 null semantics (e.g., `$-1` vs `_`) and protocol-gated types returning errors under RESP2.
