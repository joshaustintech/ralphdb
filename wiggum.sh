#!/usr/bin/env bash
set -u -o pipefail

MODEL="${MODEL:-gpt-5.3-codex}"
EFFORT="${EFFORT:-medium}"
# Conservative default pacing for ChatGPT Pro Codex usage windows.
SLEEP_SECS="${SLEEP_SECS:-90}"
DEFAULT_GREEN_CMD="RUSTFLAGS='-D warnings' cargo build --all-targets --all-features && RUSTFLAGS='-D warnings' cargo test --all-targets --all-features && cargo clippy --all-targets --all-features -- -D warnings"
GREEN_CMD="${GREEN_CMD:-$DEFAULT_GREEN_CMD}"
MAX_ITERS="${MAX_ITERS:-}"
TODO_FILE="${TODO_FILE:-TODO.md}"
TODO_MARKER_KEY="${TODO_MARKER_KEY:-WIGGUM_REMAINING_WORK}"
REQUIRE_TODO_GREEN="${REQUIRE_TODO_GREEN:-1}"
AUTO_COMMIT_PUSH="${AUTO_COMMIT_PUSH:-1}"

usage() {
  cat <<'EOF'
Usage: ./wiggum.sh [--max-iters N] [--green-cmd "command"] [--model NAME] [--effort LEVEL]

Options:
  --max-iters N       Maximum loop iterations (required unless MAX_ITERS env var is set).
  --green-cmd CMD     Command that defines "green" status.
                      Default: build + test + clippy with warnings denied.
  --model NAME        Codex model to use (default: gpt-5.3-codex).
  --effort LEVEL      model_reasoning_effort value (default: medium).
  -h, --help          Show this help.
EOF
}

todo_is_green() {
  if [ "$REQUIRE_TODO_GREEN" != "1" ]; then
    return 0
  fi

  if [ ! -f "$TODO_FILE" ]; then
    return 0
  fi

  marker_line="$(rg -N "^${TODO_MARKER_KEY}=(yes|no)$" "$TODO_FILE" | tail -n 1 || true)"
  [ "$marker_line" = "${TODO_MARKER_KEY}=no" ]
}

run_green_check() {
  eval "$GREEN_CMD" && todo_is_green
}

todo_status_details() {
  if [ ! -f "$TODO_FILE" ]; then
    echo "TODO file '$TODO_FILE' not found."
    return
  fi
  marker_line="$(rg -N "^${TODO_MARKER_KEY}=(yes|no)$" "$TODO_FILE" | tail -n 1 || true)"
  if [ -n "$marker_line" ]; then
    echo "Marker: $marker_line"
  else
    echo "Marker: missing (${TODO_MARKER_KEY}=yes|no required)"
  fi
  echo "TODO tail:"
  tail -n 40 "$TODO_FILE"
}

ensure_pushed() {
  local branch
  local ahead

  branch="$(git branch --show-current)"
  if [ -z "$branch" ]; then
    echo "Unable to determine current git branch for push." >&2
    return 1
  fi

  if git rev-parse --abbrev-ref --symbolic-full-name "@{u}" >/dev/null 2>&1; then
    ahead="$(git rev-list --count "@{u}..HEAD" 2>/dev/null || echo "0")"
    if [ "$ahead" -gt 0 ]; then
      echo "Pushing $ahead unpushed commit(s) to upstream..."
      git push
    fi
    return 0
  fi

  echo "No upstream configured for '$branch'; pushing with -u."
  git push -u origin "$branch"
}

sync_iteration_changes() {
  local context="$1"

  if [ "$AUTO_COMMIT_PUSH" != "1" ]; then
    return 0
  fi

  if [ -n "$(git status --porcelain)" ]; then
    git add -A
    if git commit -m "Wiggum iteration ${iteration}: ${context}"; then
      echo "Committed iteration changes."
    else
      echo "Failed to commit iteration changes." >&2
      return 1
    fi
  fi

  ensure_pushed
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
if [ "$REQUIRE_TODO_GREEN" = "1" ]; then
  echo "TODO gate: enabled (${TODO_FILE}, marker ${TODO_MARKER_KEY}=yes|no)"
fi
if [ "$AUTO_COMMIT_PUSH" = "1" ]; then
  echo "Git sync: auto-commit + push enabled."
fi

iteration=1

while [ "$iteration" -le "$MAX_ITERS" ]; do
  echo
  echo "=== Wiggum iteration $iteration/$MAX_ITERS ==="

  green_log="$(mktemp)"

  if run_green_check >"$green_log" 2>&1; then
    if ! sync_iteration_changes "green-checkpoint"; then
      echo "Failed to sync repository before exiting green." >&2
      rm -f "$green_log"
      exit 1
    fi
    echo "Green condition satisfied before iteration work. Stopping."
    rm -f "$green_log"
    exit 0
  fi

  green_tail="$(tail -n 120 "$green_log")"
  git_status="$(git status --short | sed -n '1,40p')"
  todo_snapshot="$(todo_status_details)"
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
- Do not run git commit/push yourself; the loop script handles commit/push each iteration.
- Update $TODO_FILE at the end of your work:
  - Explicitly evaluate this question before finishing: "Are there important TODO items not yet captured?"
  - If yes, add the missing TODO items immediately.
  - Keep/insert exactly one marker line: ${TODO_MARKER_KEY}=yes or ${TODO_MARKER_KEY}=no
  - Set to "yes" if any meaningful work remains for this repository.
  - Set to "no" only when no meaningful next engineering tasks remain.
  - Maintain a short checklist of remaining items when marker is "yes".

Current git status (trimmed):
$git_status

Recent green-condition output (tail):
$green_tail

TODO status snapshot:
$todo_snapshot
EOF
)"

  if ! codex exec -m "$MODEL" --config model_reasoning_effort="$EFFORT" --yolo "$prompt"; then
    echo "codex exec returned non-zero (continuing)."
  fi
  if ! sync_iteration_changes "post-codex"; then
    echo "Failed to sync repository changes for iteration $iteration." >&2
    exit 1
  fi

  iteration=$((iteration + 1))
  if [ "$iteration" -le "$MAX_ITERS" ]; then
    sleep "$SLEEP_SECS"
  fi
done

if ! sync_iteration_changes "post-loop"; then
  echo "Failed to sync repository after loop completion." >&2
  exit 1
fi

echo
echo "Reached MAX_ITERS=$MAX_ITERS. Running final green check..."
if run_green_check; then
  echo "Green condition satisfied."
  exit 0
fi

echo "Green condition not satisfied after MAX_ITERS=$MAX_ITERS."
if [ "$REQUIRE_TODO_GREEN" = "1" ]; then
  todo_status_details
fi
exit 1
