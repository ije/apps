use super::gpu::{Alloc, Output, alloc, drop_waker, wake, waker};
use anyhow::{Context, Result, anyhow};
use std::{ffi::c_void, mem::MaybeUninit, path::Path, ptr::NonNull, sync::OnceLock};

// The source of the media library, compiled here without FFmpeg: the built-in player of WAV files.
#[path = "../../../media/src/lib.rs"]
mod audio;
use audio::{ABI, Frame, Host};

static HOST: Host = Host {
    waker,
    wake,
    drop_waker,
    alloc: alloc_pixels,
    free: free_pixels,
};

extern "C" fn alloc_pixels(length: usize) -> *mut u8 {
    // Media files pick the frame size, so a failed allocation returns null instead of aborting.
    let mut pixels = Vec::<MaybeUninit<u8>>::new();
    if pixels.try_reserve_exact(length).is_err() {
        return std::ptr::null_mut();
    }
    unsafe { pixels.set_len(length) };
    Box::into_raw(pixels.into_boxed_slice()).cast()
}

extern "C" fn free_pixels(bytes: *mut u8, length: usize) {
    drop(unsafe {
        Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            bytes.cast::<MaybeUninit<u8>>(),
            length,
        ))
    });
}

struct Library {
    new: unsafe extern "C" fn(*const c_void) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
    open: unsafe extern "C" fn(*mut c_void, u64, *const u8, *const u8, usize, bool),
    control: unsafe extern "C" fn(*mut c_void, u64, u32, f64),
    close: unsafe extern "C" fn(*mut c_void, u64),
    time: unsafe extern "C" fn(*mut c_void, u64) -> f64,
    poll: unsafe extern "C" fn(*mut c_void, *mut c_void, Alloc),
    frame: unsafe extern "C" fn(*mut c_void, u64, *mut Frame) -> bool,
    recycle: unsafe extern "C" fn(*mut c_void, u64, *mut u8, u32, u32),
}

static BUILTIN: Library = Library {
    new: audio::apps_media_new,
    free: audio::apps_media_free,
    open: audio::apps_media_open,
    control: audio::apps_media_control,
    close: audio::apps_media_close,
    time: audio::apps_media_time,
    poll: audio::apps_media_poll,
    frame: audio::apps_media_frame,
    recycle: audio::apps_media_recycle,
};

pub fn library_name(platform: &str) -> &'static str {
    match platform {
        "macos" => "libapps_media.dylib",
        "windows" => "apps_media.dll",
        _ => "libapps_media.so",
    }
}

pub fn ffmpeg_name(platform: &str) -> &'static str {
    match platform {
        "macos" => "libffmpeg.dylib",
        "windows" => "ffmpeg.dll",
        _ => "libffmpeg.so",
    }
}

fn load() -> Result<Library> {
    let library = super::gpu::library(
        library_name(std::env::consts::OS),
        "media",
        b"apps_media_abi\0",
        ABI,
    )?;
    unsafe {
        Ok(Library {
            new: *library.get(b"apps_media_new\0")?,
            free: *library.get(b"apps_media_free\0")?,
            open: *library.get(b"apps_media_open\0")?,
            control: *library.get(b"apps_media_control\0")?,
            close: *library.get(b"apps_media_close\0")?,
            time: *library.get(b"apps_media_time\0")?,
            poll: *library.get(b"apps_media_poll\0")?,
            frame: *library.get(b"apps_media_frame\0")?,
            recycle: *library.get(b"apps_media_recycle\0")?,
        })
    }
}

#[derive(Default)]
pub struct Media {
    raw: Option<(NonNull<c_void>, &'static Library)>,
}

impl Media {
    fn raw(&mut self, ffmpeg: bool) -> Result<(NonNull<c_void>, &'static Library)> {
        if let Some(raw) = self.raw {
            return Ok(raw);
        }
        static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();
        let library = if ffmpeg {
            LIBRARY
                .get_or_init(|| load().map_err(|error| format!("{error:#}")))
                .as_ref()
                .map_err(|error| anyhow!("{error}"))?
        } else {
            &BUILTIN
        };
        let raw = NonNull::new(unsafe { (library.new)((&raw const HOST).cast()) })
            .context("Cannot create the media state")?;
        Ok(*self.raw.insert((raw, library)))
    }

    pub fn open(
        &mut self,
        id: u64,
        source: Result<&Path, &[u8]>,
        video: bool,
        ffmpeg: bool,
    ) -> Result<()> {
        let (raw, library) = self.raw(ffmpeg)?;
        let (data, file) = match source {
            Ok(path) => (
                path.to_str()
                    .context("The media path is not UTF-8")?
                    .as_bytes(),
                true,
            ),
            Err(bytes) => (bytes, false),
        };
        unsafe {
            (library.open)(
                raw.as_ptr(),
                id,
                if file {
                    data.as_ptr()
                } else {
                    std::ptr::null()
                },
                data.as_ptr(),
                data.len(),
                video,
            )
        };
        Ok(())
    }

    pub fn control(&mut self, id: u64, op: u32, value: f64) {
        if let Some((raw, library)) = self.raw {
            unsafe { (library.control)(raw.as_ptr(), id, op, value) };
        }
    }

    pub fn close(&mut self, id: u64) {
        if let Some((raw, library)) = self.raw {
            unsafe { (library.close)(raw.as_ptr(), id) };
        }
    }

    pub fn time(&self, id: u64) -> f64 {
        self.raw.map_or(0., |(raw, library)| unsafe {
            (library.time)(raw.as_ptr(), id)
        })
    }

    pub fn poll(&mut self) -> Vec<u8> {
        let Some((raw, library)) = self.raw else {
            return b"[]".to_vec();
        };
        let mut output = Output::default();
        unsafe { (library.poll)(raw.as_ptr(), (&raw mut output).cast(), alloc) };
        output.finish()
    }

    pub fn frame(&mut self, id: u64) -> Option<renderer::RenderImage> {
        let (raw, library) = self.raw?;
        let mut frame = Frame {
            pixels: std::ptr::null_mut(),
            width: 0,
            height: 0,
        };
        if !unsafe { (library.frame)(raw.as_ptr(), id, &mut frame) } {
            return None;
        }
        let length = frame.width as usize * frame.height as usize * 4;
        let pixels =
            unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(frame.pixels, length)) };
        Some(renderer::RenderImage::new(
            frame.width,
            frame.height,
            pixels.into_vec(),
        ))
    }

    pub fn recycle(&mut self, id: u64, image: renderer::RenderImage) {
        let Some((raw, library)) = self.raw else {
            return;
        };
        let size = image.size();
        let pixels = Box::into_raw(image.into_bytes().into_boxed_slice());
        unsafe {
            (library.recycle)(
                raw.as_ptr(),
                id,
                pixels.cast(),
                size.width as u32,
                size.height as u32,
            )
        };
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Some((raw, library)) = self.raw {
            unsafe { (library.free)(raw.as_ptr()) };
        }
    }
}
