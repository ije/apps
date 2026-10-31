//! Video and audio playback for pages. The runtime loads this library only for apps with the video or
//! audio feature, and the library links FFmpeg dynamically. Each media element gets one thread that
//! demuxes and decodes, and one shared output stream mixes the audio of the playing elements.

use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_char, c_int, c_void},
    fmt::Write as _,
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, mpsc},
    time::{Duration, Instant},
};

/// Version of the C ABI. The runtime rejects a library with another version.
pub const ABI: u32 = 1;

/// Returns a runtime buffer of `length` bytes that the library fills.
pub type Alloc = extern "C" fn(output: *mut c_void, length: usize) -> *mut u8;

/// Runtime callbacks. All but `waker` also run on player threads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Host {
    pub waker: extern "C" fn() -> *mut c_void,
    pub wake: extern "C" fn(waker: *mut c_void),
    pub drop_waker: extern "C" fn(waker: *mut c_void),
    /// Returns `length` bytes for pixels, which the runtime owns once `apps_media_frame` returns them.
    pub alloc: extern "C" fn(length: usize) -> *mut u8,
    pub free: extern "C" fn(bytes: *mut u8, length: usize),
}

/// A video frame of straight-alpha BGRA pixels from `Host::alloc`.
#[repr(C)]
pub struct Frame {
    pub pixels: *mut u8,
    pub width: u32,
    pub height: u32,
}

pub const PLAY: u32 = 0;
pub const PAUSE: u32 = 1;
pub const SEEK: u32 = 2;
pub const VOLUME: u32 = 3;
pub const MUTED: u32 = 4;
pub const SPEED: u32 = 5;

/// Decoded audio that plays ahead of the clock, in seconds.
const AHEAD: f64 = 0.3;
/// Decoded video frames that wait for their time.
const FRAMES: usize = 3;

static HOST: OnceLock<Host> = OnceLock::new();

fn host() -> &'static Host {
    HOST.get().expect("apps_media_new sets the host")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Waker(*mut c_void);

// WARNING: runtime wakers are `Send + Sync`, so player threads can wake the loop with them.
unsafe impl Send for Waker {}

impl Waker {
    fn wake(&self) {
        (host().wake)(self.0)
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        (host().drop_waker)(self.0)
    }
}

/// Pixels from `Host::alloc`, freed unless the runtime takes them.
struct Pixels {
    bytes: *mut u8,
    width: u32,
    height: u32,
    time: f64,
}

unsafe impl Send for Pixels {}

impl Drop for Pixels {
    fn drop(&mut self) {
        (host().free)(self.bytes, self.width as usize * self.height as usize * 4)
    }
}

#[repr(C)]
#[derive(Default)]
struct Info {
    duration: f64,
    width: c_int,
    height: c_int,
    audio: c_int,
}

#[cfg(ffmpeg)]
unsafe extern "C" {
    fn apps_ffmpeg_open(
        path: *const c_char,
        data: *const u8,
        size: i64,
        video: c_int,
        rate: c_int,
        channels: c_int,
        info: *mut Info,
        error: *mut c_char,
        error_size: c_int,
    ) -> *mut c_void;
    fn apps_ffmpeg_next(
        media: *mut c_void,
        want: c_int,
        time: *mut f64,
        width: *mut c_int,
        height: *mut c_int,
        samples: *mut *const f32,
        count: *mut c_int,
    ) -> c_int;
    fn apps_ffmpeg_pixels(media: *mut c_void, pixels: *mut u8) -> c_int;
    fn apps_ffmpeg_seek(media: *mut c_void, time: f64) -> c_int;
    fn apps_ffmpeg_speed(media: *mut c_void, speed: f64);
    fn apps_ffmpeg_close(media: *mut c_void);
}

