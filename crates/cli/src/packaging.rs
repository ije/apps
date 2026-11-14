use crate::build_target::BuildTarget;
use anyhow::{Context, Result, ensure};
use bbtree::unsync::BBTreeMap;
use std::{fs, path::Path};

#[allow(clippy::too_many_arguments)]
pub fn package_app(
    out: &Path,
    files: &BBTreeMap<String, String>,
    runtime: &Path,
    root: &Path,
    target: BuildTarget,
    app_binary: Option<&Path>,
    bytecode: bool,
    aot: Option<&Path>,
) -> Result<usize> {
    let package = files.get("package.json").map(String::as_str);
    let (name, product_name, version, identifier) =
        apps_runtime::bundle::package_metadata(package)?;
    let library_name = apps_runtime::jsc::library_name(target.platform)?;
    let disabled = apps_runtime::bundle::disabled_features(package)?;
    let enabled = |feature| disabled & apps_runtime::bundle::feature_bit(feature) == 0;
    let webgpu = apps_runtime::gpu::library_name(target.platform).filter(|_| enabled("webgpu"));
    // FFmpeg is LGPL, so it ships as its own library with its license, which users can replace.
    let media = if enabled("media") {
        let ffmpeg = apps_runtime::media::ffmpeg_name(target.platform);
        ensure!(
            runtime.canonicalize()?.with_file_name(ffmpeg).is_file(),
            "The media feature needs the media library and its FFmpeg next to the runtime. Run bash scripts/build-ffmpeg.sh, then cargo build --workspace --release."
        );
        vec![
            apps_runtime::media::library_name(target.platform),
            ffmpeg,
            "FFmpeg.LICENSE",
        ]
    } else {
        Vec::new()
    };
    // libghostty-vt is in the terminal library, so its license ships with it.
    let terminal = enabled("terminal").then(|| {
        [
            apps_runtime::terminal::library_name(target.platform),
            apps_runtime::terminal::LICENSE,
        ]
    });
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
                "app.bundle" | "app.dylib" | ".apps-build" | "node_modules"
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
    let mut bytes = apps_runtime::bundle::write(&assets.join("app.bundle"), files, &bytecode)?;
    if let Some(aot) = aot {
        target
            .validate_binary(aot)
            .context("app.dylib does not match the build target")?;
        bytes += fs::copy(aot, assets.join("app.dylib"))? as usize;
    }
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
    bytes += fs::copy(app_binary.unwrap_or(runtime), &executable).context("Cannot copy the runtime. Build the workspace first with cargo build --workspace --release.")? as usize;
    #[cfg(unix)]
    if target.platform != "windows" {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
    }
    // AOT code runs only in the JSC that its compiler was built with.
    let jsc = if aot.is_some() {
        jsc.with_file_name("libJavaScriptCoreAOT.dylib")
    } else {
        jsc
    };
    target
        .validate_binary(&jsc)
        .context("JavaScriptCore does not match the build target")?;
    let libraries = if target.platform == "macos" {
        app_directory.join("Contents/Frameworks")
    } else {
        stage.path().to_owned()
    };
    fs::create_dir_all(&libraries)?;
    let folder = runtime.canonicalize()?;
    let license = jsc.with_file_name("JavaScriptCore.LICENSE");
    let mut shipped = vec![(jsc, library_name, None)];
    if license.is_file() {
        shipped.push((license, "JavaScriptCore.LICENSE", None));
    }
    if let Some(name) = webgpu {
        shipped.push((folder.with_file_name(name), name, Some("The webgpu feature needs the WebGPU library next to the runtime. Build the workspace first with cargo build --workspace --release.")));
    }
    for name in media {
        shipped.push((folder.with_file_name(name), name, (name != "FFmpeg.LICENSE").then_some("The media feature needs its library next to the runtime. Build the workspace first with cargo build --workspace --release.")));
    }
    for name in terminal.into_iter().flatten() {
        shipped.push((folder.with_file_name(name), name, (name != apps_runtime::terminal::LICENSE).then_some("The terminal feature needs the terminal library next to the runtime. Run bash scripts/build-ghostty.sh, then cargo build --workspace --release.")));
    }
    for (source, name, context) in &shipped {
        if let Some(context) = context {
            target.validate_binary(source).context(*context)?;
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
                            name.eq_ignore_ascii_case("JavaScriptCore.LICENSE")
                                || shipped
                                    .iter()
                                    .any(|(_, library, _)| name.eq_ignore_ascii_case(library))
                        }))
                    && relative != Path::new(&executable_name)
                    && !matches!(
                        relative.to_str(),
                        Some(
                            "app.bundle"
                                | "app.dylib"
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
    Ok(bytes)
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
            package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
                None,
            )
            .unwrap();
            let filename = if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            };
            assert_eq!(fs::read(out.join(&filename)).unwrap(), b"runtime");
            let archive = out.join(format!("{product_name}.zip"));
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
                    assert!(plist.contains(&format!("<key>{key}</key><string>{escaped}</string>")));
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
                assert!(resources.join("app.bundle").is_file());
                assert!(!resources.join("package.json").exists());
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
                package_app(
                    &out,
                    &files,
                    &runtime,
                    dir.path(),
                    BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                    None,
                    false,
                    None,
                )
                .unwrap();
                assert!(
                    out.join(name)
                        .with_extension(std::env::consts::EXE_EXTENSION)
                        .is_file()
                );
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
                target,
                None,
                false,
                None,
            )
        };
        let error = build(r#"["media"]"#).unwrap_err();
        assert!(error.to_string().contains("FFmpeg"), "{error}");
        let names = [
            apps_runtime::media::library_name(target.platform),
            apps_runtime::media::ffmpeg_name(target.platform),
            "FFmpeg.LICENSE",
        ];
        for name in &names {
            fs::copy(
                apps_runtime::app::library_exe(name)
                    .unwrap()
                    .with_file_name(name),
                dir.path().join(name),
            )
            .unwrap();
        }
        for (features, shipped) in [
            ("[]", false),
            (r#"["media"]"#, true),
            (r#"["media","-media"]"#, false),
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
                target,
                None,
                false,
                None,
            )
        };
        let error = build(r#"["webgpu"]"#).unwrap_err();
        assert!(error.to_string().contains("WebGPU library"), "{error}");
        fs::copy(
            apps_runtime::app::library_exe(name)
                .unwrap()
                .with_file_name(name),
            dir.path().join(name),
        )
        .unwrap();
        for (features, shipped) in [
            ("[]", false),
            (r#"["webgpu"]"#, true),
            (r#"["*","-webgpu","-media","-terminal"]"#, false),
        ] {
            build(features).unwrap();
            assert_eq!(libraries.join(name).is_file(), shipped, "{features}");
        }
    }

    #[test]
    fn terminal_library_ships_only_with_the_feature() {
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
                target,
                None,
                false,
                None,
            )
        };
        let error = build(r#"["terminal"]"#).unwrap_err();
        assert!(error.to_string().contains("terminal library"), "{error}");
        let names = [
            apps_runtime::terminal::library_name(target.platform),
            apps_runtime::terminal::LICENSE,
        ];
        for name in names {
            fs::copy(
                apps_runtime::app::library_exe(name)
                    .unwrap()
                    .with_file_name(name),
                dir.path().join(name),
            )
            .unwrap();
        }
        for (features, shipped) in [("[]", false), (r#"["terminal"]"#, true)] {
            build(features).unwrap();
            for name in names {
                assert_eq!(libraries.join(name).is_file(), shipped, "{features} {name}");
            }
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
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
        )
        .unwrap();
        fs::write(assets.join("apps.json"), b"old manifest").unwrap();
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(assets.join("app.bundle").is_file());
        assert!(!assets.join("app.js").exists());
        assert!(!assets.join("page-0.html").exists());
        assert!(!assets.join("apps.json").exists());
        assert_eq!(apps_runtime::bundle::read(&out).unwrap().pages.len(), 1);
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
        fs::write(data.join("message.txt"), "first").unwrap();
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
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
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(!assets.join("data/message.txt").exists());
        assert_eq!(fs::read(assets.join("data/new.txt")).unwrap(), b"second");
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
        let executable = if cfg!(windows) { "App.exe" } else { "App" };
        fs::write(&manifest, r#"{"files":["data"]}"#).unwrap();
        package_app(
            &out,
            &files,
            &runtime,
            dir.path(),
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
        )
        .unwrap();
        for name in [
            "app.js",
            "app.bundle",
            "native.json",
            "package.json",
            ".apps-build",
            executable,
        ] {
            let source = dir.path().join(name);
            fs::write(&source, "conflict").unwrap();
            fs::write(&manifest, format!(r#"{{"files":["{name}"]}}"#)).unwrap();
            let error = package_app(
                &out,
                &files,
                &runtime,
                dir.path(),
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
                None,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("conflicts with a build file"),
                "{error}"
            );
            assert_eq!(fs::read(out.join(executable)).unwrap(), b"runtime");
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
                BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
                None,
                false,
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains("without backslashes"), "{error}");
            fs::remove_file(source).unwrap();
        }
        let error = package_app(
            &data.join("dist"),
            &files,
            &runtime,
            dir.path(),
            BuildTarget::new(std::env::consts::OS, std::env::consts::ARCH).unwrap(),
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside data"), "{error}");
        assert!(!data.join("dist").exists());
    }
}
