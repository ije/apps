use apps_runtime::{
  dom::Document, host::Host, host::context::JsContext, jsc::JavaScriptCore, jsc::Role, render::Page,
};
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};

fn page(features: apps_runtime::bundle::Features) -> (Rc<RefCell<Host>>, JsContext, String) {
  let host = Rc::new(RefCell::new(Host::default()));
  let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/");
  let base = url::Url::from_directory_path(&folder).unwrap();
  {
    let mut host = host.borrow_mut();
    host.update_app(|app| app.features = features);
    let mut store = host.store.borrow_mut();
    store.pages.insert(
      1,
      Page {
        doc: Document::parse("<html><body></body></html>"),
        dirty: false,
        ..Default::default()
      },
    );
    store.state(1).base = Some(base.clone());
  }
  let engine = JsContext::new(
    JavaScriptCore::load().unwrap(),
    host.clone(),
    Role::Renderer(1),
  )
  .unwrap();
  (host, engine, base.to_string())
}

// Allow CoreAudio's one-time executable-folder scan to finish. target/debug/deps can make it take seconds.
// A value that starts with "pending" waits, and the last one shows in the panic.
fn wait(engine: &JsContext, ready: &str) -> String {
  let started = std::time::Instant::now();
  let mut value = String::new();
  for _ in 0..30000 {
    engine.tick().unwrap();
    value = engine.eval(ready, "media-test").unwrap();
    if !value.starts_with("pending") {
      return value;
    }
    std::thread::sleep(Duration::from_millis(2));
  }
  panic!(
    "Media did not finish after {:?}: {ready} is {value}",
    started.elapsed()
  );
}

const EVENTS: &str = "globalThis.events = []; for (const type of ['loadstart', 'durationchange', 'resize', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'play', 'playing', 'pause', 'seeking', 'seeked', 'ended', 'error', 'emptied']) media.addEventListener(type, () => events.push(type));";

#[test]
fn videos_decode_frames_seek_and_end() {
  let (host, engine, base) = page(apps_runtime::bundle::Features::default());
  for (file, codecs) in [
    ("clip.mp4", "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""),
    ("clip.webm", "video/webm; codecs=\"vp9, opus\""),
  ] {
    engine
            .eval(
                &format!(
                    "globalThis.media = document.createElement('video'); document.body.replaceChildren(media); {EVENTS} media.src = '{base}{file}'; media.canPlayType('{}')",
                    codecs.replace('"', "\\\"")
                ),
                "media-test",
            )
            .map(|value| assert_eq!(value, "probably"))
            .unwrap();
    assert_eq!(
      wait(
        &engine,
        "media.readyState === 4 ? [media.duration.toFixed(1), media.videoWidth, media.videoHeight, media.currentTime].join() : media.error?.message ?? 'pending'"
      ),
      "1.0,64,48,0"
    );
    let id: usize = engine
      .eval("media._id", "media-test")
      .unwrap()
      .parse()
      .unwrap();
    let image = host.borrow().store.borrow().pages[&1].state.media.images[&id].clone();
    assert_eq!(
      [image.size().width as u32, image.size().height as u32],
      [64, 48]
    );
    for (value, expected) in image.bytes()[..4].iter().zip([32, 128, 224, 255]) {
      assert!(value.abs_diff(expected) <= 6, "{:?}", &image.bytes()[..4]);
    }
    engine
      .eval("media.currentTime = 0.5", "media-test")
      .unwrap();
    assert_eq!(
      wait(
        &engine,
        "media.seeking ? 'pending' : media.currentTime.toFixed(1)"
      ),
      "0.5"
    );
    engine
      .eval(
        "media.play().then(() => globalThis.started = true)",
        "media-test",
      )
      .unwrap();
    assert_eq!(
      wait(
        &engine,
        "media.ended ? [started, media.paused, media.currentTime.toFixed(1)].join() : 'pending'"
      ),
      "true,true,1.0"
    );
    assert_eq!(
      wait(
        &engine,
        "events.includes('ended') ? events.join() : 'pending'"
      ),
      "loadstart,durationchange,resize,loadedmetadata,loadeddata,canplay,canplaythrough,seeking,seeked,play,playing,pause,ended"
    );
    assert!(!std::sync::Arc::ptr_eq(
      &image,
      &host.borrow().store.borrow().pages[&1].state.media.images[&id]
    ));
  }
}

#[test]
fn seeking_past_the_last_frame_shows_it() {
  let (host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
        .eval(
            &format!("globalThis.media = document.createElement('video'); document.body.replaceChildren(media); media.src = '{base}clip.mp4'"),
            "media-test",
        )
        .unwrap();
  wait(&engine, "media.readyState === 4 ? 'ready' : 'pending'");
  let id: usize = engine
    .eval("media._id", "media-test")
    .unwrap()
    .parse()
    .unwrap();
  let first = host.borrow().store.borrow().pages[&1].state.media.images[&id].clone();
  engine
    .eval("media.currentTime = media.duration", "media-test")
    .unwrap();
  wait(&engine, "media.seeking ? 'pending' : 'done'");
  assert!(!std::sync::Arc::ptr_eq(
    &first,
    &host.borrow().store.borrow().pages[&1].state.media.images[&id]
  ));
}

#[test]
fn audio_plays_to_the_end_and_loops() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
        .eval(
            &format!("globalThis.media = new Audio('{base}tone.mp3'); {EVENTS} globalThis.loops = 0; media.addEventListener('seeked', () => {{ if (++loops === 2) media.loop = false; }}); media.loop = true; media.play()"),
            "media-test",
        )
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "media.ended ? [loops, media.duration > 0.9, media instanceof HTMLAudioElement].join() : media.error?.message ?? 'pending'"
    ),
    "2,true,true"
  );
}

