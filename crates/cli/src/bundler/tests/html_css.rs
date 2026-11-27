use super::*;

fn asset_name(root: &Path, path: &Path) -> Option<String> {
    Shared {
        root: root.to_owned(),
        ..Shared::default()
    }
    .asset_name(path)
}

#[test]
fn html_and_css_images_become_bundled_assets() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.ts");
    fs::write(
        &entry,
        "console.log(import.meta.resolve('./web/index.html'));",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("web/css")).unwrap();
    fs::create_dir_all(dir.path().join("img")).unwrap();
    fs::write(dir.path().join("img/logo.png"), b"png").unwrap();
    fs::write(dir.path().join("img/icon.svg"), b"<svg/>").unwrap();
    for name in ["clip.mp4", "clip.webm", "tone.mp3"] {
        fs::write(dir.path().join("img").join(name), b"media").unwrap();
    }
    fs::write(
        dir.path().join("web/css/style.css"),
        ".a { background: url(\"../../img/icon.svg\") }",
    )
    .unwrap();
    fs::write(
            dir.path().join("web/index.html"),
            r#"<link rel="stylesheet" href="css/style.css"><style>.b{background:url(../img/logo.png)}.c{background:url(data:image/png;base64,AA==)}</style><img src="../img/logo.png"><img src="https://example.com/x.png"><div style="background-image:url('../img/missing.png')"></div><video src="../img/clip.mp4" poster="../img/logo.png"><source src="../img/clip.webm"></video><audio src="../img/tone.mp3"></audio>"#,
        )
        .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let assets: std::collections::BTreeMap<String, PathBuf> =
        serde_json::from_str(&files["assets.json"]).unwrap();
    assert_eq!(assets.len(), 5);
    let root = dir.path().canonicalize().unwrap();
    let logo = asset_name(&root, &root.join("img/logo.png")).unwrap();
    let icon = asset_name(&root, &root.join("img/icon.svg")).unwrap();
    assert!(logo.starts_with("assets/") && logo.ends_with(".png"));
    assert_eq!(
        assets[&icon],
        dir.path().join("img/icon.svg").canonicalize().unwrap()
    );
    let page = &files["web/index.html"];
    assert_eq!(page.matches(&format!("apps:///{logo}")).count(), 3);
    for name in ["clip.mp4", "clip.webm", "tone.mp3"] {
        let media = asset_name(&root, &root.join("img").join(name)).unwrap();
        assert!(page.contains(&format!("src=\"apps:///{media}\"")), "{page}");
    }
    assert!(linked_css(page).contains(&format!("url(\"apps:///{icon}\")")));
    assert!(
        page.contains("https://example.com/x.png") && page.contains("data:image/png;base64,AA==")
    );
    assert!(page.contains("../img/missing.png"));

    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();
    apps_runtime::bundle::write(&out.join("app.bundle"), &files, &[]).unwrap();
    assert_eq!(fs::read(out.join(&logo)).unwrap(), b"png");
    let bundle = apps_runtime::bundle::read(&out).unwrap();
    let resolved: std::collections::BTreeMap<String, PathBuf> =
        serde_json::from_str(&bundle.files["assets.json"]).unwrap();
    assert_eq!(resolved[&logo], out.canonicalize().unwrap().join(&logo));
}

#[test]
fn page_script_asset_paths_become_bundled_assets() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    fs::create_dir_all(app.join("assets")).unwrap();
    fs::write(app.join("assets/cover.jpg"), b"jpg").unwrap();
    fs::write(app.join("assets/font.woff2"), b"woff2").unwrap();
    fs::write(dir.path().join("outside.png"), b"png").unwrap();
    fs::write(
        app.join("page.js"),
        "const o = {'/assets/cover.jpg': 1}; document.body.title = ['/assets/cover.jpg', `/assets/font.woff2`, './assets/font.woff2', '/assets/missing.png', '/../outside.png', '/assets/data.json', o].join();",
    )
    .unwrap();
    fs::write(
        app.join("index.html"),
        "<script type='module' src='./page.js'></script>",
    )
    .unwrap();
    let entry = app.join("main.js");
    fs::write(
        &entry,
        "console.log(import.meta.resolve('./index.html'), '/assets/cover.jpg');",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let root = app.canonicalize().unwrap();
    let cover = asset_name(&root, &root.join("assets/cover.jpg")).unwrap();
    let font = asset_name(&root, &root.join("assets/font.woff2")).unwrap();
    let assets: std::collections::BTreeMap<String, PathBuf> =
        serde_json::from_str(&files["assets.json"]).unwrap();
    assert!(assets.contains_key(&cover) && assets.contains_key(&font) && assets.len() == 2);
    let output = files.values().cloned().collect::<String>();
    assert_eq!(output.matches(&format!("apps:///{cover}")).count(), 1);
    assert!(output.contains(&format!("apps:///{font}")));
    // A `./` string is relative to the page when it runs, not to the module, so it stays.
    for kept in [
        "./assets/font.woff2",
        "/assets/missing.png",
        "/../outside.png",
        "/assets/data.json",
        "'/assets/cover.jpg': 1",
    ] {
        assert!(output.contains(kept), "{kept}");
    }
    assert!(files["app.js"].contains("/assets/cover.jpg"));
}

