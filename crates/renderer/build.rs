use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(renderer_shaders)");
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        #[cfg(target_os = "macos")]
        Ok("macos") => metal(),
        Ok("windows") if env::var("PROFILE").as_deref() == Ok("release") => hlsl(),
        _ => {}
    }
}

fn hlsl() {
    let source = "src/platform/windows/shaders.hlsl";
    println!("cargo::rerun-if-changed={source}");
    println!("cargo::rerun-if-env-changed=RENDERER_FXC");
    let sdk = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = std::fs::read_dir(&sdk)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("10."))
        .collect();
    versions.sort_by_key(|version| {
        version
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect::<Vec<u32>>()
    });
    let fxc = env::var_os("RENDERER_FXC")
        .map(PathBuf::from)
        .or_else(|| {
            versions
                .last()
                .map(|version| sdk.join(version).join("x64").join("fxc.exe"))
        })
        .filter(|fxc| fxc.exists())
        .unwrap_or_else(|| "fxc.exe".into());
    if Command::new(&fxc).arg("/?").output().is_err() {
        println!("cargo::warning=fxc is missing, shaders are compiled at runtime");
        return;
    }
    let out = env::var("OUT_DIR").unwrap();
    // The entry points of shaders.hlsl. Feature level 10_0 runs shader model 4.0.
    for module in ["draw", "composite", "blur"] {
        for (stage, target) in [("vertex", "vs_4_0"), ("fragment", "ps_4_0")] {
            let entry = format!("{module}_{stage}");
            let output = format!("{out}/{entry}.cso");
            run(Command::new(&fxc).args([
                "/nologo", "/O3", "/T", target, "/E", &entry, "/Fo", &output, source,
            ]));
        }
    }
    println!("cargo::rustc-cfg=renderer_shaders");
}

#[cfg(target_os = "macos")]
fn metal() {
    let source = "src/shaders.wgsl";
    println!("cargo::rerun-if-changed={source}");
    let out = env::var("OUT_DIR").unwrap();
    let msl = format!("{out}/shaders.metal");
    std::fs::write(&msl, translate(source)).unwrap();
    if !Command::new("xcrun")
        .args(["-sdk", "macosx", "metal", "--version"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        println!("cargo::warning=the Metal toolchain is missing, shaders are compiled at runtime");
        return;
    }
    let air = format!("{out}/shaders.air");
    run(Command::new("xcrun").args([
        "-sdk",
        "macosx",
        "metal",
        "-mmacosx-version-min=10.15.7",
        "-c",
        &msl,
        "-o",
        &air,
    ]));
    let library = format!("{out}/shaders.metallib");
    run(Command::new("xcrun").args(["-sdk", "macosx", "metallib", &air, "-o", &library]));
    let data = miniz_oxide::deflate::compress_to_vec(&std::fs::read(&library).unwrap(), 10);
    std::fs::write(format!("{out}/library.deflate"), data).unwrap();
    println!("cargo::rustc-cfg=renderer_shaders");
}

// Metal slots of the bindings that the top of shaders.wgsl lists.
#[cfg(target_os = "macos")]
fn translate(path: &str) -> String {
    use naga::back::msl::{self, sampler};
    let wgsl = std::fs::read_to_string(path).unwrap();
    let module = naga::front::wgsl::parse_str(&wgsl)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string_with_path(&wgsl, path)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{}", error.emit_to_string_with_path(&wgsl, path)));
    let mut slots = msl::EntryPointResources::default();
    for (group, binding, buffer, texture, sampler) in [
        (0, 0, Some(1), None, None),
        (0, 1, None, None, Some(msl::BindSamplerTarget::Inline(0))),
        (1, 0, None, Some(0), None),
        (1, 1, None, Some(1), None),
        (1, 2, None, Some(2), None),
        (2, 0, Some(2), None, None),
        (2, 1, Some(2), None, None),
        (2, 2, Some(2), None, None),
    ] {
        slots.resources.insert(
            naga::ResourceBinding { group, binding },
            msl::BindTarget {
                buffer,
                texture,
                sampler,
                ..Default::default()
            },
        );
    }
    let options = msl::Options {
        lang_version: (2, 1),
        per_entry_point_map: module
            .entry_points
            .iter()
            .map(|entry| (entry.name.clone(), slots.clone()))
            .collect(),
        inline_samplers: vec![sampler::InlineSampler {
            coord: sampler::Coord::Normalized,
            address: [sampler::Address::ClampToEdge; 3],
            border_color: sampler::BorderColor::TransparentBlack,
            mag_filter: sampler::Filter::Linear,
            min_filter: sampler::Filter::Linear,
            mip_filter: None,
            lod_clamp: None,
            max_anisotropy: None,
            compare_func: sampler::CompareFunc::Never,
        }],
        fake_missing_bindings: false,
        force_loop_bounding: false,
        ..Default::default()
    };
    msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
        .unwrap()
        .0
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "shader compilation failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
