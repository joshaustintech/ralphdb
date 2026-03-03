# Next Steps
Are there important TODO items not yet captured? No.
Evaluation: No. This iteration added protocol coverage for tab-only inline command rejection (`reject_tab_only_inline_command`) and revalidated the full build/test/clippy gate under warnings-as-errors; remaining meaningful work is still benchmark reruns and refreshed baseline-vs-candidate reporting.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors plus finalized metadata (`total_runs_completed/remaining`, `run_completion_state`, `script_exit_kind`, `script_stage` with per-run context, `last_run_started_index`, and `last_run_started_label`).
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun, including `run-metadata.txt` context with expected/completed run counts.

WIGGUM_REMAINING_WORK=yes
