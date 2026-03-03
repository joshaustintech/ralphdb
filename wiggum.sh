#!/usr/bin/env bash
set -u -o pipefail

MODEL="${MODEL:-gpt-5.3-codex}"
EFFORT="${EFFORT:-medium}"
SLEEP_SECS="${SLEEP_SECS:-1}"
DEFAULT_GREEN_CMD='cargo test --quiet && cargo check --quiet'
GREEN_CMD="${GREEN_CMD:-$DEFAULT_GREEN_CMD}"
MAX_ITERS="${MAX_ITERS:-}"

usage() {
  cat <<'EOF'
Usage: ./wiggum.sh [--max-iters N] [--green-cmd "command"] [--model NAME] [--effort LEVEL]

Options:
  --max-iters N       Maximum loop iterations (required unless MAX_ITERS env var is set).
  --green-cmd CMD     Command that defines "green" status.
                      Default: cargo test --quiet && cargo check --quiet
  --model NAME        Codex model to use (default: gpt-5.3-codex).
  --effort LEVEL      model_reasoning_effort value (default: medium).
  -h, --help          Show this help.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --max-iters)
      MAX_ITERS="${2:-}"
      shift 2
      ;;
    --green-cmd)
      GREEN_CMD="${2:-}"
      shift 2
      ;;
    --model)
      MODEL="${2:-}"
      shift 2
      ;;
    --effort)
      EFFORT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [ -z "$MAX_ITERS" ]; then
  read -r -p "Max iterations: " MAX_ITERS
fi

if ! [[ "$MAX_ITERS" =~ ^[1-9][0-9]*$ ]]; then
  echo "MAX_ITERS must be a positive integer." >&2
  exit 2
fi

if [ -z "$GREEN_CMD" ]; then
  echo "GREEN_CMD cannot be empty." >&2
  exit 2
fi

echo "Starting Wiggum loop (max iterations: $MAX_ITERS)"
echo "Green condition: $GREEN_CMD"

iteration=1

while [ "$iteration" -le "$MAX_ITERS" ]; do
  echo
  echo "=== Wiggum iteration $iteration/$MAX_ITERS ==="

  green_log="$(mktemp)"

  if eval "$GREEN_CMD" >"$green_log" 2>&1; then
    echo "Green condition satisfied before iteration work. Stopping."
    rm -f "$green_log"
    exit 0
  fi

  green_tail="$(tail -n 120 "$green_log")"
  git_status="$(git status --short | sed -n '1,40p')"
  rm -f "$green_log"

  prompt="$(cat <<EOF
You are running in a Wiggum loop to get to a green state.

Repository: $(pwd)
Iteration: $iteration of $MAX_ITERS
Green condition command:
$GREEN_CMD

Goal:
- Make one focused, high-confidence change that moves the repo toward the green condition.
- Follow AGENTS.md exactly.
- Keep scope small and verifiable.
- Run formatting/tests/checks relevant to the change.

Current git status (trimmed):
$git_status

Recent green-condition output (tail):
$green_tail
EOF
)"

  if ! codex exec -m "$MODEL" --config model_reasoning_effort="$EFFORT" --yolo "$prompt"; then
    echo "codex exec returned non-zero (continuing)."
  fi

  iteration=$((iteration + 1))
  if [ "$iteration" -le "$MAX_ITERS" ]; then
    sleep "$SLEEP_SECS"
  fi
done

echo
echo "Reached MAX_ITERS=$MAX_ITERS. Running final green check..."
if eval "$GREEN_CMD"; then
  echo "Green condition satisfied."
  exit 0
fi

echo "Green condition not satisfied after MAX_ITERS=$MAX_ITERS."
exit 1
