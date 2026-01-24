for i in {0..20}; do
  codex exec -m gpt-5.1-codex-mini --config model_reasoning_effort="medium" --yolo "Using RESP3_PLAN.md, AGENTS.md, and TODO.md (if existing), implement and test a complete working Redis server clone called 'ralphdb'."
  codex exec -m gpt-5.2-codex --config model_reasoning_effort="medium" --yolo "Review changes and add/replace next steps in TODO.md. Create the file if it doesn't exist yet."
  codex exec -m gpt-5.1-codex-mini --config model_reasoning_effort="medium" --yolo "Fix any compilation errors if any and commit/push to GitHub."
done