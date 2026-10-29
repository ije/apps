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

pub fn bundle_source(
    files: &bbtree::unsync::BBTreeMap<String, String>,
    execute: bool,
) -> anyhow::Result<String> {
    let modules = include_str!("../js/modules.js");
    let mut source = String::with_capacity(
        modules.len()
            + 256
            + files
                .iter()
                .map(|(_, source)| source.len() + 1)
                .sum::<usize>(),
    );
    source.push_str("(()=>{\n");
    source.push_str(modules);
    for (name, file) in files.iter() {
        if name.ends_with(".js") {
            source.push('\n');
            source.push_str(file);
        }
    }
    source.push_str("\nprepare();");
    if execute {
        source.push_str(
            "const start=()=>globalThis.__appsStart?.(modules.get(0).namespace.default);const result=load(0);if(result instanceof Promise)globalThis.__appsTrackModule(result.then(start));else start();",
        );
    }
    source.push_str("\n})();");
    Ok(source)
}