#[cfg(not(ffmpeg))]
mod missing {
    use super::*;
    pub unsafe fn apps_ffmpeg_open(
        _: *const c_char,
        _: *const u8,
        _: i64,
        _: c_int,
        _: c_int,
        _: c_int,
        _: *mut Info,
        error: *mut c_char,
        size: c_int,
    ) -> *mut c_void {
        let message = b"FFmpeg is missing from this build. Run bash scripts/build-ffmpeg.sh\0";
        let length = message.len().min(size as usize);
        unsafe { std::ptr::copy_nonoverlapping(message.as_ptr().cast(), error, length) };
        std::ptr::null_mut()
    }
    pub unsafe fn apps_ffmpeg_next(
        _: *mut c_void,
        _: c_int,
        _: *mut f64,
        _: *mut c_int,
        _: *mut c_int,
        _: *mut *const f32,
        _: *mut c_int,
    ) -> c_int {
        0
    }
    pub unsafe fn apps_ffmpeg_pixels(_: *mut c_void, _: *mut u8) -> c_int {
        -1
    }
    pub unsafe fn apps_ffmpeg_seek(_: *mut c_void, _: f64) -> c_int {
        -1
    }
    pub unsafe fn apps_ffmpeg_speed(_: *mut c_void, _: f64) {}
    pub unsafe fn apps_ffmpeg_close(_: *mut c_void) {}
}
#[cfg(not(ffmpeg))]
use missing::*;

/// The media time, which runs while the element plays. The mixer moves it to the audio it plays.
struct Clock {
    time: f64,
    at: Instant,
    running: bool,
    speed: f64,
}

impl Clock {
    fn now(&self) -> f64 {
        self.time
            + if self.running {
                self.at.elapsed().as_secs_f64() * self.speed
            } else {
                0.
            }
    }

    fn run(&mut self, running: bool) {
        self.set(self.now());
        self.running = running;
    }

    fn set(&mut self, time: f64) {
        self.time = time;
        self.at = Instant::now();
    }
}

/// Interleaved samples at the output rate. Each output frame moves the media time by `step` seconds.
struct Chunk {
    time: f64,
    step: f64,
    samples: Vec<f32>,
    read: usize,
}

/// The audio of one element and its clock, shared with the mixer.
struct Audio {
    chunks: VecDeque<Chunk>,
    /// Samples in `chunks` that did not play yet.
    queued: usize,
    gain: f32,
    clock: Clock,
}

impl Audio {
    fn mix(&mut self, out: &mut [f32], channels: usize, now: Instant) {
        let mut written = 0;
        while written < out.len()
            && let Some(chunk) = self.chunks.front_mut()
        {
            if written == 0 {
                self.clock.time = chunk.time + (chunk.read / channels) as f64 * chunk.step;
                self.clock.at = now;
            }
            let count = (chunk.samples.len() - chunk.read).min(out.len() - written);
            for (out, sample) in out[written..written + count]
                .iter_mut()
                .zip(&chunk.samples[chunk.read..chunk.read + count])
            {
                *out += sample * self.gain;
            }
            chunk.read += count;
            written += count;
            if chunk.read == chunk.samples.len() {
                self.chunks.pop_front();
            }
        }
        self.queued -= written;
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.queued = 0;
    }

    /// Drops the samples that the clock passed, for apps without an audio output.
    fn skip(&mut self, channels: usize) {
        let now = self.clock.now();
        while let Some(chunk) = self.chunks.front()
            && chunk.time + (chunk.samples.len() / channels) as f64 * chunk.step <= now
        {
            self.queued -= chunk.samples.len() - chunk.read;
            self.chunks.pop_front();
        }
    }
}

/// One output stream for the app. It pauses while no element plays, so idle apps keep the device asleep.
struct Mixer {
    rate: u32,
    channels: usize,
    playing: Arc<Mutex<Vec<Arc<Mutex<Audio>>>>>,
    control: mpsc::Sender<bool>,
}

impl Mixer {
    fn set(&self, audio: &Arc<Mutex<Audio>>, on: bool) {
        let mut playing = lock(&self.playing);
        let idle = playing.is_empty();
        playing.retain(|other| !Arc::ptr_eq(other, audio));
        if on {
            playing.push(audio.clone());
        }
        if idle != playing.is_empty() {
            let _ = self.control.send(idle);
        }
    }
}

