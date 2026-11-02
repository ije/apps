#[path = "../../scripts/napi_exports.rs"]
mod napi_exports;

fn main() {
    napi_exports::export_symbols(std::path::Path::new("../runtime/src/node/napi.rs"));
}
