use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_char, c_int, c_void},
    fmt::Write as _,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering::*, fence},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const ABI: u32 = 2;

pub type Alloc = extern "C" fn(output: *mut c_void, length: usize) -> *mut u8;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Host {
    pub waker: extern "C" fn() -> *mut c_void,
    pub wake: extern "C" fn(waker: *mut c_void),
    pub drop_waker: extern "C" fn(waker: *mut c_void),
    pub alloc: extern "C" fn(length: usize) -> *mut u8,
    pub free: extern "C" fn(bytes: *mut u8, length: usize),
}

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

const AHEAD: f64 = 0.3;
const FRAMES: usize = 3;

static HOST: OnceLock<Host> = OnceLock::new();

fn host() -> &'static Host {
    HOST.get().expect("apps_media_new sets the host")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Waker(*mut c_void);

// SAFETY: runtime wakers are `Send + Sync`, so player threads may wake the loop with them.
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
    #[allow(clippy::too_many_arguments)]
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

static PLAYING: [AtomicPtr<Sound>; 64] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 64];
// WARNING: odd while the audio callback reads `PLAYING`, so a removed sound is freed only after that read ends.
static MIXING: AtomicU64 = AtomicU64::new(0);

fn ticks(at: Instant) -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    at.saturating_duration_since(*BASE.get_or_init(Instant::now))
        .as_nanos() as u64
}

// WARNING: only the decoder writes samples and only the audio callback reads them, through a ring of atomic samples,
// so neither waits for the other.
struct Sound {
    ring: OnceLock<(Box<[AtomicU32]>, usize)>,
    write: AtomicUsize,
    read: AtomicUsize,
    // WARNING: samples before `skip` were dropped by a seek. Only the reader moves `read`: the audio callback, or
    // `drop_played` when there is no output.
    skip: AtomicUsize,
    gain: AtomicU32,
    played: [AtomicU64; 3],
    clock: Mutex<Clock>,
}

struct Clock {
    time: f64,
    at: Instant,
    running: bool,
    speed: f64,
    marks: VecDeque<(usize, f64, f64)>,
}

