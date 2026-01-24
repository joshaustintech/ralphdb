# Next Steps
- Fix RESP3 verbatim string parsing/encoding to use length-prefixed payloads (`=<len>\r\nfmt:payload\r\n`) and validate the 3-byte format tag; add round-trip tests.
- Add RESP3 encode tests for map/set/attribute/push/bignum frames (and null map/set) to ensure protocol version gating is correct.
- Extend protocol tests for collection length caps on maps/sets/attributes/push frames (including rejecting negative lengths where applicable).
- Add integration tests for basic Redis command flows using `redis-cli` or golden RESP frames.
