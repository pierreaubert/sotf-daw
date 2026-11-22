#!/bin/bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
au_dir="$(dirname -- "$script_dir")"
daw_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
target_dir="${CARGO_TARGET_DIR:-$daw_root/target}"
build_dir="${SOTF_AU_SMOKE_BUILD_DIR:-$daw_root/tmp/au-native-smoke}"
case "$target_dir" in
    /*) ;;
    *) target_dir="$daw_root/$target_dir" ;;
esac
case "$build_dir" in
    /*) ;;
    *) build_dir="$daw_root/$build_dir" ;;
esac
ffi_dir="$daw_root/crates/sotf-plugins/crates/plugins-ffi"
cargo_bin="${SOTF_CARGO_BIN:-cargo}"
mkdir -p "$build_dir/clang-cache" "$build_dir/swift-cache"

cd "$daw_root"
CARGO_TARGET_DIR="$target_dir" SOTF_FFI_SKIP_HEADER_SYNC=1 \
    "$cargo_bin" build --locked -p plugins-ffi --lib

xcrun clang -dynamiclib -O2 "$script_dir/allocator_interpose.c" \
    -install_name @rpath/libsotf-au-alloc-counter.dylib \
    -o "$build_dir/libsotf-au-alloc-counter.dylib"
xcrun clang -dynamiclib -fobjc-arc -fblocks -O0 "$script_dir/NativeHostProbe.m" \
    -L "$build_dir" -lsotf-au-alloc-counter \
    -install_name @rpath/libsotf-au-hostprobe.dylib \
    -o "$build_dir/libsotf-au-hostprobe.dylib"
xcrun clang -fobjc-arc -fblocks -O2 -c "$au_dir/GenericAU/NativeAudioUnit.m" \
    -o "$build_dir/NativeAudioUnit.o"
xcrun clang -O2 -Wall -Wextra -Werror -I "$ffi_dir" \
    "$script_dir/upmixer_ffi_allocation.c" \
    -L "$build_dir" -lsotf-au-alloc-counter -L "$target_dir/debug" -lplugins_ffi \
    -Wl,-rpath,"$build_dir" -Wl,-rpath,"$target_dir/debug" \
    -o "$build_dir/upmixer-ffi-allocation"
"$build_dir/upmixer-ffi-allocation"

sources=()
while IFS= read -r source; do
    case "$source" in
        */EQiOSAudioUnit/*) continue ;;
    esac
    sources+=("$source")
done < <(find "$au_dir" -name '*AudioUnit.swift' -type f)

swift_optimization=-O
if [[ "${SOTF_AU_SMOKE_DEBUG:-0}" == 1 ]]; then
    swift_optimization=-Onone
fi
CLANG_MODULE_CACHE_PATH="$build_dir/clang-cache" xcrun swiftc "$swift_optimization" -D SOTF_NATIVE_SMOKE \
    -module-cache-path "$build_dir/swift-cache" \
    -Xcc "-I$ffi_dir" -Xcc "-I$au_dir/GenericAU" \
    -import-objc-header "$script_dir/BridgingHeader.h" \
    "${sources[@]}" "$script_dir/main.swift" "$build_dir/NativeAudioUnit.o" \
    -L "$build_dir" -lsotf-au-hostprobe -lsotf-au-alloc-counter \
    -L "$target_dir/debug" -lplugins_ffi \
    -Xlinker -rpath -Xlinker "$build_dir" \
    -Xlinker -rpath -Xlinker "$target_dir/debug" \
    -o "$build_dir/native-smoke"

if [[ "${SOTF_AU_SMOKE_DEBUG:-0}" == 1 ]]; then
    SOTF_AU_ALLOCATION_ONLY=1 "$build_dir/native-smoke"
else
    SOTF_AU_PROJECT_YML="$au_dir/project.yml" "$build_dir/native-smoke"
fi
