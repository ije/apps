#include <stdint.h>

int32_t add(int32_t a, int32_t b) { return a + b; }
int32_t apply(int32_t (*callback)(int32_t), int32_t value) { return callback(value); }
