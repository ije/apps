#[path = "../../scripts/napi_exports.rs"]
mod napi_exports;

fn main() {
  let source = napi_exports::export_symbols(std::path::Path::new("../runtime/src/node/napi"));
  // Rust apps link with the same exports.
  let out = std::env::var_os("OUT_DIR").unwrap();
  std::fs::write(std::path::Path::new(&out).join("napi.txt"), source).unwrap();
}