fn mixer() -> Option<&'static Mixer> {
    static MIXER: OnceLock<Option<Mixer>> = OnceLock::new();
    MIXER
        .get_or_init(|| {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            let (ready, config) = mpsc::channel();
            let (control, commands) = mpsc::channel::<bool>();
            let playing = Arc::new(Mutex::new(Vec::<Arc<Mutex<Audio>>>::new()));
            let tracks = playing.clone();
            // WARNING: cpal streams are not `Send` on every platform, so one thread owns the stream for the app's life.
            std::thread::Builder::new()
                .name("apps-audio".into())
                .spawn(move || {
                    let stream = (|| -> Result<_, String> {
                        let device = cpal::default_host()
                            .default_output_device()
                            .ok_or("No audio output device")?;
                        let config = device
                            .default_output_config()
                            .map_err(|error| error.to_string())?;
                        let (rate, channels) = (config.sample_rate(), config.channels() as usize);
                        let format = config.sample_format();
                        let mix = move |out: &mut [f32]| {
                            out.fill(0.);
                            let now = Instant::now();
                            for audio in lock(&tracks).iter() {
                                lock(audio).mix(out, channels, now);
                            }
                        };
                        let error = |error| eprintln!("apps: audio output: {error}");
                        let config = config.config();
                        let stream = match format {
                            cpal::SampleFormat::F32 => device.build_output_stream(
                                config,
                                move |out: &mut [f32], _: &_| mix(out),
                                error,
                                None,
                            ),
                            cpal::SampleFormat::I16 => {
                                let mut buffer = Vec::new();
                                device.build_output_stream(
                                    config,
                                    move |out: &mut [i16], _: &_| {
                                        buffer.resize(out.len(), 0.);
                                        mix(&mut buffer);
                                        for (out, sample) in out.iter_mut().zip(&buffer) {
                                            *out = (sample.clamp(-1., 1.) * 32767.) as i16;
                                        }
                                    },
                                    error,
                                    None,
                                )
                            }
                            format => return Err(format!("Unsupported sample format {format}")),
                        }
                        .map_err(|error| error.to_string())?;
                        let _ = stream.pause();
                        Ok((stream, rate, channels))
                    })();
                    match stream {
                        Ok((stream, rate, channels)) => {
                            let _ = ready.send(Some((rate, channels)));
                            for play in commands {
                                let _ = if play { stream.play() } else { stream.pause() };
                            }
                        }
                        Err(error) => {
                            eprintln!("apps: audio output: {error}");
                            let _ = ready.send(None);
                        }
                    }
                })
                .ok()?;
            let (rate, channels) = config.recv().ok().flatten()?;
            Some(Mixer {
                rate,
                channels,
                playing,
                control,
            })
        })
        .as_ref()
}

#[derive(Default)]
struct State {
    playing: bool,
    /// The first frame and some audio are decoded since the load or the last seek.
    ready: bool,
    closed: bool,
    seek: Option<f64>,
    speed: Option<f64>,
    /// Counts seeks, so a decode that started before one is dropped.
    generation: u64,
    frames: VecDeque<Pixels>,
    /// Shows the next frame at once, as after a load or a seek.
    show: bool,
    /// The loop was woken for the first frame in `frames`.
    woken: bool,
    /// JSON event arguments after the element id.
    events: Vec<String>,
}

struct Shared {
    state: Mutex<State>,
    signal: Condvar,
    audio: Arc<Mutex<Audio>>,
}

impl Shared {
    /// Runs the clock and the element's audio while it plays and is ready.
    fn update(&self, state: &State) {
        let running = state.playing && state.ready && !state.closed;
        let mut audio = lock(&self.audio);
        if audio.clock.running != running {
            audio.clock.run(running);
            drop(audio);
            if let Some(mixer) = mixer() {
                mixer.set(&self.audio, running);
            }
        }
    }
}

struct Player {
    shared: Arc<Shared>,
    video: bool,
    volume: f32,
    muted: bool,
}

#[derive(Default)]
struct Media {
    players: HashMap<u64, Player>,
}

enum Source {
    Path(std::ffi::CString),
    Bytes(Vec<u8>),
}

