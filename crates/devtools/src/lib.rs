mod hud;
mod mcp;

// Update this ABI version with `mcp::Host`, `hud::Item` or function signatures, and match the runtime.
const ABI: u32 = 3;

#[unsafe(no_mangle)]
pub extern "C" fn apps_devtools_abi() -> u32 {
    ABI
}

// The page of the developer tools panel, or the script that inspects a context for it.
#[unsafe(no_mangle)]
pub extern "C" fn apps_devtools_panel(agent: bool, len: &mut usize) -> *const u8 {
    let text = if agent {
        include_str!("../assets/devtools-agent.js")
    } else {
        include_str!("../assets/devtools.html")
    };
    *len = text.len();
    text.as_ptr()
}
