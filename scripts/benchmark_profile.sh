#!/usr/bin/env bash
set -euo pipefail

if ! command -v redis-benchmark >/dev/null 2>&1; then
  echo "redis-benchmark is required but not installed." >&2
  exit 1
fi

redis_benchmark_help="$(redis-benchmark --help 2>&1 || true)"
if ! grep -Eq -- '(^|[[:space:]])-3([[:space:]]|$)|--resp3' <<<"${redis_benchmark_help}"; then
  echo "redis-benchmark does not advertise RESP3 (-3/--resp3) support." >&2
  echo "Install Redis tools with RESP3 support, or use a newer redis-benchmark binary." >&2
  exit 1
fi

RESP3_FLAG="-3"
if grep -Eq -- '(^|[[:space:]])--resp3([[:space:]]|$)' <<<"${redis_benchmark_help}" &&
  ! grep -Eq -- '(^|[[:space:]])-3([[:space:]]|$)' <<<"${redis_benchmark_help}"; then
  RESP3_FLAG="--resp3"
fi

if ! command -v redis-cli >/dev/null 2>&1; then
  echo "redis-cli is required but not installed." >&2
  exit 1
fi

HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-6379}"
REQUESTS="${REQUESTS:-10000}"
REPEATS="${REPEATS:-3}"
MIXES="${MIXES:-1:1 8:1 32:1 32:8}"
BENCH_TIMEOUT_SECONDS="${BENCH_TIMEOUT_SECONDS:-120}"
LABEL="${1:-manual}"
STAMP="$(date +%Y%m%d-%H%M%S)"
SCRIPT_START_EPOCH="$(date +%s)"

if [[ -z "${HOST}" ]]; then
  echo "HOST must be a non-empty hostname or IP address." >&2
  exit 1
fi

if ! [[ "${PORT}" =~ ^[1-9][0-9]{0,4}$ ]] || ((PORT > 65535)); then
  echo "PORT must be an integer between 1 and 65535 (got: ${PORT})." >&2
  exit 1
fi