fn run(shared: Arc<Shared>, source: Source, video: bool, waker: Waker) {
    // WARNING: without an audio output the audio still decodes, so audio files keep their time and end.
    let output = mixer().map(|mixer| (mixer.rate, mixer.channels));
    let (rate, channels) = output.unwrap_or((8000, 1));
    let event = |text: String| {
        lock(&shared.state).events.push(text);
        waker.wake();
    };
    let mut info = Info::default();
    let mut error = [0 as c_char; 256];
    let (path, data, size) = match &source {
        Source::Path(path) => (path.as_ptr(), std::ptr::null(), 0),
        Source::Bytes(bytes) => (std::ptr::null(), bytes.as_ptr(), bytes.len() as i64),
    };
    let media = unsafe {
        apps_ffmpeg_open(
            path,
            data,
            size,
            c_int::from(video),
            rate as c_int,
            channels as c_int,
            &mut info,
            error.as_mut_ptr(),
            error.len() as c_int,
        )
    };
    if media.is_null() {
        let message = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        return event(format!("\"error\",4,{:?}", format!("Cannot open the media: {message}")));
    }
    let duration = info.duration;
    event(format!(
        "\"meta\",{},{},{},{}",
        if duration.is_finite() {
            duration.to_string()
        } else {
            "null".into()
        },
        info.width,
        info.height,
        info.audio
    ));
    let (has_video, has_audio) = (info.width > 0, info.audio != 0);
    let samples_per_second = (rate as usize * channels).max(1) as f64;
    let (mut video_done, mut audio_done) = (!has_video, !has_audio);
    let (mut speed, mut generation) = (1., 0);
    let (mut audio_end, mut last_frame) = (0., -1. / 30.);
    // The seek target that frames and samples before it are dropped for, and whether it was reported.
    let (mut target, mut loaded) = (Some(0.), false);
    let mut tick = Instant::now();
    loop {
        let mut state = lock(&shared.state);
        if state.closed {
            break;
        }
        if let Some(time) = state.seek.take() {
            generation = state.generation;
            drop(state);
            unsafe { apps_ffmpeg_seek(media, time) };
            (video_done, audio_done) = (!has_video, !has_audio);
            (audio_end, last_frame, target) = (time, time - 1. / 30., Some(time));
            continue;
        }
        if let Some(value) = state.speed.take() {
            speed = value;
            unsafe { apps_ffmpeg_speed(media, value) };
        }
        let mut audio = lock(&shared.audio);
        if output.is_none() {
            audio.skip(channels);
        }
        let buffered = audio.queued as f64 / samples_per_second;
        drop(audio);
        let ready = (video_done || !state.frames.is_empty()) && (audio_done || buffered >= AHEAD / 2.);
        if target.is_some() && ready && state.generation == generation {
            target = None;
            state.ready = true;
            state.show = true;
            state.events.push(if loaded { "\"seeked\"" } else { "\"loaded\"" }.into());
            loaded = true;
            shared.update(&state);
            waker.wake();
        }
        let want = c_int::from(!video_done && state.frames.len() < FRAMES)
            | c_int::from(!audio_done && buffered < AHEAD) << 1;
        // WARNING: paused elements decode only up to their first frame, so play starts at once and idle pages use no CPU.
        if want != 0 && (state.playing || target.is_some()) {
            drop(state);
            let (mut time, mut width, mut height) = (0., 0, 0);
            let (mut samples, mut count) = (std::ptr::null(), 0);
            let kind = unsafe {
                apps_ffmpeg_next(
                    media,
                    want,
                    &mut time,
                    &mut width,
                    &mut height,
                    &mut samples,
                    &mut count,
                )
            };
            match kind {
                1 => {
                    let time = if time.is_finite() {
                        time
                    } else {
                        last_frame + 1. / 30.
                    };
                    last_frame = time;
                    if target.is_some_and(|target| time < target - 0.02) {
                        continue;
                    }
                    let length = width as usize * height as usize * 4;
                    let bytes = (host().alloc)(length);
                    if bytes.is_null() {
                        continue;
                    }
                    let pixels = Pixels {
                        bytes,
                        width: width as u32,
                        height: height as u32,
                        time,
                    };
                    if unsafe { apps_ffmpeg_pixels(media, bytes) } < 0 {
                        continue;
                    }
                    let mut state = lock(&shared.state);
                    if state.generation == generation {
                        state.frames.push_back(pixels);
                    }
                }
                2 => {
                    let mut time = if time.is_finite() { time } else { audio_end };
                    let step = speed / rate as f64;
                    let mut samples = unsafe {
                        std::slice::from_raw_parts(samples, count as usize * channels)
                    };
                    if let Some(target) = target
                        && time < target
                    {
                        let skip = (((target - time) / step).ceil() as usize).min(count as usize);
                        samples = &samples[skip * channels..];
                        time += skip as f64 * step;
                    }
                    audio_end = time + (samples.len() / channels) as f64 * step;
                    let state = lock(&shared.state);
                    if state.generation == generation && !samples.is_empty() {
                        let mut audio = lock(&shared.audio);
                        audio.queued += samples.len();
                        audio.chunks.push_back(Chunk {
                            time,
                            step,
                            samples: samples.to_vec(),
                            read: 0,
                        });
                    }
                }
                3 => {}
                0 => {
                    video_done |= want & 1 != 0;
                    audio_done |= want & 2 != 0;
                }
                error => {
                    let mut state = lock(&shared.state);
                    state.ready = false;
                    shared.update(&state);
                    state.events.push(format!("\"error\",3,\"Cannot decode the media ({error})\""));
                    waker.wake();
                    break;
                }
            }
            continue;
        }
        let mut wait = None::<f64>;
        let mut until = |seconds: f64| wait = Some(wait.map_or(seconds, |wait| wait.min(seconds)));
        if state.playing && state.ready {
            let now = lock(&shared.audio).clock.now();
            if video_done && audio_done && state.frames.is_empty() && buffered == 0. {
                state.playing = false;
                shared.update(&state);
                lock(&shared.audio)
                    .clock
                    .set(if duration.is_finite() { duration } else { now });
                state.events.push("\"ended\"".into());
                waker.wake();
                continue;
            }
            if let Some(head) = state.frames.front() {
                let due = (head.time - now) / speed;
                if due > 0. {
                    until(due);
                } else if !state.woken {
                    state.woken = true;
                    waker.wake();
                }
            }
            if !audio_done {
                until((buffered - AHEAD / 2.).max(0.005));
            } else if buffered > 0. {
                until(buffered + 0.005);
            } else if video_done && state.frames.is_empty() {
                until(0.005);
            }
            let elapsed = tick.elapsed().as_secs_f64();
            if elapsed >= 0.25 {
                tick = Instant::now();
                state.events.push("\"time\"".into());
                waker.wake();
                until(0.25);
            } else {
                until(0.25 - elapsed);
            }
        }
        if state.show && !state.frames.is_empty() && !state.woken {
            state.woken = true;
            waker.wake();
        }
        drop(match wait {
            Some(wait) => {
                shared
                    .signal
                    .wait_timeout(state, Duration::from_secs_f64(wait))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .signal
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        });
    }
    unsafe { apps_ffmpeg_close(media) };
}

impl Media {
    fn close(&mut self, id: u64) {
        if let Some(player) = self.players.remove(&id) {
            let mut state = lock(&player.shared.state);
            state.closed = true;
            state.frames.clear();
            player.shared.update(&state);
            player.shared.signal.notify_one();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_media_abi() -> u32 {
    ABI
}

/// # Safety
/// `host` points to a `Host`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_new(host: *const c_void) -> *mut c_void {
    HOST.get_or_init(|| unsafe { *host.cast::<Host>() });
    Box::into_raw(Box::<Media>::default()).cast()
}

/// # Safety
/// `media` comes from `apps_media_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_free(media: *mut c_void) {
    let mut media = unsafe { Box::from_raw(media.cast::<Media>()) };
    for id in media.players.keys().copied().collect::<Vec<_>>() {
        media.close(id);
    }
}

/// Starts to load element `id` from the UTF-8 file path, or from a copy of the bytes when `path` is null.
/// Its events arrive through `apps_media_poll`.
///
/// # Safety
/// `media` comes from `apps_media_new`, and `path` and `bytes` point to `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_open(
    media: *mut c_void,
    id: u64,
    path: *const u8,
    bytes: *const u8,
    length: usize,
    video: bool,
) {
    let media = unsafe { &mut *media.cast::<Media>() };
    media.close(id);
    let source = if path.is_null() {
        Source::Bytes(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())
    } else {
        let path = unsafe { std::slice::from_raw_parts(path, length) };
        Source::Path(std::ffi::CString::new(path).unwrap_or_default())
    };
    let shared = Arc::new(Shared {
        state: Mutex::default(),
        signal: Condvar::new(),
        audio: Arc::new(Mutex::new(Audio {
            chunks: VecDeque::new(),
            queued: 0,
            gain: 1.,
            clock: Clock {
                time: 0.,
                at: Instant::now(),
                running: false,
                speed: 1.,
            },
        })),
    });
    let waker = Waker((host().waker)());
    let player = shared.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("apps-media".into())
        .spawn(move || run(player, source, video, waker))
    {
        lock(&shared.state)
            .events
            .push(format!("\"error\",4,{:?}", error.to_string()));
    }
    media.players.insert(
        id,
        Player {
            shared,
            video,
            volume: 1.,
            muted: false,
        },
    );
}

/// Runs a control op of element `id` with its value.
///
/// # Safety
/// `media` comes from `apps_media_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_control(media: *mut c_void, id: u64, op: u32, value: f64) {
    let media = unsafe { &mut *media.cast::<Media>() };
    let Some(player) = media.players.get_mut(&id) else {
        return;
    };
    let shared = &player.shared;
    let mut state = lock(&shared.state);
    match op {
        PLAY => state.playing = true,
        PAUSE => state.playing = false,
        SEEK => {
            let time = if value.is_finite() { value.max(0.) } else { 0. };
            state.generation += 1;
            state.seek = Some(time);
            state.ready = false;
            state.frames.clear();
            state.woken = false;
            let mut audio = lock(&shared.audio);
            audio.clear();
            audio.clock.set(time);
        }
        VOLUME | MUTED => {
            if op == VOLUME {
                player.volume = value.clamp(0., 1.) as f32;
            } else {
                player.muted = value != 0.;
            }
            lock(&shared.audio).gain = if player.muted { 0. } else { player.volume };
        }
        SPEED if value > 0. && value.is_finite() => {
            state.speed = Some(value);
            let mut audio = lock(&shared.audio);
            let running = audio.clock.running;
            audio.clock.run(running);
            audio.clock.speed = value;
        }
        _ => {}
    }
    shared.update(&state);
    shared.signal.notify_one();
}

