use anyhow::{Context, Result, bail, ensure};
use object::{Architecture, BinaryFormat, Object, read::macho::FatArch};
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub platform: &'static str,
    pub arch: &'static str,
}

impl Target {
    pub fn new(platform: &str, arch: &str) -> Result<Self> {
        Ok(Self {
            platform: match platform {
                "macos" | "darwin" => "macos",
                "linux" => "linux",
                "windows" | "win32" => "windows",
                _ => bail!("Unsupported platform: {platform}. Use macos, linux, or windows."),
            },
            arch: match arch {
                "arm64" | "aarch64" => "arm64",
                "x64" | "x86_64" => "x64",
                _ => bail!("Unsupported architecture: {arch}. Use x64 or arm64."),
            },
        })
    }

    pub fn validate(self, path: &Path) -> Result<()> {
        let data = object::ReadCache::new(
            fs::File::open(path).with_context(|| format!("Cannot read {}", path.display()))?,
        );
        let architecture = if self.arch == "arm64" {
            Architecture::Aarch64
        } else {
            Architecture::X86_64
        };
        let range = match object::FileKind::parse(&data)
            .with_context(|| format!("Invalid native binary: {}", path.display()))?
        {
            object::FileKind::MachOFat32 => object::read::macho::MachOFatFile32::parse(&data)?
                .arches()
                .iter()
                .find(|arch| arch.architecture() == architecture)
                .map(FatArch::file_range),
            object::FileKind::MachOFat64 => object::read::macho::MachOFatFile64::parse(&data)?
                .arches()
                .iter()
                .find(|arch| arch.architecture() == architecture)
                .map(FatArch::file_range),
            _ => Some((0, fs::metadata(path)?.len())),
        }
        .with_context(|| format!("{} has no {} binary", path.display(), self.arch))?;
        let file = object::File::parse(data.range(range.0, range.1))
            .with_context(|| format!("Invalid native binary: {}", path.display()))?;
        let format = match self.platform {
            "macos" => BinaryFormat::MachO,
            "windows" => BinaryFormat::Pe,
            _ => BinaryFormat::Elf,
        };
        ensure!(
            file.format() == format && file.architecture() == architecture,
            "{} does not match {}-{} ({:?}, {:?})",
            path.display(),
            self.platform,
            self.arch,
            file.format(),
            file.architecture()
        );
        Ok(())
    }
}
