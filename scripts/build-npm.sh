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
let cargo = fs.readFileSync("Cargo.toml", "utf8")
for (const pattern of [/(\[workspace\.package\]\nversion = )"[^"]*"/, /(apps-runtime = \{ version = )"[^"]*"/]) {
  if (!pattern.test(cargo)) throw new Error("Cannot find the crate version in Cargo.toml")
  cargo = cargo.replace(pattern, `$1"${version}"`)
}
fs.writeFileSync("Cargo.toml", cargo)' "$version"

# Panic messages keep source paths. Make the paths of dependencies and std relative to save about 36 KB.
# Cargo passes them as absolute paths, so read them from cargo metadata to get the form of this machine.
flags=$(cargo metadata --format-version 1 --locked | node -e '
const path = require("path"), meta = JSON.parse(require("fs").readFileSync(0, "utf8"))
const flags = (process.env.RUSTFLAGS || "").split(/\s+/).filter(Boolean)
const roots = new Set([path.join(process.argv[1], "lib", "rustlib", "src", "rust")])
for (const { source, manifest_path } of meta.packages) {
  if (!source) continue
  const dir = path.dirname(path.dirname(manifest_path))
  roots.add(source.startsWith("git+") ? dir.slice(0, dir.indexOf(`${path.sep}checkouts${path.sep}`) + 10) : dir)
}
for (const root of roots) flags.push(`--remap-path-prefix=${root}=`)
process.stdout.write(flags.join("\x1f"))' "$(rustc --print sysroot)")
CARGO_ENCODED_RUSTFLAGS=$flags cargo build --release --workspace

bin=packages/apps-$os-$cpu/bin
files=("apps$exe" "apps-runtime$exe" "${lib}apps_media$ext" "${lib}ffmpeg$ext" FFmpeg.LICENSE "${lib}apps_devtools$ext" "${lib}Ghostty$ext" Ghostty.LICENSE)
jsc=target/bun-jsc/$system-$arch
jscs=("${lib}JavaScriptCore$ext" JavaScriptCore.LICENSE)
# Linux links WebGPU into the runtime.
[[ $os != linux ]] && files+=("${lib}apps_webgpu$ext")
# Linux loads the system web view from its own library.
[[ $os == linux ]] && files+=(libapps_webview.so)
# build-jsc.sh --aot writes these.
[[ $os-$cpu == darwin-arm64 ]] && jscs+=(libJavaScriptCoreAOT.dylib libJavaScriptCoreCompiler.dylib libapps_aot.dylib)
[[ $os == linux ]] && jscs+=(libJavaScriptCoreAOT.so libJavaScriptCoreCompiler.so libapps_aot.so)
rm -rf "$bin"
mkdir -p "$bin"
for file in "${files[@]}"; do cp "target/release/$file" "$bin/"; done
for file in "${jscs[@]}"; do cp "$jsc/$file" "$bin/"; done
cp README.md packages/apps/README.md
du -sh "$bin"