#[test]
fn audio_params_clamp_and_periodic_waves_check_their_arrays() {
  let (_host, engine, _base) = page(apps_runtime::bundle::Features::NONE);
  let script = "(() => {
        const context = new OfflineAudioContext(1, 128, 8000), frequency = context.createOscillator().frequency;
        frequency.setValueAtTime(100, 0).linearRampToValueAtTime(10000, 1);
        const values = [frequency._at(0.25), frequency._at(0.5), ...frequency._run(0, 4, 4)];
        frequency.cancelScheduledValues(0); frequency.value = 99999; values.push(frequency.value);
        const error = fn => { try { fn(); return 'none'; } catch (e) { return e.name; } };
        const wave = new PeriodicWave(context), real = new PeriodicWave(context, { real: [5, 2, 3] });
        const osc = context.createOscillator(), filter = context.createBiquadFilter();
        values.push(osc.detune.maxValue, -osc.detune.minValue, filter.detune.maxValue, filter.gain.maxValue.toFixed(3), filter.gain.minValue < -1e38);
        osc.detune.value = 1e6; values.push(osc.detune.value);
        values.push((1 / new PeriodicWave(context, { imag: [0, 1, 1] })._scale).toFixed(3), new PeriodicWave(context, { imag: [0, 1, 1], disableNormalization: true })._scale);
        return [...values, error(() => context.createPeriodicWave(new Float32Array(3), new Float32Array(4))), error(() => context.createPeriodicWave([0], [1])), error(() => new PeriodicWave(context, { imag: [1] })), error(() => context.createPeriodicWave(undefined, undefined)), error(() => context.createPeriodicWave([0, NaN], [0, 1])), (1 / new PeriodicWave(context, { imag: Array.from({ length: 40 }, (_, k) => k && 1 / k) })._scale).toFixed(3), ...wave._real, ...wave._imag, ...real._real, ...real._imag].join();
    })()";
  assert_eq!(
    engine.eval(script, "media-test").unwrap(),
    "2575,4000,100,2575,4000,4000,4000,153600,153600,153600,1541.274,true,153600,1.760,1,IndexSizeError,IndexSizeError,IndexSizeError,TypeError,TypeError,1.812,0,0,0,1,0,2,3,0,0,0"
  );
}

