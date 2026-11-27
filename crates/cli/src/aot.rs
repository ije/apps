use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use bbtree::unsync::BBTreeMap;
use object::{Object, ObjectSegment, ObjectSymbol};
use oxc_ast::{
    AstKind,
    ast::{Argument, Statement},
};
use oxc_span::GetSpan;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString, c_char},
    fs,
    path::Path,
    process::Command,
};

// The front end stores no source for entries that start with this, so their functions show `[native code]`.
const NO_SOURCE: &str = "/*apps:no-source*/";

pub fn compile(files: &mut BBTreeMap<String, String>, runtime: &Path) -> Result<tempfile::TempDir> {
    let platform = std::env::consts::OS;
    let library = apps_runtime::jsc::library_path(runtime, platform)?;
    let engine = apps_runtime::jsc::aot::engine_library(platform);
    ensure!(
        library.with_file_name(engine).is_file(),
        "--aot needs {engine}; run bash scripts/build-jsc.sh --aot"
    );
    let compiler_name = apps_runtime::jsc::aot::compiler_library(platform);
    let compiler = library.with_file_name(compiler_name);
    let compiler = unsafe { apps_runtime::jsc::open_library(&compiler) }.with_context(|| {
        format!(
            "Cannot load {}; run bash scripts/build-jsc.sh --aot",
            compiler.display()
        )
    })?;
    type Compile = unsafe extern "C" fn(
        *const u8,
        usize,
        *const c_char,
        *mut u32,
        usize,
        *mut usize,
        *mut c_char,
        usize,
    ) -> bool;
    let compile = unsafe { *compiler.get::<Compile>(b"JSCompileAOT\0")? };
    let front_name = apps_runtime::jsc::aot::front_end_library(platform);
    let front = library.with_file_name(front_name);
    let front = unsafe { libloading::Library::new(&front) }.with_context(|| {
        format!(
            "Cannot load {}; run bash scripts/build-jsc.sh --aot",
            front.display()
        )
    })?;
    type FrontEnd = unsafe extern "C" fn(
        *const c_char,
        *const *const c_char,
        usize,
        *mut *mut u8,
        *mut usize,
    ) -> bool;
    type Free = unsafe extern "C" fn(*mut u8, usize);
    type Revision = unsafe extern "C" fn() -> *const c_char;
    let (front_end, free) = unsafe {
        (
            *front.get::<FrontEnd>(b"apps_aot_front_end\0")?,
            *front.get::<Free>(b"apps_aot_free\0")?,
        )
    };
    // the front end writes Bun's IR and built-in IDs. The compiler reads them without a version check.
    let revisions = unsafe {
        [
            compiler.get::<Revision>(b"JSGetAOTCompilerRevision\0"),
            front.get::<Revision>(b"apps_aot_revision\0"),
        ]
        .map(|revision| revision.map(|revision| CStr::from_ptr(revision()).to_owned()))
    };
    ensure!(
        matches!(&revisions, [Ok(compiler), Ok(front)] if compiler == front),
        "{front_name} and {compiler_name} are from different Bun revisions; run bash scripts/build-jsc.sh --aot"
    );
    let directory = tempfile::Builder::new().prefix("apps-aot-").tempdir()?;
    fs::write(
        directory.path().join("package.json"),
        r#"{"type":"module"}"#,
    )?;
    fs::write(
        directory.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"allowJs":true},"include":["*.js"]}"#,
    )?;
    let mut wasm = BTreeSet::new();
    if let Some(assets) = files.get("assets.json") {
        let assets: std::collections::BTreeMap<String, std::path::PathBuf> =
            serde_json::from_str(assets)?;
        for (name, path) in assets {
            if name.ends_with(".wasm") {
                let bytes =
                    fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
                if bytes.starts_with(b"\0asm\x01\0\0\0") {
                    wasm.insert(bytes);
                }
            }
        }
    }
    let app = apps_runtime::jsc::script_source(files, true);
    let wrapper = apps_runtime::jsc::script_source(&BBTreeMap::new(), true);
    let loader = wrapper.strip_suffix("\n},true);").unwrap();
    let mut scripts: Vec<(&str, &str)> = files
        .iter()
        .filter(|(name, _)| name.starts_with("script-") && name.ends_with(".js"))
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    scripts.push(("app.js", &app));
    let webgpu =
        apps_runtime::bundle::Package::parse(files.get("package.json").map(String::as_str))?
            .features
            .has(apps_runtime::bundle::Feature::WebGpu);
    for script in apps_runtime::jsc::runtime_scripts() {
        if webgpu || script.0 != "apps:gpu" {
            scripts.push(script);
        }
    }
    let mut names = Vec::new();
    let mut paths = Vec::new();
    let mut globals = BTreeSet::new();
    let mut skipped = Vec::new();
    for (name, source) in scripts {
        // Do not wrap classic scripts: wrappers change their global declarations.
        if name.starts_with("script-")
            && !source.starts_with(loader)
            && !source.starts_with("(($appsDefine)=>{\n")
        {
            skipped.push(name);
            continue;
        }
        // the front end is built without Bun's built-in sources, which hold the helper of its own
        // `using`. So `using` becomes try and finally blocks here, with the helper in using.js.
        let mut lowered = String::new();
        if source.contains("using ") {
            let allocator = oxc_allocator::Allocator::default();
            let mut program =
                oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::cjs())
                    .parse()
                    .program;
            let scoping = oxc_semantic::SemanticBuilder::new()
                .build(&program)
                .semantic
                .into_scoping();
            let mut options = oxc_transformer::TransformOptions::default();
            options.env.es2026.explicit_resource_management = true;
            options.helper_loader.mode = oxc_transformer::HelperLoaderMode::External;
            oxc_transformer::Transformer::new(&allocator, Path::new(name), &options)
                .build_with_scoping(scoping, &mut program);
            lowered = oxc_codegen::Codegen::new()
                .with_options(oxc_codegen::CodegenOptions {
                    minify: true,
                    ascii_only: true,
                    ..Default::default()
                })
                .build(&program)
                .code;
        }
        let using = lowered.contains("babelHelpers.usingCtx(");
        let source = if using { lowered.as_str() } else { source };
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::cjs()).parse();
        ensure!(parsed.diagnostics.is_empty(), "Invalid AOT script {name}");
        let Some((Statement::ExpressionStatement(expression), before)) =
            parsed.program.body.split_last()
        else {
            skipped.push(name);
            continue;
        };
        if before
            .iter()
            .any(|statement| !matches!(statement, Statement::ExpressionStatement(_)))
        {
            skipped.push(name);
            continue;
        }
        let semantic = oxc_semantic::SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program);
        // Wrappers can hide Wasm from Bun's scan. Supply its bytes separately so JSC can match them by
        // digest.
        for node in semantic.semantic.nodes().iter() {
            let AstKind::CallExpression(call) = node.kind() else {
                continue;
            };
            if !call.callee.is_specific_id("atob") || call.arguments.len() != 1 {
                continue;
            }
            let text = match &call.arguments[0] {
                Argument::StringLiteral(literal) => literal.value.as_str(),
                Argument::TemplateLiteral(literal) if literal.expressions.is_empty() => literal
                    .quasis[0]
                    .value
                    .cooked
                    .as_ref()
                    .and_then(|text| text.as_str()),
                _ => None,
            };
            if let Some(text) = text.filter(|text| text.starts_with("AGFzbQEAAA"))
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text)
                && bytes.starts_with(b"\0asm\x01\0\0\0")
            {
                wasm.insert(bytes);
            }
        }
        let (scoping, nodes) = (semantic.semantic.scoping(), semantic.semantic.nodes());
        // Bun's front end takes `process` and `Bun` for objects that always exist, also on `globalThis`,
        // and a .cjs entry binds the other names. apps has no global `Bun`, and a page has none of these names.
        // So the runtime passes the global object of the realm as the `globalThis` parameter, and each free
        // reference reads from it: `typeof Bun` is `undefined`.
        let mut edits = Vec::new();
        for (name, references) in scoping.root_unresolved_references() {
            globals.insert(name.to_string());
            if !matches!(
                name.as_str(),
                "process" | "Bun" | "require" | "module" | "exports" | "__filename" | "__dirname"
            ) {
                continue;
            }
            for &reference in references {
                let node = scoping.get_reference(reference).node_id();
                let shorthand = match nodes.parent_kind(node) {
                    AstKind::ObjectProperty(property) => property.shorthand,
                    AstKind::AssignmentTargetPropertyIdentifier(_) => true,
                    _ => false,
                };
                let span = nodes.kind(node).span();
                edits.push((span.start as usize, span.end as usize, shorthand));
            }
        }
        edits.sort_unstable();
        let span = expression.expression.span();
        let (result, end) = (span.start as usize, span.end as usize);
        let mut at = before
            .first()
            .map_or(result, |statement| statement.span().start as usize);
        let mut text = String::with_capacity(end - at + edits.len() * 16 + 64);
        if name.starts_with("apps:") {
            text.push_str(NO_SOURCE);
        }
        text.push_str("module.exports=function(globalThis){");
        if using {
            text.push_str(include_str!("using.js"));
        }
        let mut returned = false;
        for (from, to, shorthand) in edits.into_iter().chain([(end, end, false)]) {
            if !returned && from >= result {
                text.push_str(&source[at..result]);
                text.push_str("return (\n");
                (at, returned) = (result, true);
            }
            text.push_str(&source[at..from]);
            at = from;
            if from == end {
                break;
            }
            if shorthand {
                text.push_str(&source[from..to]);
                text.push(':');
            }
            text.push_str("globalThis.");
        }
        text.push_str("\n);};\n");
        let path = directory.path().join(format!("entry-{}.cjs", names.len()));
        fs::write(&path, text)?;
        paths.push(path);
        names.push(name.to_owned());
    }
    ensure!(
        !names.is_empty(),
        "No bundled modules to compile with --aot"
    );
    if !skipped.is_empty() {
        eprintln!(
            "apps: warning: --aot cannot compile these scripts, so they run in the interpreter: {}",
            skipped.join(", ")
        );
    }
    // This entry only declares host globals for Bun. It never runs.
    let mut host =
        NO_SOURCE.to_owned() + "const unknown=JSON.parse('{}',function(){return this;});\n";
    if !wasm.is_empty() {
        let mut wasm_paths = Vec::with_capacity(wasm.len());
        for (index, bytes) in wasm.iter().enumerate() {
            let name = format!("wasm-{index}.wasm");
            fs::write(directory.path().join(&name), bytes)?;
            wasm_paths.push(name);
        }
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load()?,
            Default::default(),
            apps_runtime::jsc::Role::Main,
        )?;
        let modules = serde_json::to_string(
            &wasm
                .iter()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect::<Vec<_>>(),
        )?;
        let valid = engine
            .eval(
                &format!("JSON.stringify({modules}.map(text=>WebAssembly.validate(Uint8Array.from(atob(text),c=>c.charCodeAt(0)))))"),
                "apps:aot",
            )
            .context("Cannot validate embedded Wasm")?;
        let valid: Vec<bool> = serde_json::from_str(&valid)?;
        ensure!(
            valid.len() == wasm_paths.len(),
            "Invalid Wasm validation result"
        );
        for (name, valid) in wasm_paths.iter().zip(valid) {
            if valid {
                host.push_str(&format!("import './{name}';\n"));
            }
        }
    }
    for name in globals {
        if name != "globalThis" {
            let name = serde_json::to_string(&name)?;
            host.push_str(&format!("globalThis[{name}]=unknown[{name}];\n"));
        }
    }
    let host_path = directory.path().join("host.js");
    fs::write(&host_path, host)?;
    paths.push(host_path);
    let folder = CString::new(directory.path().as_os_str().as_encoded_bytes())?;
    let entries = paths
        .iter()
        .map(|path| CString::new(path.as_os_str().as_encoded_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let entries: Vec<_> = entries.iter().map(|entry| entry.as_ptr()).collect();
    let (mut output, mut length) = (std::ptr::null_mut(), 0);
    let built = unsafe {
        front_end(
            folder.as_ptr(),
            entries.as_ptr(),
            entries.len(),
            &mut output,
            &mut length,
        )
    };
    let program = unsafe { std::slice::from_raw_parts(output, length) }.to_vec();
    unsafe { free(output, length) };
    ensure!(
        built,
        "Cannot compile the app for --aot:\n{}",
        String::from_utf8_lossy(&program)
    );
    let image = directory.path().join("image.bin");
    let image_path = CString::new(image.as_os_str().as_encoded_bytes())?;
    let mut modules = vec![0; paths.len()];
    let mut layout = [0; 2];
    let mut error = vec![0; 8192];
    ensure!(
        unsafe {
            compile(
                program.as_ptr(),
                program.len(),
                image_path.as_ptr(),
                modules.as_mut_ptr(),
                modules.len(),
                layout.as_mut_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        },
        "{}",
        unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
    );
    let build = Sha256::digest(&program)[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "runtime": apps_runtime::jsc::RUNTIME_SCRIPTS,
        "build": build,
        "scripts": names
            .iter()
            .zip(modules)
            .map(|(name, module)| (name.clone(), module.into()))
            .collect::<serde_json::Map<_, _>>(),
    }))?;
    for name in &names {
        if let Some(source) = files.get_mut(name) {
            *source = apps_runtime::jsc::aot::source(&build, name);
        }
    }
    fs::write(directory.path().join("manifest.json"), &manifest)?;
    let link = if cfg!(target_os = "linux") {
        link_elf
    } else {
        link_macho
    };
    link(
        directory.path(),
        layout[0],
        layout[1],
        fs::metadata(&image)?.len() as usize,
        manifest.len(),
    )?;
    Ok(directory)
}

