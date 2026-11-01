use crate::{bundler, target::Target};
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

pub fn run(entry: &Path, out: &Path, runtime: &Path, target: Target) -> Result<()> {
    let started = Instant::now();
    target.validate(runtime).context(
        "Invalid runtime. Build the workspace or set APPS_RUNTIME to a matching executable",
    )?;
    let files = bundler::compile(entry, bundler::Realm::Main, None, Rc::default(), true, None)?;
    if let Some(source) = files.get("native.json") {
        let addons: std::collections::BTreeMap<String, PathBuf> = serde_json::from_str(source)?;
        for path in addons.values() {
            target
                .validate(path)
                .context("Native addon does not match the build target")?;
        }
    }
    let app = (
        "app.js".to_owned(),
        apps_runtime::bundle_source(&files, false),
    );
    for (name, source) in files
        .iter()
        .filter(|(name, _)| name.starts_with("script-"))
        .chain([(&app.0, &app.1)])
    {
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::cjs()).parse();
        ensure!(
            parsed.diagnostics.is_empty(),
            "Invalid app bundle {name}: {:?}",
            parsed.diagnostics
        );
    }
    let public = entry
        .parent()
        .context("Missing project directory")?
        .join("public");
    let (bytes, _) = write_bundle(out, &files, runtime, &public, true, target)?;
    println!(
        "apps: built {} bytes in {} ms → {}",
        bytes,
        started.elapsed().as_millis(),
        out.display()
    );
    Ok(())
}