#[test]
fn web_audio_renders_oscillators_filters_and_automation_like_chrome() {
  let (_host, engine, _base) = page(apps_runtime::bundle::Features::NONE);
  let script = "(async () => {
        const offline = new OfflineAudioContext(1, 9600, 48000);
        const osc = offline.createOscillator(), gain = offline.createGain();
        osc.frequency.value = 1000;
        gain.gain.setValueAtTime(0, 0);
        gain.gain.linearRampToValueAtTime(1, 0.05);
        osc.connect(gain).connect(offline.destination);
        osc.start(0);
        osc.stop(0.1);
        const high = offline.createOscillator(), filter = offline.createBiquadFilter();
        high.frequency.value = 8000;
        filter.frequency.value = 500;
        high.connect(filter).connect(offline.destination);
        high.start(0.15);
        const data = (await offline.startRendering()).getChannelData(0);
        const peak = data.slice(8400).reduce((max, value) => Math.max(max, Math.abs(value)), 0);
        const param = new AudioContext().createGain().gain;
        param.setValueAtTime(0, 1).linearRampToValueAtTime(1, 2).exponentialRampToValueAtTime(0.01, 3);
        const response = [new Float32Array(1), new Float32Array(1)], probe = offline.createBiquadFilter();
        probe.frequency.value = 1000;
        probe.Q.value = 0;
        probe.getFrequencyResponse(Float32Array.of(1000), ...response);
        const live = new AudioContext(), tone = live.createOscillator();
        tone.connect(live.destination);
        tone.onended = () => globalThis.ended = true;
        tone.start();
        tone.stop(live.currentTime + 0.05);
        globalThis.result = [data[12], data[2412], peak < 0.05, param._at(0.5), param._at(1.5), param._at(2.5), response[0][0]].map(value => typeof value === 'number' ? value.toFixed(3) : value).join();
    })(); 'started'";
  assert_eq!(engine.eval(script, "media-test").unwrap(), "started");
  // Chrome's values: the ramp's start and end on the crests of the sine, a filtered tone, the automation timeline and
  // the gain of a low-pass filter at its cutoff.
  assert_eq!(
    wait(
      &engine,
      "globalThis.result && globalThis.ended ? result : 'pending'"
    ),
    "0.005,1.000,true,1.000,0.500,0.100,1.000"
  );
}

#[cfg(target_os = "macos")]
fn media_threads() -> usize {
  let pid = std::process::id() as i32;
  let mut ids = vec![0u64; 4096];
  let size = (ids.len() * 8) as i32;
  let bytes = unsafe {
    libc::proc_pidinfo(
      pid,
      6, // PROC_PIDLISTTHREADS
      0,
      ids.as_mut_ptr().cast(),
      size,
    )
  };
  ids[..bytes.max(0) as usize / 8]
    .iter()
    .filter(|&&id| {
      let mut info = unsafe { std::mem::zeroed::<libc::proc_threadinfo>() };
      let size = std::mem::size_of_val(&info) as i32;
      let read = unsafe {
        libc::proc_pidinfo(
          pid,
          libc::PROC_PIDTHREADINFO,
          id,
          (&raw mut info).cast(),
          size,
        )
      };
      read == size
        && unsafe { std::ffi::CStr::from_ptr(info.pth_name.as_ptr()) }.to_bytes() == b"apps-media"
    })
    .count()
}

#[cfg(target_os = "linux")]
fn media_threads() -> usize {
  std::fs::read_dir("/proc/self/task")
    .unwrap()
    .filter(|task| {
      std::fs::read_to_string(task.as_ref().unwrap().path().join("comm"))
        .is_ok_and(|name| name.trim() == "apps-media")
    })
    .count()
}

#[cfg(not(windows))]
#[test]
fn web_audio_sounds_free_their_players() {
  let (_host, engine, _base) = page(apps_runtime::bundle::Features::NONE);
  engine
        .eval(
            "globalThis.ended = 0; globalThis.live = new AudioContext();
            for (let i = 0; i < 40; i++) setTimeout(() => { const tone = live.createOscillator(); tone.connect(live.destination); tone.onended = () => { globalThis.twice ||= tone.ended; tone.ended = true; ended++; }; tone.start(); tone.stop(live.currentTime + 0.02); }, i * 5)",
            "media-test",
        )
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "globalThis.twice ? 'twice' : ended === 40 ? 'done' : `pending: ${ended} ended at ${live.currentTime.toFixed(1)} s`",
    ),
    "done"
  );
  // Other tests in this binary can hold a few players at the same time.
  for _ in 0..2500 {
    engine.tick().unwrap();
    if media_threads() < 20 {
      return;
    }
    std::thread::sleep(Duration::from_millis(2));
  }
  panic!(
    "{} media threads stay after the sounds end",
    media_threads()
  );
}

#[cfg(not(windows))]
#[test]
fn paused_players_hold_no_threads() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
    .eval(
      &format!(
        "globalThis.sounds = Array.from({{ length: 40 }}, () => new Audio('{base}tone.mp3'))"
      ),
      "media-test",
    )
    .unwrap();
  wait(
    &engine,
    "sounds.every(sound => sound.readyState === 4) ? 'ready' : 'pending'",
  );
  // Other tests in this binary can hold a few players at the same time.
  for _ in 0..2500 {
    engine.tick().unwrap();
    if media_threads() < 20 {
      break;
    }
    std::thread::sleep(Duration::from_millis(2));
  }
  assert!(
    media_threads() < 20,
    "{} media threads for paused players",
    media_threads()
  );
  engine
    .eval(
      "sounds[0].currentTime = 0.5; sounds[0].play(); sounds[1].play()",
      "media-test",
    )
    .unwrap();
  assert_eq!(
    wait(
      &engine,
      "sounds[0].ended && sounds[1].ended ? [sounds[0].error, sounds[1].currentTime > 0.9].join() : 'pending'"
    ),
    ",true"
  );
}

