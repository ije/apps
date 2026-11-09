pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// WARNING: the inflated text is leaked on purpose, because JSC reads runtime scripts in place.
macro_rules! js {
    ($path:literal) => {{
        static SOURCE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        *SOURCE.get_or_init(|| {
            let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/js/", $path));
            let mut text = String::with_capacity(bytes.len() * 4);
            std::io::Read::read_to_string(
                &mut flate2::read::DeflateDecoder::new(&bytes[..]),
                &mut text,
            )
            .unwrap();
            Box::leak(text.into_boxed_str())
        })
    }};
}

#[cfg(target_os = "macos")]
mod malloc;
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOCATOR: malloc::Allocator = malloc::Allocator;

pub mod bundle;
pub mod desktop;
pub mod dom;
pub mod host;
pub mod jsc;
pub mod node;
pub mod render;
pub mod style;
mod web;
pub use web::download;
pub use web::{gpu, media};

pub use anyhow::Result;
pub use desktop::app::{self, App, AppHandle, BrowserWindow, WindowOptions};
pub use serde_json::Value;
