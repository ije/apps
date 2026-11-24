use anyhow::{Context, Result, ensure};

static DEV: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

// The executable whose folder holds the library `name`.
#[doc(hidden)]
// Only debug builds read `name`.
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
pub fn exe(name: &str) -> std::io::Result<std::path::PathBuf> {
    if let Some(exe) = DEV.get() {
        return Ok(exe.clone());
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    // Since Cargo 1.100, a test runs in `build/<package>/<hash>/out` of the profile folder, and a library that
    // only a test needs stays in such a folder. The newest one goes to the profile folder, where FFmpeg is.
    #[cfg(debug_assertions)]
    if let Some(profile) = exe
        .ancestors()
        .nth(4)
        .filter(|build| {
            build.ends_with("build") && exe.parent().is_some_and(|out| out.ends_with("out"))
        })
        .and_then(std::path::Path::parent)
        .or(exe.parent())
        && let Some(file) = exe.file_name()
    {
        let package = name
            .trim_start_matches("lib")
            .split('.')
            .next()
            .unwrap_or(name);
        let package = if package == "Ghostty" {
            "apps-terminal"
        } else {
            package
        };
        let modified =
            |path: &std::path::Path| path.metadata().and_then(|file| file.modified()).ok();
        let target = profile.join(name);
        let built = std::fs::read_dir(profile.join("build").join(package.replace('_', "-")))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|hash| {
                let path = hash.path().join("out").join(name);
                Some((modified(&path)?, path))
            })
            .max();
        // The threads of a test share the name of the temporary file, so one of them copies at a time.
        static COPY: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _copy = COPY.lock();
        if let Some((time, path)) = built
            && modified(&target).is_none_or(|old| old < time)
        {
            // MacOS kills processes that load a library rewritten in place, so copy to a new file.
            let temporary = profile.join(format!(".{name}.{}", std::process::id()));
            if std::fs::copy(&path, &temporary).is_ok() {
                let _ = std::fs::rename(&temporary, &target);
            }
        }
        return Ok(profile.join(file));
    }
    Ok(exe)
}

pub(crate) fn set_dev_path(path: std::path::PathBuf) {
    let _ = DEV.set(path);
}

// Like JavaScriptCore, the library only loads from the app, never from the environment. It is never unloaded,
// because its threads may outlive every page and device.
pub(crate) fn load(
    name: &str,
    what: &str,
    abi_symbol: &[u8],
    abi: u32,
) -> Result<&'static libloading::Library> {
    let executable = exe(name)?;
    let directory = executable
        .parent()
        .context("Missing executable directory")?;
    let path = ["../Frameworks", ".", "deps"]
        .iter()
        .map(|folder| directory.join(folder).join(name))
        .find(|path| path.is_file())
        .with_context(|| format!("The {what} library {name} is missing next to the runtime"))?;
    let library = unsafe { libloading::Library::new(&path) }
        .with_context(|| format!("Cannot load the {what} library from {}", path.display()))?;
    let version: libloading::Symbol<extern "C" fn() -> u32> = unsafe { library.get(abi_symbol)? };
    ensure!(
        version() == abi,
        "{} does not match this runtime",
        path.display()
    );
    Ok(Box::leak(Box::new(library)))
}