impl Sound {
    fn new() -> Self {
        Self {
            ring: OnceLock::new(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            skip: AtomicUsize::new(0),
            gain: AtomicU32::new(1f32.to_bits()),
            played: Default::default(),
            clock: Mutex::new(Clock {
                time: 0.,
                at: Instant::now(),
                running: false,
                speed: 1.,
                marks: VecDeque::new(),
            }),
        }
    }

    fn queued(&self) -> usize {
        self.write.load(Acquire) - self.read.load(Acquire).max(self.skip.load(Acquire))
    }

    // WARNING: runs on the audio callback: no locks, allocations or frees.
    fn mix(&self, out: &mut [f32], now: u64) {
        let Some((ring, _)) = self.ring.get() else {
            return;
        };
        let read = self.read.load(Relaxed).max(self.skip.load(Acquire));
        let count = (self.write.load(Acquire) - read).min(out.len());
        if count > 0 {
            let sequence = self.played[0].load(Relaxed);
            self.played[0].store(sequence + 1, Relaxed);
            fence(Release);
            self.played[1].store(read as u64, Relaxed);
            self.played[2].store(now, Relaxed);
            self.played[0].store(sequence + 2, Release);
        }
        let gain = f32::from_bits(self.gain.load(Relaxed));
        for (index, out) in out[..count].iter_mut().enumerate() {
            *out += f32::from_bits(ring[(read + index) & (ring.len() - 1)].load(Relaxed)) * gain;
        }
        self.read.store(read + count, Release);
    }

    fn played(&self) -> Option<(usize, u64)> {
        for _ in 0..64 {
            let sequence = self.played[0].load(Acquire);
            let (position, at) = (self.played[1].load(Relaxed), self.played[2].load(Relaxed));
            fence(Acquire);
            if sequence & 1 == 0 && self.played[0].load(Relaxed) == sequence {
                return (sequence > 0).then_some((position as usize, at));
            }
            std::hint::spin_loop();
        }
        None
    }

    fn now(&self, clock: &Clock) -> f64 {
        if !clock.running {
            return clock.time;
        }
        if let Some((position, at)) = self.played()
            && at >= ticks(clock.at)
            && position >= self.skip.load(Acquire)
            && let Some((_, channels)) = self.ring.get()
            && let Some(&(start, time, step)) = clock
                .marks
                .iter()
                .rev()
                .find(|(start, ..)| *start <= position)
        {
            let elapsed = ticks(Instant::now()).saturating_sub(at) as f64 / 1e9;
            return time + ((position - start) / channels) as f64 * step + elapsed * clock.speed;
        }
        clock.time + clock.at.elapsed().as_secs_f64() * clock.speed
    }

    fn time(&self) -> (f64, bool) {
        let clock = lock(&self.clock);
        (self.now(&clock), clock.running)
    }

    fn set(&self, time: Option<f64>, running: Option<bool>, speed: Option<f64>) {
        let mut clock = lock(&self.clock);
        clock.time = time.unwrap_or_else(|| self.now(&clock));
        clock.at = Instant::now();
        clock.running = running.unwrap_or(clock.running);
        clock.speed = speed.unwrap_or(clock.speed);
    }

    // WARNING: call with the player's state lock held, or the decoder can queue samples of the old position.
    fn flush(&self) {
        lock(&self.clock).marks.clear();
        self.skip.store(self.write.load(Acquire), Release);
    }

    fn push(&self, samples: &[f32], time: f64, step: f64) -> usize {
        let Some((ring, channels)) = self.ring.get() else {
            return samples.len();
        };
        let write = self.write.load(Relaxed);
        let free = ring.len() - (write - self.read.load(Acquire).max(self.skip.load(Acquire)));
        let count = samples.len().min(free / channels * channels);
        if count == 0 {
            return 0;
        }
        for (index, sample) in samples[..count].iter().enumerate() {
            ring[(write + index) & (ring.len() - 1)].store(sample.to_bits(), Relaxed);
        }
        let keep = self
            .played()
            .map_or(self.read.load(Acquire), |(position, _)| position)
            .max(self.skip.load(Acquire));
        let mut clock = lock(&self.clock);
        while clock.marks.get(1).is_some_and(|(start, ..)| *start <= keep) {
            clock.marks.pop_front();
        }
        clock.marks.push_back((write, time, step));
        drop(clock);
        self.write.store(write + count, Release);
        count
    }

    // WARNING: call only without an audio output, since this moves `read`, which otherwise only the audio callback
    // does.
    fn drop_played(&self) {
        let Some((_, channels)) = self.ring.get() else {
            return;
        };
        let clock = lock(&self.clock);
        let now = self.now(&clock);
        let write = self.write.load(Acquire);
        let mut read = self.read.load(Relaxed).max(self.skip.load(Acquire));
        for (index, &(start, time, step)) in clock.marks.iter().enumerate() {
            let end = clock.marks.get(index + 1).map_or(write, |(next, ..)| *next);
            if end > read && time + ((end - start) / channels) as f64 * step > now {
                break;
            }
            read = read.max(end);
        }
        self.read.store(read, Release);
    }
}

struct Mixer {
    rate: u32,
    channels: usize,
    retired: Mutex<Vec<(Arc<Sound>, u64)>>,
    control: mpsc::Sender<bool>,
}

impl Mixer {
    fn set(&self, sound: &Arc<Sound>, on: bool) {
        let mut retired = lock(&self.retired);
        let idle = PLAYING.iter().all(|slot| slot.load(Relaxed).is_null());
        let pointer = Arc::as_ptr(sound).cast_mut();
        match PLAYING.iter().find(|slot| slot.load(Relaxed) == pointer) {
            Some(slot) if !on => {
                slot.store(std::ptr::null_mut(), SeqCst);
                retired.push((unsafe { Arc::from_raw(pointer) }, MIXING.load(SeqCst)));
            }
            None if on => {
                if let Some(slot) = PLAYING.iter().find(|slot| slot.load(Relaxed).is_null()) {
                    slot.store(Arc::into_raw(sound.clone()).cast_mut(), SeqCst);
                }
            }
            _ => {}
        }
        let mixing = MIXING.load(SeqCst);
        retired.retain(|(_, at)| at % 2 == 1 && *at == mixing);
        if idle != PLAYING.iter().all(|slot| slot.load(Relaxed).is_null()) {
            let _ = self.control.send(idle);
        }
    }
}

fn mixer() -> Option<&'static Mixer> {
    static MIXER: OnceLock<Option<Mixer>> = OnceLock::new();
    MIXER
        .get_or_init(|| {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            // WARNING: sets the time base here, so the audio callback never waits to set it.
            ticks(Instant::now());
            let (ready, config) = mpsc::channel();
            let (control, commands) = mpsc::channel::<bool>();
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
                            let now = ticks(Instant::now());
                            MIXING.fetch_add(1, SeqCst);
                            for slot in &PLAYING {
                                let sound = slot.load(SeqCst);
                                if !sound.is_null() {
                                    unsafe { &*sound }.mix(out, now);
                                }
                            }
                            MIXING.fetch_add(1, SeqCst);
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
                retired: Mutex::default(),
                control,
            })
        })
        .as_ref()
}

