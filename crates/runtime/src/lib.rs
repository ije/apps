pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Text that build.rs deflated, inflated once.
macro_rules! packed {
    ($($path:literal),+) => {{
        static SOURCE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        *SOURCE.get_or_init(|| {
            crate::inflate(include_bytes!(concat!(env!("OUT_DIR"), "/", $($path),+)))
        })
    }};
}

macro_rules! js {
  ($path:literal) => {
    packed!("js/", $path)
  };
}

// The text is leaked on purpose, because JSC reads runtime scripts in place.
#[inline(never)]
fn inflate(bytes: &[u8]) -> &'static str {
  let mut text = String::with_capacity(bytes.len() * 4);
  std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(bytes), &mut text).unwrap();
  Box::leak(text.into_boxed_str())
}

#[cfg(target_os = "macos")]
mod malloc;
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOCATOR: malloc::Allocator = malloc::Allocator;

pub mod bundle;
pub mod css;
pub mod desktop;
pub mod dom;
pub mod host;
pub mod jsc;
pub mod native;
pub mod node;
pub mod render;
pub mod script;
pub mod style;
mod web;
#[cfg(not(target_os = "linux"))]
pub(crate) use apps_webview as webview;
#[cfg(target_os = "linux")]
pub(crate) use native::webview;
pub use render::media::{gpu, player, terminal};
pub use web::download;
// The DOM operations of page scripts, which tests drive directly.
pub use web::dom::call as dom_call;

pub use anyhow::Result;
pub use desktop::app::{self, App, AppHandle, BrowserWindow, WindowOptions};
pub use serde_json::Value;
