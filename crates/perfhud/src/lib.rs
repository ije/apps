mod probe;

use std::{
    collections::VecDeque,
    ffi::c_void,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

// WARNING: changes with `Item` or a function signature, and must match the runtime.
const ABI: u32 = 1;
const CAPACITY: usize = 120;
const READOUT: Duration = Duration::from_millis(500);
const FPS_WINDOW: Duration = Duration::from_secs(1);
const RATES: usize = 10;
const AXIS_DECAY: f32 = 0.04;

const TEXT: f32 = 12.;
const CHAR: f32 = TEXT * 0.6;
const INNER: f32 = CHAR * 34.;
const VALUE: f32 = INNER - CHAR * 16.;
const ROW: f32 = 16.;
const PAD: f32 = 10.;
const MARGIN: f32 = 12.;
const GAP: f32 = 8.;
const CHART: f32 = 60.;
const LINE: f32 = 1.5;
const COLUMNS: usize = 8;

// WARNING: the page shows through the backdrop, so text colors stay bright to be readable over light pages.
const BACKGROUND: u32 = 0x0a0c10a6;
const FOREGROUND: u32 = 0xffffffff;
const RULE: u32 = 0xffffff59;
const TRACE: u32 = 0x2f9bffff;
const GOOD: u32 = 0x30d158ff;
const WARN: u32 = 0xff9f0aff;
const BAD: u32 = 0xff453aff;

#[repr(C)]
pub struct Item {
    pub bounds: [f32; 4],
    pub color: u32,
    pub size: f32,
    pub align: u32,
    pub text: *const u8,
    pub len: usize,
}

#[derive(Clone, Copy, Default)]
struct Readout {
    hz: Option<f32>,
    max: f32,
    fps: f32,
    fps_range: [f32; 2],
    frame: f32,
    frame_range: [f32; 2],
    p95: f32,
    dropped: f32,
}

pub struct Hud {
    frames: VecDeque<f32>,
    times: VecDeque<Instant>,
    rates: VecDeque<f32>,
    own: bool,
    recorded: bool,
    drawn: bool,
    readout: Readout,
    readout_at: Option<Instant>,
    budget: f32,
    axis: f32,
    usage: Arc<Mutex<Option<probe::Usage>>>,
    _stop: mpsc::Sender<()>,
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_perfhud_abi() -> u32 {
    ABI
}

#[unsafe(no_mangle)]
pub extern "C" fn apps_perfhud_new() -> *mut Hud {
    let usage = Arc::default();
    let (stop, stopped) = mpsc::channel();
    let shared = Arc::clone(&usage);
    let _ = std::thread::Builder::new()
        .name("perfhud".into())
        .spawn(move || probe::run(&shared, &stopped));
    Box::into_raw(Box::new(Hud {
        frames: VecDeque::with_capacity(CAPACITY),
        times: VecDeque::new(),
        rates: VecDeque::with_capacity(RATES),
        own: false,
        recorded: false,
        drawn: false,
        readout: Readout::default(),
        readout_at: None,
        budget: 1. / 60.,
        axis: 2. / 60.,
        usage,
        _stop: stop,
    }))
}

/// # Safety
/// `hud` comes from `apps_perfhud_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_perfhud_free(hud: *mut Hud) {
    drop(unsafe { Box::from_raw(hud) });
}

/// Runs every half second. Returns true when the page drew nothing since the last tick, so the runtime
/// draws one frame to update the readings. That frame is not a sample.
///
/// # Safety
/// `hud` comes from `apps_perfhud_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn apps_perfhud_tick(hud: *mut Hud) -> bool {
    let hud = unsafe { &mut *hud };
    let idle = !std::mem::take(&mut hud.drawn);
    hud.own |= idle;
    idle
}

