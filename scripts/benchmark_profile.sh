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

if [[ -z "${MIXES//[[:space:]]/}" ]]; then
  echo "MIXES must include at least one clients:pipeline entry (for example: 1:1 8:1)." >&2
  exit 1
fi

read -r -a mix_entries <<<"${MIXES}"

for mix in "${mix_entries[@]}"; do
  if ! [[ "${mix}" =~ ^[1-9][0-9]*:[1-9][0-9]*$ ]]; then
    echo "Invalid MIXES entry '${mix}'. Expected clients:pipeline with positive integers (example: 32:1)." >&2
    exit 1
  fi
done

if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]] &&
  ! command -v timeout >/dev/null 2>&1 &&
  ! command -v gtimeout >/dev/null 2>&1; then
  echo "BENCH_TIMEOUT_SECONDS is set but neither timeout nor gtimeout is installed." >&2
  echo "Install one of them or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
  exit 1
fi

TIMEOUT_BIN=""
TIMEOUT_CMD=()
if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]]; then
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

  TIMEOUT_CMD=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}")
fi

mkdir -p "${OUT_DIR}"

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

run_case() {
  local protocol="$1"
  local mode="$2"
  local clients="$3"
  local pipeline="$4"
  local repeat="$5"
  local proto_name="resp2"
  local cmd_base=(redis-benchmark -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}")

  run_or_report() {
    local out_file="$1"
    shift

    if "$@" >"${out_file}" 2>&1; then
      return 0
    fi

    local status=$?
    local quoted_cmd
    quoted_cmd="$(printf '%q ' "$@")"

    if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]] && [[ "${status}" -eq 124 || "${status}" -eq 137 ]]; then
      echo "Benchmark timed out after ${BENCH_TIMEOUT_SECONDS}s (exit ${status})." >&2
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
  }

  if [[ "${protocol}" == "resp3" ]]; then
    proto_name="resp3"
    cmd_base+=( "${RESP3_FLAG}" )
  fi

  local out_file="${OUT_DIR}/${proto_name}-${mode}-c${clients}-p${pipeline}-r${repeat}.txt"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}"

  if [[ "${mode}" == "basic" ]]; then
    if ! run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" -t set,get,incr; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  elif [[ "${mode}" == "mget" ]]; then
    if ! run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" MGET bench:k1 bench:k2; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  else
    if ! run_or_report "${out_file}" "${TIMEOUT_CMD[@]}" "${cmd_base[@]}" MSET bench:k1 v1 bench:k2 v2; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  fi
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
Mixes: ${MIXES}
Requests per run: ${REQUESTS}
Repeats per mix/protocol/mode: ${REPEATS}
EOF