fn link_macho(
    directory: &Path,
    header: usize,
    executable: usize,
    image_size: usize,
    manifest_size: usize,
) -> Result<()> {
    fs::write(
        directory.join("image.s"),
        format!(
            r#".section __AOT_HEADER,__header,regular,no_dead_strip
.p2align 14
.globl _apps_aot_image
_apps_aot_image:
.incbin "image.bin", 0, {header}
.section __AOT_CODE,__code,regular,pure_instructions+no_dead_strip
.incbin "image.bin", {header}, {executable}
.section __AOT_CONST,__const,regular,no_dead_strip
.incbin "image.bin", {data}
.section __DATA_CONST,__const
.p2align 3
.globl _apps_aot_image_size
_apps_aot_image_size:
.quad {image_size}
.globl _apps_aot_manifest_size
_apps_aot_manifest_size:
.quad {manifest_size}
.globl _apps_aot_manifest
_apps_aot_manifest:
.incbin "manifest.json"
"#,
            data = header + executable,
        ),
    )?;
    let output = Command::new("xcrun")
        .current_dir(directory)
        .args([
            "clang",
            "-dynamiclib",
            "-arch",
            "arm64",
            "-mmacosx-version-min=13.0",
            "-Wl,-install_name,@rpath/app.dylib",
            "-Wl,-segprot,__AOT_HEADER,r,r",
            "-Wl,-segprot,__AOT_CODE,rx,rx",
            "-Wl,-segprot,__AOT_CONST,r,r",
            "image.s",
            "-o",
            "app.dylib",
        ])
        .output()
        .context("--aot needs the Xcode command line tools")?;
    ensure!(
        output.status.success(),
        "Cannot link app.dylib: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

// Bun splits an ELF payload at the image's code into three PT_LOAD segments: read-only, executable,
// then read-only. The parts stay contiguous and page-aligned, which is what JavaScriptCore checks.
fn link_elf(
    directory: &Path,
    header: usize,
    executable: usize,
    image_size: usize,
    manifest_size: usize,
) -> Result<()> {
    let align = if cfg!(target_arch = "aarch64") {
        65536
    } else {
        16384
    };
    ensure!(
        header.is_multiple_of(align)
            && executable.is_multiple_of(align)
            && image_size.is_multiple_of(align)
            && header + executable <= image_size
            && executable > 0,
        "AOT image layout is not aligned to {align} bytes"
    );
    let data = header + executable;
    let constants = image_size - data;
    fs::write(
        directory.join("image.s"),
        format!(
            r#".section .aot_header, "a", @progbits
.balign {align}
.globl apps_aot_image
apps_aot_image:
.incbin "image.bin", 0, {header}
.section .aot_code, "ax", @progbits
.balign {align}
.incbin "image.bin", {header}, {executable}
.section .aot_const, "a", @progbits
.balign {align}
.incbin "image.bin", {data}, {constants}
.section .rodata
.balign 8
.globl apps_aot_image_size
apps_aot_image_size:
.quad {image_size}
.globl apps_aot_manifest_size
apps_aot_manifest_size:
.quad {manifest_size}
.globl apps_aot_manifest
apps_aot_manifest:
.incbin "manifest.json"
"#
        ),
    )?;
    fs::write(
        directory.join("aot.lds"),
        format!(
            r#"SECTIONS
{{
  .aot_header ALIGN({align}) : {{ KEEP(*(.aot_header)) }}
  .aot_code ALIGN({align}) : {{ KEEP(*(.aot_code)) }}
  .aot_const ALIGN({align}) : {{ KEEP(*(.aot_const)) }}
}}
INSERT AFTER .text;
"#
        ),
    )?;
    let args = [
        "-shared".to_string(),
        "-fPIC".to_string(),
        format!("-Wl,-z,max-page-size={align}"),
        format!("-Wl,-z,common-page-size={align}"),
        "-Wl,-z,noexecstack".to_string(),
        // GNU ld separates code by default on x86 only.
        "-Wl,-z,separate-code".to_string(),
        "-Wl,-T,aot.lds".to_string(),
        "image.s".to_string(),
        "-o".to_string(),
        "app.so".to_string(),
    ];
    let output = Command::new("cc")
        .current_dir(directory)
        .args(&args)
        .output()
        .context("--aot needs a C compiler")?;
    ensure!(
        output.status.success(),
        "Cannot link app.so: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure_elf_layout(&directory.join("app.so"), header, executable, image_size)
}

fn ensure_elf_layout(
    path: &Path,
    header: usize,
    executable: usize,
    image_size: usize,
) -> Result<()> {
    let bytes = fs::read(path)?;
    let file = object::File::parse(bytes.as_slice())
        .with_context(|| format!("Invalid ELF image {}", path.display()))?;
    let start = file
        .dynamic_symbols()
        .find(|symbol| symbol.name() == Ok("apps_aot_image"))
        .context("app.so does not export apps_aot_image")?
        .address();
    check_elf_range(&file, start, header as u64, false)?;
    check_elf_range(&file, start + header as u64, executable as u64, true)?;
    check_elf_range(
        &file,
        start + (header + executable) as u64,
        (image_size - header - executable) as u64,
        false,
    )?;
    for name in [
        "apps_aot_image_size",
        "apps_aot_manifest",
        "apps_aot_manifest_size",
    ] {
        ensure!(
            file.dynamic_symbols()
                .any(|symbol| symbol.name() == Ok(name)),
            "app.so does not export {name}"
        );
    }
    Ok(())
}

fn check_elf_range(file: &object::File, address: u64, size: u64, executable: bool) -> Result<()> {
    if size == 0 {
        ensure!(!executable, "AOT image has no executable bytes");
        return Ok(());
    }
    let segment = file
        .segments()
        .find(|segment| {
            let object::SegmentFlags::Elf { p_type, .. } = segment.flags() else {
                return false;
            };
            if p_type != object::elf::PT_LOAD {
                return false;
            }
            let begin = segment.address();
            let (_, filesz) = segment.file_range();
            address >= begin && filesz.saturating_sub(address - begin) >= size
        })
        .context("AOT image crosses an ELF segment boundary")?;
    let object::SegmentFlags::Elf { p_flags, .. } = segment.flags() else {
        bail!("app.so is not an ELF image");
    };
    let writable = p_flags.0 & object::elf::PF_W.0 != 0;
    let execute = p_flags.0 & object::elf::PF_X.0 != 0;
    ensure!(!writable, "AOT image segment is writable");
    ensure!(
        execute == executable,
        "AOT image segment protections do not match the image"
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn elf_image_uses_bun_segment_protections() {
        let directory = tempfile::tempdir().unwrap();
        let align = if cfg!(target_arch = "aarch64") {
            65536
        } else {
            16384
        };
        let header = align;
        let executable = align * 2;
        let image_size = align * 4;
        fs::write(directory.path().join("image.bin"), vec![0; image_size]).unwrap();
        fs::write(directory.path().join("manifest.json"), b"{}").unwrap();
        link_elf(directory.path(), header, executable, image_size, 2).unwrap();
    }
}
