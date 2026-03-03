# Next Steps
Are there important TODO items not yet captured? No.
Evaluation: No. This iteration replaced hardcoded benchmark run counting with protocol/mode-derived counts in `scripts/benchmark_profile.sh` and recorded those dimensions in metadata, reducing drift risk between scheduled runs and reported totals; remaining meaningful work is still to execute the default full benchmark matrix and publish refreshed baseline-vs-candidate deltas with run metadata context.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors plus finalized metadata (`total_runs_completed/remaining`, `run_completion_state`, `script_exit_kind`, `script_stage` with per-run context, `last_run_started_index`, and `last_run_started_label`).
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun, including `run-metadata.txt` context with expected/completed run counts.

WIGGUM_REMAINING_WORK=yes
