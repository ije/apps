//! Media elements need FFmpeg from scripts/build-ffmpeg.sh. Without an audio output, audio keeps its time silently.

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

// WARNING: CoreAudio reads the folder of the executable once per process, and target/debug/deps can make that take seconds.
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
            [
                u32::from(image.size().width),
                u32::from(image.size().height)
            ],
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
        [
            u32::from(image.size().width),
            u32::from(image.size().height)
        ],
        [8192, 32]
    );
}

#[test]
fn media_features_are_disabled_by_default() {
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
            .contains(r#"Add "video" to "features""#)
    );
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
