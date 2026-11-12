#!/usr/bin/env bash
set -euo pipefail

revision=6fc709c298409fe433a60f6aff328a45a8974532
bun_revision=ccb3611f2ade6226369384e0f14b5e8f16207710

aot=0
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)

jobs=${NUMBER_OF_PROCESSORS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '1\n')}
if ((jobs > 8)); then
  jobs=8
fi

while (($#)); do
  case "$1" in
    --aot)
      aot=1
      shift
      ;;
    --jobs)
      if (($# < 2)); then
        printf '%s\n' '--jobs requires a positive integer' >&2
        exit 2
      fi
      jobs=$2
      shift 2
      ;;
    --jobs=*)
      jobs=${1#*=}
      shift
      ;;
    -h | --help)
      printf 'Usage: %s [--aot] [--jobs N] [-- CMake options...]\n' "${0##*/}"
      exit 0
      ;;
    --)
      shift
      break
      ;;
    *)
      printf 'Unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

if [[ ! $jobs =~ ^[1-9][0-9]*$ ]]; then
  printf '%s\n' '--jobs requires a positive integer' >&2
  exit 2
fi

case "$(uname -s)" in
  Darwin)
    system=macos
    ;;
  Linux)
    system=linux
    ;;
  MINGW* | MSYS* | CYGWIN*)
    system=windows
    ;;
  *)
    printf 'Unsupported operating system\n' >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64 | aarch64)
    arch=aarch64
    ;;
  x86_64 | amd64 | AMD64)
    arch=x86_64
    ;;
  *)
    printf 'Unsupported architecture\n' >&2
    exit 1
    ;;
esac

if ((aot)) && [[ $system != macos || $arch != aarch64 ]]; then
  printf '%s\n' '--aot currently requires macOS arm64' >&2
  exit 1
fi

source_dir=$root/target/bun-jsc/source
output=$root/target/bun-jsc/$system-$arch
build=$output/build

if [[ ! -d $source_dir ]]; then
  git clone --config core.autocrlf=false --depth 1 --filter=blob:none --sparse \
    --branch "autobuild-$revision" https://github.com/oven-sh/WebKit.git "$source_dir"
fi

if [[ $(git -C "$source_dir" rev-parse HEAD) != "$revision" ]]; then
  printf '%s must be at %s; move it aside before rebuilding\n' "$source_dir" "$revision" >&2
  exit 1
fi

git -C "$source_dir" sparse-checkout set \
  Source/JavaScriptCore Source/WTF Source/bmalloc Source/cmake \
  Source/ThirdParty/gtest Source/ThirdParty/gmock Source/ThirdParty/unifdef \
  Source/ThirdParty/capstone Source/WebGPU Tools/Scripts Tools/jsc

patch=$root/scripts/jsc.patch
# Bun removes C API locks. Keep this patch when updating the pinned source.
if ! git -C "$source_dir" apply --reverse --check "$patch" 2>/dev/null; then
  git -C "$source_dir" apply "$patch"
fi

options=(
  -DPORT=JSCOnly
  -DCMAKE_BUILD_TYPE=Release
  -DENABLE_STATIC_JSC=OFF
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON
  -DUSE_BUN_JSC_ADDITIONS=ON
  -DEVENT_LOOP_TYPE=Generic
  -DENABLE_REMOTE_INSPECTOR=ON
  -DENABLE_TOOLS=OFF
  -DENABLE_API_TESTS=OFF
  -DENABLE_BACK_FORWARD_LIST_SWIFT=OFF
  -DENABLE_WEBGPU=OFF
  -DENABLE_WEBGPU_SWIFT=OFF
  '-DCMAKE_INSTALL_RPATH=$ORIGIN'
  -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON
  '-DCMAKE_C_FLAGS=-DENABLE_AOT=0 -DAPPS_JSC_AOT_RUNTIME=0'
  '-DCMAKE_CXX_FLAGS=-DENABLE_AOT=0 -DAPPS_JSC_AOT_RUNTIME=0'
)

case "$system" in
  macos)
    # jsc-cc.sh picks -O3, -Os or -Oz per file. Clang rejects a PCH built at another level.
    options+=(
      -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0
      '-DCMAKE_C_FLAGS_RELEASE=-Os -DNDEBUG'
      '-DCMAKE_CXX_FLAGS_RELEASE=-Os -DNDEBUG'
      -DCMAKE_DISABLE_PRECOMPILE_HEADERS=ON
      "-DCMAKE_C_COMPILER_LAUNCHER=$root/scripts/jsc-cc.sh"
      "-DCMAKE_CXX_COMPILER_LAUNCHER=$root/scripts/jsc-cc.sh"
      "-DCMAKE_SHARED_LINKER_FLAGS=-Wl,-dead_strip -Wl,-exported_symbols_list,$root/scripts/jsc.exports"
    )
    ;;
  windows)
    options+=('-DCMAKE_CXX_FLAGS=-DENABLE_AOT=0 -DAPPS_JSC_AOT_RUNTIME=0 -DU_STATIC_IMPLEMENTATION')
    ;;
esac

cmake -S "$source_dir" -B "$build" -G Ninja "${options[@]}" "$@"
cmake --build "$build" --target JavaScriptCore --parallel "$jobs"

case "$system" in
  macos)
    name=libJavaScriptCore.dylib
    library=$build/lib/JavaScriptCore.framework/JavaScriptCore
    ;;
  linux)
    name=libJavaScriptCore.so
    library=$build/lib/$name
    ;;
  windows)
    name=JavaScriptCore.dll
    library=$build/bin/$name
    ;;
esac

