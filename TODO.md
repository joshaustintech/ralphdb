# Next Steps
WIGGUM_REMAINING_WORK=yes

Are there important TODO items not yet captured? Yes.

- [ ] Add parser fuzz/property testing (partial frames, malformed RESP3 composites, and negotiation transitions) to harden protocol handling beyond current unit coverage.
- [ ] Add a benchmark profile with multi-client concurrency (`-c` and pipelining mixes) and document stability/latency deltas in `README.md`.