#[test]
fn production_optimizes_html_scripts() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("page.ts"), "const unused: string = 'UNUSED_PAGE_MARKER'; await Promise.resolve(); document.querySelector('h1').textContent = '</script>';").unwrap();
    fs::write(dir.path().join("index.html"), "<html><head><script type='preload'>const unused = 'UNUSED_PRELOAD_MARKER'; if (!process.versions.apps) throw Error('preload');</script></head><body><h1>Hello</h1><script type='module' src='./page.ts'></script><script type='module'>const unused = 'UNUSED_INLINE_MARKER'; if (!document.querySelector('h1')) throw Error('page');</script></body></html>").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let output = files.values().cloned().collect::<String>();
    for marker in [
        "UNUSED_PAGE_MARKER",
        "UNUSED_PRELOAD_MARKER",
        "UNUSED_INLINE_MARKER",
    ] {
        assert!(!output.contains(marker), "{marker}");
    }
    assert_eq!(
        apps_runtime::desktop::Desktop::new(&files)
            .unwrap()
            .check()
            .unwrap(),
        0
    );
}

#[test]
fn glass_classes_work_with_nested_css_and_no_glass_import() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("style.css"), "@import './base.css';").unwrap();
    let path = dir.path().join("index.html");
    for (tailwind, classes, material) in [
        (
            true,
            "bg-glass-interactive bg-glass-blue-500 px-4 w-12 h-6",
            "regular",
        ),
        (false, "bg-glass bg-glass-interactive", "regular"),
        (false, "bg-glass-clear bg-glass-interactive", "clear"),
    ] {
        fs::write(
            dir.path().join("base.css"),
            if tailwind {
                "@import 'tailwindcss';"
            } else {
                "button{padding-left:16px;width:48px;height:24px}"
            },
        )
        .unwrap();
        fs::write(
            &path,
            format!(
                "<link rel='stylesheet' href='./style.css'><button class='{classes}'></button>"
            ),
        )
        .unwrap();
        let source = Rc::<Shared>::default().html(&path).unwrap();
        assert_eq!(
            linked_css(&source).contains("@import \"tailwindcss\";"),
            tailwind
        );
        let style = apps_runtime::style::styles(&apps_runtime::bundle::page(&source), 800., 600.)
            .pop()
            .unwrap();
        assert_eq!(
            style.get("-macos-glass").map(String::as_str),
            cfg!(target_os = "macos").then_some(material)
        );
        assert_eq!(
            style.get("-macos-glass-interactive").map(String::as_str),
            cfg!(target_os = "macos").then_some("true")
        );
        if tailwind {
            assert_eq!(
                style
                    .get("-macos-glass-tint-color")
                    .and_then(|value| apps_runtime::css::value::color::color(value)),
                apps_runtime::css::value::color::color("oklch(62.3% 0.214 259.815)")
            );
        }
        for (property, expected) in [("padding-left", 16.), ("width", 48.), ("height", 24.)] {
            assert_eq!(
                apps_runtime::css::value::length(&style[property], 0., 16., 800., 600.),
                expected
            );
        }
    }
}

