# Next Steps
Are there important TODO items not yet captured? Yes.
Evaluation: Yes. `scripts/benchmark_profile.sh` now captures both stdout and stderr per run (alongside timeout vs command-failure metadata), but default-profile reruns and published baseline/candidate deltas are still pending.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors.
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun.

WIGGUM_REMAINING_WORK=yes
