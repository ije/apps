#!/usr/bin/env bash
# Builds the static libghostty-vt for the terminal feature. It needs the Zig version that build.zig.zon of
# this libghostty-vt release names (0.17 for 1.0).
set -euo pipefail

version=1.0.1
sha256=333dfce97641ef9b536bb9e08907ad86abaaada50e8f7417b70d9284adef37aa
deps_sha256=91a4e10960a52bf5bf4b15aa2a2cddaff8a89d6cd903d3055bda4fd17de3356d
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)

case "$(uname -s)" in
  Darwin) system=macos ;;
  Linux) system=linux ;;
  *) printf 'Unsupported operating system\n' >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64 | AMD64) arch=x86_64 ;;
  *) printf 'Unsupported architecture\n' >&2; exit 1 ;;
esac

sources=$root/target/ghostty/source
output=$root/target/ghostty/$system-$arch
mkdir -p "$sources"

for name in libghostty-vt-$version libghostty-vt-$version-deps; do
  [[ -d $sources/$name ]] && continue
  archive=$sources/$name.tar.gz
  if [[ ! -f $archive ]]; then
    curl -fL --retry 5 -o "$archive.part" "https://release.files.ghostty.org/libghostty-vt/$version/$name.tar.gz"
    mv -f "$archive.part" "$archive"
  fi
  expected=$sha256
  [[ $name == *-deps ]] && expected=$deps_sha256
  if [[ $( (command -v sha256sum >/dev/null && sha256sum "$archive" || shasum -a 256 "$archive") | cut -d' ' -f1) != "$expected" ]]; then
    printf '%s does not match its pinned checksum\n' "$archive" >&2
    exit 1
  fi
  tar -xzf "$archive" -C "$sources"
done

# libghostty-vt converts image pixels only to RGBA, so the conversions of wuffs to other formats stay out.
wuffs=$sources/libghostty-vt-$version/pkg/wuffs/build.zig
if ! grep -q DST_PIXEL_FORMAT "$wuffs"; then
  perl -pi -e 's/^(\s*)"WUFFS_CONFIG__MODULES",$/$1"WUFFS_CONFIG__MODULES",\n$1"WUFFS_CONFIG__DST_PIXEL_FORMAT__ENABLE_ALLOWLIST",\n$1"WUFFS_CONFIG__DST_PIXEL_FORMAT__ALLOW_RGBA_NONPREMUL",\n$1"WUFFS_CONFIG__DST_PIXEL_FORMAT__ALLOW_RGBA_PREMUL",/' "$wuffs"
  grep -q DST_PIXEL_FORMAT "$wuffs" || { printf 'Cannot patch %s\n' "$wuffs" >&2; exit 1; }
fi

# The snapshot, search and glyph parts are not used by the terminal element. The other features only add
# exports, which the linker drops. ReleaseSmall would save 0.5 MB, but it reads output 16% to 39% slower.
options=(
  --prefix "$output"
  --system "$sources/libghostty-vt-$version-deps"
  -Doptimize=ReleaseFast
  -Dstrip=true
  -Dcpu=baseline
  -Dvt-features=-snapshot,-search,-glyph-protocol
)
[[ $system == macos ]] && options+=("-Dtarget=$arch-macos.13.0")
options+=("$@")

# Zig 0.17 can crash on a cache that holds a package archive, so the build gets a new cache.
rm -rf "$output" "$output.cache"
(cd "$sources/libghostty-vt-$version" && ZIG_GLOBAL_CACHE_DIR=$output.cache zig build "${options[@]}")
# Only the static library is used. The linker would take a shared library next to it.
rm -rf "$output.cache" "$sources/libghostty-vt-$version/.zig-cache" "$output/bin" "$output/lib/cmake" "$output/lib/pkgconfig"
rm -f "$output"/lib/*.dylib "$output"/lib/*.so*
cp "$sources/libghostty-vt-$version/LICENSE" "$output/Ghostty.LICENSE"
printf 'Built libghostty-vt %s in %s\n' "$version" "$output"
