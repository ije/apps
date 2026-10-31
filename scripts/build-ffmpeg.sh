#!/usr/bin/env bash
# Builds the FFmpeg shared libraries for the video and audio features into target/ffmpeg/<system>-<arch>.
# WARNING: FFmpeg is LGPL only while GPL and nonfree parts stay off. Apps load these libraries
# dynamically and ship FFmpeg.LICENSE next to them, so users can replace them.
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
case "$(uname -m)" in
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
  --enable-shared
  --disable-static
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
      --install-name-dir=@rpath
      --extra-cflags=-mmacosx-version-min=13.0
      --extra-ldflags=-mmacosx-version-min=13.0
      --enable-videotoolbox
      --enable-hwaccel=h264_videotoolbox,hevc_videotoolbox,vp9_videotoolbox,av1_videotoolbox
    )
    ;;
  windows)
    options+=(--enable-d3d11va --enable-hwaccel=h264_d3d11va2,hevc_d3d11va2,vp9_d3d11va2,av1_d3d11va2)
    ;;
esac
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
# WARNING: the prefix is /, so the libraries do not carry a build path.
make -C "$build" install DESTDIR="$build/install"
rm -rf "$build/install/share" "$build/install/lib/pkgconfig"

{
  printf 'FFmpeg %s from https://ffmpeg.org/releases/ffmpeg-%s.tar.xz\n' "$version" "$version"
  printf 'is licensed under the GNU Lesser General Public License version 2.1 or later.\n'
  printf 'It was built unmodified by scripts/build-ffmpeg.sh with:\n\n  configure %s\n\n' "${options[*]}"
  printf 'The app loads the FFmpeg shared libraries next to this file. You may replace them\n'
  printf 'with your own build of the same FFmpeg version.\n\n'
  cat "$source_dir/COPYING.LGPLv2.1"
} > "$build/install/FFmpeg.LICENSE"

# WARNING: macOS caches a library's code signature per file, so a rewrite in place kills every process that loads it. Replace the folder.
rm -rf "$output"
mv "$build/install" "$output"
rm -rf "$build"
printf '%s\n' "$output"