#[test]
fn audio_survives_layout_and_rate_changes() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
    .eval(
      &format!("globalThis.media = new Audio('{base}switch.mp3'); {EVENTS} media.play()"),
      "media-test",
    )
    .unwrap();
  assert_eq!(
    wait(
      &engine,
      "media.ended ? [media.error, media.currentTime > 0.9, events.includes('error')].join() : media.error?.message ?? 'pending'"
    ),
    ",true,false"
  );
}

#[test]
fn absurd_pixel_aspect_ratios_fit_the_frame_size_limit() {
  let (host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
        .eval(
            &format!("globalThis.media = document.createElement('video'); document.body.replaceChildren(media); media.src = '{base}wide.mp4'"),
            "media-test",
        )
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "media.readyState === 4 ? [media.videoWidth, media.videoHeight].join() : media.error?.message ?? 'pending'"
    ),
    "8192,32"
  );
  let id: usize = engine
    .eval("media._id", "media-test")
    .unwrap()
    .parse()
    .unwrap();
  let image = host.borrow().store.borrow().pages[&1].state.media.images[&id].clone();
  assert_eq!(
    [image.size().width as u32, image.size().height as u32],
    [8192, 32]
  );
}

#[test]
fn videos_need_the_media_feature() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::NONE);
  engine
        .eval(&format!("globalThis.media = document.createElement('video'); {EVENTS} media.src = '{base}clip.mp4'; globalThis.rejected = ''; media.play().catch(error => rejected = error.name)"), "media-test")
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "rejected ? [rejected, media.error.code, media.canPlayType('video/mp4'), events.join()].join() : 'pending'"
    ),
    "NotSupportedError,4,,loadstart,error"
  );
  assert!(
    engine
      .eval("media.error.message", "media-test")
      .unwrap()
      .contains(r#"Add "media" to "features""#)
  );
}

#[test]
fn audio_plays_only_wave_files_without_the_media_feature() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::NONE);
  engine
        .eval(
            &format!("globalThis.media = new Audio('{base}tone.wav'); {EVENTS} media.currentTime = 0.2; media.play()"),
            "media-test",
        )
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "media.ended ? [media.duration, media.currentTime, events.includes('seeked'), media.canPlayType('audio/wav; codecs=1'), media.canPlayType('audio/mpeg'), document.createElement('video').canPlayType('video/mp4')].join() : media.error?.message ?? 'pending'"
    ),
    "0.5,0.5,true,probably,,"
  );
  // 0.1 seconds of 8-bit silence, from memory.
  engine
        .eval(
            "globalThis.media = new Audio('data:audio/wav;base64,' + btoa(atob('UklGRkQDAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YSADAAA=') + '\\x80'.repeat(800)))",
            "media-test",
        )
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "media.readyState === 4 ? media.duration : media.error?.message ?? 'pending'"
    ),
    "0.1"
  );
  for (element, file, message) in [
    ("audio", "tone.mp3", "Only PCM WAV files play"),
    ("video", "clip.mp4", r#"Add "media" to "features""#),
  ] {
    engine
      .eval(
        &format!(
          "globalThis.media = document.createElement('{element}'); media.src = '{base}{file}'"
        ),
        "media-test",
      )
      .unwrap();
    let error = wait(&engine, "media.error?.message ?? 'pending'");
    assert!(error.contains(message), "{error}");
  }
}

#[test]
fn broken_media_reports_an_error() {
  let (_host, engine, _) = page(apps_runtime::bundle::Features::default());
  engine
        .eval(&format!("globalThis.media = document.createElement('audio'); {EVENTS} media.src = 'data:audio/mpeg;base64,AAAAAAAA'"), "media-test")
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "events.includes('error') ? [media.error.code, media.networkState, events.join()].join() : 'pending'"
    ),
    "4,3,loadstart,error"
  );
}

#[test]
fn sources_from_inner_html_start_the_load() {
  let (_host, engine, base) = page(apps_runtime::bundle::Features::default());
  engine
        .eval(&format!("globalThis.media = document.createElement('audio'); {EVENTS} media.innerHTML = '<source src=\"{base}tone.wav\">'"), "media-test")
        .unwrap();
  assert_eq!(
    wait(
      &engine,
      "events.includes('loadedmetadata') ? media.currentSrc.endsWith('/tone.wav') : 'pending ' + events.join()"
    ),
    "true"
  );
}
