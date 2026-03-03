#!/usr/bin/env bash
set -u -o pipefail

MODEL="${MODEL:-gpt-5.3-codex}"
EFFORT="${EFFORT:-medium}"
MAX_ITERS="${MAX_ITERS:-0}"        # 0 = run forever
SLEEP_SECS="${SLEEP_SECS:-1}"
STOP_ON_GREEN="${STOP_ON_GREEN:-0}" # 1 = stop after clean test/check pass
TEST_CMD="${TEST_CMD:-cargo test --quiet}"
CHECK_CMD="${CHECK_CMD:-cargo check --quiet}"

iteration=1

while :; do
  if [ "$MAX_ITERS" -gt 0 ] && [ "$iteration" -gt "$MAX_ITERS" ]; then
    echo "Reached MAX_ITERS=$MAX_ITERS. Stopping."
    exit 0
  fi

  echo
  echo "=== Wiggum iteration $iteration ==="

  test_log="$(mktemp)"
  check_log="$(mktemp)"

  if eval "$TEST_CMD" >"$test_log" 2>&1; then
    test_ok=1
  else
    test_ok=0
  fi

  if eval "$CHECK_CMD" >"$check_log" 2>&1; then
    check_ok=1
  else
    check_ok=0
  fi

  test_tail="$(tail -n 80 "$test_log")"
  check_tail="$(tail -n 80 "$check_log")"
  git_status="$(git status --short | sed -n '1,40p')"

  rm -f "$test_log" "$check_log"

  if [ "$test_ok" -eq 1 ] && [ "$check_ok" -eq 1 ]; then
    phase="green"
    task="Make exactly one small, high-confidence correctness improvement aligned with AGENTS.md. Add/adjust tests in the same change. Keep scope tight and avoid speculative refactors."
  else
    phase="red"
    task="Fix the highest-leverage correctness issue shown by the diagnostics. Make the smallest safe change that moves the repo toward passing tests/checks, and include/adjust tests if needed."
  fi

  prompt="$(cat <<EOF
You are running in a Wiggum loop to get gradually correct results.

Repository: $(pwd)
Phase: $phase

Task:
$task

Hard constraints:
- Follow AGENTS.md literally.
- Keep changes minimal and verifiable.
- Prefer correctness over cleverness.
- Run fmt + tests before finishing.

Current git status (trimmed):
$git_status

Recent cargo test output (tail):
$test_tail

Recent cargo check output (tail):
$check_tail
EOF
)"

  codex exec -m "$MODEL" --config model_reasoning_effort="$EFFORT" --yolo "$prompt"
  rc=$?

  if [ "$rc" -ne 0 ]; then
    echo "codex exec failed with exit code $rc (continuing loop)."
  fi

  if [ "$phase" = "green" ] && [ "$STOP_ON_GREEN" -eq 1 ]; then
    echo "Clean test/check state observed and STOP_ON_GREEN=1. Stopping."
    exit 0
  fi

  iteration=$((iteration + 1))
  sleep "$SLEEP_SECS"
done
