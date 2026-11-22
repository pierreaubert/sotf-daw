#include "sotf_audio_plugin_ffi.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void sotf_malloc_counter_begin(void);
void sotf_malloc_counter_end(uint64_t *, uint64_t *);

static int check_controls(struct PluginHandle *unit) {
    const char *ids[] = {
        "bass_mode", "bass_cutoff_hz", "bass_gain_db", "lfe_playback_gain_db",
        "combine_input_lfe", "bass_source_front_left", "bass_source_front_right",
        "bass_source_center", "bass_source_side_left", "bass_source_side_right",
        "bass_source_back_left", "bass_source_back_right", "input_format"
    };
    for (size_t i = 0; i < sizeof(ids) / sizeof(ids[0]); i++) {
        double current = plugin_get_parameter(unit, ids[i]);
        if (current < 0.0) return 1;
        double changed = current < 0.5 ? 1.0 : 0.0;
        // Initialize the calling thread's error storage before measuring.
        (void)plugin_set_parameter(unit, ids[i], changed);
        uint64_t allocations = 0, frees = 0;
        int failed = 0;
        sotf_malloc_counter_begin();
        for (int iteration = 0; iteration < 512; iteration++) {
            failed |= plugin_set_parameter(unit, ids[i], current) != 0;
            failed |= plugin_set_parameter(unit, ids[i], changed) == 0;
        }
        sotf_malloc_counter_end(&allocations, &frees);
        if (failed || allocations || frees) {
            fprintf(stderr, "%s: invalid status or %llu allocations, %llu frees\n", ids[i],
                    (unsigned long long)allocations, (unsigned long long)frees);
            return 1;
        }
    }
    const char *bypasses[] = {"bypass_heights", "bypass_surround"};
    for (size_t i = 0; i < sizeof(bypasses) / sizeof(bypasses[0]); i++) {
        uint64_t allocations = 0, frees = 0;
        int failed = 0;
        (void)plugin_set_parameter(unit, bypasses[i], 0.0);
        sotf_malloc_counter_begin();
        for (int iteration = 0; iteration < 512; iteration++) {
            failed |= plugin_set_parameter(unit, bypasses[i], 1.0) != 0;
            failed |= plugin_set_parameter(unit, bypasses[i], 0.0) != 0;
        }
        sotf_malloc_counter_end(&allocations, &frees);
        if (failed || allocations || frees) {
            fprintf(stderr, "%s: invalid status or %llu allocations, %llu frees\n", bypasses[i],
                    (unsigned long long)allocations, (unsigned long long)frees);
            return 1;
        }
    }
    const struct { const char *id; double first, second; } prepared[] = {
        {"lfe_cutoff_hz", 0.2, 0.6},
        {"bandpass_hz", 0.3, 0.7},
        {"height_hf_cap_hz", 0.2, 0.8},
        {"bypass_all_processing", 0.0, 1.0},
    };
    for (size_t i = 0; i < sizeof(prepared) / sizeof(prepared[0]); i++) {
        uint64_t allocations = 0, frees = 0;
        int failed = 0;
        sotf_malloc_counter_begin();
        for (int iteration = 0; iteration < 64; iteration++) {
            failed |= plugin_set_parameter(unit, prepared[i].id, prepared[i].first) != 0;
            failed |= plugin_set_parameter(unit, prepared[i].id, prepared[i].second) != 0;
        }
        sotf_malloc_counter_end(&allocations, &frees);
        if (failed || allocations || frees) {
            fprintf(stderr, "%s: invalid status or %llu allocations, %llu frees\n", prepared[i].id,
                    (unsigned long long)allocations, (unsigned long long)frees);
            return 1;
        }
    }
    uint64_t allocations = 0, frees = 0;
    int failed = 0;
    sotf_malloc_counter_begin();
    for (int iteration = 0; iteration < 1024; iteration++) {
        double value = (double)(iteration % 997) / 996.0;
        failed |= plugin_set_parameter_precise(unit, "height_gain", value) != 0;
    }
    sotf_malloc_counter_end(&allocations, &frees);
    if (failed || allocations || frees) {
        fprintf(stderr, "height gain ramp: invalid status or %llu allocations, %llu frees\n",
                (unsigned long long)allocations, (unsigned long long)frees);
        return 1;
    }
    return 0;
}

int main(void) {
    uint64_t allocations = 0, frees = 0;
    void *(*volatile allocate)(size_t) = malloc;
    void (*volatile release)(void *) = free;
    sotf_malloc_counter_begin();
    void *pointer = allocate(128);
    release(pointer);
    sotf_malloc_counter_end(&allocations, &frees);
    if (!allocations || !frees) {
        fputs("allocation counter self-test failed\n", stderr);
        return 1;
    }

    struct PluginHandle *stereo = plugin_create("Upmixer", "{}", 48000, 2, 6);
    if (!stereo) return 1;
    int failed = check_controls(stereo);
    plugin_destroy(stereo);
    if (failed) return 1;

    const char *configuration =
        "{\"speaker_config\":\"5.1.4\",\"input_layout\":{\"channels\":["
        "{\"index\":0,\"role\":\"front_left\"},{\"index\":1,\"role\":\"front_right\"},"
        "{\"index\":2,\"role\":\"front_center\"},{\"index\":3,\"role\":\"side_left\"},"
        "{\"index\":4,\"role\":\"side_right\"}]}}";
    struct PluginHandle *surround = plugin_create("Upmixer", configuration, 48000, 5, 10);
    if (!surround) return 1;
    failed = check_controls(surround);
    const char *state = "{\"bass_mode\":1,\"bass_cutoff_hz\":90.0,\"bass_gain_db\":-8.0,"
                        "\"lfe_playback_gain_db\":4.5,\"bass_source_center\":false}";
    if (!failed) {
        failed = plugin_load_state(surround, (const uint8_t *)state, strlen(state)) != 0;
    }
    if (!failed) failed = check_controls(surround);
    plugin_destroy(surround);
    if (failed) return 1;
    puts("Upmixer FFI structural-control allocation tests passed");
    return 0;
}