/// Closes element `id`.
///
/// # Safety
/// `media` comes from `apps_media_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_close(media: *mut c_void, id: u64) {
    unsafe { &mut *media.cast::<Media>() }.close(id);
}

/// The current time of element `id` in seconds.
///
/// # Safety
/// `media` comes from `apps_media_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_time(media: *mut c_void, id: u64) -> f64 {
    let media = unsafe { &*media.cast::<Media>() };
    media
        .players
        .get(&id)
        .map_or(0., |player| lock(&player.shared.audio).clock.now())
}

/// Writes the events since the last poll as a JSON array of `[id, kind, ...values]`. Kind `frame` means
/// that `apps_media_frame` has a new frame for the element.
///
/// # Safety
/// `media` comes from `apps_media_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_poll(media: *mut c_void, out: *mut c_void, alloc: Alloc) {
    let media = unsafe { &*media.cast::<Media>() };
    let mut json = String::from("[");
    for (id, player) in &media.players {
        let mut state = lock(&player.shared.state);
        for event in state.events.drain(..) {
            let _ = write!(json, "[{id},{event}],");
        }
        if player.video
            && let Some(head) = state.frames.front()
            && (state.show || head.time <= lock(&player.shared.audio).clock.now())
        {
            let _ = write!(json, "[{id},\"frame\"],");
        }
    }
    if json.len() > 1 {
        json.pop();
    }
    json.push(']');
    let target = alloc(out, json.len());
    unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), target, json.len()) };
}

/// Takes the latest due frame of element `id`. The runtime owns its pixels.
///
/// # Safety
/// `media` comes from `apps_media_new`, and `frame` points to a `Frame`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_frame(media: *mut c_void, id: u64, frame: *mut Frame) -> bool {
    let media = unsafe { &*media.cast::<Media>() };
    let Some(player) = media.players.get(&id) else {
        return false;
    };
    let mut state = lock(&player.shared.state);
    let now = lock(&player.shared.audio).clock.now();
    let mut shown = None;
    while let Some(head) = state.frames.front()
        && (state.show || head.time <= now)
    {
        state.show = false;
        shown = state.frames.pop_front();
    }
    state.woken = false;
    player.shared.signal.notify_one();
    let Some(pixels) = shown else {
        return false;
    };
    unsafe {
        *frame = Frame {
            pixels: pixels.bytes,
            width: pixels.width,
            height: pixels.height,
        }
    };
    std::mem::forget(pixels);
    true
}
