#!/bin/bash
set -euo pipefail

if [[ $# != 1 || ! -f "$1" ]]; then
    printf 'Usage: %s /absolute/path/to/libplugins_ffi.dylib\n' "$0" >&2
    exit 2
fi
if [[ "$(uname -s)" != Darwin ]]; then
    printf 'This thread-exit regression requires macOS.\n' >&2
    exit 2
fi

test_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
crate_dir="$(dirname -- "$test_dir")"
library="$(cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/sotf-ffi-error-asan.XXXXXXXX")"
trap 'rm -rf -- "$build_dir"' EXIT

xcrun clang -O1 -g -fsanitize=address -Wall -Wextra -Werror -pthread \
    -I "$crate_dir" "$test_dir/error_thread_exit_asan.c" \
    -o "$build_dir/error-thread-exit-asan"
ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0}" "$build_dir/error-thread-exit-asan" "$library"
