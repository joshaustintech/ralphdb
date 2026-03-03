# Next Steps
Are there important TODO items not yet captured? No.
Evaluation: No. This iteration added integration coverage that verifies invalid `HELLO` versions are rejected and the connection remains on RESP2 semantics; the remaining meaningful work is still the benchmark rerun and publication tasks already listed below.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors from the new timeout probe/reporting path.
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun.

WIGGUM_REMAINING_WORK=yes