pub fn write_bundle(
    out: &Path,
    files: &BBTreeMap<String, String>,
    runtime: &Path,
    public: &Path,
    production: bool,
    target: Target,
) -> Result<(usize, PathBuf)> {
    let package = files.get("package.json").map(String::as_str);
    let (name, product_name, version) = apps_runtime::bundle::package_metadata(package)?;
    let library_name = apps_runtime::jsc::library_name(target.platform)?;
    let disabled = apps_runtime::bundle::disabled_features(package)?;
    let enabled = |feature| disabled & apps_runtime::bundle::feature_bit(feature) == 0;
    let webgpu = apps_runtime::gpu::library_name(target.platform).filter(|_| enabled("webgpu"));
    // WARNING: FFmpeg is LGPL, so it ships as its own libraries with its license, which users can replace.
    let media = if enabled("video") || enabled("audio") {
        let folder = runtime.canonicalize()?;
        let folder = folder.parent().context("Missing runtime directory")?;
        let library = apps_runtime::media::library_name(target.platform);
        // WARNING: older builds can leave other FFmpeg versions next to the runtime, so each file must be the name
        // that the media library links, which it keeps as a NUL-terminated string.
        let linked = fs::read(folder.join(library)).unwrap_or_default();
        let mut names = vec![library.to_owned(), "FFmpeg.LICENSE".to_owned()];
        for stem in apps_runtime::media::FFMPEG {
            names.push(
                fs::read_dir(folder)?
                    .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                    .find(|name| {
                        apps_runtime::media::is_ffmpeg(target.platform, stem, name) && {
                            let needle = [name.as_bytes(), b"\0"].concat();
                            linked.windows(needle.len()).any(|window| window == needle)
                        }
                    })
                    .context("The video and audio features need the media library and its FFmpeg next to the runtime. Run bash scripts/build-ffmpeg.sh, then cargo build --workspace --release.")?,
            );
        }
        names
    } else {
        Vec::new()
    };
    if target.platform == "windows" {
        for value in [&name, &product_name] {
            let stem = value.split('.').next().unwrap().to_ascii_uppercase();
            ensure!(
                !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    && !matches!(
                        stem.strip_prefix("COM")
                            .or_else(|| stem.strip_prefix("LPT")),
                        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
                    ),
                "Invalid Windows filename: {value}"
            );
        }
    }
    let executable_name = if target.platform == "windows" {
        format!("{name}.exe")
    } else {
        name.clone()
    };
    ensure!(
        !files.contains_key(&executable_name)
            && !matches!(
                executable_name.to_ascii_lowercase().as_str(),
                "app.h2o" | ".apps-build" | "node_modules"
            ),
        "Executable name conflicts with a bundle file: {executable_name}"
    );
    ensure!(
        !out.exists() || out.join(".apps-build").is_file() || fs::read_dir(out)?.next().is_none(),
        "Output directory is not an apps build: {}",
        out.display()
    );
    fs::create_dir_all(out.parent().context("Missing output parent")?)?;
    let public = if public.try_exists()? {
        let public = public.canonicalize()?;
        let output = out
            .parent()
            .unwrap()
            .canonicalize()?
            .join(out.file_name().context("Missing output name")?);
        ensure!(
            !output.starts_with(&public),
            "Output directory must be outside public: {}",
            out.display()
        );
        Some(public)
    } else {
        None
    };
    let stage = tempfile::Builder::new()
        .prefix(".apps-build-")
        .tempdir_in(out.parent().unwrap())?;
    let app_directory = stage.path().join(format!("{product_name}.app"));
    let assets = if target.platform == "macos" {
        app_directory.join("Contents/Resources")
    } else {
        stage.path().to_owned()
    };
    fs::create_dir_all(&assets)?;
    let mut bytes = if production {
        apps_runtime::bundle::write(&assets.join("app.h2o"), files)?
    } else {
        for (name, source) in files {
            if name == "native.json" {
                fs::write(
                    assets.join(name),
                    apps_runtime::bundle::copy_native(&assets, source)?,
                )?;
            } else if name == "assets.json" {
                fs::write(
                    assets.join(name),
                    apps_runtime::bundle::copy_assets(&assets, source)?,
                )?;
            } else {
                fs::write(assets.join(name), source)?;
            }
        }
        files.values().map(String::len).sum()
    };
    fs::write(stage.path().join(".apps-build"), b"1")?;
    let executable = if target.platform == "macos" {
        app_directory.join("Contents/MacOS").join(&executable_name)
    } else {
        stage.path().join(&executable_name)
    };
    fs::create_dir_all(executable.parent().unwrap())?;
    if target.platform == "macos" {
        let identifier = name.replace('_', "-");
        let name = apps_runtime::dom::escape(&name);
        let product_name = apps_runtime::dom::escape(&product_name);
        let build = version.split('-').next().unwrap();
        let version = apps_runtime::dom::escape(&version);
        fs::write(
            app_directory.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>{name}</string><key>CFBundleIdentifier</key><string>dev.pierre.apps.{identifier}</string><key>CFBundleName</key><string>{product_name}</string><key>CFBundleDisplayName</key><string>{product_name}</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>{build}</string><key>CFBundleShortVersionString</key><string>{version}</string><key>NSHighResolutionCapable</key><true/></dict></plist>"#
            ),
        )?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            executable.strip_prefix(stage.path())?,
            stage.path().join(&executable_name),
        )?;
    }
    fs::copy(runtime,&executable).context("Cannot copy the runtime. Build the workspace first with cargo build --workspace --release.")?;
    #[cfg(unix)]
    if target.platform != "windows" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }
    let jsc = apps_runtime::jsc::library_path(runtime, target.platform)?;
    target
        .validate(&jsc)
        .context("JavaScriptCore does not match the build target")?;
    let libraries = if target.platform == "macos" {
        app_directory.join("Contents/Frameworks")
    } else {
        stage.path().to_owned()
    };
    fs::create_dir_all(&libraries)?;
    let destination = libraries.join(library_name);
    ensure!(
        !destination.exists(),
        "JavaScriptCore conflicts with a build file"
    );
    bytes += fs::copy(&jsc, destination)? as usize;
    let license = jsc.with_file_name("JavaScriptCore.LICENSE");
    if license.is_file() {
        let destination = libraries.join("JavaScriptCore.LICENSE");
        ensure!(
            !destination.exists(),
            "JavaScriptCore license conflicts with a build file"
        );
        bytes += fs::copy(license, destination)? as usize;
    }
    if let Some(name) = webgpu {
        let source = runtime.canonicalize()?.with_file_name(name);
        target.validate(&source).context(
            "The webgpu feature needs the WebGPU library next to the runtime. Build the workspace first with cargo build --workspace --release.",
        )?;
        let destination = libraries.join(name);
        ensure!(!destination.exists(), "WebGPU conflicts with a build file");
        bytes += fs::copy(source, destination)? as usize;
    }
    let folder = runtime.canonicalize()?;
    for name in &media {
        let source = folder.with_file_name(name);
        if name != "FFmpeg.LICENSE" {
            target.validate(&source).context(
                "The video and audio features need the media library next to the runtime. Build the workspace first with cargo build --workspace --release.",
            )?;
        }
        let destination = libraries.join(name);
        ensure!(!destination.exists(), "{name} conflicts with a build file");
        bytes += fs::copy(source, destination)? as usize;
    }
    if let Some(public) = public {
        let mut directories = vec![public.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let source = entry.path();
                let relative = source.strip_prefix(&public)?;
                let destination = assets.join(relative);
                let kind = entry.file_type()?;
                ensure!(
                    kind.is_file() || kind.is_dir(),
                    "Public assets must be regular files or directories: {}",
                    source.display()
                );
                ensure!(
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| !name.contains('\\')),
                    "Public asset names must be UTF-8 without backslashes: {}",
                    source.display()
                );
                ensure!(
                    !files.contains_key(relative.to_string_lossy().as_ref())
                        && !(target.platform == "windows"
                            && relative.to_str().is_some_and(|name| {
                                name.eq_ignore_ascii_case(library_name)
                                    || name.eq_ignore_ascii_case("JavaScriptCore.LICENSE")
                                    || webgpu
                                        .is_some_and(|webgpu| name.eq_ignore_ascii_case(webgpu))
                                    || media.iter().any(|media| name.eq_ignore_ascii_case(media))
                            }))
                        && relative != Path::new(&executable_name)
                        && !matches!(
                            relative.to_str(),
                            Some(
                                "app.h2o"
                                    | "native.json"
                                    | "assets.json"
                                    | "package.json"
                                    | ".apps-build"
                            )
                        )
                        && (!destination.try_exists()? || (kind.is_dir() && destination.is_dir())),
                    "Public asset conflicts with a build file: {}",
                    relative.display()
                );
                if kind.is_dir() {
                    fs::create_dir_all(&destination)?;
                    directories.push(source);
                } else {
                    bytes += fs::copy(&source, &destination)
                        .with_context(|| format!("Cannot copy public asset {}", source.display()))?
                        as usize;
                }
            }
        }
    }
    if production {
        let archive = stage.path().join(format!("{product_name}.zip"));
        ensure!(!archive.exists(), "ZIP name conflicts with a build file");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive)?);
        let mut directories = vec![if target.platform == "macos" {
            app_directory
        } else {
            stage.path().to_owned()
        }];
        while let Some(directory) = directories.pop() {
            let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                if path == archive || path == stage.path().join(".apps-build") {
                    continue;
                }
                let relative = path
                    .strip_prefix(stage.path())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let metadata = entry.metadata()?;
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .large_file(metadata.len() >= u32::MAX as u64);
                #[cfg(unix)]
                let options = {
                    use std::os::unix::fs::PermissionsExt;
                    options.unix_permissions(metadata.permissions().mode())
                };
                let options =
                    if metadata.is_dir() || (target.platform != "windows" && path == executable) {
                        options.unix_permissions(0o755)
                    } else {
                        options
                    };
                if metadata.is_dir() {
                    zip.add_directory(format!("{relative}/"), options)?;
                    directories.push(path);
                } else {
                    zip.start_file(relative, options)?;
                    std::io::copy(&mut fs::File::open(path)?, &mut zip)?;
                }
            }
        }
        zip.finish()?;
    }
    let backup = tempfile::Builder::new()
        .prefix(".apps-build-")
        .tempdir_in(out.parent().unwrap())?;
    let previous = backup.path().join("previous");
    if out.exists() {
        fs::rename(out, &previous)?;
    }
    if let Err(error) = fs::rename(stage.path(), out) {
        if previous.exists() {
            fs::rename(previous, out)?;
        }
        return Err(error.into());
    }
    #[cfg(windows)]
    if target.platform == "macos" {
        return Ok((bytes, out.join(executable.strip_prefix(stage.path())?)));
    }
    Ok((bytes, out.join(executable_name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_metadata_names_executables_and_mac_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = dir.path().join("dist");
        for (package, name, product_name, version) in [
            (serde_json::json!({}), "App", "App", "0.0.0"),
            (
                serde_json::json!({"name":"package_name","version":"1.2.3"}),
                "package_name",
                "package_name",
                "1.2.3",
            ),
            (
                serde_json::json!({"name":"tea_code","productName":"Tea & Code","version":"2.3.4-beta_1"}),
                "tea_code",
                "Tea & Code",
                "2.3.4-beta_1",
            ),
        ] {
            let files = BBTreeMap::from([
                ("app.js".into(), String::new()),
                ("package.json".into(), package.to_string()),
            ]);
            for production in [false, true] {
                let (_, executable) = write_bundle(
                    &out,
                    &files,
                    &runtime,
                    &dir.path().join("public"),
                    production,
                    Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                )
                .unwrap();
                let filename = if cfg!(windows) {
                    format!("{name}.exe")
                } else {
                    name.into()
                };
                assert_eq!(executable, out.join(&filename));
                assert_eq!(fs::read(executable).unwrap(), b"runtime");
                let archive = out.join(format!("{product_name}.zip"));
                assert_eq!(archive.is_file(), production);
                if production {
                    let mut zip = zip::ZipArchive::new(fs::File::open(archive).unwrap()).unwrap();
                    let binary = if cfg!(target_os = "macos") {
                        format!("{product_name}.app/Contents/MacOS/{filename}")
                    } else {
                        filename.clone()
                    };
                    assert!(zip.file_names().all(|name| {
                        let name = name.unwrap();
                        name != ".apps-build" && !name.ends_with(".zip")
                    }));
                    let mut binary = zip.by_name(&binary).unwrap();
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut binary, &mut bytes).unwrap();
                    assert_eq!(bytes, b"runtime");
                    #[cfg(unix)]
                    assert_eq!(binary.unix_mode().unwrap() & 0o777, 0o755);
                }
                let bundle = apps_runtime::bundle::read(&out).unwrap();
                assert_eq!(
                    apps_runtime::bundle::package_metadata(Some(&bundle.files["package.json"]))
                        .unwrap(),
                    (name.into(), product_name.into(), version.into())
                );
                #[cfg(target_os = "macos")]
                {
                    let app = out.join(format!("{product_name}.app"));
                    let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
                    let escaped = apps_runtime::dom::escape(name);
                    assert!(plist.contains(&format!(
                        "<key>CFBundleExecutable</key><string>{escaped}</string>"
                    )));
                    assert!(plist.contains(&format!(
                        "<key>CFBundleIdentifier</key><string>dev.pierre.apps.{}</string>",
                        name.replace('_', "-")
                    )));
                    let escaped = apps_runtime::dom::escape(product_name);
                    for key in ["CFBundleName", "CFBundleDisplayName"] {
                        assert!(
                            plist.contains(&format!("<key>{key}</key><string>{escaped}</string>"))
                        );
                    }
                    assert!(plist.contains(&format!(
                        "<key>CFBundleShortVersionString</key><string>{version}</string>"
                    )));
                    assert!(plist.contains(&format!(
                        "<key>CFBundleVersion</key><string>{}</string>",
                        version.split('-').next().unwrap()
                    )));
                    assert!(apps_runtime::bundle::read(&app).is_ok());
                    let resources = app.join("Contents/Resources");
                    assert_eq!(resources.join("app.h2o").is_file(), production);
                    assert_eq!(resources.join("package.json").is_file(), !production);
                }
            }
        }
    }

    #[test]
    fn app_name_falls_back_to_index_title() {
        for (package, html, name, product_name) in [
            (None, None, "App", "App"),
            (Some("{}"), None, "App", "App"),
            (None, Some("<h1>Hello</h1>"), "App", "App"),
            (Some("{}"), Some("<title> \n </title>"), "App", "App"),
            (
                None,
                Some("<title>Tea_Code</title>"),
                "Tea_Code",
                "Tea_Code",
            ),
            (
                Some("{}"),
                Some("<title>Tea_Code</title>"),
                "Tea_Code",
                "Tea_Code",
            ),
            (
                Some(r#"{"version":"1.2.3"}"#),
                Some("<!-- <title>Wrong</title> --><TITLE> \n Tea&#95;Code \t </TITLE>"),
                "Tea_Code",
                "Tea_Code",
            ),
            (
                Some(r#"{"name":"Package_Name"}"#),
                Some("<title>Tea_Code</title>"),
                "Package_Name",
                "Package_Name",
            ),
            (
                Some(r#"{"productName":"Product_Name","name":"Package_Name"}"#),
                Some("<title>Tea_Code</title>"),
                "Package_Name",
                "Product_Name",
            ),
            (
                Some(r#"{"productName":"Tea Product"}"#),
                Some("<title>Tea_Code</title>"),
                "Tea_Code",
                "Tea Product",
            ),
            (
                Some(r#"{"productName":"Tea Product"}"#),
                None,
                "App",
                "Tea Product",
            ),
            (
                None,
                Some("<title>Three.js · WebGPU</title>"),
                "App",
                "Three.js · WebGPU",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let entry = dir.path().join("app.js");
            fs::write(&entry, format!("import {{app}} from '@pierre/apps'; if(app.getName() !== '{product_name}') throw Error('App name');")).unwrap();
            if let Some(source) = package {
                fs::write(dir.path().join("package.json"), source).unwrap();
            }
            if let Some(source) = html {
                fs::write(dir.path().join("index.html"), source).unwrap();
            }
            let runtime = dir.path().join("runtime");
            fs::write(&runtime, b"runtime").unwrap();
            let out = dir.path().join("dist");
            for production in [false, true] {
                let files = bundler::compile(
                    &entry,
                    bundler::Realm::Main,
                    None,
                    Default::default(),
                    production,
                    None,
                )
                .unwrap();
                let (_, executable) = write_bundle(
                    &out,
                    &files,
                    &runtime,
                    &dir.path().join("public"),
                    production,
                    Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                )
                .unwrap();
                assert_eq!(executable.file_stem().unwrap(), name);
                let bundle = apps_runtime::bundle::read(&out).unwrap();
                assert_eq!(
                    apps_runtime::desktop::Desktop::from_bundle(bundle)
                        .unwrap()
                        .check()
                        .unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn media_libraries_ship_only_with_the_features() {
        let target = Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let libraries = if cfg!(target_os = "macos") {
            out.join("App.app/Contents/Frameworks")
        } else {
            out.clone()
        };
        let build = |features: &str| {
            let files = BBTreeMap::from([
                ("app.js".into(), String::new()),
                (
                    "package.json".into(),
                    format!(r#"{{"features":{features}}}"#),
                ),
            ]);
            write_bundle(&out, &files, &runtime, &dir.path().join("public"), true, target)
        };
        let error = build(r#"["video"]"#).unwrap_err();
        assert!(error.to_string().contains("FFmpeg"), "{error}");
        let mut names = vec![
            apps_runtime::media::library_name(target.platform).to_owned(),
            "FFmpeg.LICENSE".to_owned(),
        ];
        for entry in fs::read_dir(std::env::current_exe().unwrap().parent().unwrap()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if apps_runtime::media::FFMPEG
                .iter()
                .any(|stem| apps_runtime::media::is_ffmpeg(target.platform, stem, &name))
            {
                names.push(name);
            }
        }
        assert_eq!(names.len(), 7, "{names:?}");
        for name in &names {
            fs::copy(
                std::env::current_exe().unwrap().with_file_name(name),
                dir.path().join(name),
            )
            .unwrap();
        }
        let stale = match target.platform {
            "macos" => "libavcodec.1.dylib",
            "windows" => "avcodec-1.dll",
            _ => "libavcodec.so.1",
        };
        fs::write(dir.path().join(stale), b"stale").unwrap();
        for (features, shipped) in [
            ("[]", false),
            (r#"["audio"]"#, true),
            (r#"["video","-audio"]"#, true),
            (r#"["video","-video"]"#, false),
        ] {
            build(features).unwrap();
            for name in &names {
                assert_eq!(libraries.join(name).is_file(), shipped, "{features} {name}");
            }
            assert!(!libraries.join(stale).exists(), "{features}");
        }
    }

    #[test]
    fn webgpu_library_ships_only_with_the_feature() {
        let target = Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let Some(name) = apps_runtime::gpu::library_name(target.platform) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let libraries = if cfg!(target_os = "macos") {
            out.join("App.app/Contents/Frameworks")
        } else {
            out.clone()
        };
        let build = |features: &str| {
            let files = BBTreeMap::from([
                ("app.js".into(), String::new()),
                (
                    "package.json".into(),
                    format!(r#"{{"features":{features}}}"#),
                ),
            ]);
            write_bundle(
                &out,
                &files,
                &runtime,
                &dir.path().join("public"),
                true,
                target,
            )
        };
        let error = build(r#"["webgpu"]"#).unwrap_err();
        assert!(error.to_string().contains("WebGPU library"), "{error}");
        fs::copy(
            std::env::current_exe().unwrap().with_file_name(name),
            dir.path().join(name),
        )
        .unwrap();
        for (features, shipped) in [
            ("[]", false),
            (r#"["webgpu"]"#, true),
            (r#"["*","-webgpu","-video","-audio"]"#, false),
        ] {
            build(features).unwrap();
            assert_eq!(libraries.join(name).is_file(), shipped, "{features}");
        }
    }

    #[test]
    fn builds_replace_text_files_with_one_binary_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let files = BBTreeMap::from([
            ("app.js".into(), String::new()),
            (
                "page-0.html".into(),
                "<style>body {color:red}</style><body>Hello</body>".into(),
            ),
        ]);
        let assets = if cfg!(target_os = "macos") {
            out.join("App.app/Contents/Resources")
        } else {
            out.clone()
        };
        let public = dir.path().join("public");
        write_bundle(
            &out,
            &files,
            &runtime,
            &public,
            false,
            Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
        )
        .unwrap();
        assert!(assets.join("app.js").is_file());
        assert!(assets.join("page-0.html").is_file());
        assert!(!assets.join("apps.json").exists());
        assert!(apps_runtime::bundle::read(&out).unwrap().pages.is_empty());
        fs::write(assets.join("apps.json"), b"old manifest").unwrap();
        write_bundle(
            &out,
            &files,
            &runtime,
            &public,
            true,
            Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
        )
        .unwrap();
        assert!(assets.join("app.h2o").is_file());
        assert!(!assets.join("app.js").exists());
        assert!(!assets.join("page-0.html").exists());
        assert!(!assets.join("apps.json").exists());
        assert_eq!(apps_runtime::bundle::read(&out).unwrap().pages.len(), 1);
        write_bundle(
            &out,
            &files,
            &runtime,
            &public,
            false,
            Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
        )
        .unwrap();
        assert!(!assets.join("app.h2o").exists());
        assert!(assets.join("app.js").is_file());
    }

    #[test]
    fn public_assets_copy_unchanged_and_refresh_on_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        fs::create_dir_all(public.join("nested/empty")).unwrap();
        fs::write(public.join("nested/data.bin"), [0, 255, 128]).unwrap();
        fs::write(
            public.join("static.js"),
            "throw new Error('static file executed')",
        )
        .unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let files = BBTreeMap::from([("app.js".into(), String::new())]);
        let assets = if cfg!(target_os = "macos") {
            out.join("App.app/Contents/Resources")
        } else {
            out.clone()
        };
        for production in [false, true] {
            fs::write(public.join("message.txt"), "first").unwrap();
            write_bundle(
                &out,
                &files,
                &runtime,
                &public,
                production,
                Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            )
            .unwrap();
            assert_eq!(
                fs::read(assets.join("nested/data.bin")).unwrap(),
                [0, 255, 128]
            );
            assert!(assets.join("nested/empty").is_dir());
            assert_eq!(fs::read(assets.join("message.txt")).unwrap(), b"first");
            assert_eq!(
                fs::read(assets.join("static.js")).unwrap(),
                fs::read(public.join("static.js")).unwrap()
            );
            assert!(
                !apps_runtime::bundle::read(&out)
                    .unwrap()
                    .files
                    .contains_key("static.js")
            );

            fs::remove_file(public.join("message.txt")).unwrap();
            fs::write(public.join("new.txt"), "second").unwrap();
            write_bundle(
                &out,
                &files,
                &runtime,
                &public,
                production,
                Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            )
            .unwrap();
            assert!(!assets.join("message.txt").exists());
            assert_eq!(fs::read(assets.join("new.txt")).unwrap(), b"second");
        }
    }

    #[test]
    fn public_assets_cannot_replace_build_files_or_contain_output() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        fs::create_dir(&public).unwrap();
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let files = BBTreeMap::from([("app.js".into(), String::new())]);
        for production in [false, true] {
            let (_, executable) = write_bundle(
                &out,
                &files,
                &runtime,
                &public,
                production,
                Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            )
            .unwrap();
            for name in [
                "app.js",
                "app.h2o",
                "native.json",
                "package.json",
                ".apps-build",
                if cfg!(windows) { "App.exe" } else { "App" },
            ] {
                let source = public.join(name);
                fs::write(&source, "conflict").unwrap();
                let error = write_bundle(
                    &out,
                    &files,
                    &runtime,
                    &public,
                    production,
                    Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                )
                .unwrap_err();
                assert!(
                    error.to_string().contains("conflicts with a build file"),
                    "{error}"
                );
                assert_eq!(fs::read(&executable).unwrap(), b"runtime");
                assert!(apps_runtime::bundle::read(&out).is_ok());
                fs::remove_file(source).unwrap();
            }
            #[cfg(unix)]
            for name in [
                std::ffi::OsStr::new("..\\..\\escape.txt"),
                std::os::unix::ffi::OsStrExt::from_bytes(b"invalid-\xff.txt"),
            ] {
                let source = public.join(name);
                if fs::write(&source, "escape").is_err() {
                    continue;
                }
                let error = write_bundle(
                    &out,
                    &files,
                    &runtime,
                    &public,
                    production,
                    Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                )
                .unwrap_err();
                assert!(error.to_string().contains("without backslashes"), "{error}");
                fs::remove_file(source).unwrap();
            }
        }
        let error = write_bundle(
            &public.join("dist"),
            &files,
            &runtime,
            &public,
            true,
            Target::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside public"), "{error}");
        assert!(!public.join("dist").exists());
    }
}
