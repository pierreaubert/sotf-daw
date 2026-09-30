#!/usr/bin/env bash
set -uo pipefail
artifact="/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud139-cpu-peak-shelf-r1"
bin="$artifact/bin/aud139_cpu_shape_comparison-release"
log="$artifact/logs/release-cpu-8.log"
{
  printf 'start_utc='
  date -u +%Y-%m-%dT%H:%M:%SZ
  printf 'requested_affinity_cpu=8\n'
  taskset -c 8 taskset -pc "$$"
  printf 'binary_sha256='
  sha256sum "$bin"
  printf 'shape_order=warmup_round_r uses (r+offset)%3; measured_round_t uses (t+offset)%3\n'
} > "$log"
set +e
taskset -c 8 bash -c 'taskset -pc "$$"; exec "$@"' aud139-cpu-launcher "$bin" --exact capture_aud139_matched_peak_and_shelf_callback_cpu --ignored --nocapture >> "$log" 2>&1
status=$?
set -e
printf 'terminal_exit_code=%s\n' "$status" >> "$log"
printf 'end_utc=' >> "$log"
date -u +%Y-%m-%dT%H:%M:%SZ >> "$log"
exit "$status"
