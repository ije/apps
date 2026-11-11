use std::process::Command;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::{
    fs,
    path::Path,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run(path: &Path) -> Output {
    let mut child = Command::new(path)
        .env_remove("APPS_JSC_LIBRARY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "AOT app timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn rejects_unsupported_aot_targets() {
    let output = Command::new(env!("CARGO_BIN_EXE_apps"))
        .args(["build", "--aot", "--platform", "windows"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--aot currently requires a native macOS arm64 build")
    );
}

#[test]
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[ignore = "requires an AOT-enabled APPS_JSC_LIBRARY and apps-runtime"]
fn packages_and_runs_native_modules() {
    use object::{Object, ObjectSection};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("package.json"),
        r#"{"name":"AotTest","features":[]}"#,
    )
    .unwrap();
    fs::write(root.join("sum.wasm"), b"\x00\x61\x73\x6d\x01\x00\x00\x00\x01\x06\x01\x60\x01\x7f\x01\x7f\x03\x02\x01\x00\x07\x07\x01\x03\x72\x75\x6e\x00\x00\x0a\x1b\x01\x19\x01\x01\x7f\x03\x40\x20\x01\x20\x00\x6a\x21\x01\x20\x00\x41\x01\x6b\x22\x00\x0d\x00\x0b\x20\x01\x0b").unwrap();
    fs::create_dir_all(root.join("node_modules/sloppy")).unwrap();
    fs::write(
        root.join("node_modules/sloppy/package.json"),
        r#"{"main":"index.cjs"}"#,
    )
    .unwrap();
    fs::write(
        root.join("node_modules/sloppy/index.cjs"),
        "module.exports = function () { return this === globalThis; };",
    )
    .unwrap();
    fs::write(
        root.join("lazy.js"),
        "export const answer = () => globalThis.aotValue + 2;",
    )
    .unwrap();
    fs::write(root.join("worker.js"), "import {parentPort} from 'node:worker_threads'; globalThis.workerAotMarker='worker-aot-sentinel'; parentPort.postMessage([1,2,3].map(x => x * 7).join(','));").unwrap();
    fs::write(root.join("page.html"), r#"<h1>AOT</h1><script>(()=>{globalThis.classicArguments=typeof arguments;})();</script><script type="preload" src="./preload.js"></script><script type="module" src="./page.js"></script>"#).unwrap();
    fs::write(root.join("preload.js"), "import {contextBridge,ipcRenderer} from 'electron'; contextBridge.exposeInMainWorld('bridge',{done:value=>ipcRenderer.invoke('aot-page',value)});").unwrap();
    fs::write(root.join("page.js"), r#"const {answer}=await import('./page-lazy.js');
let blocked=0;
for (const call of [()=>eval('1'),()=>Function('return 1')(),()=> (function(){}).constructor('return 1')(),()=> (async function(){}).constructor('return 1')(),()=> (function*(){}).constructor('yield 1')(),()=> (async function*(){}).constructor('yield 1')()]) {
    try { call(); } catch(error) { if(error instanceof EvalError) blocked++; else throw error; }
}
await bridge.done([answer(),document.querySelector('h1').textContent,globalThis.classicArguments,blocked].join(','));"#).unwrap();
    fs::write(root.join("page-lazy.js"), "export const answer=()=>42;").unwrap();
    fs::write(
        root.join("server.js"),
        "console.log('headless-aot'); process.exit(0);",
    )
    .unwrap();
    fs::write(root.join("unhandled.js"), "queueMicrotask(()=>{throw Error('aot-unhandled-microtask')}); setTimeout(()=>process.exit(0),10);").unwrap();
    fs::write(root.join("main.js"), r#"
import assert from 'node:assert/strict';
import {app,BrowserWindow,ipcMain} from 'electron';
import {Worker} from 'node:worker_threads';
import {execFileSync,spawnSync} from 'node:child_process';
import sloppy from 'sloppy';
import wasmURL from './sum.wasm';
assert.equal(sloppy(), true);
function aotSource(value) { return 918273645 + value; }
assert.ok(aotSource.toString().includes(aotSource(0)));
assert.throws(() => globalThis.aotMissing.call918(), /evaluating 'globalThis\.aotMissing\.call918'/);
const entries = [];
for (const [key, value] of Object.entries(JSON.parse('{"left":3,"right":4}'))) entries.push(key, value);
assert.equal(entries.join(','), 'left,3,right,4');
const bytes = Uint8Array.from(atob('AGFzbQEAAAABBgFgAX8BfwMCAQAHBwEDc3VtAAAKGwEZAQF/A0AgASAAaiEBIABBAWsiAA0ACyABCw=='), c => c.charCodeAt(0));
const wasm = new WebAssembly.Instance(new WebAssembly.Module(bytes));
assert.equal(wasm.exports.sum(10000), 50005000);
const malformed = atob('AGFzbQEAAAD/');
assert.equal(malformed.length, 9);
assert.equal(WebAssembly.validate(Uint8Array.from(malformed, c => c.charCodeAt(0))), false);
globalThis.aotValue = 40;
setTimeout(async () => {
    try {
        const asset = new WebAssembly.Instance(new WebAssembly.Module(await (await fetch(wasmURL)).arrayBuffer()));
        assert.equal(asset.exports.run(10000), 50005000);
        const {answer} = await import('./lazy.js');
        assert.equal(answer(), 42);
        globalThis.aotValue = 41;
        assert.equal(answer(), 43);
        const worker = new Worker(new URL('./worker.js', import.meta.url));
        assert.equal(await new Promise((resolve, reject) => {worker.on('message', resolve);worker.on('error', reject)}), '7,14,21');
        await worker.terminate();
        assert.equal(execFileSync(process.execPath, [import.meta.resolve('./server.js')], {env:{...process.env,ELECTRON_RUN_AS_NODE:'1'},encoding:'utf8'}).trim(), 'headless-aot');
        const unhandled = spawnSync(process.execPath, [import.meta.resolve('./unhandled.js')], {env:{...process.env,ELECTRON_RUN_AS_NODE:'1'},encoding:'utf8'});
        assert.equal(unhandled.status, 1);
        assert.match(unhandled.stderr, /aot-unhandled-microtask/);
        assert.throws(() => { throw Error('aot exception'); }, /aot exception/);
        app.on('window-all-closed', () => {});
        await app.whenReady();
        app.setActivationPolicy('prohibited');
        for (let index=0; index<2; index++) {
            let done;
            const result = new Promise(resolve => {done=resolve});
            ipcMain.removeHandler('aot-page');
            ipcMain.handle('aot-page', (_event,value) => {done(value)});
            const window = new BrowserWindow({show:false, focusable:false});
            await window.loadFile(import.meta.resolve('./page.html'));
            assert.equal(await result, '42,AOT,undefined,6');
            window.destroy();
        }
        console.log('aot-ok');
        process.exit(0);
    } catch (error) {
        console.error(error);
        process.exit(1);
    }
}, 0);
"#).unwrap();
    let mut build = Command::new(env!("CARGO_BIN_EXE_apps"));
    build.current_dir(root).args(["build", "main.js"]);
    let output = build.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let baseline =
        apps_runtime::bundle::read(&root.join("dist/AotTest.app/Contents/Resources")).unwrap();
    let worker = baseline
        .scripts
        .iter()
        .find(|(_, source)| String::from_utf8_lossy(source).contains("worker-aot-sentinel"))
        .unwrap()
        .0
        .clone();
    drop(baseline);
    let output = build.arg("--aot").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = root.join("dist");
    let resources = out.join("AotTest.app/Contents/Resources");
    let dylib = resources.join("app.dylib");
    let executable = out.join("AotTest");
    let native = fs::read(&dylib).unwrap();
    assert_eq!(&native[..4], &0xfeed_facfu32.to_le_bytes());
    let file =
        object::read::macho::MachOFile64::<object::Endianness>::parse(native.as_slice()).unwrap();
    let header = file.section_by_name("__header").unwrap();
    assert_eq!(&header.data().unwrap()[..8], b"BUNAOT01");
    let header = header.file_range().unwrap().0 as usize;
    let jsc =
        fs::read(out.join("AotTest.app/Contents/Frameworks/libJavaScriptCore.dylib")).unwrap();
    for (bytes, prefix, names) in [
        (
            &native,
            "_apps_aot_",
            &["_apps_aot_image", "_apps_aot_manifest"][..],
        ),
        (
            &jsc,
            "_JS",
            &[
                "_JSContextGroupCreate",
                "_JSLoadAOT",
                "_JSAttachAOT",
                "_JSGetAOTFunction",
            ][..],
        ),
    ] {
        let file = object::read::macho::MachOFile64::<object::Endianness>::parse(bytes.as_slice())
            .unwrap();
        let mut commands = file.macho_load_commands().unwrap();
        while let Some(command) = commands.next().unwrap() {
            if let Some(dylib) = command.dylib().unwrap() {
                let name =
                    std::str::from_utf8(command.string(file.endian(), dylib.dylib.name).unwrap())
                        .unwrap();
                assert!(
                    name.starts_with("/usr/lib/") || name.starts_with("/System/Library/"),
                    "AOT libraries must only depend on system libraries: {name}"
                );
            }
        }
        let exports: std::collections::BTreeSet<_> = file
            .exports()
            .unwrap()
            .map(|symbol| {
                std::str::from_utf8(symbol.unwrap().name().name().unwrap())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        for name in names {
            assert!(exports.contains(*name), "Missing export: {name}");
        }
        assert!(!exports.contains("_JSCompileAOT"));
        assert!(!exports.contains("_JSGetAOTCompilerRevision"));
        assert!(exports.iter().all(|name| name.starts_with(prefix)));
    }
    assert_eq!(
        u32::from_le_bytes(native[header + 72..header + 76].try_into().unwrap()),
        4
    );
    assert!(
        !native.windows(9).any(|bytes| bytes == b"918273645"),
        "function source must be compressed"
    );
    assert!(
        u32::from_le_bytes(native[header + 352..header + 356].try_into().unwrap()) > 0,
        "embedded Wasm must compile to native code"
    );
    let wasm_directory =
        u32::from_le_bytes(native[header + 280..header + 284].try_into().unwrap()) as usize;
    assert_eq!(
        u32::from_le_bytes(
            native[header + wasm_directory..header + wasm_directory + 4]
                .try_into()
                .unwrap()
        ),
        2,
        "embedded and packaged Wasm must both compile"
    );
    let bundle = apps_runtime::bundle::read(&resources).unwrap();
    let start = native
        .windows(12)
        .position(|bytes| bytes == br#"{"version":1"#)
        .unwrap();
    let manifest: serde_json::Value = serde_json::Deserializer::from_slice(&native[start..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(manifest["runtime"], apps_runtime::jsc::RUNTIME_SCRIPTS);
    let id = manifest["build"].as_str().unwrap();
    let scripts = manifest["scripts"].as_object().unwrap();
    for (name, _) in apps_runtime::jsc::runtime_scripts() {
        assert_eq!(scripts.contains_key(name), name != "apps:gpu", "{name}");
    }
    for name in ["app.js", worker.as_str()] {
        assert!(scripts[name].as_u64().unwrap() > 0, "{name} must compile");
    }
    for name in scripts.keys().filter(|name| !name.starts_with("apps:")) {
        let expected = apps_runtime::bundle::aot_source(id, name);
        if let Some(source) = bundle.files.get(name) {
            assert_eq!(source, &expected);
        } else {
            assert_eq!(&*bundle.scripts[name], expected.as_bytes());
        }
    }
    assert!(
        !String::from_utf8_lossy(&fs::read(resources.join("app.bundle")).unwrap())
            .contains("worker-aot-sentinel")
    );
    let mut zip = zip::ZipArchive::new(fs::File::open(out.join("AotTest.zip")).unwrap()).unwrap();
    for name in ["app.bundle", "app.dylib"] {
        assert!(
            zip.by_name(&format!("AotTest.app/Contents/Resources/{name}"))
                .is_ok()
        );
    }
    assert!(
        zip.by_name("AotTest.app/Contents/Frameworks/libJavaScriptCore.dylib")
            .is_ok()
    );
    drop(zip);
    let output = run(&executable);
    if !output.status.success() {
        let path = directory.keep();
        panic!(
            "AOT app failed: {:?}\nstdout: {}\nstderr: {}\nfixture: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            path.display()
        );
    }
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("aot-ok"),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::rename(&dylib, resources.join("saved.dylib")).unwrap();
    let output = run(&executable);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Missing AOT image"));
    fs::rename(resources.join("saved.dylib"), &dylib).unwrap();

    let original = fs::read(resources.join("app.bundle")).unwrap();
    let mut files = bundle.files;
    let mut assets: std::collections::BTreeMap<String, std::path::PathBuf> =
        serde_json::from_str(&files["assets.json"]).unwrap();
    for path in assets.values_mut() {
        *path = root.join("sum.wasm");
    }
    files.insert(
        "assets.json".into(),
        serde_json::to_string(&assets).unwrap(),
    );
    for (name, page) in bundle.pages {
        files.insert(name, page.serialize(0));
    }
    for (name, bytes) in bundle.scripts {
        files.insert(name, String::from_utf8(bytes.to_vec()).unwrap());
    }
    for name in ["app.js", worker.as_str()] {
        let mut changed = files.clone();
        changed.insert(name.into(), apps_runtime::bundle::aot_source("0", name));
        apps_runtime::bundle::write(&resources.join("app.bundle"), &changed, &[]).unwrap();
        let output = run(&executable);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("app.dylib does not match app.bundle"),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(resources.join("app.bundle"), &original).unwrap();

    let jsc = std::path::PathBuf::from(std::env::var_os("APPS_JSC_LIBRARY").unwrap());
    let without_aot = root.join("without-aot").join(jsc.file_name().unwrap());
    fs::create_dir_all(without_aot.parent().unwrap()).unwrap();
    fs::copy(&jsc, &without_aot).unwrap();
    let output = build.env("APPS_JSC_LIBRARY", without_aot).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(resources.join("app.bundle")).unwrap(), original);

    let output = Command::new(env!("CARGO_BIN_EXE_apps"))
        .current_dir(root)
        .args(["build", "main.js"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dylib.exists());
    assert!(
        !apps_runtime::bundle::read(&resources).unwrap().files["app.js"]
            .starts_with(apps_runtime::bundle::AOT_PREFIX)
    );
    let output = run(&executable);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("aot-ok"),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for feature in ["jsExecute", "httpImport"] {
        fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"AotTest","features":["{feature}"]}}"#),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_apps"))
            .current_dir(root)
            .args(["build", "main.js", "--aot"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&format!("--aot is ignored, because the {feature} feature")),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!dylib.exists());
    }
}
