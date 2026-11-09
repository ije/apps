use crate::build_target::BuildTarget;
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn package_app(
    out: &Path,
    files: &BBTreeMap<String, String>,
    runtime: &Path,
    root: &Path,
    production: bool,
    target: BuildTarget,
    app_binary: Option<&Path>,
    bytecode: bool,
) -> Result<(usize, PathBuf)> {
    let package = files.get("package.json").map(String::as_str);
    let (name, product_name, version, identifier) =
        apps_runtime::bundle::package_metadata(package)?;
    let library_name = apps_runtime::jsc::library_name(target.platform)?;
    let disabled = apps_runtime::bundle::disabled_features(package)?;
    let enabled = |feature| disabled & apps_runtime::bundle::feature_bit(feature) == 0;
    let webgpu = apps_runtime::gpu::library_name(target.platform).filter(|_| enabled("webgpu"));
    // WARNING: FFmpeg is LGPL, so it ships as its own library with its license, which users can replace.
    let media = if enabled("video") || enabled("audio") {
        let ffmpeg = apps_runtime::media::ffmpeg_name(target.platform);
        ensure!(
            runtime.canonicalize()?.with_file_name(ffmpeg).is_file(),
            "The video and audio features need the media library and its FFmpeg next to the runtime. Run bash scripts/build-ffmpeg.sh, then cargo build --workspace --release."
        );
        vec![
            apps_runtime::media::library_name(target.platform),
            ffmpeg,
            "FFmpeg.LICENSE",
        ]
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
                "app.bundle" | ".apps-build" | "node_modules"
            ),
        "Executable name conflicts with a bundle file: {executable_name}"
    );
    ensure!(
        !out.exists() || out.join(".apps-build").is_file() || fs::read_dir(out)?.next().is_none(),
        "Output directory is not an apps build: {}",
        out.display()
    );
    fs::create_dir_all(out.parent().context("Missing output parent")?)?;
    let root = root.canonicalize()?;
    let mut copies = Vec::new();
    if let Some(manifest) = root
        .ancestors()
        .map(|directory| directory.join("package.json"))
        .find(|path| path.is_file())
    {
        let package: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest)?)
            .with_context(|| format!("Invalid package metadata in {}", manifest.display()))?;
        for item in package
            .get("files")
            .map(|files| {
                files
                    .as_array()
                    .context("package.json files must be an array")
            })
            .transpose()?
            .into_iter()
            .flatten()
        {
            let item = item
                .as_str()
                .context("package.json files must be strings")?;
            let source = manifest
                .parent()
                .unwrap()
                .join(item)
                .canonicalize()
                .with_context(|| format!("Cannot find {item} of package.json files"))?;
            let relative = source
                .strip_prefix(&root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .with_context(|| {
                    format!("{item} of package.json files must be inside the app folder")
                })?
                .to_owned();
            copies.push((item.to_owned(), source, relative));
        }
    }
    let output = out
        .parent()
        .unwrap()
        .canonicalize()?
        .join(out.file_name().context("Missing output name")?);
    for (name, source, _) in &copies {
        ensure!(
            !output.starts_with(source),
            "Output directory must be outside {name}: {}",
            out.display()
        );
    }
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
    let jsc = apps_runtime::jsc::library_path(runtime, target.platform)?;
    let main = (bytecode && files.contains_key("app.js"))
        .then(|| apps_runtime::bundle::script_source(files, true));
    let bytecode = if bytecode {
        // WARNING: bytecode is only valid for the JavaScriptCore that built it.
        ensure!(
            jsc == apps_runtime::jsc::library_path(
                &std::env::current_exe()?,
                std::env::consts::OS
            )?,
            "--bytecode needs the runtime and JavaScriptCore next to apps, built for this machine"
        );
        let app: Vec<_> = files
            .iter()
            .filter(|(name, _)| name.starts_with("script-"))
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .chain(main.as_deref().map(|main| ("app.js", main)))
            .collect();
        apps_runtime::jsc::bytecode(disabled, &app)?
    } else {
        Vec::new()
    };
    let mut bytes = if production {
        apps_runtime::bundle::write(&assets.join("app.bundle"), files, &bytecode)?
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
        let name = apps_runtime::dom::escape(&name);
        let product_name = apps_runtime::dom::escape(&product_name);
        let build = version.split('-').next().unwrap();
        let version = apps_runtime::dom::escape(&version);
        fs::write(
            app_directory.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>{name}</string><key>CFBundleIdentifier</key><string>{identifier}</string><key>CFBundleName</key><string>{product_name}</string><key>CFBundleDisplayName</key><string>{product_name}</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>{build}</string><key>CFBundleShortVersionString</key><string>{version}</string><key>NSHighResolutionCapable</key><true/><key>LSEnvironment</key><dict><key>MallocSpaceEfficient</key><string>1</string></dict></dict></plist>"#
            ),
        )?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            executable.strip_prefix(stage.path())?,
            stage.path().join(&executable_name),
        )?;
    }
    fs::copy(app_binary.unwrap_or(runtime), &executable).context("Cannot copy the runtime. Build the workspace first with cargo build --workspace --release.")?;
    #[cfg(unix)]
    if target.platform != "windows" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }
    target
        .validate_binary(&jsc)
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
        target.validate_binary(&source).context(
            "The webgpu feature needs the WebGPU library next to the runtime. Build the workspace first with cargo build --workspace --release.",
        )?;
        let destination = libraries.join(name);
        ensure!(!destination.exists(), "WebGPU conflicts with a build file");
        bytes += fs::copy(source, destination)? as usize;
    }
    let folder = runtime.canonicalize()?;
    for name in &media {
        let source = folder.with_file_name(name);
        if *name != "FFmpeg.LICENSE" {
            target.validate_binary(&source).context(
                "The video and audio features need the media library next to the runtime. Build the workspace first with cargo build --workspace --release.",
            )?;
        }
        let destination = libraries.join(name);
        ensure!(!destination.exists(), "{name} conflicts with a build file");
        bytes += fs::copy(source, destination)? as usize;
    }
    for (_, source, relative) in copies {
        let mut entries = vec![(source, relative)];
        while let Some((source, relative)) = entries.pop() {
            let destination = assets.join(&relative);
            let kind = fs::symlink_metadata(&source)?.file_type();
            ensure!(
                kind.is_file() || kind.is_dir(),
                "App files must be regular files or directories: {}",
                source.display()
            );
            ensure!(
                relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.contains('\\')),
                "App file names must be UTF-8 without backslashes: {}",
                source.display()
            );
            ensure!(
                !files.contains_key(relative.to_string_lossy().as_ref())
                    && !(target.platform == "windows"
                        && relative.to_str().is_some_and(|name| {
                            name.eq_ignore_ascii_case(library_name)
                                || name.eq_ignore_ascii_case("JavaScriptCore.LICENSE")
                                || webgpu.is_some_and(|webgpu| name.eq_ignore_ascii_case(webgpu))
                                || media.iter().any(|media| name.eq_ignore_ascii_case(media))
                        }))
                    && relative != Path::new(&executable_name)
                    && !matches!(
                        relative.to_str(),
                        Some(
                            "app.bundle"
                                | "native.json"
                                | "assets.json"
                                | "package.json"
                                | ".apps-build"
                        )
                    )
                    && (!destination.try_exists()? || (kind.is_dir() && destination.is_dir())),
                "App file conflicts with a build file: {}",
                relative.display()
            );
            if kind.is_dir() {
                fs::create_dir_all(&destination)?;
                for entry in fs::read_dir(&source)? {
                    let entry = entry?;
                    entries.push((entry.path(), relative.join(entry.file_name())));
                }
            } else {
                fs::create_dir_all(destination.parent().unwrap())?;
                bytes += fs::copy(&source, &destination)
                    .with_context(|| format!("Cannot copy {}", source.display()))?
                    as usize;
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
    use crate::bundler;

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
        for (package, name, product_name, version, identifier) in [
            (serde_json::json!({}), "App", "App", "0.0.0", "com.apps.App"),
            (
                serde_json::json!({"name":"package_name","version":"1.2.3"}),
                "package_name",
                "package_name",
                "1.2.3",
                "com.apps.package-name",
            ),
            (
                serde_json::json!({"name":"tea_code","productName":"Tea & Code","version":"2.3.4-beta_1","identifier":"dev.tea.Code"}),
                "tea_code",
                "Tea & Code",
                "2.3.4-beta_1",
                "dev.tea.Code",
            ),
        ] {
            let files = BBTreeMap::from([
                ("app.js".into(), String::new()),
                ("package.json".into(), package.to_string()),
            ]);
            for production in [false, true] {
                let (_, executable) = package_app(
                    &out,
                    &files,
                    &runtime,
                    dir.path(),
                    production,
                    BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                    None,
                    false,
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
                    (
                        name.into(),
                        product_name.into(),
                        version.into(),
                        identifier.into()
                    )
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
                        "<key>CFBundleIdentifier</key><string>{identifier}</string>"
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
                    assert_eq!(resources.join("app.bundle").is_file(), production);
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
                let (_, executable) = package_app(
                    &out,
                    &files,
                    &runtime,
                    dir.path(),
                    production,
                    BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                    None,
                    false,
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
        let target = BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap();
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
            package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                true,
                target,
                None,
                false,
            )
        };
        let error = build(r#"["video"]"#).unwrap_err();
        assert!(error.to_string().contains("FFmpeg"), "{error}");
        let names = [
            apps_runtime::media::library_name(target.platform),
            apps_runtime::media::ffmpeg_name(target.platform),
            "FFmpeg.LICENSE",
        ];
        for name in &names {
            fs::copy(
                std::env::current_exe().unwrap().with_file_name(name),
                dir.path().join(name),
            )
            .unwrap();
        }
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
        }
    }

    #[test]
    fn webgpu_library_ships_only_with_the_feature() {
        let target = BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap();
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
            package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                true,
                target,
                None,
                false,
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
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            false,
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert!(assets.join("app.js").is_file());
        assert!(assets.join("page-0.html").is_file());
        assert!(!assets.join("apps.json").exists());
        assert!(apps_runtime::bundle::read(&out).unwrap().pages.is_empty());
        fs::write(assets.join("apps.json"), b"old manifest").unwrap();
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            true,
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert!(assets.join("app.bundle").is_file());
        assert!(!assets.join("app.js").exists());
        assert!(!assets.join("page-0.html").exists());
        assert!(!assets.join("apps.json").exists());
        assert_eq!(apps_runtime::bundle::read(&out).unwrap().pages.len(), 1);
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            false,
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert!(!assets.join("app.bundle").exists());
        assert!(assets.join("app.js").is_file());
    }

    #[test]
    fn package_files_copy_unchanged_and_refresh_on_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(data.join("nested/empty")).unwrap();
        fs::write(data.join("nested/data.bin"), [0, 255, 128]).unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        fs::create_dir_all(dir.path().join("public")).unwrap();
        fs::write(dir.path().join("lib/math.bin"), [1, 2]).unwrap();
        fs::write(dir.path().join("lib/skipped.bin"), [3]).unwrap();
        fs::write(dir.path().join("public/skipped.txt"), "x").unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"files":["./lib/math.bin","data"]}"#,
        )
        .unwrap();
        fs::write(
            data.join("static.js"),
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
            fs::write(data.join("message.txt"), "first").unwrap();
            package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                production,
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
            )
            .unwrap();
            assert_eq!(
                fs::read(assets.join("data/nested/data.bin")).unwrap(),
                [0, 255, 128]
            );
            assert!(assets.join("data/nested/empty").is_dir());
            assert_eq!(fs::read(assets.join("lib/math.bin")).unwrap(), [1, 2]);
            assert!(!assets.join("lib/skipped.bin").exists());
            assert!(!assets.join("public").exists());
            assert!(!assets.join("skipped.txt").exists());
            assert_eq!(fs::read(assets.join("data/message.txt")).unwrap(), b"first");
            assert_eq!(
                fs::read(assets.join("data/static.js")).unwrap(),
                fs::read(data.join("static.js")).unwrap()
            );
            assert!(
                !apps_runtime::bundle::read(&out)
                    .unwrap()
                    .files
                    .contains_key("data/static.js")
            );

            fs::remove_file(data.join("message.txt")).unwrap();
            fs::write(data.join("new.txt"), "second").unwrap();
            package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                production,
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
            )
            .unwrap();
            assert!(!assets.join("data/message.txt").exists());
            assert_eq!(fs::read(assets.join("data/new.txt")).unwrap(), b"second");
        }
    }

    #[test]
    fn package_files_cannot_replace_build_files_or_contain_output() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        let manifest = dir.path().join("package.json");
        let runtime = dir.path().join("runtime");
        fs::write(&runtime, b"runtime").unwrap();
        let out = dir.path().join("dist");
        let files = BBTreeMap::from([("app.js".into(), String::new())]);
        for production in [false, true] {
            fs::write(&manifest, r#"{"files":["data"]}"#).unwrap();
            let (_, executable) = package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                production,
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
            )
            .unwrap();
            for name in [
                "app.js",
                "app.bundle",
                "native.json",
                "package.json",
                ".apps-build",
                if cfg!(windows) { "App.exe" } else { "App" },
            ] {
                let source = dir.path().join(name);
                fs::write(&source, "conflict").unwrap();
                fs::write(&manifest, format!(r#"{{"files":["{name}"]}}"#)).unwrap();
                let error = package_app(
                    &out,
                    &files,
                    &runtime,
                    dir.path(),
                    production,
                    BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                    None,
                    false,
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
            fs::write(&manifest, r#"{"files":["data"]}"#).unwrap();
            #[cfg(unix)]
            for name in [
                std::ffi::OsStr::new("..\\..\\escape.txt"),
                std::os::unix::ffi::OsStrExt::from_bytes(b"invalid-\xff.txt"),
            ] {
                let source = data.join(name);
                if fs::write(&source, "escape").is_err() {
                    continue;
                }
                let error = package_app(
                    &out,
                    &files,
                    &runtime,
                    dir.path(),
                    production,
                    BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                    None,
                    false,
                )
                .unwrap_err();
                assert!(error.to_string().contains("without backslashes"), "{error}");
                fs::remove_file(source).unwrap();
            }
        }
        let error = package_app(
            &data.join("dist"),
            &files,
            &runtime,
            dir.path(),
            true,
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside data"), "{error}");
        assert!(!data.join("dist").exists());
    }
}
