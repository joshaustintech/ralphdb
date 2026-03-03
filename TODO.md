# Next Steps
Are there important TODO items not yet captured? Yes.
Evaluation: Yes. This iteration added `script_stage` tracking to `scripts/benchmark_profile.sh`; the rerun checklist now explicitly includes validating that stage metadata on both success and failure paths.

- [ ] Rerun `scripts/benchmark_profile.sh` with the default `MIXES` (including `32:1`) and verify timeout-protected runs complete or fail fast with actionable errors plus finalized metadata (`total_runs_completed/remaining`, `run_completion_state`, `script_exit_kind`, `script_stage`).
- [ ] Publish updated full default-profile baseline vs candidate benchmark deltas after the rerun, including `run-metadata.txt` context with expected/completed run counts.

WIGGUM_REMAINING_WORK=yes
