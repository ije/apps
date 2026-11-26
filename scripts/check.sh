#!/usr/bin/env bash
# Runs the checks of the Linux CI job: format, clippy, tests and the system web view tests.
# Needs the native libraries of .github/actions/native in target/.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked --no-fail-fast

# The web view tests need an X11 display: Xvfb where it is installed, else the current one.
xvfb=()
if command -v xvfb-run >/dev/null; then
  xvfb=(xvfb-run -a)
elif [[ -z ${DISPLAY-} ]]; then
  echo 'Skip the web view tests: no xvfb-run and no DISPLAY' >&2
  exit
fi
GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 dbus-run-session -- "${xvfb[@]}" \
  cargo test -p apps-webview --lib wry::tests::system_webview --locked -- --ignored --test-threads=1