#[test]
fn tailwind_import_survives_nested_css_without_a_package() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("base.css"),
        "@import 'tailwindcss'; @theme { --color-mint-500: oklch(0.72 0.11 178); }",
    )
    .unwrap();
    fs::write(
        dir.path().join("style.css"),
        "@import './base.css'; div { color: #123456; @apply rounded-b-lg shadow-md; }",
    )
    .unwrap();
    let path = dir.path().join("index.html");
    fs::write(
        &path,
        "<link rel='stylesheet' href='./style.css'><div class='h-4 w-4 flex bg-mint-500'></div>",
    )
    .unwrap();
    let files: Rc<RefCell<BTreeSet<PathBuf>>> = Rc::default();
    let shared = Rc::new(Shared {
        files: Rc::clone(&files),
        ..Shared::default()
    });
    let source = shared.html(&path).unwrap();
    assert!(linked_css(&source).contains("@import \"tailwindcss\";"));
    assert!(!source.contains(".h-4"));
    assert_eq!(files.borrow().len(), 2);
    let doc = apps_runtime::bundle::page(&source);
    let style = apps_runtime::style::styles(&doc, 800., 600.).pop().unwrap();
    let px = |key: &str| apps_runtime::css::value::length(&style[key], 0., 16., 800., 600.);
    assert_eq!(px("height"), 16.);
    assert_eq!(px("width"), 16.);
    assert_eq!(style["display"], "flex");
    assert_eq!(style["color"], "#123456");
    assert_eq!(style["background-color"], "oklch(0.72 0.11 178)");
    assert_eq!(px("border-bottom-left-radius"), 8.);
    assert!(style.contains_key("box-shadow"));
}

#[test]
fn tailwind_parts_keep_their_imports() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("style.css"),
        "@layer theme, base, components, utilities;\n@import 'tailwindcss/theme.css' layer(theme);\n@import \"tailwindcss/utilities\" layer(utilities);",
    )
    .unwrap();
    let path = dir.path().join("index.html");
    fs::write(
        &path,
        "<link rel='stylesheet' href='./style.css'><ul class='flex'></ul>",
    )
    .unwrap();
    let source = Rc::<Shared>::default().html(&path).unwrap();
    let css = linked_css(&source);
    assert!(css.contains("@import \"tailwindcss/theme.css\" layer(theme);"));
    assert!(css.contains("@import \"tailwindcss/utilities\" layer(utilities);"));
    let style = apps_runtime::style::styles(&apps_runtime::bundle::page(&source), 800., 600.)
        .pop()
        .unwrap();
    assert_eq!(style["display"], "flex");
    assert_eq!(style["list-style-type"], "disc");
}

#[test]
fn tailwind_import_options_reach_imported_files() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
    write(
        "app.css",
        "@import 'tailwindcss'; @theme { --color-brand: #010203; } .card { --at: '@theme' }",
    );
    write(
        "style.css",
        "@import './app.css' layer(app) prefix(tw) important theme(static) source('./x');",
    );
    let path = dir.path().join("index.html");
    write(
        "index.html",
        "<link rel='stylesheet' href='./style.css'><p class='tw:bg-brand flex'></p>",
    );
    let source = Rc::<Shared>::default().html(&path).unwrap();
    let css = linked_css(&source);
    assert!(css.contains("@import \"tailwindcss\" prefix(tw) important theme(static) layer(app);"));
    assert!(css.contains("@theme prefix(tw)  static  {"), "{css}");
    assert!(css.contains("--at: '@theme'"));
    let style = apps_runtime::style::styles(&apps_runtime::bundle::page(&source), 800., 600.)
        .pop()
        .unwrap();
    assert_eq!(style["background-color"], "#010203");
    assert_eq!(style["display"], "block");
}

