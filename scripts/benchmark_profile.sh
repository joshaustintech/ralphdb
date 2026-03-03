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
TOTAL_RUNS=$((MIX_COUNT * REPEATS * 6))
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
  fi
  timeout_probe_status=$?
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

mkdir -p "${OUT_DIR}"

metadata_file="${OUT_DIR}/run-metadata.txt"
{
  echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host=${HOST}"
  echo "port=${PORT}"
  echo "requests=${REQUESTS}"
  echo "repeats=${REPEATS}"
  echo "mixes=${MIXES}"
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
finalize_metadata() {
  local exit_status=$?
  local completion_state="incomplete"
  local runs_remaining=$((TOTAL_RUNS - COMPLETED_RUNS))
  local exit_kind="failure"

  if ((COMPLETED_RUNS >= TOTAL_RUNS)); then
    completion_state="complete"
    runs_remaining=0
  fi

  if [[ "${exit_status}" -eq 0 ]]; then
    exit_kind="success"
  elif ((BENCH_TIMEOUT_SECONDS_NUM > 0)) &&
    [[ "${exit_status}" -eq 124 || "${exit_status}" -eq 137 || "${exit_status}" -eq "${TIMEOUT_PROBE_EXIT}" ]]; then
    exit_kind="timeout"
  elif [[ "${exit_status}" -ge 128 ]]; then
    exit_kind="signal"
  fi

  if [[ "${metadata_finalized}" == "0" ]]; then
    {
      echo "total_runs_completed=${COMPLETED_RUNS}"
      echo "total_runs_remaining=${runs_remaining}"
      echo "run_completion_state=${completion_state}"
      echo "script_exit_kind=${exit_kind}"
      echo "script_exit_status=${exit_status}"
    } >>"${metadata_file}"
    metadata_finalized=1
  fi
}
trap finalize_metadata EXIT

ping_output="$(redis-cli --raw -h "${HOST}" -p "${PORT}" PING 2>&1 || true)"
if [[ "${ping_output}" != "PONG" ]]; then
  echo "Unable to validate Redis endpoint at ${HOST}:${PORT} with PING." >&2
  echo "Expected response: PONG; received: ${ping_output}" >&2
  echo "Run the server first (for example: cargo run --release) or set HOST/PORT to a reachable endpoint." >&2
  exit 1
fi

seed_output="$(redis-cli --raw -h "${HOST}" -p "${PORT}" MSET bench:k1 v1 bench:k2 v2 2>&1 || true)"
if [[ "${seed_output}" != "OK" ]]; then
  echo "Failed to seed benchmark keys via redis-cli at ${HOST}:${PORT}." >&2
  echo "Expected response: OK; received: ${seed_output}" >&2
  echo "Verify the server is writable and reachable before rerunning the profile." >&2
  exit 1
fi

resp3_probe_cmd=("${TIMEOUT_CMD[@]}" redis-benchmark -h "${HOST}" -p "${PORT}" -n 1 -c 1 -P 1 "${RESP3_FLAG}" -t ping)
if ! "${resp3_probe_cmd[@]}" >/dev/null 2>&1; then
  resp3_probe_status=$?
  quoted_resp3_probe_cmd="$(printf '%q ' "${resp3_probe_cmd[@]}")"
  echo "RESP3 benchmark preflight failed with exit ${resp3_probe_status}." >&2
  echo "Command: ${quoted_resp3_probe_cmd}" >&2
  echo "Verify redis-benchmark RESP3 support and endpoint compatibility before rerunning the full profile." >&2
  exit "${resp3_probe_status}"
fi

run_case() {
  local protocol="$1"
  local mode="$2"
  local clients="$3"
  local pipeline="$4"
  local repeat="$5"
  local proto_name="resp2"
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

      if ((BENCH_TIMEOUT_SECONDS_NUM > 0)) &&
        [[ "${status}" -eq 124 || "${status}" -eq 137 || "${status}" -eq "${TIMEOUT_PROBE_EXIT}" ]]; then
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
    proto_name="resp3"
    cmd_base+=( "${RESP3_FLAG}" )
  fi

  local out_file="${OUT_DIR}/${proto_name}-${mode}-c${clients}-p${pipeline}-r${repeat}.txt"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}"

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
}

for mix in "${mix_entries[@]}"; do
  clients="${mix%%:*}"
  pipeline="${mix##*:}"
  for ((repeat = 1; repeat <= REPEATS; repeat++)); do
    run_case "" "basic" "${clients}" "${pipeline}" "${repeat}"
    run_case "" "mget" "${clients}" "${pipeline}" "${repeat}"
    run_case "" "mset" "${clients}" "${pipeline}" "${repeat}"
    run_case "resp3" "basic" "${clients}" "${pipeline}" "${repeat}"
    run_case "resp3" "mget" "${clients}" "${pipeline}" "${repeat}"
    run_case "resp3" "mset" "${clients}" "${pipeline}" "${repeat}"
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
