#!/usr/bin/env bash
# WARNING: CMake runs every JSC compile through this. The runtime, GC, WTF and bmalloc keep -O3,
# the JIT compiler back ends and the inspector use -Oz, and the rest keeps the -Os default.
# Operations and slow paths that JIT code calls stay at -Os.
source=${!#}
case $source in
  */WTF/* | */bmalloc/* | */JavaScriptCore/runtime/* | */JavaScriptCore/heap/* | \
    */UnifiedSource-runtime-* | */UnifiedSource-heap-*)
    exec "$@" -O3
    ;;
  */JavaScriptCore/b3/* | */JavaScriptCore/dfg/* | */JavaScriptCore/ftl/* | */JavaScriptCore/wasm/* | \
    */JavaScriptCore/inspector/* | */UnifiedSource-b3-* | */UnifiedSource-dfg-* | \
    */UnifiedSource-ftl-* | */UnifiedSource-wasm-* | */UnifiedSource-inspector-*)
    [[ $source =~ (Operations|SlowPaths)\.cpp$ ]] || grep -qE '(Operations|SlowPaths)\.cpp"' "$source" ||
      exec "$@" -Oz
    ;;
esac
exec "$@"
