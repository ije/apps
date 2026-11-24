#include <stdint.h>
#include <string.h>
#include <pthread.h>

#define ROUNDTRIP(name, type) \
  type name(type value) { return value; } \
  type call_##name(type (*callback)(type), type value) { return callback(value); }

ROUNDTRIP(i8, int8_t)
ROUNDTRIP(u8, uint8_t)
ROUNDTRIP(i16, int16_t)
ROUNDTRIP(u16, uint16_t)
ROUNDTRIP(i32, int32_t)
ROUNDTRIP(u32, uint32_t)
ROUNDTRIP(i64, int64_t)
ROUNDTRIP(u64, uint64_t)
ROUNDTRIP(f32, float)
ROUNDTRIP(f64, double)
ROUNDTRIP(character, char)
ROUNDTRIP(pointer, void *)

int32_t answer(void) { return 42; }
int32_t add(int32_t a, int32_t b) { return a + b; }
double mixed(int32_t a, double b, uint64_t c, float d) { return a + b + c + d; }
int32_t many(int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f,
             int32_t g, int32_t h, int32_t i, int32_t j) { return a+b+c+d+e+f+g+h+i+j; }
void mutate(uint8_t *bytes, uint32_t length) { for (uint32_t i = 0; i < length; i++) bytes[i]++; }
uint64_t string_length(const char *value) { return strlen(value); }
const char *greeting(void) { return "hello"; }
void call_void(void (*callback)(void)) { callback(); }
int32_t number = 123;

static void *thread_callback(void *callback) { ((void (*)(void))callback)(); return NULL; }
void call_thread(void (*callback)(void)) {
  pthread_t thread;
  if (pthread_create(&thread, NULL, thread_callback, (void *)callback) == 0) pthread_join(thread, NULL);
}