#[test]
fn referenced_tailwind_files_give_no_rules() {
    for option in ["reference", "theme(reference)"] {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
        write(
            "app.css",
            "/* ; */ @import 'tailwindcss'; @theme { --color-brand: #010203; --x: ';}'; }
            @layer base { @theme { --color-old: #040506; } p { margin: 9px } }
            @utility box { color: #070809; &:hover { color: red } } p { padding: 9px }
            @custom-variant on (&.on); @custom-variant off { &:not(.on) { @slot; } }",
        );
        write(
            "style.css",
            &format!(
                "@import 'tailwindcss'; @import './app.css' {option}; @theme {{ --color-new: red; }}"
            ),
        );
        let path = dir.path().join("index.html");
        write(
            "index.html",
            "<link rel='stylesheet' href='./style.css'><p class='box on on:bg-brand off:hidden border-old p-2'></p>",
        );
        let source = Rc::<Shared>::default().html(&path).unwrap();
        let css = linked_css(&source);
        assert!(!css.contains("9px"), "{css}");
        let style = apps_runtime::style::styles(&apps_runtime::bundle::page(&source), 800., 600.)
            .pop()
            .unwrap();
        assert_eq!(style["background-color"], "#010203");
        assert_eq!(style["border-top-color"], "#040506");
        assert_eq!(style["color"], "#070809");
        assert_eq!(style["display"], "block");
        let px = |key: &str| apps_runtime::css::value::length(&style[key], 0., 16., 800., 600.);
        assert_eq!(px("padding-left"), 8.);
    }
}

#[test]
fn html_bundles_css_scripts_and_preload() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("base.css"), "h1 { color: #123456 }").unwrap();
    fs::write(
        dir.path().join("style.css"),
        "@import './base.css'; body { margin: 0 }",
    )
    .unwrap();
    fs::write(dir.path().join("page.ts"),r#"const end: string = "</script>"; if (end !== '<' + '/script>') throw Error('escape'); document.querySelector('h1').textContent = end; if(typeof process !== 'undefined') throw Error('privilege leak'); if (!(globalThis as any).preloaded) throw Error('preload');"#).unwrap();
    fs::write(dir.path().join("index.html"),"<html><head><link rel='stylesheet' href='./style.css'><script type='preload'>__apps.contextBridge.exposeInMainWorld('preloaded', process.versions.apps)</script></head><body><h1>Hello</h1><script type='module' src='./page.ts'></script></body></html>").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry,"import {app,BrowserWindow} from '@pierre/apps';import html from './index.html' with {type:'text'};if(!html.includes('#123456')) throw Error('css');app.whenReady().then(()=>new BrowserWindow().loadFile('index.html'));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    assert_eq!(
        apps_runtime::desktop::Desktop::new(&files)
            .unwrap()
            .check()
            .unwrap(),
        0
    );
}

#[test]
fn load_file_literals_bundle_plain_electron_pages() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("styles.css"),
            "#color { color: rgb(1, 2, 3) }",
        )
        .unwrap();
        fs::write(dir.path().join("preload.js"), "import {contextBridge} from 'electron'; contextBridge.exposeInMainWorld('versions', {electron: process.versions.electron});").unwrap();
        fs::write(dir.path().join("renderer.js"), "if (versions.electron !== '40.0.0' || getComputedStyle(document.getElementById('color')).color !== 'rgb(1, 2, 3)' || document.querySelector('script[type=\"application/json\"]').textContent !== '{}') throw Error('renderer');").unwrap();
        fs::write(dir.path().join("index.html"), "<link rel='stylesheet' href='styles.css'><script type='preload' src='./preload.js'></script><p id='color'>x</p><script type='application/json'>{}</script><script type='text/javascript' src='renderer.js'></script>").unwrap();
        let entry = dir.path().join("main.js");
        fs::write(&entry, "import {app, BrowserWindow} from 'electron'; const page = 'missing.html'; app.whenReady().then(() => { new BrowserWindow().loadFile('index.html', {hash: 'top'}); new BrowserWindow().webContents.loadFile('./index.html'); globalThis.missing = new BrowserWindow().loadFile(page).catch(() => {}); });").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(
            files.contains_key("index.html")
                && files.keys().filter(|name| name.ends_with(".html")).count() == 1
        );
        let app = apps_runtime::jsc::script_source(&files, false);
        assert!(app.matches("apps:///index.html").count() == 2 && app.contains("missing.html"));
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
    }
}

#[test]
fn resolved_html_survives_relocation() {
    for production in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("style.css"), "h1 { color: #123456 }").unwrap();
        fs::write(nested.join("page.ts"), "if (document.querySelector('h1').textContent !== 'Hello') throw Error('page'); document.querySelector('h1').textContent = 'Loaded';").unwrap();
        fs::write(nested.join("index.html"), "<link rel='stylesheet' href='./style.css'><script type='preload'>if (!process.versions.apps) throw Error('preload');</script><h1>Hello</h1><script type='module' src='./page.ts'></script>").unwrap();
        fs::write(
            nested.join("window.ts"),
            "export const page = import.meta.resolve('./index.html');",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; import {page} from './nested/window'; if (page !== import.meta.resolve('./nested/index.html')) throw Error('resolve'); app.whenReady().then(() => new BrowserWindow().loadFile(page));").unwrap();
        let watched = Rc::default();
        let files = compile(
            &entry,
            Realm::Main,
            None,
            Rc::clone(&watched),
            production,
            None,
        )
        .unwrap();
        assert_eq!(watched.borrow().len(), 5);
        assert_eq!(
            files.keys().filter(|name| name.ends_with(".html")).count(),
            1
        );
        assert!(files["nested/index.html"].contains("#123456"));
        let output = tempfile::tempdir().unwrap();
        apps_runtime::bundle::write(&output.path().join("app.bundle"), &files, &[]).unwrap();
        dir.close().unwrap();
        let bundle = apps_runtime::bundle::read(output.path()).unwrap();
        let engine = apps_runtime::host::context::JsContext::new(
            apps_runtime::jsc::JavaScriptCore::load().unwrap(),
            Rc::default(),
            apps_runtime::jsc::Role::Main,
        )
        .unwrap();
        engine.validate_bundle(&files).unwrap();
        assert_eq!(
            apps_runtime::desktop::Desktop::from_bundle(bundle)
                .unwrap()
                .check()
                .unwrap(),
            0
        );
    }
}

