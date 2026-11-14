#!/usr/bin/env bash
# Builds the npm package of this platform and sets the version of all npm packages.
#
#   bash scripts/build-ffmpeg.sh && bash scripts/build-ghostty.sh && bash scripts/build-jsc.sh
#   bash scripts/build-npm.sh 0.1.0
#   npm publish packages/apps-darwin-arm64 --access public
#
# Publish each platform package from its own machine, and packages/apps last.
# .github/workflows/release.yml does all of this for a pushed v* tag.
set -euo pipefail

version=${1:?Usage: bash scripts/build-npm.sh <version>}
cd "$(dirname "$0")/.."

# npm picks the package by Node's platform and arch, which uname can't tell under emulation.
read -r os cpu < <(node -p 'process.platform + " " + process.arch')
case $os in
  darwin) system=macos lib=lib ext=.dylib exe= ;;
  linux) system=linux lib=lib ext=.so exe= ;;
  win32) system=windows lib= ext=.dll exe=.exe ;;
  *) echo "Unsupported platform: $os" >&2; exit 1 ;;
esac
case $cpu in
  arm64) arch=aarch64 ;;
  x64) arch=x86_64 ;;
  *) echo "Unsupported architecture: $cpu" >&2; exit 1 ;;
esac

node -e '
const fs = require("fs"), version = process.argv[1]
for (const dir of fs.readdirSync("packages")) {
  const file = `packages/${dir}/package.json`
  if (!fs.existsSync(file)) continue
  const pkg = JSON.parse(fs.readFileSync(file, "utf8"))
  pkg.version = version
  for (const name in pkg.optionalDependencies) pkg.optionalDependencies[name] = version
  fs.writeFileSync(file, JSON.stringify(pkg, null, 2) + "\n")
}
const cargo = fs.readFileSync("Cargo.toml", "utf8"), pattern = /(\[workspace\.package\]\nversion = )"[^"]*"/
if (!pattern.test(cargo)) throw new Error("Cannot find the workspace version in Cargo.toml")
fs.writeFileSync("Cargo.toml", cargo.replace(pattern, `$1"${version}"`))' "$version"

cargo build --release --workspace

bin=packages/apps-$os-$cpu/bin
files=("apps$exe" "apps-runtime$exe" "${lib}apps_media$ext" "${lib}apps_audio$ext" "${lib}ffmpeg$ext" FFmpeg.LICENSE "${lib}apps_devtools$ext")
jsc=target/bun-jsc/$system-$arch
jscs=("${lib}JavaScriptCore$ext" JavaScriptCore.LICENSE)
# Linux links WebGPU into the runtime, and apps build --aot works only on macOS arm64.
[[ $os != linux ]] && files+=("${lib}apps_webgpu$ext")
# The terminal library has no Windows build yet.
[[ $os != win32 ]] && files+=("${lib}Ghostty$ext" Ghostty.LICENSE)
[[ $os-$cpu == darwin-arm64 ]] && jscs+=(libJavaScriptCoreAOT.dylib libJavaScriptCoreCompiler.dylib libapps_aot.dylib)
rm -rf "$bin"
mkdir -p "$bin"
for file in "${files[@]}"; do cp "target/release/$file" "$bin/"; done
for file in "${jscs[@]}"; do cp "$jsc/$file" "$bin/"; done
cp README.md packages/apps/README.md
du -sh "$bin"
