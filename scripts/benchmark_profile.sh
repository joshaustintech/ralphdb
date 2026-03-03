#!/usr/bin/env bash
set -euo pipefail

if ! command -v redis-benchmark >/dev/null 2>&1; then
  echo "redis-benchmark is required but not installed." >&2
  exit 1
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
OUT_DIR="benchmark-results/${STAMP}-${LABEL}"

if ! [[ "${BENCH_TIMEOUT_SECONDS}" =~ ^[0-9]+$ ]]; then
  echo "BENCH_TIMEOUT_SECONDS must be a non-negative integer (got: ${BENCH_TIMEOUT_SECONDS})." >&2
  exit 1
fi

if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]] &&
  ! command -v timeout >/dev/null 2>&1 &&
  ! command -v gtimeout >/dev/null 2>&1; then
  echo "BENCH_TIMEOUT_SECONDS is set but neither timeout nor gtimeout is installed." >&2
  echo "Install one of them or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

redis-cli -h "${HOST}" -p "${PORT}" MSET bench:k1 v1 bench:k2 v2 >/dev/null

run_case() {
  local protocol="$1"
  local mode="$2"
  local clients="$3"
  local pipeline="$4"
  local repeat="$5"
  local proto_name="resp2"
  local cmd_base=(redis-benchmark -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}")

  if [[ "${protocol}" == "-3" ]]; then
    proto_name="resp3"
    cmd_base+=( -3 )
  fi

  local out_file="${OUT_DIR}/${proto_name}-${mode}-c${clients}-p${pipeline}-r${repeat}.txt"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}"

  local timeout_cmd=()
  if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]]; then
    if command -v timeout >/dev/null 2>&1; then
      timeout_cmd=(timeout "${BENCH_TIMEOUT_SECONDS}")
    else
      timeout_cmd=(gtimeout "${BENCH_TIMEOUT_SECONDS}")
    fi
  fi

  if [[ "${mode}" == "basic" ]]; then
    if ! "${timeout_cmd[@]}" "${cmd_base[@]}" -t set,get,incr >"${out_file}"; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  elif [[ "${mode}" == "mget" ]]; then
    if ! "${timeout_cmd[@]}" "${cmd_base[@]}" MGET bench:k1 bench:k2 >"${out_file}"; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  else
    if ! "${timeout_cmd[@]}" "${cmd_base[@]}" MSET bench:k1 v1 bench:k2 v2 >"${out_file}"; then
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit 1
    fi
  fi
}

for mix in ${MIXES}; do
  clients="${mix%%:*}"
  pipeline="${mix##*:}"
  for repeat in $(seq 1 "${REPEATS}"); do
    run_case "" "basic" "${clients}" "${pipeline}" "${repeat}"
    run_case "" "mget" "${clients}" "${pipeline}" "${repeat}"
    run_case "" "mset" "${clients}" "${pipeline}" "${repeat}"
    run_case "-3" "basic" "${clients}" "${pipeline}" "${repeat}"
    run_case "-3" "mget" "${clients}" "${pipeline}" "${repeat}"
    run_case "-3" "mset" "${clients}" "${pipeline}" "${repeat}"
  done
done

cat <<EOF
Saved profile results to ${OUT_DIR}
Mixes: ${MIXES}
Requests per run: ${REQUESTS}
Repeats per mix/protocol/mode: ${REPEATS}
EOF
