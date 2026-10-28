#include <node_api.h>

static napi_value add(napi_env env, napi_callback_info info) {
  napi_value args[2], result;
  size_t count = 2;
  double a, b;
  if (napi_get_cb_info(env, info, &count, args, NULL, NULL) != napi_ok || count != 2 ||
      napi_get_value_double(env, args[0], &a) != napi_ok ||
      napi_get_value_double(env, args[1], &b) != napi_ok) {
    napi_throw_type_error(env, NULL, "add needs two numbers");
    return NULL;
  }
  if (napi_create_double(env, a + b, &result) != napi_ok) return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "add", function) != napi_ok) return NULL;
  return exports;
}
