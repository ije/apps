#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <unistd.h>

typedef void *napi_env;
typedef void *napi_value;
typedef void *napi_callback_info;
typedef void *napi_ref;
typedef int napi_status;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);
typedef void (*napi_finalize)(napi_env, void *, void *);
typedef struct {
  const char *utf8name;
  napi_value name;
  napi_callback method, getter, setter;
  napi_value value;
  unsigned attributes;
  void *data;
} napi_property_descriptor;

extern napi_status napi_create_object(napi_env, napi_value *);
extern napi_status napi_create_external(napi_env, void *, napi_finalize, void *, napi_value *);
extern napi_status napi_create_function(napi_env, const char *, size_t, napi_callback, void *, napi_value *);
extern napi_status napi_add_finalizer(napi_env, napi_value, void *, napi_finalize, void *, napi_ref *);
extern napi_status napi_get_date_value(napi_env, napi_value, double *);
extern napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t *, napi_value *, napi_value *, void **);
extern napi_status napi_get_new_target(napi_env, napi_callback_info, napi_value *);
extern napi_status napi_create_double(napi_env, double, napi_value *);
extern napi_status napi_get_value_double(napi_env, napi_value, double *);
extern napi_status napi_create_string_utf8(napi_env, const char *, size_t, napi_value *);
extern napi_status napi_get_value_string_utf8(napi_env, napi_value, char *, size_t, size_t *);
extern napi_status napi_create_buffer_copy(napi_env, size_t, const void *, void **, napi_value *);
extern napi_status napi_get_buffer_info(napi_env, napi_value, void **, size_t *);
extern napi_status napi_call_function(napi_env, napi_value, napi_value, size_t, const napi_value *, napi_value *);
extern napi_status napi_define_properties(napi_env, napi_value, size_t, const napi_property_descriptor *);
extern napi_status napi_define_class(napi_env, const char *, size_t, napi_callback, void *, size_t, const napi_property_descriptor *, napi_value *);
extern napi_status napi_set_named_property(napi_env, napi_value, const char *, napi_value);
extern napi_status napi_get_named_property(napi_env, napi_value, const char *, napi_value *);
extern napi_status napi_create_reference(napi_env, napi_value, uint32_t, napi_ref *);
extern napi_status napi_get_reference_value(napi_env, napi_ref, napi_value *);
extern napi_status napi_delete_reference(napi_env, napi_ref);
extern napi_status napi_wrap(napi_env, napi_value, void *, napi_finalize, void *, napi_ref *);
extern napi_status napi_unwrap(napi_env, napi_value, void **);
extern napi_status napi_remove_wrap(napi_env, napi_value, void **);
extern napi_status napi_throw_type_error(napi_env, const char *, const char *);
extern napi_status napi_is_exception_pending(napi_env, bool *);
extern napi_status napi_get_and_clear_last_exception(napi_env, napi_value *);
extern napi_status napi_open_escapable_handle_scope(napi_env, void **);
extern napi_status napi_escape_handle(napi_env, void *, napi_value, napi_value *);
extern napi_status napi_close_escapable_handle_scope(napi_env, void *);
extern napi_status napi_create_bigint_int64(napi_env, int64_t, napi_value *);
extern napi_status napi_get_value_bigint_int64(napi_env, napi_value, int64_t *, bool *);
extern napi_status napi_create_promise(napi_env, void **, napi_value *);
extern napi_status napi_resolve_deferred(napi_env, void *, napi_value);
extern napi_status napi_set_instance_data(napi_env, void *, napi_finalize, void *);
extern napi_status napi_get_instance_data(napi_env, void **);
extern napi_status napi_get_dataview_info(napi_env, napi_value, size_t *, void **, napi_value *, size_t *);
extern napi_status napi_get_value_int64(napi_env, napi_value, int64_t *);
extern napi_status napi_get_global(napi_env, napi_value *);
extern napi_status napi_create_external_buffer(napi_env, size_t, void *, napi_finalize, void *, napi_value *);