#[test]
fn stylesheets_cannot_close_style_or_read_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let theme = dir.path().join("node_modules/theme");
    fs::create_dir_all(&theme).unwrap();
    fs::write(dir.path().join("secret.txt"), ".secret { color: red }").unwrap();
    fs::write(theme.join("base.css"), "h1 { color: #123456 }").unwrap();
    fs::write(
        theme.join("screen.css"),
        "@IMPORT './base.css'; p { color: #654321 }",
    )
    .unwrap();
    fs::write(
        theme.join("evil.css"),
        "a{}</STYLE><script type=\"preload\">__apps.child_process.exec('touch pwned')</script><style>",
    )
    .unwrap();
    fs::write(theme.join("split.css"), "a{}<@import './tail.css';").unwrap();
    fs::write(
        theme.join("tail.css"),
        "/style><script type=\"preload\">__apps.child_process.exec('touch pwned')</script><style>",
    )
    .unwrap();
    let page = dir.path().join("index.html");
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "console.log(import.meta.resolve('./index.html'));").unwrap();
    fs::write(&page, "<style>@import './node_modules/theme/screen.css'; body { margin: 0 }</style><link rel='stylesheet' media='print' href='./node_modules/theme/base.css'>").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let doc = Document::parse(&files["index.html"]);
    let [style, link] =
        ["style", "link"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
    let style = doc.text(style);
    assert!(style.contains("#123456") && style.contains("#654321"));
    assert!(!style.contains("@IMPORT"));
    assert_eq!(
        doc.nodes[link].attrs.get("media").map(String::as_str),
        Some("print")
    );
    assert!(linked_css(&files["index.html"]).contains("#123456"));
    for (html, message) in [
        (
            "<link rel='stylesheet' href='./node_modules/theme/evil.css'>",
            "</style>",
        ),
        (
            "<style>@import './node_modules/theme/evil.css';</style>",
            "</style>",
        ),
        (
            "<link rel='stylesheet' href='./node_modules/theme/split.css'>",
            "</style>",
        ),
        ("<style>@import './secret.txt';</style>", ".css files"),
        ("<link rel='stylesheet' href='./secret.txt'>", ".css files"),
    ] {
        fs::write(&page, html).unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{html}: {error:#}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), theme.join("link.css")).unwrap();
        fs::write(
            &page,
            "<link rel='stylesheet' href='./node_modules/theme/link.css'>",
        )
        .unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
        assert!(format!("{error:#}").contains(".css files"), "{error:#}");
    }
}

#[test]
fn backslash_paths_are_root_relative_and_stay_in_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    fs::create_dir_all(app.join("img")).unwrap();
    fs::write(dir.path().join("outside.css"), "p { color: red }").unwrap();
    fs::write(dir.path().join("outside.png"), "png").unwrap();
    fs::write(app.join("img/a.png"), "png").unwrap();
    fs::write(app.join("style.css"), "p { color: #123456 }").unwrap();
    let entry = app.join("main.js");
    fs::write(&entry, "console.log(import.meta.resolve('./index.html'));").unwrap();
    fs::write(
        app.join("index.html"),
        r"<link rel='stylesheet' href='\style.css'><img src='\img\a.png'><img src='\..\outside.png'>",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let page = &files["index.html"];
    assert!(linked_css(page).contains("#123456"), "{page}");
    assert!(page.contains("src=\"apps:///assets/"), "{page}");
    assert!(page.contains(r"\..\outside.png"), "{page}");
    for html in [
        r"<link rel='stylesheet' href='\..\outside.css'>",
        r"<script type='module' src='\..\outside.css'></script>",
        "<style>@import '/../outside.css';</style>",
    ] {
        fs::write(app.join("index.html"), html).unwrap();
        let error = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap_err();
        assert!(
            format!("{error:#}").contains("outside the app folder"),
            "{html}: {error:#}"
        );
    }
}

#[test]
fn pages_keep_a_leading_newline_in_pre_and_textarea() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<html><body><pre id=a>\n\ncode</pre><textarea id=b>\n\ntext</textarea><pre id=c>\nplain</pre></body></html>",
    )
    .unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let page = Document::parse(&files["index.html"]);
    let text = |id: &str| {
        let node = (0..page.nodes.len())
            .find(|&node| page.nodes[node].attrs.get("id").map(String::as_str) == Some(id))
            .unwrap();
        page.text(node)
    };
    assert_eq!(text("a"), "\ncode");
    assert_eq!(text("b"), "\ntext");
    assert_eq!(text("c"), "plain");
}

