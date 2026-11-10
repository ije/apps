#!/usr/bin/env bash
# Keep GPL and nonfree components disabled. Ship the replaceable shared library with FFmpeg.LICENSE.
set -euo pipefail

version=9.0.2
sha256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
jobs=$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '4\n')

case "$(uname -s)" in
  Darwin) system=macos ;;
  Linux) system=linux ;;
  MINGW* | MSYS* | CYGWIN*) system=windows ;;
  *) printf 'Unsupported operating system\n' >&2; exit 1 ;;
esac
# MSYS2 runs emulated on Windows arm64, so uname reports x86_64 there.
case "${MSYSTEM_CARCH:-$(uname -m)}" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64 | AMD64) arch=x86_64 ;;
  *) printf 'Unsupported architecture\n' >&2; exit 1 ;;
esac

sources=$root/target/ffmpeg/source
source_dir=$sources/ffmpeg-$version
output=$root/target/ffmpeg/$system-$arch
build=$output.build
mkdir -p "$sources"

if [[ ! -d $source_dir ]]; then
  archive=$sources/ffmpeg-$version.tar.xz
  if [[ ! -f $archive ]]; then
    curl -fL --retry 5 -o "$archive.part" "https://ffmpeg.org/releases/ffmpeg-$version.tar.xz"
    mv -f "$archive.part" "$archive"
  fi
  if [[ $( (command -v sha256sum >/dev/null && sha256sum "$archive" || shasum -a 256 "$archive") | cut -d' ' -f1) != "$sha256" ]]; then
    printf '%s does not match its pinned checksum\n' "$archive" >&2
    exit 1
  fi
  tar -xf "$archive" -C "$sources"
fi

options=(
  --prefix=/
  --enable-static
  --disable-shared
  --enable-small
  --disable-programs
  --disable-doc
  --disable-debug
  --disable-avdevice
  --disable-avfilter
  --disable-network
  --disable-autodetect
  --disable-everything
  --enable-pic
  --enable-protocol=file
  --enable-demuxer=mov,matroska,ogg,mp3,wav,flac,aac
  --enable-parser=h264,hevc,vp8,vp9,av1,aac,mpegaudio,opus,vorbis,flac
  --enable-decoder=h264,hevc,vp8,vp9,av1,aac,mp3float,opus,vorbis,flac,alac,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s32le,pcm_f32le,pcm_f64le,pcm_u8,pcm_alaw,pcm_mulaw
)
case "$system" in
  macos)
    options+=(
      "--extra-cflags=-mmacosx-version-min=13.0 -fno-unwind-tables -fno-asynchronous-unwind-tables"
      --extra-ldflags=-mmacosx-version-min=13.0
      --enable-lto
      --enable-videotoolbox
      --enable-hwaccel=h264_videotoolbox,hevc_videotoolbox,vp9_videotoolbox,av1_videotoolbox
    )
    ;;
  windows)
    options+=(--enable-d3d11va --enable-hwaccel=h264_d3d11va2,hevc_d3d11va2,vp9_d3d11va2,av1_d3d11va2)
    ;;
  linux)
    options+=("--extra-cflags=-ffunction-sections -fdata-sections -fno-unwind-tables -fno-asynchronous-unwind-tables")
    ;;
esac
options+=("$@")
if [[ $arch == x86_64 ]] && ! command -v nasm >/dev/null; then
  printf 'FFmpeg needs nasm on x86_64\n' >&2
  exit 1
fi

rm -rf "$build"
mkdir -p "$build"
(cd "$build" && "$source_dir/configure" "${options[@]}" | tee configure.log)
if ! grep -q '^License: LGPL version 2.1 or later' "$build/configure.log"; then
  printf 'FFmpeg must be configured as LGPL 2.1 or later\n' >&2
  exit 1
fi
make -C "$build" -j"$jobs"
# Use / as the prefix to exclude build paths. Install only headers to prevent static linkage.
make -C "$build" install-headers DESTDIR="$build/install"
rm -rf "$build/install/share" "$build/install/lib/pkgconfig"

# The library exports only what the media library calls, so the linker drops the rest of FFmpeg.
lib=$build/install/lib
mkdir -p "$lib"
config=$build/ffbuild/config.mak
cc=$(sed -n 's/^CC=//p' "$config")
"$cc" -c "$root/crates/media/native/ffmpeg.c" -I"$build/install/include" -o "$build/ffmpeg.o"
symbols=($(nm -u "$build/ffmpeg.o" | awk '{print $NF}' | grep -E '^_?(av|sws|swr)'))
case "$system" in
  macos)
    printf '%s\n' "${symbols[@]}" > "$build/exports"
    link=(-dynamiclib -install_name @loader_path/libffmpeg.dylib -Wl,-dead_strip,-dead_strip_dylibs,-x
      -Wl,-exported_symbols_list,"$build/exports" -o "$lib/libffmpeg.dylib")
    ;;
  windows)
    { printf 'EXPORTS\n'; printf '%s\n' "${symbols[@]}"; } > "$build/exports.def"
    link=("$build/exports.def" -shared -static-libgcc -s -Wl,--out-implib,"$lib/ffmpeg.lib" -o "$lib/ffmpeg.dll")
    ;;
  linux)
    { printf '{ global:\n'; printf '%s;\n' "${symbols[@]}"; printf 'local: *; };\n'; } > "$build/exports"
    link=(-shared -s -Wl,-soname,libffmpeg.so -Wl,-Bsymbolic -Wl,--gc-sections
      -Wl,--version-script="$build/exports" -o "$lib/libffmpeg.so")
    ;;
esac
"$cc" $(sed -n 's/^LDFLAGS=//p' "$config") "${link[@]}" "${symbols[@]/#/-Wl,-u,}" \
  "$build/libavformat/libavformat.a" "$build/libavcodec/libavcodec.a" "$build/libswscale/libswscale.a" \
  "$build/libswresample/libswresample.a" "$build/libavutil/libavutil.a" \
  $(sed -n 's/^EXTRALIBS[^=]*=//p' "$config")

{
  printf 'FFmpeg %s from https://ffmpeg.org/releases/ffmpeg-%s.tar.xz\n' "$version" "$version"
  printf 'is licensed under the GNU Lesser General Public License version 2.1 or later.\n'
  printf 'It was built unmodified by scripts/build-ffmpeg.sh with:\n\n  configure %s\n\n' "${options[*]}"
  printf 'Its static libraries were linked into one shared library that exports:\n\n  %s\n\n' "${symbols[*]}"
  printf 'The app loads that library next to this file. You may replace it with your own\n'
  printf 'build of the same FFmpeg version that exports the same functions.\n\n'
  cat "$source_dir/COPYING.LGPLv2.1"
} > "$build/install/FFmpeg.LICENSE"

# Replace the folder. In-place writes invalidate macOS code signatures and kill processes that load the
# library.
rm -rf "$output"
mv "$build/install" "$output"
rm -rf "$build"
printf '%s\n' "$output"