/// Records a frame that took `draw` nanoseconds from render to paint, then paints the HUD at the top right
/// of a window of `width` by `height` pixels. `redrawing` is a second pass of the same frame.
///
/// # Safety
/// `hud` comes from `apps_perfhud_new`. `paint` gets `cx` and an item that lives during the call.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn apps_perfhud_paint(
    hud: *mut Hud,
    draw: u64,
    redrawing: bool,
    width: f32,
    height: f32,
    scale: f32,
    cx: *mut c_void,
    paint: extern "C" fn(*mut c_void, &Item),
) {
    let hud = unsafe { &mut *hud };
    let now = Instant::now();
    let own = if redrawing {
        !hud.recorded
    } else {
        std::mem::take(&mut hud.own)
    };
    if redrawing && hud.recorded {
        hud.frames.pop_back();
        hud.times.pop_back();
    }
    hud.recorded = !own;
    if !own {
        hud.drawn = true;
        if hud.frames.len() == CAPACITY {
            hud.frames.pop_front();
        }
        hud.frames.push_back(draw as f32 / 1e9);
        hud.times.push_back(now);
    }
    if own
        || hud
            .readout_at
            .is_none_or(|at| now.duration_since(at) >= READOUT)
    {
        hud.readout_at = Some(now);
        let refresh = probe::refresh_period();
        hud.budget = refresh.unwrap_or(1. / 60.);
        while hud
            .times
            .front()
            .is_some_and(|&at| now.duration_since(at) > FPS_WINDOW)
        {
            hud.times.pop_front();
        }
        let fps = hud.times.len() as f32;
        if hud.rates.len() == RATES {
            hud.rates.pop_front();
        }
        hud.rates.push_back(fps);
        let mut draws: Vec<f32> = hud.frames.iter().map(|draw| draw * 1e3).collect();
        draws.sort_unstable_by(f32::total_cmp);
        let count = draws.len().max(1) as f32;
        // WARNING: `sum()` of no floats is -0.
        let mean = draws.iter().fold(0., |sum, draw| sum + draw) / count;
        hud.readout = Readout {
            hz: refresh.map(|period| 1. / period),
            max: if mean > 0. {
                (1e3 / mean).min(1. / hud.budget)
            } else {
                0.
            },
            fps,
            fps_range: [
                hud.rates.iter().copied().fold(f32::INFINITY, f32::min),
                hud.rates.iter().copied().fold(0., f32::max),
            ],
            frame: mean,
            frame_range: [
                draws.first().copied().unwrap_or(0.),
                draws.last().copied().unwrap_or(0.),
            ],
            p95: draws
                .get((draws.len() * 95).div_ceil(100).saturating_sub(1))
                .copied()
                .unwrap_or(0.),
            dropped: draws
                .iter()
                .filter(|&&draw| draw > hud.budget * 1e3)
                .count() as f32
                / count
                * 100.,
        };
    }
    let floor = hud.budget * 2.;
    let peak = hud.frames.iter().copied().fold(floor, f32::max);
    hud.axis = if peak > hud.axis {
        peak
    } else {
        hud.axis + (peak - hud.axis) * AXIS_DECAY
    };

    let quad = |bounds, color, radius| {
        paint(
            cx,
            &Item {
                bounds,
                color,
                size: radius,
                align: 0,
                text: std::ptr::null(),
                len: 0,
            },
        );
    };
    let text = |[x, y, width]: [f32; 3], align, color, text: &str| {
        if !text.is_empty() {
            paint(
                cx,
                &Item {
                    bounds: [x, y, width, ROW],
                    color,
                    size: TEXT,
                    align,
                    text: text.as_ptr(),
                    len: text.len(),
                },
            );
        }
    };
    let grade = |draw: f32, good| {
        if draw <= hud.budget {
            good
        } else if draw <= hud.budget * 2. {
            WARN
        } else {
            BAD
        }
    };
    let Readout {
        hz,
        max,
        fps,
        fps_range,
        frame,
        frame_range,
        p95,
        dropped,
    } = hud.readout;
    let usage = *hud.usage.lock().unwrap_or_else(|error| error.into_inner());

    let left = width - MARGIN - INNER - PAD * 2.;
    quad(
        [
            left,
            MARGIN,
            INNER + PAD * 2.,
            PAD * 2. + ROW * 8. + GAP * 2. + CHART,
        ],
        BACKGROUND,
        8.,
    );
    let x = left + PAD;
    let mut y = MARGIN + PAD;
    text(
        [x, y, INNER],
        0,
        FOREGROUND,
        &format!("{width:.0}x{height:.0} @{scale}x"),
    );
    text(
        [x, y, INNER],
        2,
        GOOD,
        &hz.map_or(String::new(), |hz| format!("{hz:.0} Hz")),
    );
    y += ROW;
    // WARNING: 100% is one busy core, like Activity Monitor and top.
    let cpu = usage.map_or("-".into(), |usage| {
        if usage.cpu < 10. {
            format!("{:.1}%", usage.cpu)
        } else {
            format!("{:.0}%", usage.cpu)
        }
    });
    text([x, y, INNER], 0, FOREGROUND, &format!("CPU: {cpu}"));
    if let Some(gpu) = usage.and_then(|usage| usage.gpu) {
        text([x, y, INNER], 2, FOREGROUND, &format!("GPU: {gpu:.1}%"));
    }
    y += ROW;
    let memory = usage.map_or("-".into(), |usage| {
        let memory = usage.memory as f64 / (1 << 20) as f64;
        if memory >= 1024. {
            format!("{:.2} GB", memory / 1024.)
        } else {
            format!("{memory:.0} MB")
        }
    });
    text([x, y, INNER], 0, FOREGROUND, "Memory");
    text([x, y, INNER], 2, FOREGROUND, &memory);

    y += ROW + GAP;
    let rows = [
        (
            "FPS",
            format!("{fps:.0}"),
            FOREGROUND,
            format!("[{:>6.0} {:>6.0}]", fps_range[0], fps_range[1]),
        ),
        ("Max FPS", format!("{max:.0}"), FOREGROUND, String::new()),
        (
            "Frame",
            format!("{frame:.2}ms"),
            grade(frame / 1e3, FOREGROUND),
            format!("[{:>6.2} {:>6.2}]", frame_range[0], frame_range[1]),
        ),
        (
            "P95",
            format!("{p95:.2}ms"),
            grade(p95 / 1e3, FOREGROUND),
            String::new(),
        ),
        (
            "Dropped",
            format!("{dropped:.1}%"),
            if dropped > 0. { WARN } else { FOREGROUND },
            String::new(),
        ),
    ];
    for (label, value, color, range) in rows {
        text([x, y, INNER], 0, FOREGROUND, label);
        text([x, y, VALUE], 2, color, &value);
        text([x, y, INNER], 2, FOREGROUND, &range);
        y += ROW;
    }

    y += GAP;
    for column in 0..=COLUMNS {
        quad(
            [
                x + (INNER - 1.) * column as f32 / COLUMNS as f32,
                y,
                1.,
                CHART,
            ],
            RULE,
            0.,
        );
    }
    quad([x, y + CHART - 1., INNER, 1.], RULE, 0.);
    let level = |draw: f32| y + (CHART - LINE) * (1. - (draw / hud.axis).min(1.));
    quad([x, level(hud.budget), INNER, 1.], RULE, 0.);
    let slot = INNER / CAPACITY as f32;
    let lead = CAPACITY - hud.frames.len();
    let mut last = None;
    for (index, &draw) in hud.frames.iter().enumerate() {
        let top = level(draw);
        let previous = last.replace(top).unwrap_or(top);
        quad(
            [
                x + slot * (lead + index) as f32,
                previous.min(top),
                slot,
                (previous - top).abs() + LINE,
            ],
            grade(draw, TRACE),
            0.,
        );
    }
}
