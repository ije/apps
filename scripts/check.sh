#!/usr/bin/env bash
# Runs the checks of the Linux CI job: format, clippy, tests and the system web view tests.
# Needs the native libraries of .github/actions/native in target/.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast

# The web view tests need a display, so they use Xvfb where it is installed.
xvfb=()
if command -v xvfb-run >/dev/null; then xvfb=(xvfb-run -a); fi
GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 dbus-run-session -- "${xvfb[@]}" \
  cargo test -p apps-webview --lib wry::tests::system_webview --locked -- --ignored --test-threads=1
