# Next Steps
Are there important TODO items not yet captured? Yes.
Evaluation: Yes. Added per-run benchmark timeout handling in `scripts/benchmark_profile.sh`, but the default profile still needs a full rerun and published baseline/candidate deltas.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors.
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun.

WIGGUM_REMAINING_WORK=yes
