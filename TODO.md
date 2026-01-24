# Next Steps
- Relax HELLO 3 map assertions to require required keys only (avoid `len == 3`) so extra metadata can be added safely.
- Expand `HELLO 3` metadata to include RESP3 fields (`id`, `mode`, `role`, `modules`) and update tests accordingly.
- Add integration coverage for RESP2/RESP3 differences beyond nulls (booleans, doubles, maps/sets/pushes) to ensure RESP2 fallback/error behavior is stable.
- Extend protocol edge-case tests to cover oversized sets/pushes/attributes and malformed numeric frames (invalid integers, doubles, and big numbers).