if ! [[ "${LABEL}" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "Label must contain only letters, numbers, dot, underscore, or dash (got: ${LABEL})." >&2
  exit 1
fi

OUT_DIR="benchmark-results/${STAMP}-${LABEL}"

if ! [[ "${REQUESTS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "REQUESTS must be a positive integer (got: ${REQUESTS})." >&2
  exit 1
fi

if ! [[ "${REPEATS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "REPEATS must be a positive integer (got: ${REPEATS})." >&2
  exit 1
fi

if ! [[ "${BENCH_TIMEOUT_SECONDS}" =~ ^[0-9]+$ ]]; then
  echo "BENCH_TIMEOUT_SECONDS must be a non-negative integer (got: ${BENCH_TIMEOUT_SECONDS})." >&2
  exit 1
fi

BENCH_TIMEOUT_SECONDS_NUM=$((10#${BENCH_TIMEOUT_SECONDS}))

if [[ -z "${MIXES//[[:space:]]/}" ]]; then
  echo "MIXES must include at least one clients:pipeline entry (for example: 1:1 8:1)." >&2
  exit 1
fi

read -r -a mix_entries <<<"${MIXES}"
MIX_COUNT="${#mix_entries[@]}"
MODES=("basic" "mget" "mset")
PROTOCOLS=("resp2" "resp3")
MODE_COUNT="${#MODES[@]}"
PROTOCOL_COUNT="${#PROTOCOLS[@]}"
TOTAL_RUNS=$((MIX_COUNT * REPEATS * MODE_COUNT * PROTOCOL_COUNT))
COMPLETED_RUNS=0

for mix in "${mix_entries[@]}"; do
  if ! [[ "${mix}" =~ ^[1-9][0-9]*:[1-9][0-9]*$ ]]; then
    echo "Invalid MIXES entry '${mix}'. Expected clients:pipeline with positive integers (example: 32:1)." >&2
    exit 1
  fi
done

if ((BENCH_TIMEOUT_SECONDS_NUM > 0)) &&
  ! command -v timeout >/dev/null 2>&1 &&
  ! command -v gtimeout >/dev/null 2>&1; then
  echo "BENCH_TIMEOUT_SECONDS is set but neither timeout nor gtimeout is installed." >&2
  echo "Install one of them or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
  exit 1
fi

TIMEOUT_BIN=""
TIMEOUT_CMD=()
TIMEOUT_PROBE_EXIT=""
if ((BENCH_TIMEOUT_SECONDS_NUM > 0)); then
  if command -v timeout >/dev/null 2>&1; then
    TIMEOUT_BIN="timeout"
  else
    TIMEOUT_BIN="gtimeout"
  fi

  if "${TIMEOUT_BIN}" 1 sh -c "sleep 2" >/dev/null 2>&1; then
    echo "${TIMEOUT_BIN} completed a timeout probe without timing out." >&2
    echo "Verify ${TIMEOUT_BIN} is functioning correctly or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  else
    timeout_probe_status=$?
  fi
  if [[ "${timeout_probe_status}" -eq 125 || "${timeout_probe_status}" -eq 126 || "${timeout_probe_status}" -eq 127 ]]; then
    echo "${TIMEOUT_BIN} failed timeout probing with exit ${timeout_probe_status}." >&2
    echo "Install a working timeout tool or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  fi
  if [[ "${timeout_probe_status}" -ne 124 && "${timeout_probe_status}" -lt 128 ]]; then
    echo "${TIMEOUT_BIN} probe ended with unexpected exit ${timeout_probe_status}." >&2
    echo "Expected a timeout status (124 or signal-based >=128)." >&2
    echo "Install a compatible timeout tool or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  fi
  TIMEOUT_PROBE_EXIT="${timeout_probe_status}"

  TIMEOUT_CMD=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS_NUM}")
fi

is_timeout_status() {
  local status="$1"

  if ((BENCH_TIMEOUT_SECONDS_NUM <= 0)); then
    return 1
  fi

  if [[ "${status}" -eq 124 || "${status}" -eq 137 || "${status}" -eq 143 ]]; then
    return 0
  fi

  if [[ -n "${TIMEOUT_PROBE_EXIT}" && "${status}" -eq "${TIMEOUT_PROBE_EXIT}" ]]; then
    return 0
  fi

  return 1
}

mkdir -p "${OUT_DIR}"

metadata_file="${OUT_DIR}/run-metadata.txt"
{
  echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "label=${LABEL}"
  echo "out_dir=${OUT_DIR}"
  echo "host=${HOST}"
  echo "port=${PORT}"
  echo "requests=${REQUESTS}"
  echo "repeats=${REPEATS}"
  echo "mixes=${MIXES}"
  echo "modes=${MODES[*]}"
  echo "protocols=${PROTOCOLS[*]}"
  echo "run_iteration_order=mix,repeat,protocol,mode"
  echo "mode_count=${MODE_COUNT}"
  echo "protocol_count=${PROTOCOL_COUNT}"
  echo "bench_timeout_seconds=${BENCH_TIMEOUT_SECONDS}"
  echo "timeout_bin=${TIMEOUT_BIN:-disabled}"
  echo "timeout_probe_exit=${TIMEOUT_PROBE_EXIT:-disabled}"
  echo "resp3_flag=${RESP3_FLAG}"
  echo "mix_count=${MIX_COUNT}"
  echo "total_runs_expected=${TOTAL_RUNS}"
  echo "redis_benchmark=$(redis-benchmark --version 2>&1 | head -n 1)"
  echo "redis_cli=$(redis-cli --version 2>&1 | head -n 1)"
} >"${metadata_file}"

metadata_finalized=0
script_stage="preflight:connectivity"
last_run_started_index=0
last_run_started_label="none"
last_run_completed_index=0
last_run_completed_label="none"
finalize_metadata() {
  local exit_status=$?
  local completion_state="incomplete"
  local runs_remaining=$((TOTAL_RUNS - COMPLETED_RUNS))
  local exit_kind="failure"
  local completion_timestamp
  local elapsed_seconds

  completion_timestamp="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  elapsed_seconds=$(( $(date +%s) - SCRIPT_START_EPOCH ))

  if ((COMPLETED_RUNS >= TOTAL_RUNS)); then
    completion_state="complete"
    runs_remaining=0
  fi

  if [[ "${exit_status}" -eq 0 ]]; then
    exit_kind="success"
  elif is_timeout_status "${exit_status}"; then
    exit_kind="timeout"
  elif [[ "${exit_status}" -ge 128 ]]; then
    exit_kind="signal"
  fi

  if [[ "${metadata_finalized}" == "0" ]]; then
    if [[ "${exit_status}" -eq 0 ]]; then
      script_stage="complete"
    fi
    {
      echo "total_runs_completed=${COMPLETED_RUNS}"
      echo "total_runs_remaining=${runs_remaining}"
      echo "run_completion_state=${completion_state}"
      echo "script_exit_kind=${exit_kind}"
      echo "script_exit_status=${exit_status}"
      echo "completion_timestamp=${completion_timestamp}"
      echo "elapsed_seconds=${elapsed_seconds}"
      echo "script_stage=${script_stage}"
      echo "last_run_started_index=${last_run_started_index}"
      echo "last_run_started_label=${last_run_started_label}"
      echo "last_run_completed_index=${last_run_completed_index}"
      echo "last_run_completed_label=${last_run_completed_label}"
    } >>"${metadata_file}"
    metadata_finalized=1
  fi
}
trap finalize_metadata EXIT

run_cli_preflight_or_report() {
  local preflight_name="$1"
  shift
  local status
  local output
  local quoted_cmd
  local cmd=("${TIMEOUT_CMD[@]}" redis-cli --raw -h "${HOST}" -p "${PORT}" "$@")

  if output="$("${cmd[@]}" 2>&1)"; then
    printf '%s' "${output}"
    return 0
  fi

  status=$?
  quoted_cmd="$(printf '%q ' "${cmd[@]}")"
  if is_timeout_status "${status}"; then
    echo "Preflight '${preflight_name}' timed out after ${BENCH_TIMEOUT_SECONDS_NUM}s (exit ${status})." >&2
  else
    echo "Preflight '${preflight_name}' failed with exit ${status}." >&2
  fi
  echo "Command: ${quoted_cmd}" >&2
  if [[ -n "${output}" ]]; then
    echo "Output: ${output}" >&2
  fi
  return "${status}"
}

last_non_empty_line() {
  local input="$1"
  local line
  local last=""

  while IFS= read -r line; do
    if [[ -n "${line//[[:space:]]/}" ]]; then
      last="${line}"
    fi
  done <<<"${input}"

  printf '%s' "${last}"
}

script_stage="preflight:connectivity"
ping_status=0
ping_output="$(run_cli_preflight_or_report "connectivity" PING)" || ping_status=$?
if ((ping_status != 0)); then
  exit "${ping_status}"
fi
ping_response="$(last_non_empty_line "${ping_output}")"
if [[ "${ping_response}" != "PONG" ]]; then
  echo "Unable to validate Redis endpoint at ${HOST}:${PORT} with PING." >&2
  echo "Expected response: PONG; last response line: ${ping_response}" >&2
  if [[ "${ping_output}" != "${ping_response}" ]]; then
    echo "Full preflight output: ${ping_output}" >&2
  fi
  echo "Run the server first (for example: cargo run --release) or set HOST/PORT to a reachable endpoint." >&2
  exit 1
fi

script_stage="preflight:seed"
seed_status=0
seed_output="$(run_cli_preflight_or_report "seed" MSET bench:k1 v1 bench:k2 v2)" || seed_status=$?
if ((seed_status != 0)); then
  exit "${seed_status}"
fi
seed_response="$(last_non_empty_line "${seed_output}")"
if [[ "${seed_response}" != "OK" ]]; then
  echo "Failed to seed benchmark keys via redis-cli at ${HOST}:${PORT}." >&2
  echo "Expected response: OK; last response line: ${seed_response}" >&2
  if [[ "${seed_output}" != "${seed_response}" ]]; then
    echo "Full preflight output: ${seed_output}" >&2
  fi
  echo "Verify the server is writable and reachable before rerunning the profile." >&2
  exit 1
fi

script_stage="preflight:resp3_probe"
resp3_probe_cmd=("${TIMEOUT_CMD[@]}" redis-benchmark -h "${HOST}" -p "${PORT}" -n 1 -c 1 -P 1 "${RESP3_FLAG}" -t ping)
resp3_probe_output=""
resp3_probe_status=0
resp3_probe_output="$("${resp3_probe_cmd[@]}" 2>&1)" || resp3_probe_status=$?
if ((resp3_probe_status != 0)); then
  quoted_resp3_probe_cmd="$(printf '%q ' "${resp3_probe_cmd[@]}")"
  if is_timeout_status "${resp3_probe_status}"; then
    echo "RESP3 benchmark preflight timed out after ${BENCH_TIMEOUT_SECONDS_NUM}s (exit ${resp3_probe_status})." >&2
  else
    echo "RESP3 benchmark preflight failed with exit ${resp3_probe_status}." >&2
  fi
  echo "Command: ${quoted_resp3_probe_cmd}" >&2
  if [[ -n "${resp3_probe_output}" ]]; then
    echo "Output: ${resp3_probe_output}" >&2
  fi
  echo "Verify redis-benchmark RESP3 support and endpoint compatibility before rerunning the full profile." >&2
  exit "${resp3_probe_status}"
fi

script_stage="benchmark:runs"
run_case() {
  local protocol="$1"
  local mode="$2"
  local clients="$3"
  local pipeline="$4"
  local repeat="$5"
  local run_index=$((COMPLETED_RUNS + 1))
  local proto_name="${protocol}"
  local run_status
  local cmd_base=(redis-benchmark -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}")

  run_or_report() {
    local out_file="$1"
    shift

    if "$@" >"${out_file}" 2>&1; then
      return 0
    else
      local status=$?
      local quoted_cmd
      quoted_cmd="$(printf '%q ' "$@")"

      if is_timeout_status "${status}"; then
        echo "Benchmark timed out after ${BENCH_TIMEOUT_SECONDS_NUM}s (exit ${status})." >&2
      else
        echo "Benchmark command failed with exit ${status}." >&2
      fi
      echo "Command: ${quoted_cmd}" >&2
      echo "Output file: ${out_file}" >&2
      if [[ -s "${out_file}" ]]; then
        echo "Last 20 output lines:" >&2
        tail -n 20 "${out_file}" >&2
      fi
      return "${status}"
    fi
  }

  if [[ "${protocol}" == "resp3" ]]; then
    cmd_base+=( "${RESP3_FLAG}" )
  fi

  local out_file="${OUT_DIR}/${proto_name}-${mode}-c${clients}-p${pipeline}-r${repeat}.txt"
  last_run_started_index="${run_index}"
  last_run_started_label="${proto_name}:${mode}:c${clients}:p${pipeline}:r${repeat}"
  script_stage="benchmark:run${run_index}of${TOTAL_RUNS}:${proto_name}:${mode}:c${clients}:p${pipeline}:r${repeat}"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat} (run ${run_index}/${TOTAL_RUNS})"

  if [[ "${mode}" == "basic" ]]; then
    run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" -t set,get,incr || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  elif [[ "${mode}" == "mget" ]]; then
    run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" MGET bench:k1 bench:k2 || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  else
    run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" MSET bench:k1 v1 bench:k2 v2 || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  fi

  COMPLETED_RUNS=$((COMPLETED_RUNS + 1))
  last_run_completed_index="${run_index}"
  last_run_completed_label="${last_run_started_label}"
}

for mix in "${mix_entries[@]}"; do
  clients="${mix%%:*}"
  pipeline="${mix##*:}"
  for ((repeat = 1; repeat <= REPEATS; repeat++)); do
    for protocol in "${PROTOCOLS[@]}"; do
      for mode in "${MODES[@]}"; do
        run_case "${protocol}" "${mode}" "${clients}" "${pipeline}" "${repeat}"
      done
    done
  done
done

cat <<EOF
Saved profile results to ${OUT_DIR}
Run metadata: ${metadata_file}
Mixes: ${MIXES}
Requests per run: ${REQUESTS}
Repeats per mix/protocol/mode: ${REPEATS}
Completed runs: ${COMPLETED_RUNS}/${TOTAL_RUNS}
EOF