typedef void *napi_threadsafe_function;
typedef void *napi_async_work;
typedef void *napi_async_cleanup_hook_handle;
typedef struct { uint32_t major, minor, patch; const char *release; } napi_node_version;
typedef struct { const char *error_message; void *engine_reserved; uint32_t engine_error_code; napi_status error_code; } napi_extended_error_info;
extern napi_status napi_create_threadsafe_function(napi_env, napi_value, napi_value, napi_value, size_t, size_t, void *, napi_finalize, void *, void (*)(napi_env, napi_value, void *, void *), napi_threadsafe_function *);
extern napi_status napi_call_threadsafe_function(napi_threadsafe_function, void *, int);
extern napi_status napi_acquire_threadsafe_function(napi_threadsafe_function);
extern napi_status napi_release_threadsafe_function(napi_threadsafe_function, int);
extern napi_status napi_create_async_work(napi_env, napi_value, napi_value, void (*)(napi_env, void *), void (*)(napi_env, napi_status, void *), void *, napi_async_work *);
extern napi_status napi_queue_async_work(napi_env, napi_async_work);
extern napi_status napi_cancel_async_work(napi_env, napi_async_work);
extern napi_status napi_delete_async_work(napi_env, napi_async_work);
extern napi_status napi_reject_deferred(napi_env, void *, napi_value);
extern napi_status napi_get_node_version(napi_env, const napi_node_version **);
extern napi_status napi_get_uv_event_loop(napi_env, void **);
extern napi_status napi_get_all_property_names(napi_env, napi_value, int, int, int, napi_value *);
extern napi_status napi_detach_arraybuffer(napi_env, napi_value);
extern napi_status napi_is_detached_arraybuffer(napi_env, napi_value, bool *);
extern napi_status napi_create_bigint_words(napi_env, int, size_t, const uint64_t *, napi_value *);
extern napi_status napi_async_init(napi_env, napi_value, napi_value, void **);
extern napi_status napi_async_destroy(napi_env, void *);
extern napi_status napi_make_callback(napi_env, void *, napi_value, napi_value, size_t, const napi_value *, napi_value *);
extern napi_status napi_open_callback_scope(napi_env, napi_value, void *, void **);
extern napi_status napi_close_callback_scope(napi_env, void *);
extern napi_status napi_add_async_cleanup_hook(napi_env, void (*)(napi_async_cleanup_hook_handle, void *), void *, napi_async_cleanup_hook_handle *);
extern napi_status napi_remove_async_cleanup_hook(napi_async_cleanup_hook_handle);
extern napi_status napi_adjust_external_memory(napi_env, int64_t, int64_t *);
extern napi_status napi_get_last_error_info(napi_env, const napi_extended_error_info **);
extern napi_status napi_throw_error(napi_env, const char *, const char *);
extern napi_status napi_create_arraybuffer(napi_env, size_t, void **, napi_value *);
extern napi_status napi_get_boolean(napi_env, bool, napi_value *);

#define OK(call) do { if ((call) != 0) { napi_throw_type_error(env, "ERR_FIXTURE", #call); return NULL; } } while (0)

static void release(napi_env env, void *data, void *hint) {
  (void)env; (void)hint;
  free(data);
}

static napi_value add(napi_env env, napi_callback_info info) {
  napi_value args[3], result;
  size_t count = 3;
  void *data;
  OK(napi_get_cb_info(env, info, &count, args, NULL, &data));
  double a, b;
  OK(napi_get_value_double(env, args[0], &a));
  OK(napi_get_value_double(env, args[1], &b));
  OK(napi_create_double(env, a + b + (intptr_t)data, &result));
  return result;
}

static napi_value text(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1, size;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  char buffer[5];
  OK(napi_get_value_string_utf8(env, arg, buffer, sizeof(buffer), &size));
  OK(napi_create_string_utf8(env, buffer, size, &result));
  return result;
}

static napi_value mutate(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1, size;
  unsigned char *bytes;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_get_buffer_info(env, arg, (void **)&bytes, &size));
  for (size_t i = 0; i < size; i++) bytes[i]++;
  OK(napi_create_buffer_copy(env, size, bytes, NULL, &result));
  return result;
}

static napi_value view(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1, size;
  unsigned char *bytes;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_get_dataview_info(env, arg, &size, (void **)&bytes, NULL, NULL));
  for (size_t i = 0; i < size; i++) bytes[i]++;
  OK(napi_create_double(env, size, &result));
  return result;
}

static napi_value int64(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1;
  int64_t value;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_get_value_int64(env, arg, &value));
  OK(napi_create_double(env, (double)value, &result));
  return result;
}

