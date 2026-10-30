#!/usr/bin/env bash
set -euo pipefail

revision=cdce396d50b4ffbb9125a4cb3b3fb76a8d59a62f
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)

jobs=${NUMBER_OF_PROCESSORS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '1\n')}
if ((jobs > 8)); then
  jobs=8
fi

while (($#)); do
  case "$1" in
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
      printf 'Usage: %s [--jobs N] [-- CMake options...]\n' "${0##*/}"
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

source_dir=$root/target/bun-jsc/source
output=$root/target/bun-jsc/$system-$arch
build=$output/build

if [[ ! -d $source_dir ]]; then
  git clone --depth 1 --filter=blob:none --sparse \
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
# WARNING: Bun removes C API locks. Keep this patch when updating the pinned source.
if ! git -C "$source_dir" apply --reverse --check "$patch" 2>/dev/null; then
  git -C "$source_dir" apply "$patch"
fi

options=(
  -DPORT=JSCOnly
  -DCMAKE_BUILD_TYPE=Release
  -DENABLE_STATIC_JSC=OFF
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
  -DCMAKE_C_FLAGS=-DENABLE_AOT=0
  -DCMAKE_CXX_FLAGS=-DENABLE_AOT=0
)

case "$system" in
  macos)
    # WARNING: jsc-cc.sh picks -O3, -Os or -Oz per file. Clang rejects a PCH built at another level.
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
    options+=('-DCMAKE_CXX_FLAGS=-DENABLE_AOT=0 -DU_STATIC_IMPLEMENTATION')
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

# WARNING: macOS caches a library's code signature per file, so a rewrite in place kills every process that loads it. Replace it with a new file.
cp -p "$library" "$output/.$name"
if [[ $system == macos ]]; then
  install_name_tool -id "@rpath/$name" "$output/.$name"
  xcrun strip -x "$output/.$name"
  codesign --force --sign - "$output/.$name"
fi
mv -f "$output/.$name" "$output/$name"

{
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
