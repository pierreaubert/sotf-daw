#include <malloc/malloc.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static _Thread_local int count_all;
static _Thread_local int count_render;
static _Thread_local int render_depth;
static _Thread_local int in_hook;
static _Thread_local uint64_t allocations;
static _Thread_local uint64_t frees;
static _Thread_local void *callers[32];
static _Thread_local uint64_t caller_counts[32];
static _Thread_local int caller_count;

static void record_caller(void *caller) {
    for (int i = 0; i < caller_count; i++) {
        if (callers[i] == caller) {
            caller_counts[i]++;
            return;
        }
    }
    if (caller_count < 32) {
        callers[caller_count] = caller;
        caller_counts[caller_count++] = 1;
    }
}

static int counting(void) {
    return !in_hook && (count_all || (count_render && render_depth));
}

static void *counted_malloc(size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc(size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_calloc(size_t count, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = calloc(count, size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_realloc(void *pointer, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = realloc(pointer, size);
    in_hook--;
    if (active) {
        if (result) { allocations++; record_caller(__builtin_return_address(0)); }
        if (pointer && (result || size == 0)) frees++;
    }
    return result;
}

static void counted_free(void *pointer) {
    const int active = counting();
    in_hook++;
    free(pointer);
    in_hook--;
    if (active && pointer) frees++;
}

static int counted_posix_memalign(void **pointer, size_t alignment, size_t size) {
    const int active = counting();
    in_hook++;
    int result = posix_memalign(pointer, alignment, size);
    in_hook--;
    if (active && result == 0 && *pointer) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_zone_malloc(malloc_zone_t *zone, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc_zone_malloc(zone, size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_zone_calloc(malloc_zone_t *zone, size_t count, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc_zone_calloc(zone, count, size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_zone_realloc(malloc_zone_t *zone, void *pointer, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc_zone_realloc(zone, pointer, size);
    in_hook--;
    if (active) {
        if (result) { allocations++; record_caller(__builtin_return_address(0)); }
        if (pointer && (result || size == 0)) frees++;
    }
    return result;
}

static void *counted_zone_memalign(malloc_zone_t *zone, size_t alignment, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc_zone_memalign(zone, alignment, size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void *counted_zone_valloc(malloc_zone_t *zone, size_t size) {
    const int active = counting();
    in_hook++;
    void *result = malloc_zone_valloc(zone, size);
    in_hook--;
    if (active && result) { allocations++; record_caller(__builtin_return_address(0)); }
    return result;
}

static void counted_zone_free(malloc_zone_t *zone, void *pointer) {
    const int active = counting();
    in_hook++;
    malloc_zone_free(zone, pointer);
    in_hook--;
    if (active && pointer) frees++;
}

#define INTERPOSE(replacement, original) \
    __attribute__((used)) static const struct { const void *replacement; const void *original; } \
    interpose_##replacement __attribute__((section("__DATA,__interpose"))) = { \
        (const void *)(uintptr_t)&replacement, (const void *)(uintptr_t)&original \
    }

INTERPOSE(counted_malloc, malloc);
INTERPOSE(counted_calloc, calloc);
INTERPOSE(counted_realloc, realloc);
INTERPOSE(counted_free, free);
INTERPOSE(counted_posix_memalign, posix_memalign);
INTERPOSE(counted_zone_malloc, malloc_zone_malloc);
INTERPOSE(counted_zone_calloc, malloc_zone_calloc);
INTERPOSE(counted_zone_realloc, malloc_zone_realloc);
INTERPOSE(counted_zone_memalign, malloc_zone_memalign);
INTERPOSE(counted_zone_valloc, malloc_zone_valloc);
INTERPOSE(counted_zone_free, malloc_zone_free);

void sotf_malloc_counter_begin(void) {
    allocations = 0;
    frees = 0;
    caller_count = 0;
    count_all = 1;
}

void sotf_malloc_counter_end(uint64_t *a, uint64_t *f) {
    count_all = 0;
    *a = allocations;
    *f = frees;
}

void sotf_malloc_counter_dump_callers(void) {
    for (int i = 0; i < caller_count; i++) {
        Dl_info info = {0};
        dladdr(callers[i], &info);
        fprintf(stderr, "caller %p count %llu image %s symbol %s base %p\n",
                callers[i], (unsigned long long)caller_counts[i],
                info.dli_fname ? info.dli_fname : "?", info.dli_sname ? info.dli_sname : "?",
                info.dli_fbase);
    }
}
void sotf_render_counter_arm(void) {
    allocations = 0;
    frees = 0;
    caller_count = 0;
    count_render = 1;
}
void sotf_render_counter_disarm(uint64_t *a, uint64_t *f) {
    count_render = 0;
    *a = allocations;
    *f = frees;
}
void sotf_render_counter_enter(void) { render_depth++; }
void sotf_render_counter_leave(void) { render_depth--; }