static napi_value call(napi_env env, napi_callback_info info) {
  napi_value args[2], self, result;
  size_t count = 2;
  OK(napi_get_cb_info(env, info, &count, args, &self, NULL));
  OK(napi_call_function(env, self, args[0], 1, &args[1], &result));
  return result;
}

static napi_value caught(napi_env env, napi_callback_info info) {
  napi_value arg, self, result;
  size_t count = 1;
  bool pending;
  OK(napi_get_cb_info(env, info, &count, &arg, &self, NULL));
  napi_status status = napi_call_function(env, self, arg, 0, NULL, &result);
  OK(napi_is_exception_pending(env, &pending));
  if (status != 10 || !pending) { napi_throw_type_error(env, NULL, "missing exception"); return NULL; }
  OK(napi_get_and_clear_last_exception(env, &result));
  return result;
}

static napi_value scoped(napi_env env, napi_callback_info info) {
  (void)info;
  void *scope;
  napi_value value, escaped;
  OK(napi_open_escapable_handle_scope(env, &scope));
  OK(napi_create_string_utf8(env, "escaped", SIZE_MAX, &value));
  OK(napi_escape_handle(env, scope, value, &escaped));
  OK(napi_close_escapable_handle_scope(env, scope));
  return escaped;
}

static napi_value reference(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1;
  napi_ref ref;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_create_reference(env, arg, 1, &ref));
  OK(napi_get_reference_value(env, ref, &result));
  OK(napi_delete_reference(env, ref));
  return result;
}

static napi_value bigint(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1;
  int64_t value;
  bool lossless;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_get_value_bigint_int64(env, arg, &value, &lossless));
  if (!lossless) { napi_throw_type_error(env, NULL, "bigint overflow"); return NULL; }
  OK(napi_create_bigint_int64(env, value, &result));
  return result;
}

static napi_value promise(napi_env env, napi_callback_info info) {
  napi_value arg, result;
  size_t count = 1;
  void *deferred;
  OK(napi_get_cb_info(env, info, &count, &arg, NULL, NULL));
  OK(napi_create_promise(env, &deferred, &result));
  OK(napi_resolve_deferred(env, deferred, arg));
  return result;
}

static napi_value counter(napi_env env, napi_callback_info info) {
  napi_value self, target;
  OK(napi_get_cb_info(env, info, NULL, NULL, &self, NULL));
  OK(napi_get_new_target(env, info, &target));
  if (!target) { napi_throw_type_error(env, NULL, "use new"); return NULL; }
  double *value = calloc(1, sizeof(double));
  OK(napi_wrap(env, self, value, release, NULL, NULL));
  OK(napi_wrap(env, self, value, release, NULL, NULL) != 1);
  void *removed;
  OK(napi_remove_wrap(env, self, &removed));
  OK(removed != value);
  OK(napi_unwrap(env, self, &removed) != 1);
  OK(napi_wrap(env, self, value, release, NULL, NULL));
  return self;
}

static napi_value increment(napi_env env, napi_callback_info info) {
  napi_value self, result;
  double *value;
  OK(napi_get_cb_info(env, info, NULL, NULL, &self, NULL));
  OK(napi_unwrap(env, self, (void **)&value));
  OK(napi_create_double(env, ++*value, &result));
  return result;
}

static napi_value state(napi_env env, napi_callback_info info) {
  (void)info;
  int *count;
  napi_value result;
  OK(napi_get_instance_data(env, (void **)&count));
  OK(napi_create_double(env, ++*count, &result));
  return result;
}