#[test]
fn page_scripts_share_one_module_graph_per_realm() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("shared.ts"),
        "globalThis.loads = (globalThis.loads ?? 0) + 1; export const order: string[] = [];",
    )
    .unwrap();
    fs::write(dir.path().join("first.ts"), "import {order} from './shared.ts'; order.push('first'); await new Promise(resolve => setTimeout(resolve, 5)); order.push('late');").unwrap();
    fs::write(dir.path().join("index.html"), r#"<html><head><script type="preload">import {order} from './shared.ts'; order.push('preload');</script><script type="preload">import {order} from './shared.ts'; if (globalThis.loads !== 1 || order.join() !== 'preload') throw Error('preload graph ' + order);</script></head><body><script type="module" src="./first.ts"></script><script type="module">import {order} from './shared.ts'; if ([typeof compile, typeof define, typeof load, typeof setup, typeof modules, typeof chunk, typeof __appsChunk, typeof __appsCompile].some(type => type !== 'undefined')) throw Error('loader leak'); if (globalThis.loads !== 1 || order.join() !== 'first') throw Error('page graph ' + globalThis.loads + order); setTimeout(() => { if (order.join() !== 'first,late') throw Error('late ' + order); document.body.dataset.done = 'yes'; }, 20);</script></body></html>"#).unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./index.html')));").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert_eq!(
            files
                .keys()
                .filter(|name| name.starts_with("script-"))
                .count(),
            2
        );
        let page = Document::parse(&files["index.html"]);
        let sources: Vec<_> = (0..page.nodes.len())
            .filter(|&id| page.nodes[id].tag == "script")
            .map(|id| {
                (
                    page.nodes[id].attrs.get("src").cloned(),
                    page.text(id).is_empty(),
                )
            })
            .collect();
        assert_eq!(
            sources,
            [
                (Some("apps:///script-1.js".into()), true),
                (None, true),
                (Some("apps:///script-0.js".into()), true),
                (Some("./first.ts".into()), true),
                (None, true)
            ]
        );
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0,
            "production={production}"
        );
    }
}

#[test]
fn css_imports_keep_layer_supports_and_media_conditions() {
    let dir = tempfile::tempdir().unwrap();
    for (name, rule) in [
        ("a.css", ".a{}"),
        ("b.css", ".b{}"),
        ("c.css", ".c{}"),
        ("d.css", ".d{}"),
        ("e.css", ".e{}"),
    ] {
        fs::write(dir.path().join(name), rule).unwrap();
    }
    fs::write(
        dir.path().join("style.css"),
        "@import \"./a.css\" layer(base);@import url( ./b.css ) layer;@import './c.css' supports(display: grid) screen and (min-width: 1px);@import url('./d.css') print;@import './e.css' layer(x.y) supports(not (display: nonsense));",
    )
    .unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<link rel='stylesheet' href='./style.css'>",
    )
    .unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(&entry, "console.log(import.meta.resolve('./index.html'));").unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    let page = &files["index.html"];
    for expected in [
        "@layer base{.a{}}",
        "@layer{.b{}}",
        "@media screen and (min-width: 1px){@supports (display: grid){.c{}}}",
        "@media print{.d{}}",
        "@supports (not (display: nonsense)){@layer x.y{.e{}}}",
    ] {
        assert!(page.contains(expected), "{expected} in {page}");
    }
}

