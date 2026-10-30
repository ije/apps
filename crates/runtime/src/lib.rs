pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod bundle;
pub mod desktop;
pub mod dom;
pub mod host;
pub mod jsc;
mod node;
pub mod render;
pub mod style;
mod web;
pub use web::gpu;

// WARNING: module code must stay outside the loader function, or it could see loader internals such as
// `compile`. Definitions run in a second function whose only binding is `$appsDefine`.
pub fn bundle_source(files: &bbtree::unsync::BBTreeMap<String, String>, execute: bool) -> String {
    let modules = include_str!("../js/modules.js");
    let app = files.get("app.js").map_or("", String::as_str);
    let mut source = String::with_capacity(modules.len() + app.len() + 64);
    source.push_str("(()=>{");
    source.push_str(modules);
    source.push_str("})()(($appsDefine)=>{\n");
    source.push_str(app);
    source.push_str(if execute { "\n},true);" } else { "\n},false);" });
    source
}