# Replace the file. In-place writes invalidate macOS code signatures and kill processes that load the library.
cp -p "$library" "$output/.$name"
if [[ $system == macos ]]; then
  install_name_tool -id "@rpath/$name" "$output/.$name"
  xcrun strip -x "$output/.$name"
  codesign --force --sign - "$output/.$name"
fi
mv -f "$output/.$name" "$output/$name"

if ((aot)); then
  if ! grep -q "\"$bun_revision\"" "$root/scripts/jsc-aot.cpp"; then
    printf '%s\n' 'JSGetAOTCompilerRevision in scripts/jsc-aot.cpp must return bun_revision' >&2
    exit 1
  fi
  bun_source=$root/target/bun-aot/source
  if [[ ! -d $bun_source ]]; then
    git init "$bun_source"
    git -C "$bun_source" remote add origin https://github.com/oven-sh/bun.git
  fi
  git -C "$bun_source" sparse-checkout set --cone src scripts packages completions patches docs/lint test/aot/native
  if ! git -C "$bun_source" rev-parse -q --verify HEAD >/dev/null; then
    git -C "$bun_source" fetch --depth 1 origin "$bun_revision"
    git -C "$bun_source" checkout --detach FETCH_HEAD
  fi
  if [[ $(git -C "$bun_source" rev-parse HEAD) != "$bun_revision" ]]; then
    printf '%s must be at %s\n' "$bun_source" "$bun_revision" >&2
    exit 1
  fi
  # Both builds need identical sources, options and Bun built-in tables: AOT code embeds field offsets and
  # built-in IDs.
  for runtime in 0 1; do
    build=$output/build-compiler
    flags="-DENABLE_AOT=1 -DAPPS_JSC_AOT_RUNTIME=$runtime"
    aot_options=("${options[@]}" "-DAPPS_JSC_AOT_SOURCE=$root/scripts/jsc-aot.cpp" "-DCMAKE_C_FLAGS=$flags" "-DCMAKE_CXX_FLAGS=$flags")
    if ((runtime)); then
      build=$output/build-aot
      aot_options+=(-DENABLE_STATIC_JSC=ON -DENABLE_REMOTE_INSPECTOR=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DAPPS_JSC_AOT_RUNTIME=ON)
    fi
    cmake -S "$source_dir" -B "$build" -G Ninja "${aot_options[@]}" "$@"
    "${APPS_BUN:-bun}" "$root/scripts/jsc-aot-table.ts" "$bun_source" \
      "$source_dir/Source/JavaScriptCore" "$build/JavaScriptCore/DerivedSources/aot"
    cmake --build "$build" --target JavaScriptCore --parallel "$jobs"
  done
  name=libJavaScriptCoreCompiler.dylib
  cp -p "$output/build-compiler/lib/JavaScriptCore.framework/JavaScriptCore" "$output/.$name"
  install_name_tool -id "@rpath/$name" "$output/.$name"
  xcrun strip -x "$output/.$name"
  codesign --force --sign - "$output/.$name"
  mv -f "$output/.$name" "$output/$name"
  name=libJavaScriptCoreAOT.dylib
  xcrun clang++ -dynamiclib -arch arm64 -mmacosx-version-min=13.0 -o "$output/.$name" -Wl,-install_name,"@rpath/$name" \
    -Wl,-dead_strip -Wl,-x -Wl,-exported_symbols_list,"$root/scripts/jsc.exports" -Wl,-all_load \
    "$build/lib/libJavaScriptCore.a" "$build/lib/libWTF.a" "$build/lib/libbmalloc.a" -licucore -lobjc -lcompression
  mv -f "$output/.$name" "$output/$name"

  # Build the AOT front end with Bun's nightly toolchain.
  bun=$(command -v "${APPS_BUN:-bun}")
  codegen=$(PATH="$(dirname "$bun"):$PATH" "$bun" "$root/scripts/jsc-aot-front-end.ts" "$bun_source" "$source_dir/Source/JavaScriptCore")
  # Bun's release profile adds 4 MB without a speed gain. `z` saves 3.5 MB but takes 2.5x longer. -S removes 5
  # MB of debug maps.
  (cd "$bun_source" && BUN_CODEGEN_DIR=$codegen APPS_BUN_REVISION=$bun_revision cargo build --release -p apps_aot \
    --config 'profile.release.opt-level="s"' --config 'profile.release.lto="fat"')
  name=libapps_aot.dylib
  xcrun clang -dynamiclib -arch arm64 -mmacosx-version-min=13.0 -o "$output/.$name" -Wl,-install_name,"@rpath/$name" \
    -Wl,-dead_strip -Wl,-x -Wl,-S -Wl,-exported_symbol,_apps_aot_front_end -Wl,-exported_symbol,_apps_aot_free \
    -Wl,-exported_symbol,_apps_aot_revision \
    "$bun_source/target/release/libapps_aot.a" -liconv -lc -lm
  mv -f "$output/.$name" "$output/$name"
fi

{
  if [[ -f $output/libJavaScriptCoreAOT.dylib ]]; then
    cat "$root/target/bun-aot/source/LICENSE.md"
    printf '\n'
  fi
  printf 'JavaScriptCore from https://github.com/oven-sh/WebKit/tree/%s\n' "$revision"
  for component in JavaScriptCore WTF bmalloc; do
    find "$source_dir/Source/$component" \
      -type f \( -name 'LICENSE*' -o -name 'COPYING*' \) |
      LC_ALL=C sort |
      while IFS= read -r license; do
        printf '\n%s\n\n' "${license#"$source_dir/"}"
        cat "$license"
        printf '\n'
      done
  done
} > "$output/JavaScriptCore.LICENSE"

printf '%s\n' "$output/$name"
