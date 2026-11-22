#include "sotf_audio_plugin_ffi.h"
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>

int __asan_address_is_poisoned(const void *);

static __typeof__(&plugin_create) api_create;
static __typeof__(&plugin_destroy) api_destroy;
static __typeof__(&plugin_set_parameter) api_set;
static __typeof__(&plugin_get_last_error) api_error;
static pthread_key_t host_key;
static const char *owned_error;
static int callback_ok;
static unsigned destructor_attempts;

static void on_thread_exit(void *context) {
    if (!__asan_address_is_poisoned(owned_error)) {
        if (++destructor_attempts < 4) (void)pthread_setspecific(host_key, context);
        return;
    }
    const char *error = api_error();
    callback_ok = !error || !strcmp(error, "Failed to set parameter: Unknown parameter: missing_control");
}

static void *worker(void *context) {
    struct PluginHandle *unit = context;
    if (api_set(unit, "missing_control", 0.0) != InvalidParameter) return (void *)1;
    owned_error = api_error();
    if (!owned_error || pthread_setspecific(host_key, unit)) return (void *)1;
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 2 || pthread_key_create(&host_key, on_thread_exit)) return 2;
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        fprintf(stderr, "dlopen: %s\n", dlerror());
        return 3;
    }
    api_create = (__typeof__(api_create))dlsym(library, "plugin_create");
    api_destroy = (__typeof__(api_destroy))dlsym(library, "plugin_destroy");
    api_set = (__typeof__(api_set))dlsym(library, "plugin_set_parameter");
    api_error = (__typeof__(api_error))dlsym(library, "plugin_get_last_error");
    if (!api_create || !api_destroy || !api_set || !api_error) return 4;
    struct PluginHandle *unit = api_create("Upmixer", "{\"speaker_config\":\"5.1.4\"}", 48000, 2, 10);
    if (!unit || api_set(unit, "missing_control", 0.0) != InvalidParameter || !api_error()) return 5;
    pthread_t thread;
    void *result = NULL;
    if (pthread_create(&thread, NULL, worker, unit) || pthread_join(thread, &result) || result) return 6;
    if (!callback_ok) return 7;
    pthread_key_delete(host_key);
    api_destroy(unit);
    dlclose(library);
    puts("thread-exit error lookup passed");
    return 0;
}