#[test]
fn classic_scripts_share_the_global_scope_and_run_before_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    for folder in ["web/vendor", "src"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    for (name, source) in [
        (
            "web/vendor/lib.umd.js",
            "(function (root, factory) { if (typeof module === 'object' && module.exports) module.exports = factory(); else root.Lib = factory(); })(this, function () { return { answer: 42 }; });",
        ),
        (
            "web/a.js",
            "function helper() { return 'helper'; } var shared = 1; implicit = 2; order.scripts.push(document.currentScript.getAttribute('src')); Promise.resolve().then(() => order.scripts.push('then ' + document.currentScript?.getAttribute('src')));",
        ),
        (
            "web/b.js",
            "if (helper() !== 'helper' || shared !== 1 || implicit !== 2 || this !== globalThis || Lib.answer !== 42 || (function () { return this; })() !== globalThis) throw Error('classic'); try { eval('1'); throw Error('eval'); } catch (error) { if (!(error instanceof EvalError)) throw error; } order.push('classic');",
        ),
        (
            "src/main.ts",
            "const order: string[] = (globalThis as any).order; if (order.join() !== 'inline,classic') throw Error('module ' + order); order.push('module');",
        ),
        (
            "web/deferred.js",
            "order.push('deferred'); order.scripts.push(document.currentScript.getAttribute('src'));",
        ),
        ("web/style.css", "@import '/web/base.css';"),
        ("web/base.css", "#color { color: rgb(1, 2, 3) }"),
        ("web/alt.css", "#color { color: rgb(9, 9, 9) }"),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::write(dir.path().join("outside.js"), "").unwrap();
    fs::write(root.join("web/index.html"), r#"<link rel="Preload  Stylesheet" href="/web/style.css"><link rel="alternate stylesheet" href="/web/alt.css"><p id="color">x</p><script type="module" src="/src/main.ts"></script><script>globalThis.order = ['inline']; order.scripts = [document.currentScript.getAttribute('src') ?? ''];</script><script src="vendor/lib.umd.js"></script><script src="a.js"></script><script src="b.js"></script><script nomodule>throw Error('nomodule');</script><script defer src="deferred.js"></script><script type="module">const link = document.querySelector('link[href="/web/style.css"]'); if (order.join() !== 'inline,classic,module,deferred' || getComputedStyle(document.getElementById('color')).color !== 'rgb(1, 2, 3)' || !link.sheet.cssRules.length || document.styleSheets.length !== 1 || document.styleSheets[0] !== link.sheet || link.hasAttribute('apps-sheet') || document.currentScript !== null || order.scripts.join() !== ',a.js,then a.js,deferred.js') throw Error('order ' + order + ' ' + order.scripts);</script>"#).unwrap();
    let entry = root.join("main.ts");
    fs::write(&entry, "import {app, BrowserWindow} from '@pierre/apps'; app.whenReady().then(() => new BrowserWindow().loadFile(import.meta.resolve('./web/index.html')));").unwrap();
    for production in [false, true] {
        let files = compile(&entry, Realm::Main, None, Rc::default(), production, None).unwrap();
        assert!(!files.values().any(|file| file.contains("nomodule')")));
        assert_eq!(
            apps_runtime::desktop::Desktop::new(&files)
                .unwrap()
                .check()
                .unwrap(),
            0,
            "production={production}"
        );
    }
    fs::write(
        root.join("web/index.html"),
        r#"<script src="/../outside.js"></script>"#,
    )
    .unwrap();
    let error = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap_err();
    assert!(
        error.to_string().contains("outside the app folder"),
        "{error}"
    );
}

#[test]
fn css_urls_skip_comments_and_strings_and_asset_names_are_stable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("img")).unwrap();
    fs::create_dir_all(root.join("web")).unwrap();
    fs::write(root.join("img/a.png"), b"a").unwrap();
    fs::write(root.join("img/b.png"), b"b").unwrap();
    let a = asset_name(&root, &root.join("img/a.png")).unwrap();
    let b = asset_name(&root, &root.join("img/b.png")).unwrap();
    assert_eq!(a, "assets/5e9e943b2565fff5.png");
    assert_eq!(
        asset_name(Path::new("/elsewhere"), Path::new("/elsewhere/img/a.png")).unwrap(),
        a
    );
    assert_eq!(
        app_relative(
            Path::new("/repo/app"),
            Path::new("/repo/packages/a/index.js")
        ),
        Path::new("../packages/a/index.js")
    );
    let css = urls(
        "/* url( */ .a{background:URL( '../img/a.png' )} .b::after{content:'url('} .c{background:url(../img/b.png)} .d{background:image-set(url(../img/a.png) 1x, '../img/b.png' type('image/png') 2x);content:'../img/a.png'}",
        &root.join("web"),
        &Shared {
            root: root.clone(),
            ..Shared::default()
        },
    );
    assert_eq!(
        css,
        format!(
            "/* url( */ .a{{background:URL(\"apps:///{a}\")}} .b::after{{content:'url('}} .c{{background:url(\"apps:///{b}\")}} .d{{background:image-set(url(\"apps:///{a}\") 1x, url(\"apps:///{b}\") type('image/png') 2x);content:'../img/a.png'}}"
        )
    );
    fs::write(root.join("web/index.html"), r#"<img srcset="../img/a.png 1x,../img/b.png, data:image/png;base64,AA== 3x"><picture><source srcset=" ../img/b.png 2x"></picture>"#).unwrap();
    let entry = root.join("app.js");
    fs::write(
        &entry,
        "console.log(import.meta.resolve('./web/index.html'));",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None).unwrap();
    let page = &files["web/index.html"];
    assert!(
        page.contains(&format!(
            "srcset=\"apps:///{a} 1x,apps:///{b}, data:image/png;base64,AA== 3x\""
        )) && page.contains(&format!("srcset=\" apps:///{b} 2x\"")),
        "{page}"
    );
}

#[test]
fn rust_main_bundles_pages_from_the_package_root() -> Result<()> {
    let project = tempfile::tempdir()?;
    let root = project.path();
    fs::create_dir(root.join("src"))?;
    fs::create_dir(root.join("ui"))?;
    let entry = root.join("src/main.rs");
    fs::write(&entry, "fn main() {}")?;
    fs::write(
        root.join("package.json"),
        r#"{"type":"rust","name":"my-app","pages":["ui/main.html"],"preload":"src/preload.ts"}"#,
    )?;
    fs::write(
        root.join("src/preload.ts"),
        "export const answer = () => 42",
    )?;
    fs::write(
        root.join("ui/main.html"),
        "<h1></h1><script type='module' src='page.ts'></script>",
    )?;
    fs::write(
        root.join("ui/page.ts"),
        "import {answer} from '../src/preload'; document.querySelector('h1').textContent = String(answer());",
    )?;
    for production in [false, true] {
        let tracked = Rc::default();
        let files = compile(
            &entry,
            Realm::Main,
            None,
            Rc::clone(&tracked),
            production,
            None,
        )?;
        assert!(!files.contains_key("app.js"));
        assert!(files["ui/main.html"].contains("apps:///script-"));
        assert!(files["package.json"].contains("my-app"));
        assert!(
            tracked
                .borrow()
                .contains(&root.join("ui/page.ts").canonicalize()?)
        );
    }
    fs::write(
        root.join("package.json"),
        r#"{"type":"rust","name":"my-app","main":"src/main.rs"}"#,
    )?;
    let files = compile(&entry, Realm::Main, None, Rc::default(), true, None)?;
    assert_eq!(files.keys().collect::<Vec<_>>(), ["package.json"]);
    Ok(())
}

#[test]
fn fallback_page_names_do_not_collide() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a b.html"), "<p>spaced-page</p>").unwrap();
    fs::write(dir.path().join("page-0.html"), "<p>named-page</p>").unwrap();
    let entry = dir.path().join("app.ts");
    fs::write(
        &entry,
        "globalThis.pages = [import.meta.resolve('./a b.html'), import.meta.resolve('./page-0.html')];",
    )
    .unwrap();
    let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
    for marker in ["spaced-page", "named-page"] {
        assert!(
            files
                .iter()
                .any(|(name, source)| name.ends_with(".html") && source.contains(marker)),
            "{marker}"
        );
    }
}

