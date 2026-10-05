#!/usr/bin/env bash
# Stage the verified default nnnoiseless notice payload into an app bundle.
# Arguments: DAW workspace root, destination app Resources directory.

validate_nnnoiseless_model_notice_sources() {
    local daw_root="$1"
    local source_dir="$daw_root/assets/third-party/nnnoiseless"
    local source

    for source in \
        "$source_dir/MODEL_NOTICES.txt" \
        "$source_dir/nnnoiseless-COPYING.txt"; do
        if [ ! -f "$source" ] || [ ! -s "$source" ]; then
            printf 'ERROR: Required nnnoiseless model notice input is missing or empty: %s\n' \
                "$source" >&2
            return 1
        fi
    done
}

stage_nnnoiseless_model_notices() {
    local daw_root="$1"
    local destination="$2"
    local source_dir="$daw_root/assets/third-party/nnnoiseless"
    local name

    validate_nnnoiseless_model_notice_sources "$daw_root" || return 1
    mkdir -p "$destination" || return 1
    for name in MODEL_NOTICES.txt nnnoiseless-COPYING.txt; do
        if cmp -s "$source_dir/$name" "$destination/$name"; then
            continue
        fi
        cp "$source_dir/$name" "$destination/$name" || return 1
    done
}
