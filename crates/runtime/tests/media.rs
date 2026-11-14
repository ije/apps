use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};

fn page(disabled: u32) -> (Rc<RefCell<Host>>, JsContext, String) {
    let host = Rc::new(RefCell::new(Host::default()));
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media/");
    let base = url::Url::from_directory_path(&folder).unwrap();
    {
        let mut host = host.borrow_mut();
        host.disabled = disabled;
        host.pages.insert(
            1,
            Page {
                doc: Document::parse("<html><body></body></html>"),
                dirty: false,
            },
        );
        host.dom.entry(1).or_default().base = Some(base.clone());
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
fn wait(engine: &JsContext, ready: &str) -> String {
    for _ in 0..30000 {
        engine.tick().unwrap();
        let value = engine.eval(ready, "media-test").unwrap();
        if value != "pending" {
            return value;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("Media did not finish: {ready}");
}

const EVENTS: &str = "globalThis.events = []; for (const type of ['loadstart', 'durationchange', 'resize', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'play', 'playing', 'pause', 'seeking', 'seeked', 'ended', 'error', 'emptied']) media.addEventListener(type, () => events.push(type));";

#[test]
fn videos_decode_frames_seek_and_end() {
    let (host, engine, base) = page(0);
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
        let image = host.borrow().dom[&1].images[&id].clone();
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
            &host.borrow().dom[&1].images[&id]
        ));
    }
}

#[test]
fn seeking_past_the_last_frame_shows_it() {
    let (host, engine, base) = page(0);
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
    let first = host.borrow().dom[&1].images[&id].clone();
    engine
        .eval("media.currentTime = media.duration", "media-test")
        .unwrap();
    wait(&engine, "media.seeking ? 'pending' : 'done'");
    assert!(!std::sync::Arc::ptr_eq(
        &first,
        &host.borrow().dom[&1].images[&id]
    ));
}

#[test]
fn audio_plays_to_the_end_and_loops() {
    let (_host, engine, base) = page(0);
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
fn audio_survives_layout_and_rate_changes() {
    let (_host, engine, base) = page(0);
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
    let (host, engine, base) = page(0);
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
    let image = host.borrow().dom[&1].images[&id].clone();
    assert_eq!(
        [image.size().width as u32, image.size().height as u32],
        [8192, 32]
    );
}

#[test]
fn videos_need_the_media_feature() {
    let (_host, engine, base) = page(apps_runtime::bundle::disabled_features(None).unwrap());
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
    let (_host, engine, base) = page(apps_runtime::bundle::disabled_features(None).unwrap());
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
                &format!("globalThis.media = document.createElement('{element}'); media.src = '{base}{file}'"),
                "media-test",
            )
            .unwrap();
        let error = wait(&engine, "media.error?.message ?? 'pending'");
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn broken_media_reports_an_error() {
    let (_host, engine, _) = page(0);
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