static unsigned finalized = 0;
static void counted_release(napi_env env, void *data, void *hint) {
  (void)env; (void)hint; free(data); finalized++;
}
static napi_value garbage(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value object, external, function;
  OK(napi_create_object(env, &object));
  OK(napi_wrap(env, object, malloc(32), counted_release, NULL, NULL));
  OK(napi_create_external(env, malloc(32), counted_release, NULL, &external));
  OK(napi_create_function(env, "temporary", SIZE_MAX, add, NULL, &function));
  OK(napi_add_finalizer(env, function, malloc(32), counted_release, NULL, NULL));
  OK(napi_set_named_property(env, object, "external", external));
  OK(napi_set_named_property(env, object, "function", function));
  return object;
}
static napi_value finalizer_count(napi_env env, napi_callback_info info) {
  (void)info; napi_value result; OK(napi_create_double(env, finalized, &result)); return result;
}
static unsigned external_finalized = 0;
static void external_release(napi_env env, void *data, void *hint) {
  (void)env; (void)hint; free(data); external_finalized++;
}
static napi_value external_buffer(napi_env env, napi_callback_info info) {
  (void)info; napi_value result; char *data = malloc(4); memcpy(data, "napi", 4);
  OK(napi_create_external_buffer(env, 4, data, external_release, NULL, &result)); return result;
}
static napi_value external_count(napi_env env, napi_callback_info info) {
  (void)info; napi_value result; OK(napi_create_double(env, external_finalized, &result)); return result;
}
static napi_ref loader = NULL;
static napi_value set_loader(napi_env env, napi_callback_info info) {
  napi_value fn; size_t count = 1;
  OK(napi_get_cb_info(env, info, &count, &fn, NULL, NULL));
  OK(napi_create_reference(env, fn, 1, &loader)); return NULL;
}
static void call_loader(napi_env env, void *data, void *hint) {
  (void)data; (void)hint; napi_value fn, global;
  if (napi_get_reference_value(env, loader, &fn) == 0 && napi_get_global(env, &global) == 0) napi_call_function(env, global, fn, 0, NULL, NULL);
}
static napi_value loading(napi_env env, napi_callback_info info) {
  (void)info; napi_value object;
  OK(napi_create_object(env, &object));
  OK(napi_add_finalizer(env, object, NULL, call_loader, NULL, NULL)); return object;
}
static napi_value date_status(napi_env env, napi_callback_info info) {
  napi_value value, result; size_t count=1; double date;
  OK(napi_get_cb_info(env, info, &count, &value, NULL, NULL));
  int status=napi_get_date_value(env, value, &date);
  OK(napi_create_double(env, status, &result)); return result;
}

static void put(napi_env env, napi_value object, const char *name, double number) {
  napi_value value;
  if (napi_create_double(env, number, &value) == 0) napi_set_named_property(env, object, name, value);
}
static void cleanup_hook(napi_async_cleanup_hook_handle handle, void *arg) { (void)arg; napi_remove_async_cleanup_hook(handle); }
static napi_value node8(napi_env env, napi_callback_info info) {
  napi_value args[2], result, value, name, buffer;
  size_t count = 2;
  OK(napi_get_cb_info(env, info, &count, args, NULL, NULL));
  OK(napi_create_object(env, &result));
  const napi_node_version *version;
  OK(napi_get_node_version(env, &version));
  put(env, result, "major", version->major);
  void *loop = &loop;
  put(env, result, "noLoop", napi_get_uv_event_loop(env, &loop) != 0 && loop == NULL);
  OK(napi_get_all_property_names(env, args[0], 1, 2 | 16, 0, &value));
  OK(napi_set_named_property(env, result, "keys", value));
  bool before, after;
  OK(napi_create_arraybuffer(env, 8, NULL, &buffer));
  OK(napi_is_detached_arraybuffer(env, buffer, &before));
  OK(napi_detach_arraybuffer(env, buffer));
  OK(napi_is_detached_arraybuffer(env, buffer, &after));
  put(env, result, "detached", !before && after && napi_detach_arraybuffer(env, args[0]) == 19);
  uint64_t words[2] = {1, 1};
  OK(napi_create_bigint_words(env, 1, 2, words, &value));
  OK(napi_set_named_property(env, result, "words", value));
  void *context, *scope;
  OK(napi_create_string_utf8(env, "fixture", SIZE_MAX, &name));
  OK(napi_async_init(env, NULL, name, &context));
  OK(napi_open_callback_scope(env, args[0], context, &scope));
  OK(napi_make_callback(env, context, args[0], args[1], 1, &name, &value));
  OK(napi_close_callback_scope(env, scope));
  put(env, result, "mismatch", napi_close_callback_scope(env, scope) == 14);
  OK(napi_async_destroy(env, context));
  OK(napi_set_named_property(env, result, "called", value));
  int64_t memory, released;
  OK(napi_adjust_external_memory(env, 4096, &memory));
  OK(napi_adjust_external_memory(env, -4096, &released));
  put(env, result, "memory", memory == 4096 && released == 0);
  napi_async_cleanup_hook_handle handle;
  OK(napi_add_async_cleanup_hook(env, cleanup_hook, NULL, &handle));
  OK(napi_remove_async_cleanup_hook(handle));
  double number;
  const napi_extended_error_info *error;
  int status = napi_get_value_double(env, name, &number);
  OK(napi_get_last_error_info(env, &error));
  OK(napi_create_string_utf8(env, error->error_message, SIZE_MAX, &value));
  OK(napi_set_named_property(env, result, "message", value));
  put(env, result, "status", status);
  return result;
}

