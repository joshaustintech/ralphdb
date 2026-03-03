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
LABEL="${1:-manual}"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="benchmark-results/${STAMP}-${LABEL}"

mkdir -p "${OUT_DIR}"

redis-cli -h "${HOST}" -p "${PORT}" MSET bench:k1 v1 bench:k2 v2 >/dev/null

run_case() {
  local protocol="$1"
  local mode="$2"
  local clients="$3"
  local pipeline="$4"
  local repeat="$5"
  local proto_name="resp2"

  if [[ "${protocol}" == "-3" ]]; then
    proto_name="resp3"
  fi

  local out_file="${OUT_DIR}/${proto_name}-${mode}-c${clients}-p${pipeline}-r${repeat}.txt"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}"

  if [[ "${mode}" == "basic" ]]; then
    redis-benchmark "${protocol}" -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}" -t set,get,incr >"${out_file}"
  elif [[ "${mode}" == "mget" ]]; then
    redis-benchmark "${protocol}" -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}" MGET bench:k1 bench:k2 >"${out_file}"
  else
    redis-benchmark "${protocol}" -h "${HOST}" -p "${PORT}" -n "${REQUESTS}" -c "${clients}" -P "${pipeline}" MSET bench:k1 v1 bench:k2 v2 >"${out_file}"
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
