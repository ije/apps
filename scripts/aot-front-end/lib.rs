#![feature(linkage)]

use bun_aot::emit::emit_with_bodies_in_bytes;
use bun_aot::ir::{Id, ModuleId};
use bun_aot::link::LinkedFunctions;
use bun_aot::link_program;
use bun_ast::Log;
use bun_bundler::aot::testing::{FrontEndInputs, with_front_end_inputs};
use bun_options_types::aot_options::AotOptions;
use bun_sema_driver::Libs;
use std::ffi::{CStr, c_char};
use std::fmt::Write;

#[path = "../aot/standalone/engine_stubs.rs"]
mod engine_stubs;

// bun_sema_standalone has this stand-in too, but the linker loads no archive member for a weak symbol.
#[unsafe(no_mangle)]
#[linkage = "weak"]
extern "C" fn Bun__AOT__offsetInCode(_address: *const std::ffi::c_void) -> i64 {
    -1
}

// Emit Bun's AOT IR without starting its engine. libJavaScriptCoreCompiler.dylib compiles these bytes.
fn front_end(directory: &[u8], entries: &[&[u8]]) -> Result<Vec<u8>, String> {
    let options = AotOptions {
        omits_builtin_modules: true,
        ..AotOptions::default()
    };
    let inputs = FrontEndInputs {
        working_directory: directory,
        entry_points: entries,
        options: &options,
        typescript_libs: Libs::Bundled(bun_sema_standalone::TYPESCRIPT_LIBS),
    };
    let mut log = Log::init();
    let linked = with_front_end_inputs(&inputs, &mut log, link_program);
    let Some(Ok(linked)) = linked.filter(|_| !log.has_errors()) else {
        return Err(describe(&log));
    };
    let mut program = linked.program;
    for (index, module) in program.modules.iter_mut().enumerate() {
        if let Some(source) = linked.summary.texts.get(ModuleId::new(index)) {
            module.function_text = source.function_text();
        }
    }
    let functions = LinkedFunctions::of_program_functions(std::mem::take(&mut program.functions));
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let in_parallel = |count: usize, work: &(dyn Fn(usize) + Sync)| {
        bun_sema_standalone::for_each_parallel(threads, count, work)
    };
    emit_with_bodies_in_bytes(&mut program, functions, &in_parallel)
        .map(|bytes| bytes.as_slice().to_vec())
        .map_err(|error| format!("Cannot emit the AOT program: {error:?}"))
}

fn describe(log: &Log) -> String {
    let mut text = String::new();
    for message in &log.msgs {
        if let Some(location) = message.data.location.as_ref().filter(|location| location.line > 0) {
            let file = String::from_utf8_lossy(&location.file);
            let _ = write!(text, "{file}:{}:{}: ", location.line, location.column + 1);
        }
        text.push_str(&String::from_utf8_lossy(&message.data.text));
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str("The AOT front end failed\n");
    }
    text
}

/// Writes the program, or the error text when it returns false, to `output`. Free it with `apps_aot_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_aot_front_end(
    directory: *const c_char,
    entries: *const *const c_char,
    count: usize,
    output: *mut *mut u8,
    length: *mut usize,
) -> bool {
    let directory = unsafe { CStr::from_ptr(directory) }.to_bytes();
    let entries: Vec<&[u8]> = (0..count)
        .map(|index| unsafe { CStr::from_ptr(*entries.add(index)) }.to_bytes())
        .collect();
    let (built, bytes) = match front_end(directory, &entries) {
        Ok(bytes) => (true, bytes),
        Err(text) => (false, text.into_bytes()),
    };
    let bytes = bytes.into_boxed_slice();
    unsafe {
        *length = bytes.len();
        *output = Box::into_raw(bytes).cast();
    }
    built
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_aot_free(bytes: *mut u8, length: usize) {
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(bytes, length)) });
}
