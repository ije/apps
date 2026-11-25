// Loads libva when FFmpeg opens a device, so that machines without libva decode in software.
// build-ffmpeg.sh writes libva.inc: LIBVA(name) for each libva function that FFmpeg calls.
#include <dlfcn.h>
#include <pthread.h>
#include <stddef.h>

#if defined(__x86_64__)
#define JUMP(name) "jmp *apps_" #name "(%rip)\n"
#elif defined(__aarch64__)
#define JUMP(name) "adrp x16, apps_" #name "\nldr x16, [x16, :lo12:apps_" #name "]\nbr x16\n"
#endif

// The jumps keep all arguments. FFmpeg calls these functions only with a display of vaGetDisplayDRM().
#define LIBVA(name)                                                        \
  __attribute__((visibility("hidden"), used)) void *apps_##name;           \
  __asm__(".text\n.balign 4\n.globl " #name "\n.hidden " #name             \
          "\n.type " #name ", %function\n" #name ":\n" JUMP(name));
#include "libva.inc"
#undef LIBVA

static void *(*display)(int);

static void load(void) {
  void *va = dlopen("libva.so.2", RTLD_NOW | RTLD_LOCAL);
  void *drm = va ? dlopen("libva-drm.so.2", RTLD_NOW | RTLD_LOCAL) : NULL;
  if (!drm) return;
#define LIBVA(name) \
  if (!(apps_##name = dlsym(va, #name))) return;
#include "libva.inc"
  display = (void *(*)(int))dlsym(drm, "vaGetDisplayDRM");
}

__attribute__((visibility("hidden"))) void *vaGetDisplayDRM(int fd) {
  static pthread_once_t once = PTHREAD_ONCE_INIT;
  pthread_once(&once, load);
  return display ? display(fd) : NULL;
}
