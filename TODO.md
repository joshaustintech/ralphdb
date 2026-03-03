# Next Steps
Are there important TODO items not yet captured? No.
Evaluation: No. This iteration fixed `scripts/benchmark_profile.sh` so benchmark command failures preserve and return the real exit code from `run_or_report`, preventing false-success reporting; existing benchmark rerun and result publication tasks remain the meaningful follow-up work.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors from the new timeout probe/reporting path.
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun.

WIGGUM_REMAINING_WORK=yes
