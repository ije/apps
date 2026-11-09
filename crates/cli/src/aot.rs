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
    let archive = library
        .with_file_name("libJavaScriptCoreAOT.a")
        .canonicalize()
        .context("--aot needs libJavaScriptCoreAOT.a; run bash scripts/build-jsc.sh --aot")?;
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
    let (compile, required) = unsafe {
        let revision: unsafe extern "C" fn() -> *const c_char =
            *compiler.get(b"JSGetAOTCompilerRevision\0")?;
        (
            *compiler.get::<Compile>(b"JSCompileAOT\0")?,
            CStr::from_ptr(revision()).to_str()?.to_owned(),
        )
    };
    let bun = std::env::var_os("APPS_BUN").unwrap_or_else(|| "bun".into());
    let revision = Command::new(&bun)
        .arg("--revision")
        .output()
        .context("Cannot run Bun. Set APPS_BUN to an AOT-enabled Bun compiler")?;
    let revision = String::from_utf8_lossy(&revision.stdout);
    let revision = revision.trim().rsplit('+').next().unwrap_or("");
    ensure!(
        revision.len() >= 9 && required.starts_with(revision),
        "--aot requires Bun revision {required}; set APPS_BUN to that compiler"
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
    let app = apps_runtime::bundle::script_source(files, true);
    let wrapper = apps_runtime::bundle::script_source(&BBTreeMap::new(), true);
    let loader = wrapper.strip_suffix("\n},true);").unwrap();
    let mut scripts: Vec<(&str, &str)> = files
        .iter()
        .filter(|(name, _)| name.starts_with("script-") && name.ends_with(".js"))
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    scripts.push(("app.js", &app));
    for script in apps_runtime::jsc::runtime_scripts() {
        scripts.push(script);
    }
    let mut names = Vec::new();
    let mut paths = Vec::new();
    let mut globals = BTreeSet::new();
    for (name, source) in scripts {
        // WARNING: wrapping a classic script changes its global declarations. Keep those scripts in JSC.
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
        // WARNING: wrappers can hide embedded Wasm from Bun's reachability scan. Add its bytes as compiler inputs; JSC matches them by digest at run time.
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
    // WARNING: this entry never runs. It tells Bun that the host supplies these globals at run time.
    let mut host = "const unknown=JSON.parse('{}',function(){return this;});\n".to_owned();
    if !wasm.is_empty() {
        let mut wasm_paths = Vec::with_capacity(wasm.len());
        for (index, bytes) in wasm.iter().enumerate() {
            let name = format!("wasm-{index}.wasm");
            fs::write(directory.path().join(&name), bytes)?;
            wasm_paths.push(name);
        }
        let validation = Command::new(&bun)
            .current_dir(directory.path())
            .args(["-e", "import {readFileSync} from 'node:fs'; console.log(JSON.stringify(process.argv.slice(1).map(path=>WebAssembly.validate(readFileSync(path)))));"])
            .args(&wasm_paths)
            .output()
            .context("Cannot validate embedded Wasm with Bun")?;
        ensure!(
            validation.status.success(),
            "Cannot validate embedded Wasm: {}",
            String::from_utf8_lossy(&validation.stderr)
        );
        let valid: Vec<bool> = serde_json::from_slice(&validation.stdout)?;
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
    let program = directory.path().join("program.ir");
    let output = Command::new(&bun)
        .current_dir(directory.path())
        .args(["build", "--compile", "--aot", "--no-builtin-modules"])
        .args(&paths)
        .arg(format!("--aot-out={}", program.display()))
        .arg("--outfile")
        .arg(directory.path().join("unused"))
        .output()
        .context("Cannot run Bun. Set APPS_BUN to an AOT-enabled Bun compiler")?;
    // WARNING: Bun can fail after it wrote valid IR for a standalone JSC host, so check for the IR file, not the exit
    // status.
    ensure!(
        program.is_file(),
        "Bun did not emit AOT IR. Use the AOT-enabled Bun compiler {required} and set APPS_BUN.\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let program = fs::read(program)?;
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
    fs::write(directory.path().join("exports.txt"), "_JS*\n_apps_aot_*\n")?;
    let output = Command::new("xcrun")
        .current_dir(directory.path())
        .args([
            "clang++",
            "-dynamiclib",
            "-arch",
            "arm64",
            "-mmacosx-version-min=13.0",
            "-Wl,-install_name,@rpath/app.dylib",
            "-Wl,-dead_strip",
            "-Wl,-x",
            "-Wl,-exported_symbols_list,exports.txt",
            "-Wl,-segprot,__AOT_HEADER,r,r",
            "-Wl,-segprot,__AOT_CODE,rx,rx",
            "-Wl,-segprot,__AOT_CONST,r,r",
            "image.s",
            "-o",
            "app.dylib",
            "-licucore",
            "-lobjc",
            "-Xlinker",
            "-force_load",
            "-Xlinker",
        ])
        .arg(&archive)
        .output()
        .context("--aot needs the Xcode command line tools")?;
    ensure!(
        output.status.success(),
        "Cannot link app.dylib: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(directory)
}