#[derive(Default)]
struct State {
    playing: bool,
    ready: bool,
    closed: bool,
    seek: Option<f64>,
    speed: Option<f64>,
    generation: u64,
    frames: VecDeque<Pixels>,
    spare: Vec<Pixels>,
    show: bool,
    woken: bool,
    events: Vec<String>,
    duration: Option<f64>,
}

struct Shared {
    state: Mutex<State>,
    signal: Condvar,
    sound: Arc<Sound>,
}

impl Shared {
    fn update(&self, state: &State) {
        let running = state.playing && state.ready && !state.closed;
        if lock(&self.sound.clock).running != running {
            self.sound.set(None, Some(running), None);
            if let Some(mixer) = mixer() {
                mixer.set(&self.sound, running);
            }
        }
    }
}

fn json(text: &str) -> String {
    let mut json = String::with_capacity(text.len() + 2);
    json.push('"');
    for character in text.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            character if character < ' ' => {
                let _ = write!(json, "\\u{:04x}", character as u32);
            }
            character => json.push(character),
        }
    }
    json.push('"');
    json
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
    let output = mixer().map(|mixer| (mixer.rate, mixer.channels));
    let (rate, channels) = output.unwrap_or((8000, 1));
    let event = |text: String| {
        lock(&shared.state).events.push(text);
        waker.wake();
    };
    let fail = |message: String| {
        let mut state = lock(&shared.state);
        state.ready = false;
        shared.update(&state);
        state.events.push(format!("\"error\",3,{}", json(&message)));
        waker.wake();
    };
    let sound = &shared.sound;
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
        return event(format!(
            "\"error\",4,{}",
            json(&format!("Cannot open the media: {message}"))
        ));
    }
    let duration = info.duration;
    lock(&shared.state).duration = Some(duration);
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
    if has_audio {
        let length = ((rate as usize * channels) as f64 * AHEAD) as usize;
        let ring = (0..length.next_power_of_two()).map(|_| AtomicU32::new(0));
        sound.ring.get_or_init(|| (ring.collect(), channels));
    }
    let samples_per_second = (rate as usize * channels).max(1) as f64;
    let (mut video_done, mut audio_done) = (!has_video, !has_audio);
    let (mut speed, mut generation) = (1., 0);
    let (mut audio_end, mut last_frame) = (0., -1. / 30.);
    let (mut target, mut loaded) = (Some(0.), false);
    let (mut pending, mut pending_time, mut pending_step) = (Vec::<f32>::new(), 0., 0.);
    let mut skipped = None;
    let (mut blocked, mut failures, mut converted) = (0, 0, f64::NEG_INFINITY);
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
            (audio_end, last_frame, target, skipped) = (time, time - 1. / 30., Some(time), None);
            (blocked, converted) = (0, f64::NEG_INFINITY);
            pending.clear();
            continue;
        }
        if let Some(value) = state.speed.take() {
            speed = value;
            unsafe { apps_ffmpeg_speed(media, value) };
        }
        if output.is_none() {
            sound.drop_played();
        }
        if !pending.is_empty() && state.generation == generation {
            let taken = sound.push(&pending, pending_time, pending_step);
            pending.drain(..taken);
            pending_time += (taken / channels) as f64 * pending_step;
        }
        let queued = sound.queued() as f64 / samples_per_second;
        let buffered = queued + pending.len() as f64 / samples_per_second;
        let ready = (video_done || !state.frames.is_empty() || blocked & 1 != 0)
            && (audio_done || buffered >= AHEAD / 2. || blocked & 2 != 0);
        if target.is_some() && ready && state.generation == generation {
            target = None;
            state.ready = true;
            state.show = true;
            state
                .events
                .push(if loaded { "\"seeked\"" } else { "\"loaded\"" }.into());
            loaded = true;
            shared.update(&state);
            waker.wake();
        }
        let want = (c_int::from(!video_done && state.frames.len() < FRAMES)
            | c_int::from(!audio_done && buffered < AHEAD && pending.is_empty()) << 1)
            & !blocked;
        if want != 0 && (state.playing || target.is_some()) {
            drop(state);
            let (mut time, mut width, mut height) = (0., 0, 0);
            let (mut samples, mut count) = (std::ptr::null(), 0);
            let mut kind = unsafe {
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
            let last = kind == 0 && want & 1 != 0 && skipped.is_some();
            if last && let Some(frame) = skipped.take() {
                (time, width, height, kind) = frame;
            }
            match kind {
                1 => {
                    blocked = 0;
                    let time = if time.is_finite() {
                        time
                    } else {
                        last_frame + 1. / 30.
                    };
                    let next = time + (time - last_frame).max(0.);
                    last_frame = time;
                    if !last && target.is_some_and(|target| time < target - 0.02) {
                        skipped = Some((time, width, height, 1));
                        continue;
                    }
                    skipped = None;
                    let (now, running) = sound.time();
                    if running && next <= now && time - converted < 0.1 {
                        continue;
                    }
                    let (width, height) = (width as u32, height as u32);
                    let spare = lock(&shared.state)
                        .spare
                        .pop()
                        .filter(|pixels| pixels.width == width && pixels.height == height);
                    let pixels = spare.or_else(|| {
                        let bytes = (host().alloc)(width as usize * height as usize * 4);
                        (!bytes.is_null()).then_some(Pixels {
                            bytes,
                            width,
                            height,
                            time,
                        })
                    });
                    let Some(mut pixels) = pixels
                        .filter(|pixels| unsafe { apps_ffmpeg_pixels(media, pixels.bytes) >= 0 })
                    else {
                        failures += 1;
                        if failures == 3 {
                            fail("Cannot convert the video frames".into());
                            break;
                        }
                        continue;
                    };
                    (pixels.time, failures, converted) = (time, 0, time);
                    let mut state = lock(&shared.state);
                    if state.generation == generation {
                        state.frames.push_back(pixels);
                    } else {
                        state.spare.push(pixels);
                    }
                }
                2 => {
                    blocked = 0;
                    let mut time = if time.is_finite() { time } else { audio_end };
                    let step = speed / rate as f64;
                    let mut samples =
                        unsafe { std::slice::from_raw_parts(samples, count as usize * channels) };
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
                        // WARNING: FFmpeg reuses the sample buffer, so what doesn't fit in the ring waits in a copy.
                        let taken = sound.push(samples, time, step);
                        pending.extend_from_slice(&samples[taken..]);
                        (pending_time, pending_step) =
                            (time + (taken / channels) as f64 * step, step);
                    }
                }
                3 => {}
                4 => blocked = want,
                0 => {
                    video_done |= want & 1 != 0;
                    audio_done |= want & 2 != 0;
                }
                error => {
                    fail(format!("Cannot decode the media ({error})"));
                    break;
                }
            }
            continue;
        }
        let mut wait = None::<f64>;
        let mut until = |seconds: f64| wait = Some(wait.map_or(seconds, |wait| wait.min(seconds)));
        if state.playing && state.ready {
            let (now, _) = sound.time();
            if video_done && audio_done && state.frames.is_empty() && buffered == 0. {
                state.playing = false;
                shared.update(&state);
                sound.set(
                    Some(if duration.is_finite() { duration } else { now }),
                    None,
                    None,
                );
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
            if !audio_done && blocked & 2 == 0 {
                until((queued - AHEAD / 2.).max(0.005));
            } else if buffered > 0. {
                until(queued + 0.005);
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
                    .wait_timeout(
                        state,
                        Duration::try_from_secs_f64(wait).unwrap_or(Duration::MAX),
                    )
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
            state.spare.clear();
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
        sound: Arc::new(Sound::new()),
    });
    let waker = Waker((host().waker)());
    let player = shared.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("apps-media".into())
        .spawn(move || run(player, source, video, waker))
    {
        lock(&shared.state)
            .events
            .push(format!("\"error\",4,{}", json(&error.to_string())));
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
            let end = state
                .duration
                .filter(|duration| *duration >= 0.)
                .unwrap_or(f64::INFINITY)
                .min(1e9);
            let time = if value.is_finite() {
                value.max(0.).min(end)
            } else {
                0.
            };
            state.generation += 1;
            state.seek = Some(time);
            state.ready = false;
            let State { frames, spare, .. } = &mut *state;
            spare.extend(frames.drain(..));
            state.woken = false;
            shared.sound.flush();
            shared.sound.set(Some(time), None, None);
        }
        VOLUME | MUTED => {
            if op == VOLUME {
                player.volume = if value >= 0. {
                    value.min(1.) as f32
                } else {
                    0.
                };
            } else {
                player.muted = value != 0.;
            }
            let gain = if player.muted { 0. } else { player.volume };
            shared.sound.gain.store(gain.to_bits(), Relaxed);
        }
        // WARNING: the resampler rate is the output rate divided by the speed as an int, so speeds stay in 1/16 to 16,
        // like Chromium.
        SPEED if value > 0. && value.is_finite() => {
            let value = value.clamp(1. / 16., 16.);
            state.speed = Some(value);
            shared.sound.set(None, None, Some(value));
        }
        _ => {}
    }
    shared.update(&state);
    shared.signal.notify_one();
}

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
        .map_or(0., |player| player.shared.sound.time().0)
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
            && (state.show || head.time <= player.shared.sound.time().0)
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
    let now = player.shared.sound.time().0;
    let mut shown = None;
    while let Some(head) = state.frames.front()
        && (state.show || head.time <= now)
    {
        state.show = false;
        let next = state.frames.pop_front();
        if let Some(late) = std::mem::replace(&mut shown, next) {
            state.spare.push(late);
        }
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

/// Gives the pixels of a frame from `apps_media_frame` back to element `id`, so its next frames reuse
/// them. They are freed when the element is closed or keeps enough buffers.
///
/// # Safety
/// `media` comes from `apps_media_new`, and `pixels` comes from `apps_media_frame` with its size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_media_recycle(
    media: *mut c_void,
    id: u64,
    pixels: *mut u8,
    width: u32,
    height: u32,
) {
    let media = unsafe { &*media.cast::<Media>() };
    let pixels = Pixels {
        bytes: pixels,
        width,
        height,
        time: 0.,
    };
    if let Some(player) = media.players.get(&id) {
        let mut state = lock(&player.shared.state);
        if state.spare.len() < FRAMES {
            state.spare.push(pixels);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_escape_text_as_json() {
        let text = json("a\"b\\c\n\u{1b}é\u{2028}");
        assert_eq!(text, "\"a\\\"b\\\\c\\u000a\\u001bé\u{2028}\"");
        assert!(text.starts_with('"') && text.ends_with('"'));
    }

    #[test]
    fn sounds_queue_play_and_flush_in_whole_frames() {
        let sound = Sound::new();
        sound
            .ring
            .get_or_init(|| ((0..8).map(|_| AtomicU32::new(0)).collect(), 2));
        assert_eq!(sound.push(&[1.; 6], 1., 0.5), 6);
        assert_eq!(sound.push(&[2.; 6], 2.5, 0.5), 2);
        assert_eq!(sound.queued(), 8);
        sound.set(None, Some(true), None);
        let mut out = [0.; 4];
        sound.mix(&mut out, ticks(Instant::now()));
        assert_eq!(out, [1.; 4]);
        let (now, running) = sound.time();
        assert!(running && (1. ..1.25).contains(&now), "{now}");
        assert_eq!(sound.push(&[3.; 6], 2.5, 0.5), 4);
        sound.flush();
        assert_eq!(sound.queued(), 0);
        let mut out = [0.; 4];
        sound.mix(&mut out, ticks(Instant::now()));
        assert_eq!(out, [0.; 4]);
        assert_eq!(sound.push(&[4.; 2], 7., 0.5), 2);
        sound.set(Some(7.), None, None);
        sound.mix(&mut out, ticks(Instant::now()));
        assert_eq!(out, [4., 4., 0., 0.]);
        assert!((7. ..7.25).contains(&sound.time().0));
    }

    #[test]
    fn seeks_stay_in_the_media() {
        let mut media = Media::default();
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            signal: Condvar::new(),
            sound: Arc::new(Sound::new()),
        });
        media.players.insert(
            1,
            Player {
                shared: shared.clone(),
                video: false,
                volume: 1.,
                muted: false,
            },
        );
        for (duration, value, expected) in [
            (None, 1e300, 1e9),
            (Some(2.5), 1e300, 2.5),
            (Some(f64::INFINITY), -1., 0.),
            (Some(f64::NAN), 3., 3.),
            (Some(2.5), f64::NAN, 0.),
        ] {
            lock(&shared.state).duration = duration;
            unsafe { apps_media_control((&raw mut media).cast(), 1, SEEK, value) };
            assert_eq!(lock(&shared.state).seek, Some(expected));
            assert_eq!(shared.sound.time().0, expected);
        }
    }
}
