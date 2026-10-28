#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

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
static napi_value date_status(napi_env env, napi_callback_info info) {
  napi_value value, result; size_t count=1; double date;
  OK(napi_get_cb_info(env, info, &count, &value, NULL, NULL));
  int status=napi_get_date_value(env, value, &date);
  OK(napi_create_double(env, status, &result)); return result;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  int *count = calloc(1, sizeof(int));
  OK(napi_set_instance_data(env, count, release, NULL));
  napi_property_descriptor methods[] = {
    {"garbage", NULL, garbage, NULL, NULL, NULL, 7, NULL},
    {"finalizerCount", NULL, finalizer_count, NULL, NULL, NULL, 7, NULL},
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