#[test]
fn package_marks_system_web_views_of_app_code() {
    for (page, webview) in [
        ("document.createElement('iframe')", true),
        ("import 'lib'", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("node_modules/lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("package.json"), "{}").unwrap();
        fs::write(lib.join("index.js"), "globalThis.tags = ['iframe'];").unwrap();
        fs::write(dir.path().join("page.ts"), page).unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<script type=module src=page.ts></script>",
        )
        .unwrap();
        let entry = dir.path().join("app.ts");
        fs::write(&entry, "import.meta.resolve('./index.html')").unwrap();
        let files = compile(&entry, Realm::Main, None, Rc::default(), false, None).unwrap();
        let package = files["package.json"].as_str();
        assert_eq!(package.contains(r#""webview":true"#), webview, "{page}");
    }
}

#[test]
fn referenced_legacy_variant_definitions_apply() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
    write(
        "app.css",
        "@import 'tailwindcss'; @variant pin { &.pin { @slot; } } p { padding: 9px }",
    );
    write(
        "style.css",
        "@import 'tailwindcss'; @import './app.css' reference;",
    );
    let path = dir.path().join("index.html");
    write(
        "index.html",
        "<link rel='stylesheet' href='./style.css'><p class='pin pin:hidden'></p>",
    );
    let source = Rc::<Shared>::default().html(&path).unwrap();
    assert!(!linked_css(&source).contains("9px"));
    let style = apps_runtime::style::styles(&apps_runtime::bundle::page(&source), 800., 600.)
        .pop()
        .unwrap();
    assert_eq!(style["display"], "none");
}