static unsigned threadsafe_finalized = 0;
static void threadsafe_finalize(napi_env env, void *data, void *hint) { (void)env; (void)data; (void)hint; threadsafe_finalized++; }
static void threadsafe_call(napi_env env, napi_value callback, void *context, void *data) {
  (void)context;
  if (!env) return;
  napi_value value, global;
  if (napi_create_double(env, (double)(intptr_t)data, &value) == 0 && napi_get_global(env, &global) == 0) napi_call_function(env, global, callback, 1, &value, NULL);
}
static void *threadsafe_producer(void *function) {
  for (intptr_t i = 1; i <= 50; i++) napi_call_threadsafe_function(function, (void *)i, 1);
  napi_release_threadsafe_function(function, 0);
  return NULL;
}
static void *threadsafe_spinner(void *function) {
  while (napi_call_threadsafe_function(function, (void *)-1, 0) != 16) usleep(100);
  return NULL;
}
static void *threadsafe_aborter(void *function) {
  usleep(2000);
  napi_release_threadsafe_function(function, 1);
  return NULL;
}
static napi_value threadsafe(napi_env env, napi_callback_info info) {
  napi_value callback, name;
  size_t count = 1;
  napi_threadsafe_function bounded, aborted;
  pthread_t thread;
  OK(napi_get_cb_info(env, info, &count, &callback, NULL, NULL));
  OK(napi_create_string_utf8(env, "fixture", SIZE_MAX, &name));
  OK(napi_create_threadsafe_function(env, callback, NULL, name, 2, 1, NULL, threadsafe_finalize, NULL, threadsafe_call, &bounded));
  OK(napi_call_threadsafe_function(bounded, (void *)0, 0));
  OK(napi_call_threadsafe_function(bounded, (void *)0, 0));
  OK(napi_call_threadsafe_function(bounded, (void *)0, 0) != 15);
  OK(napi_call_threadsafe_function(bounded, (void *)0, 1) != 21);
  OK(pthread_create(&thread, NULL, threadsafe_producer, bounded));
  pthread_detach(thread);
  OK(napi_create_threadsafe_function(env, callback, NULL, name, 0, 2, NULL, threadsafe_finalize, NULL, threadsafe_call, &aborted));
  OK(pthread_create(&thread, NULL, threadsafe_spinner, aborted));
  pthread_detach(thread);
  OK(pthread_create(&thread, NULL, threadsafe_aborter, aborted));
  pthread_detach(thread);
  return NULL;
}
static napi_value threadsafe_count(napi_env env, napi_callback_info info) {
  (void)info; napi_value result; OK(napi_create_double(env, threadsafe_finalized, &result)); return result;
}

static pthread_mutex_t work_lock = PTHREAD_MUTEX_INITIALIZER;
static int running = 0, most = 0, completed = 0, cancelled = 0;
static napi_async_work works[8];
static void work_execute(napi_env env, void *data) {
  (void)env; (void)data;
  pthread_mutex_lock(&work_lock); if (++running > most) most = running; pthread_mutex_unlock(&work_lock);
  usleep(30000);
  pthread_mutex_lock(&work_lock); running--; pthread_mutex_unlock(&work_lock);
}
static void work_complete(napi_env env, napi_status status, void *data) {
  if (status == 11) cancelled++;
  completed++;
  napi_delete_async_work(env, works[(intptr_t)data]);
}
static napi_value work(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value name;
  most = completed = cancelled = 0;
  OK(napi_create_string_utf8(env, "fixture", SIZE_MAX, &name));
  for (intptr_t i = 0; i < 8; i++) {
    OK(napi_create_async_work(env, NULL, name, work_execute, work_complete, (void *)i, &works[i]));
    OK(napi_queue_async_work(env, works[i]));
  }
  OK(napi_cancel_async_work(env, works[7]));
  return NULL;
}
static napi_value work_state(napi_env env, napi_callback_info info) {
  (void)info; napi_value result;
  OK(napi_create_object(env, &result));
  put(env, result, "completed", completed); put(env, result, "most", most); put(env, result, "cancelled", cancelled);
  return result;
}

