use anyhow::{Context, Result, ensure};
use base64::Engine;
use bbtree::unsync::BBTreeMap;
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

pub fn compile(files: &mut BBTreeMap<String, String>, runtime: &Path) -> Result<tempfile::TempDir> {
    let library = apps_runtime::jsc::library_path(runtime, "macos")?;
    ensure!(
        library
            .with_file_name("libJavaScriptCoreAOT.dylib")
            .is_file(),
        "--aot needs libJavaScriptCoreAOT.dylib; run bash scripts/build-jsc.sh --aot"
    );
    let compiler = library.with_file_name("libJavaScriptCoreCompiler.dylib");
    let compiler = unsafe { libloading::Library::new(&compiler) }.with_context(|| {
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
    let front = library.with_file_name("libapps_aot.dylib");
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
    let (front_end, free) = unsafe {
        (
            *front.get::<FrontEnd>(b"apps_aot_front_end\0")?,
            *front.get::<Free>(b"apps_aot_free\0")?,
        )
    };
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
    let app = apps_runtime::bundle::script_source(files, true);
    let wrapper = apps_runtime::bundle::script_source(&BBTreeMap::new(), true);
    let loader = wrapper.strip_suffix("\n},true);").unwrap();
    let mut scripts: Vec<(&str, &str)> = files
        .iter()
        .filter(|(name, _)| name.starts_with("script-") && name.ends_with(".js"))
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    scripts.push(("app.js", &app));
    let disabled =
        apps_runtime::bundle::disabled_features(files.get("package.json").map(String::as_str))?;
    let webgpu = disabled & apps_runtime::bundle::feature_bit("webgpu") == 0;
    for script in apps_runtime::jsc::runtime_scripts() {
        if webgpu || script.0 != "apps:gpu" {
            scripts.push(script);
        }
    }
    let mut names = Vec::new();
    let mut paths = Vec::new();
    let mut globals = BTreeSet::new();
    for (name, source) in scripts {
        // Do not wrap classic scripts: wrappers change their global declarations.
        if name.starts_with("script-")
            && !source.starts_with(loader)
            && !source.starts_with("(($appsDefine)=>{\n")
        {
            continue;
        }
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::cjs()).parse();
        ensure!(parsed.diagnostics.is_empty(), "Invalid AOT script {name}");
        let [Statement::ExpressionStatement(expression)] = parsed.program.body.as_slice() else {
            continue;
        };
        let semantic = oxc_semantic::SemanticBuilder::new()
            .with_build_nodes(source.contains("AGFzbQEAAA") && source.contains("atob"))
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
        globals.extend(
            semantic
                .semantic
                .scoping()
                .root_unresolved_references()
                .keys()
                .map(|name| name.to_string()),
        );
        let span = expression.expression.span();
        let path = directory.path().join(format!("entry-{}.cjs", names.len()));
        fs::write(
            &path,
            format!(
                "module.exports=function(){{return (\n{}\n);}};\n",
                &source[span.start as usize..span.end as usize]
            ),
        )?;
        paths.push(path);
        names.push(name.to_owned());
    }
    ensure!(
        !names.is_empty(),
        "No bundled modules to compile with --aot"
    );
    // This entry only declares host globals for Bun. It never runs.
    let mut host = "const unknown=JSON.parse('{}',function(){return this;});\n".to_owned();
    if !wasm.is_empty() {
        let mut wasm_paths = Vec::with_capacity(wasm.len());
        for (index, bytes) in wasm.iter().enumerate() {
            let name = format!("wasm-{index}.wasm");
            fs::write(directory.path().join(&name), bytes)?;
            wasm_paths.push(name);
        }
        let engine = apps_runtime::jsc::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load()?,
            Default::default(),
            apps_runtime::host::Role::Main,
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
            *source = apps_runtime::bundle::aot_source(&build, name);
        }
    }
    fs::write(directory.path().join("manifest.json"), &manifest)?;
    fs::write(
        directory.path().join("image.s"),
        format!(
            r#".section __AOT_HEADER,__header,regular,no_dead_strip
.p2align 14
.globl _apps_aot_image
_apps_aot_image:
.incbin "image.bin", 0, {header}
.section __AOT_CODE,__code,regular,pure_instructions+no_dead_strip
.incbin "image.bin", {header}, {code}
.section __AOT_CONST,__const,regular,no_dead_strip
.incbin "image.bin", {data}
.section __DATA_CONST,__const
.p2align 3
.globl _apps_aot_image_size
_apps_aot_image_size:
.quad {}
.globl _apps_aot_manifest_size
_apps_aot_manifest_size:
.quad {}
.globl _apps_aot_manifest
_apps_aot_manifest:
.incbin "manifest.json"
"#,
            fs::metadata(image)?.len(),
            manifest.len(),
            header = layout[0],
            code = layout[1],
            data = layout[0] + layout[1],
        ),
    )?;
    let output = Command::new("xcrun")
        .current_dir(directory.path())
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
    Ok(directory)
}
