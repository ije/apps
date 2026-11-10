#!/usr/bin/env bash
# Keep runtime, GC, WTF and bmalloc at -O3. Use -Oz for compiler backends and Inspector, -Os elsewhere,
# including JIT operations.
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