static void throwing_release(napi_env env, void *data, void *hint) { (void)data; (void)hint; napi_throw_error(env, NULL, "finalizer failed"); }
static napi_value throwing(napi_env env, napi_callback_info info) {
  (void)info; napi_value object;
  OK(napi_create_object(env, &object));
  OK(napi_add_finalizer(env, object, NULL, throwing_release, NULL, NULL));
  return object;
}
static napi_value stable(napi_env env, napi_callback_info info) {
  (void)info; unsigned before = finalized; napi_value value; const napi_extended_error_info *error;
  for (int i = 0; i < 1000; i++) { OK(napi_create_object(env, &value)); OK(napi_get_last_error_info(env, &error)); }
  OK(napi_get_boolean(env, finalized == before, &value));
  return value;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  int *count = calloc(1, sizeof(int));
  OK(napi_set_instance_data(env, count, release, NULL));
  napi_async_cleanup_hook_handle handle;
  OK(napi_add_async_cleanup_hook(env, cleanup_hook, NULL, &handle));
  napi_property_descriptor methods[] = {
    {"node8", NULL, node8, NULL, NULL, NULL, 7, NULL},
    {"threadsafe", NULL, threadsafe, NULL, NULL, NULL, 7, NULL},
    {"threadsafeCount", NULL, threadsafe_count, NULL, NULL, NULL, 7, NULL},
    {"work", NULL, work, NULL, NULL, NULL, 7, NULL},
    {"workState", NULL, work_state, NULL, NULL, NULL, 7, NULL},
    {"throwing", NULL, throwing, NULL, NULL, NULL, 7, NULL},
    {"stable", NULL, stable, NULL, NULL, NULL, 7, NULL},
    {"garbage", NULL, garbage, NULL, NULL, NULL, 7, NULL},
    {"finalizerCount", NULL, finalizer_count, NULL, NULL, NULL, 7, NULL},
    {"externalBuffer", NULL, external_buffer, NULL, NULL, NULL, 7, NULL},
    {"externalCount", NULL, external_count, NULL, NULL, NULL, 7, NULL},
    {"setLoader", NULL, set_loader, NULL, NULL, NULL, 7, NULL},
    {"loading", NULL, loading, NULL, NULL, NULL, 7, NULL},
    {"dateStatus", NULL, date_status, NULL, NULL, NULL, 7, NULL},
    {"add", NULL, add, NULL, NULL, NULL, 7, (void *)1},
    {"text", NULL, text, NULL, NULL, NULL, 7, NULL},
    {"mutate", NULL, mutate, NULL, NULL, NULL, 7, NULL},
    {"view", NULL, view, NULL, NULL, NULL, 7, NULL},
    {"int64", NULL, int64, NULL, NULL, NULL, 7, NULL},
    {"call", NULL, call, NULL, NULL, NULL, 7, NULL},
    {"caught", NULL, caught, NULL, NULL, NULL, 7, NULL},
    {"scoped", NULL, scoped, NULL, NULL, NULL, 7, NULL},
    {"reference", NULL, reference, NULL, NULL, NULL, 7, NULL},
    {"bigint", NULL, bigint, NULL, NULL, NULL, 7, NULL},
    {"promise", NULL, promise, NULL, NULL, NULL, 7, NULL},
    {"state", NULL, state, NULL, NULL, NULL, 7, NULL},
  };
  OK(napi_define_properties(env, exports, sizeof(methods) / sizeof(methods[0]), methods));
  napi_property_descriptor method = {"increment", NULL, increment, NULL, NULL, NULL, 5, NULL};
  napi_value constructor;
  OK(napi_define_class(env, "Counter", SIZE_MAX, counter, NULL, 1, &method, &constructor));
  OK(napi_set_named_property(env, exports, "Counter", constructor));
  return exports;
}
