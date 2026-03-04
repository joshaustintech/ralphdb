# Next Steps
Are there important TODO items not yet captured? No.
Evaluation: No. This iteration hardened `scripts/benchmark_profile.sh` so `timeout_probe_exit_final` always records the observed timeout probe exit code, including early probe failures. Meaningful remaining work is still benchmark execution and reporting.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors plus finalized metadata (`total_runs_completed/remaining`, `run_completion_state`, `script_exit_kind`, `script_stage` with per-run context, `last_run_started_index/label/output_file`, and `last_run_completed_index/label/output_file`).
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun, including `run-metadata.txt` context with expected/completed run counts.

WIGGUM_REMAINING_WORK=yes
