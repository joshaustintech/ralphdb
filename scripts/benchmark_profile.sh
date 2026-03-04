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

HOST_RAW="${HOST:-127.0.0.1}"
PORT_RAW="${PORT:-6379}"
REQUESTS_RAW="${REQUESTS:-10000}"
REPEATS_RAW="${REPEATS:-3}"
MIXES="${MIXES:-1:1 8:1 32:1 32:8}"
MODES_RAW="${MODES:-basic mget mset}"
BENCH_TIMEOUT_SECONDS_RAW="${BENCH_TIMEOUT_SECONDS:-120}"
LABEL_RAW="${1:-manual}"
STAMP="$(date +%Y%m%d-%H%M%S)"
SCRIPT_START_EPOCH="$(date +%s)"

# Trim HOST so whitespace-only values don't pass validation and cause opaque connection errors.
HOST="$(printf '%s' "${HOST_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${HOST}" ]]; then
  HOST="127.0.0.1"
fi
if [[ "${HOST}" =~ [[:space:]] ]]; then
  echo "HOST must not contain whitespace (got: ${HOST})." >&2
  exit 1
fi

PORT="$(printf '%s' "${PORT_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${PORT}" ]]; then
  PORT="6379"
fi

REQUESTS="$(printf '%s' "${REQUESTS_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${REQUESTS}" ]]; then
  REQUESTS="10000"
fi

REPEATS="$(printf '%s' "${REPEATS_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${REPEATS}" ]]; then
  REPEATS="3"
fi

BENCH_TIMEOUT_SECONDS="$(printf '%s' "${BENCH_TIMEOUT_SECONDS_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${BENCH_TIMEOUT_SECONDS}" ]]; then
  BENCH_TIMEOUT_SECONDS="120"
fi

normalize_nonnegative_integer() {
  local value="$1"
  value="${value#"${value%%[!0]*}"}"
  if [[ -z "${value}" ]]; then
    value="0"
  fi

  printf '%s' "${value}"
}

LABEL="$(printf '%s' "${LABEL_RAW}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "${LABEL}" ]]; then
  LABEL="manual"
fi

if ! [[ "${PORT}" =~ ^[1-9][0-9]{0,4}$ ]]; then
  echo "PORT must be an integer between 1 and 65535 (got: ${PORT})." >&2
  exit 1
fi
PORT_NUM=$((10#${PORT}))
if ((PORT_NUM > 65535)); then
  echo "PORT must be an integer between 1 and 65535 (got: ${PORT})." >&2
  exit 1
fi
PORT="${PORT_NUM}"

if ! [[ "${LABEL}" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "Label must contain only letters, numbers, dot, underscore, or dash (got: ${LABEL})." >&2
  exit 1
fi

OUT_DIR="benchmark-results/${STAMP}-${LABEL}"

if ! [[ "${REQUESTS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "REQUESTS must be a positive integer (got: ${REQUESTS})." >&2
  exit 1
fi
REQUESTS="$((10#${REQUESTS}))"

if ! [[ "${REPEATS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "REPEATS must be a positive integer (got: ${REPEATS})." >&2
  exit 1
fi
REPEATS="$((10#${REPEATS}))"

if ! [[ "${BENCH_TIMEOUT_SECONDS}" =~ ^[0-9]+$ ]]; then
  echo "BENCH_TIMEOUT_SECONDS must be a non-negative integer (got: ${BENCH_TIMEOUT_SECONDS})." >&2
  exit 1
fi

BENCH_TIMEOUT_SECONDS="$(normalize_nonnegative_integer "${BENCH_TIMEOUT_SECONDS}")"
BENCH_TIMEOUT_ENABLED=0
if [[ "${BENCH_TIMEOUT_SECONDS}" != "0" ]]; then
  BENCH_TIMEOUT_ENABLED=1
fi

if [[ -z "${MIXES//[[:space:]]/}" ]]; then
  echo "MIXES must include at least one clients:pipeline entry (for example: 1:1 8:1)." >&2
  exit 1
fi

if [[ -z "${MODES_RAW//[[:space:]]/}" ]]; then
  echo "MODES must include at least one benchmark mode (supported: basic mget mset)." >&2
  exit 1
fi

read -r -a mix_entries <<<"${MIXES}"
read -r -a mode_entries <<<"${MODES_RAW}"
MIX_COUNT="${#mix_entries[@]}"
SUPPORTED_MODES=("basic" "mget" "mset")
MODES=()
seen_modes=" "
for mode in "${mode_entries[@]}"; do
  case "${mode}" in
  basic | mget | mset)
    if [[ "${seen_modes}" == *" ${mode} "* ]]; then
      echo "Duplicate MODES entry '${mode}'. Each mode may be listed only once." >&2
      exit 1
    fi
    MODES+=("${mode}")
    seen_modes+="${mode} "
    ;;
  *)
    echo "Invalid MODES entry '${mode}'. Supported modes: ${SUPPORTED_MODES[*]}." >&2
    exit 1
    ;;
  esac
done
PROTOCOLS=("resp2" "resp3")
MODE_COUNT="${#MODES[@]}"
PROTOCOL_COUNT="${#PROTOCOLS[@]}"
TOTAL_RUNS=$((MIX_COUNT * REPEATS * MODE_COUNT * PROTOCOL_COUNT))
COMPLETED_RUNS=0

normalized_mix_entries=()
seen_mixes=" "
for mix in "${mix_entries[@]}"; do
  if ! [[ "${mix}" =~ ^[1-9][0-9]*:[1-9][0-9]*$ ]]; then
    echo "Invalid MIXES entry '${mix}'. Expected clients:pipeline with positive integers (example: 32:1)." >&2
    exit 1
  fi
  mix_clients="${mix%%:*}"
  mix_pipeline="${mix##*:}"
  mix_clients="$((10#${mix_clients}))"
  mix_pipeline="$((10#${mix_pipeline}))"
  normalized_mix="${mix_clients}:${mix_pipeline}"
  if [[ "${seen_mixes}" == *" ${normalized_mix} "* ]]; then
    echo "Duplicate MIXES entry '${mix}' (normalized as '${normalized_mix}'). Each clients:pipeline mix may be listed only once." >&2
    exit 1
  fi
  normalized_mix_entries+=("${normalized_mix}")
  seen_mixes+="${normalized_mix} "
done
MIXES="${normalized_mix_entries[*]}"
mix_entries=("${normalized_mix_entries[@]}")

if ((BENCH_TIMEOUT_ENABLED)) &&
  ! command -v timeout >/dev/null 2>&1 &&
  ! command -v gtimeout >/dev/null 2>&1; then
  echo "BENCH_TIMEOUT_SECONDS is set but neither timeout nor gtimeout is installed." >&2
  echo "Install one of them or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
  exit 1
fi

TIMEOUT_BIN=""
TIMEOUT_PROBE_EXIT=""
if ((BENCH_TIMEOUT_ENABLED)); then
  if command -v timeout >/dev/null 2>&1; then
    TIMEOUT_BIN="timeout"
  else
    TIMEOUT_BIN="gtimeout"
  fi
fi

is_timeout_status() {
  local status="$1"

  if ((!BENCH_TIMEOUT_ENABLED)); then
    return 1
  fi

  if ! [[ "${status}" =~ ^[0-9]+$ ]]; then
    return 1
  fi

  if [[ "${status}" -eq 124 || "${status}" -eq 137 || "${status}" -eq 143 ]]; then
    return 0
  fi

  if [[ -n "${TIMEOUT_PROBE_EXIT}" &&
    "${TIMEOUT_PROBE_EXIT}" =~ ^[0-9]+$ &&
    "${status}" -eq "${TIMEOUT_PROBE_EXIT}" ]]; then
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
  if ((BENCH_TIMEOUT_ENABLED)); then
    echo "timeout_probe_exit=pending"
  else
    echo "timeout_probe_exit=disabled"
  fi
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
last_run_started_output_file="none"
last_run_completed_index=0
last_run_completed_label="none"
last_run_completed_output_file="none"
failure_context="none"
sanitize_metadata_value() {
  local value="$1"

  value="${value//$'\r'/ }"
  value="${value//$'\n'/ }"
  value="${value//$'\t'/ }"
  value="$(printf '%s' "${value}" | sed -e 's/[[:space:]]\+/ /g' -e 's/^ //; s/ $//')"
  if [[ -z "${value}" ]]; then
    value="none"
  fi

  printf '%s' "${value}"
}

finalize_metadata() {
  local exit_status=$?
  local completion_state="incomplete"
  local runs_remaining=$((TOTAL_RUNS - COMPLETED_RUNS))
  local exit_kind="failure"
  local completion_timestamp
  local elapsed_seconds
  local timeout_probe_exit_final
  local sanitized_last_run_started_label
  local sanitized_last_run_started_output_file
  local sanitized_last_run_completed_label
  local sanitized_last_run_completed_output_file
  local sanitized_failure_context
  local sanitized_script_stage

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
    if ((BENCH_TIMEOUT_ENABLED)); then
      timeout_probe_exit_final="${TIMEOUT_PROBE_EXIT:-pending}"
    else
      timeout_probe_exit_final="disabled"
    fi
    sanitized_script_stage="$(sanitize_metadata_value "${script_stage}")"
    sanitized_last_run_started_label="$(sanitize_metadata_value "${last_run_started_label}")"
    sanitized_last_run_started_output_file="$(sanitize_metadata_value "${last_run_started_output_file}")"
    sanitized_last_run_completed_label="$(sanitize_metadata_value "${last_run_completed_label}")"
    sanitized_last_run_completed_output_file="$(sanitize_metadata_value "${last_run_completed_output_file}")"
    sanitized_failure_context="$(sanitize_metadata_value "${failure_context}")"
    {
      echo "total_runs_completed=${COMPLETED_RUNS}"
      echo "total_runs_remaining=${runs_remaining}"
      echo "run_completion_state=${completion_state}"
      echo "script_exit_kind=${exit_kind}"
      echo "script_exit_status=${exit_status}"
      echo "completion_timestamp=${completion_timestamp}"
      echo "elapsed_seconds=${elapsed_seconds}"
      echo "script_stage=${sanitized_script_stage}"
      echo "timeout_probe_exit_final=${timeout_probe_exit_final}"
      echo "last_run_started_index=${last_run_started_index}"
      echo "last_run_started_label=${sanitized_last_run_started_label}"
      echo "last_run_started_output_file=${sanitized_last_run_started_output_file}"
      echo "last_run_completed_index=${last_run_completed_index}"
      echo "last_run_completed_label=${sanitized_last_run_completed_label}"
      echo "last_run_completed_output_file=${sanitized_last_run_completed_output_file}"
      echo "failure_context=${sanitized_failure_context}"
    } >>"${metadata_file}"
    metadata_finalized=1
  fi
}
trap finalize_metadata EXIT

timeout_probe_status=0
timeout_probe_output=""
if ((BENCH_TIMEOUT_ENABLED)); then
  script_stage="preflight:timeout_probe"
  timeout_probe_output="$("${TIMEOUT_BIN}" 1 sh -c "sleep 2" 2>&1)" || timeout_probe_status=$?
  TIMEOUT_PROBE_EXIT="${timeout_probe_status}"
  if [[ "${timeout_probe_status}" -eq 0 ]]; then
    failure_context="preflight:timeout_probe:unexpected-success"
    echo "${TIMEOUT_BIN} completed a timeout probe without timing out." >&2
    echo "Verify ${TIMEOUT_BIN} is functioning correctly or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  fi
  if [[ "${timeout_probe_status}" -eq 125 || "${timeout_probe_status}" -eq 126 || "${timeout_probe_status}" -eq 127 ]]; then
    failure_context="preflight:timeout_probe:failure:${timeout_probe_status}"
    echo "${TIMEOUT_BIN} failed timeout probing with exit ${timeout_probe_status}." >&2
    if [[ -n "${timeout_probe_output}" ]]; then
      echo "Probe output: ${timeout_probe_output}" >&2
    fi
    echo "Install a working timeout tool or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  fi
  if [[ "${timeout_probe_status}" -ne 124 && "${timeout_probe_status}" -lt 128 ]]; then
    failure_context="preflight:timeout_probe:unexpected-exit:${timeout_probe_status}"
    echo "${TIMEOUT_BIN} probe ended with unexpected exit ${timeout_probe_status}." >&2
    if [[ -n "${timeout_probe_output}" ]]; then
      echo "Probe output: ${timeout_probe_output}" >&2
    fi
    echo "Expected a timeout status (124 or signal-based >=128)." >&2
    echo "Install a compatible timeout tool or set BENCH_TIMEOUT_SECONDS=0 to disable timeouts." >&2
    exit 1
  fi
fi

run_cli_preflight_or_report() {
  local preflight_name="$1"
  shift
  local status
  local output
  local quoted_cmd
  local cmd=(redis-cli --raw -h "${HOST}" -p "${PORT}" "$@")

  if ((BENCH_TIMEOUT_ENABLED)); then
    cmd=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}" "${cmd[@]}")
  fi

  if output="$("${cmd[@]}" 2>&1)"; then
    printf '%s' "${output}"
    return 0
  else
    status=$?
  fi

  quoted_cmd="$(printf '%q ' "${cmd[@]}")"
  if is_timeout_status "${status}"; then
    echo "Preflight '${preflight_name}' timed out after ${BENCH_TIMEOUT_SECONDS}s (exit ${status})." >&2
    failure_context="preflight:${preflight_name}:timeout:${status}"
  else
    echo "Preflight '${preflight_name}' failed with exit ${status}." >&2
    failure_context="preflight:${preflight_name}:failure:${status}"
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
    # Normalize CRLF-terminated output so response checks are stable.
    line="${line%$'\r'}"
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
  failure_context="preflight:connectivity:unexpected-response:${ping_response}"
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
  failure_context="preflight:seed:unexpected-response:${seed_response}"
  echo "Failed to seed benchmark keys via redis-cli at ${HOST}:${PORT}." >&2
  echo "Expected response: OK; last response line: ${seed_response}" >&2
  if [[ "${seed_output}" != "${seed_response}" ]]; then
    echo "Full preflight output: ${seed_output}" >&2
  fi
  echo "Verify the server is writable and reachable before rerunning the profile." >&2
  exit 1
fi

script_stage="preflight:resp3_probe"
resp3_probe_cmd=(redis-benchmark -h "${HOST}" -p "${PORT}" -n 1 -c 1 -P 1 "${RESP3_FLAG}" -t ping)
if ((BENCH_TIMEOUT_ENABLED)); then
  resp3_probe_cmd=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}" "${resp3_probe_cmd[@]}")
fi
resp3_probe_output=""
resp3_probe_status=0
resp3_probe_output="$("${resp3_probe_cmd[@]}" 2>&1)" || resp3_probe_status=$?
if ((resp3_probe_status != 0)); then
  quoted_resp3_probe_cmd="$(printf '%q ' "${resp3_probe_cmd[@]}")"
  if is_timeout_status "${resp3_probe_status}"; then
    echo "RESP3 benchmark preflight timed out after ${BENCH_TIMEOUT_SECONDS}s (exit ${resp3_probe_status})." >&2
    failure_context="preflight:resp3_probe:timeout:${resp3_probe_status}"
  else
    echo "RESP3 benchmark preflight failed with exit ${resp3_probe_status}." >&2
    failure_context="preflight:resp3_probe:failure:${resp3_probe_status}"
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
  local run_cmd=()

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
        echo "Benchmark timed out after ${BENCH_TIMEOUT_SECONDS}s (exit ${status})." >&2
        failure_context="benchmark:${last_run_started_label}:timeout:${status}:${out_file}"
      else
        echo "Benchmark command failed with exit ${status}." >&2
        failure_context="benchmark:${last_run_started_label}:failure:${status}:${out_file}"
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
  last_run_started_output_file="${out_file}"
  script_stage="benchmark:run${run_index}of${TOTAL_RUNS}:${proto_name}:${mode}:c${clients}:p${pipeline}:r${repeat}"
  echo "Running ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat} (run ${run_index}/${TOTAL_RUNS})"

  if [[ "${mode}" == "basic" ]]; then
    run_cmd=("${cmd_base[@]}" -t set,get,incr)
    if ((BENCH_TIMEOUT_ENABLED)); then
      run_cmd=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}" "${run_cmd[@]}")
    fi
    run_or_report "${out_file}" "${run_cmd[@]}" || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  elif [[ "${mode}" == "mget" ]]; then
    run_cmd=("${cmd_base[@]}" MGET bench:k1 bench:k2)
    if ((BENCH_TIMEOUT_ENABLED)); then
      run_cmd=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}" "${run_cmd[@]}")
    fi
    run_or_report "${out_file}" "${run_cmd[@]}" || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  elif [[ "${mode}" == "mset" ]]; then
    run_cmd=("${cmd_base[@]}" MSET bench:k1 v1 bench:k2 v2)
    if ((BENCH_TIMEOUT_ENABLED)); then
      run_cmd=("${TIMEOUT_BIN}" "${BENCH_TIMEOUT_SECONDS}" "${run_cmd[@]}")
    fi
    run_or_report "${out_file}" "${run_cmd[@]}" || {
      run_status=$?
      echo "Benchmark failed or timed out for ${proto_name} ${mode} c=${clients} p=${pipeline} repeat=${repeat}" >&2
      exit "${run_status}"
    }
  else
    failure_context="benchmark:${last_run_started_label}:invalid-mode:${mode}"
    echo "Unsupported benchmark mode '${mode}'. Supported modes: ${SUPPORTED_MODES[*]}." >&2
    exit 1
  fi

  COMPLETED_RUNS=$((COMPLETED_RUNS + 1))
  last_run_completed_index="${run_index}"
  last_run_completed_label="${last_run_started_label}"
  last_run_completed_output_file="${out_file}"
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
