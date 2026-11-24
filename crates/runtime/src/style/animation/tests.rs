use super::*;
use crate::css::{Easing, time};
use crate::dom::Document;
use crate::style::animation::effect::{after, tracks};
use crate::style::animation::interpolate::{Lerp, add, interpolate, lerp, transitions};
use std::time::Duration;

fn easing(value: &str, progress: f64) -> f64 {
    Easing::parse(value).unwrap().at(progress, false)
}

fn computed_styles(doc: &Document) -> ComputedStyles {
    super::super::compute_styles(
        doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut super::super::StyleCache::default(),
        None,
    )
}

#[test]
fn keyframes_follow_delay_easing_iterations_and_fill() {
    let doc = Document::parse(
        "<style>@keyframes pulse{from{opacity:1}50%{opacity:0}to{opacity:1}}div{animation:1.2s .8s infinite pulse}</style><div></div>",
    );
    let styles = computed_styles(&doc);
    let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let base = styles.values[id].clone();
    let sample = |effect: &Animation, elapsed: f64| {
        let mut out = Style::new();
        let (wake, _) = effect.sample(
            elapsed,
            effect.paused,
            &base,
            &Default::default(),
            &mut out,
            1.,
        );
        (
            out.get(Prop::OPACITY)
                .map(|value| value.parse::<f64>().unwrap()),
            wake.is_some(),
        )
    };
    let effect = &styles.animations[&id][0];
    assert_eq!(sample(effect, 0.4), (None, true));
    assert_eq!(sample(effect, 0.8), (Some(1.), true));
    assert!(sample(effect, 1.4).0.unwrap() < 0.0001);
    assert!((sample(effect, 1.1).0.unwrap() - 0.1976).abs() < 0.001);
    assert!((sample(effect, 2.3).0.unwrap() - sample(effect, 1.1).0.unwrap()).abs() < 0.001);
    let mut finite = effect.clone();
    finite.duration = 1.;
    finite.delay = -0.5;
    finite.iterations = 2.;
    finite.direction = 2;
    finite.fill = 3;
    assert_eq!(sample(&finite, 0.).0, Some(0.));
    assert_eq!(sample(&finite, 2.), (Some(1.), false));
    finite.paused = true;
    assert!(!sample(&finite, 0.).1);
    finite.duration = 0.;
    assert!(!sample(&finite, 0.).1);
    finite.iterations = f64::INFINITY;
    assert_eq!(sample(&finite, 0.), (Some(1.), false));
}

#[test]
fn keyframes_animate_many_properties_with_per_keyframe_easing() {
    let doc = Document::parse(
        "<style>@keyframes move{from{transform:translateX(0) rotate(0deg);background:red}50%{width:50%;animation-timing-function:linear}to{transform:translateX(100px) rotate(90deg);background:blue;width:100px}}@keyframes spin{to{transform:rotate(360deg)}}div{width:10px;animation:move 2s linear}p{animation:spin 1s linear infinite}</style><div></div><p></p>",
    );
    let styles = computed_styles(&doc);
    let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    let mut out = Style::new();
    let sample = |id: usize, elapsed: f64, out: &mut Style| {
        styles.animations[&id][0].sample(
            elapsed,
            false,
            &styles.values[id],
            &Default::default(),
            out,
            1.,
        )
    };
    sample(div, 1., &mut out);
    assert_eq!(&out["transform"], "translateX(50px) rotate(45deg)");
    assert_eq!(&out["background-color"], "#800080ff");
    assert_eq!(&out["width"], "50%");
    out = Style::new();
    sample(div, 0.5, &mut out);
    assert_eq!(&out["width"], "calc(10px * 0.5 + 50% * 0.5)");
    out = Style::new();
    sample(p, 0.25, &mut out);
    assert_eq!(&out["transform"], "rotate(90deg)");
}

#[test]
fn implicit_end_frames_follow_the_value_under_them() {
    let doc = Document::parse(
        "<style>@keyframes grow{to{width:100px}}div{width:0px;animation:grow 1s linear}</style><div></div>",
    );
    let styles = computed_styles(&doc);
    let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let effect = &styles.animations[&div][0];
    let sample = |under: Option<&str>| {
        let mut out = Style::new();
        if let Some(under) = under {
            out.insert(Prop::WIDTH, under);
        }
        effect.sample(
            0.5,
            false,
            &styles.values[div],
            &Default::default(),
            &mut out,
            1.,
        );
        out["width"].to_owned()
    };
    let filled = || effect.tracks[0].1[0].implicit.0.borrow().clone().unwrap();
    assert_eq!(sample(None), "50px");
    let first = filled();
    assert_eq!(sample(None), "50px");
    assert!(Rc::ptr_eq(&first, &filled()));
    assert_eq!(sample(Some("20px")), "60px");
    assert_eq!(sample(None), "50px");
    for index in 0..2000 {
        lerp("width", &format!("{index}px"), "1px");
    }
    assert_eq!(sample(Some("40px")), "70px");
}

#[test]
fn values_interpolate_by_type() {
    let mix = |property: &str, from: &str, to: &str, t: f64| {
        interpolate(lerp(property, from, to).as_deref(), from, to, t).map(|v| v.to_string())
    };
    assert_eq!(mix("color", "red", "blue", 0.5).unwrap(), "#800080ff");
    assert_eq!(
        mix("color", "transparent", "red", 0.5).unwrap(),
        "#ff000080"
    );
    assert_eq!(
        mix("color", "oklch(0.205 0 0)", "oklch(0.922 0 0)", 0.5).unwrap(),
        "oklab(0.5635 0 0)"
    );
    assert_eq!(
        mix("color", "transparent", "oklab(1 0 0 / 0.04)", 0.5).unwrap(),
        "oklab(1 0 0 / 0.02)"
    );
    assert_eq!(
        mix("color", "#000", "oklab(1 none 0)", 0.5).unwrap(),
        "oklab(0.5 0 0)"
    );
    assert_eq!(mix("width", "0", "10px", 0.5).unwrap(), "5px");
    assert_eq!(mix("opacity", "1", "0", 0.25).unwrap(), "0.75");
    assert_eq!(mix("opacity", "0%", "1", 0.5).unwrap(), "0.5");
    assert_eq!(
        mix("scale", "50% 50%", "100% 100%", 0.5).unwrap(),
        "0.75 0.75"
    );
    assert_eq!(mix("z-index", "1", "4", 0.4).unwrap(), "2");
    assert_eq!(mix("width", "auto", "10px", 0.5), None);
    assert_eq!(
        mix("transform", "none", "scale(2) rotate(1turn)", 0.5).unwrap(),
        "scale(1.5) rotate(0.5turn)"
    );
    assert_eq!(
        mix("translate", "none", "10px 20px", 0.5).unwrap(),
        "5px 10px"
    );
    assert_eq!(
        mix("transform", "rotate(10deg)", "scale(2)", 0.5).unwrap(),
        "matrix(1.49429, 0.130734, -0.130734, 1.49429, 0, 0)"
    );
    assert_eq!(
        mix(
            "transform",
            "translateY(10px) scale(0.95)",
            "translateY(0)",
            0.5
        )
        .unwrap(),
        "translateY(5px) scale(0.975)"
    );
    assert_eq!(
        mix(
            "transform",
            "translateX(10px)",
            "translate(0px, 20px) rotate(90deg)",
            0.5
        )
        .unwrap(),
        "translate(5px, 10px) rotate(45deg)"
    );
    assert_eq!(
        mix(
            "transform",
            "rotateX(12deg) skewX(4deg)",
            "skewX(2deg)",
            0.5
        )
        .unwrap(),
        "matrix3d(1, 0, 0, 0, 0.0524238, 0.994522, 0.104528, 0, 0, -0.104528, 0.994522, 0, 0, 0, 0, 1)"
    );
    assert_eq!(
        mix("transform", "translateX(10%)", "rotate(1deg)", 0.5),
        None
    );
    assert_eq!(
        mix("transform", "  ", " rotateX(10deg)   skewX(4deg) ", 0.5).unwrap(),
        "rotateX(5deg) skewX(2deg)"
    );
    assert!(
        mix(
            "transform",
            "  rotateZ(45deg)  ",
            "rotateX(10deg) rotateZ(90deg) skewX(2deg)",
            0.5
        )
        .is_some_and(|value| value.starts_with("matrix3d("))
    );
    assert_eq!(mix("display", "flex", "none", 0.9).unwrap(), "flex");
    assert_eq!(mix("display", "flex", "block", 0.4), None);
    assert_eq!(mix("font-size", "16px", "32px", 0.25).unwrap(), "20px");
    let gradients = [
        "linear-gradient(red, blue)",
        "linear-gradient(green, yellow)",
    ];
    assert_eq!(
        mix("background-image", gradients[0], gradients[1], 0.3),
        None
    );
    assert_eq!(
        mix("box-shadow", "none", "0 0 10px red", 0.5).unwrap(),
        "0px 0px 5px 0px #ff000080"
    );
    assert_eq!(
        mix("background-position", "0% 0%", "100% 50%", 0.5).unwrap(),
        "50% 25%"
    );
    assert_eq!(
        mix("visibility", "hidden", "visible", 0.1).unwrap(),
        "visible"
    );
    let units = super::super::Lengths::new(16., 800., 600.);
    assert_eq!(
        units.resolve(&mix("left", "10px", "50%", 0.5).unwrap(), 200.),
        Some(55.)
    );
    assert_eq!(
        units.resolve(&mix("left", "calc(100% - 10px)", "0px", 0.5).unwrap(), 200.),
        Some(95.)
    );
}

#[test]
fn animated_styles_drop_values_that_stop_animating() {
    let doc = Document::parse(
        "<style>@keyframes w{from{width:0px}to{width:100px}}@keyframes h{from{height:0px}to{height:10px}}div{height:3px;animation:w 1s linear,h 0.5s linear}</style><div></div>",
    );
    let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let styles = computed_styles(&doc);
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut style = |seconds: f64| {
        let (animated, _) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            start + Duration::from_secs_f64(seconds),
            false,
            &mut Vec::new(),
        );
        let style = animated.into_iter().find(|entry| entry.0 == id).unwrap().1;
        [Prop::WIDTH, Prop::HEIGHT].map(|prop| style.get(prop).map(str::to_owned))
    };
    style(0.);
    assert_eq!(style(0.25), [Some("25px".into()), Some("5px".into())]);
    assert_eq!(style(0.3), [Some("30px".into()), Some("6px".into())]);
    assert_eq!(style(0.75), [Some("75px".into()), Some("3px".into())]);
}

#[test]
fn transitions_start_reverse_and_finish() {
    let mut doc = Document::parse(
        "<style>div{color:blue;transition:color 1s linear,width 1s}div.on{color:red;width:20px}</style><div>text</div>",
    );
    let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let text = doc.nodes[id].children[0];
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    let mut events = Vec::new();
    let mut update = |motion: &mut Motion, doc: &Document, styles: &ComputedStyles, time: f64| {
        let (animated, next) = motion.update(
            doc,
            styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            at(time),
            false,
            &mut events,
        );
        let running = next.is_some();
        let values: HashMap<usize, Rc<Style>> = animated
            .into_iter()
            .map(|(id, style, _)| (id, style))
            .collect();
        (values, running, events.last().map(|event| event.1))
    };
    let (values, running, _) = update(&mut motion, &doc, &computed_styles(&doc), 0.);
    assert!(values.is_empty() && !running);
    doc.nodes[id].attrs.insert("class".into(), "on".into());
    let on = computed_styles(&doc);
    let (values, running, _) = update(&mut motion, &doc, &on, 0.);
    assert!(running);
    assert_eq!(&values[&id]["color"], "blue");
    assert_eq!(&values[&text]["color"], "blue");
    assert_eq!(&values[&id]["width"], "20px");
    let (values, ..) = update(&mut motion, &doc, &on, 0.5);
    assert_eq!(&values[&id]["color"], "#800080ff");
    doc.nodes[id].attrs.remove("class");
    let off = computed_styles(&doc);
    let (values, ..) = update(&mut motion, &doc, &off, 0.5);
    assert_eq!(&values[&id]["color"], "#800080ff");
    let (values, ..) = update(&mut motion, &doc, &off, 0.75);
    assert_eq!(&values[&id]["color"], "#4000c0ff");
    let (values, running, event) = update(&mut motion, &doc, &off, 1.);
    assert!(values.is_empty() && !running);
    assert_eq!(event, Some("transitionend"));
    let (_, running, _) = update(&mut motion, &doc, &on, 1.);
    assert!(running);
    motion.forget(id);
    let (values, ..) = update(&mut motion, &doc, &off, 1.);
    assert!(!values.contains_key(&id));
}

#[test]
fn registered_custom_properties_animate_and_transition() {
    let mut doc = Document::parse(
        "<style>@property --w{syntax:'<length>';inherits:true;initial-value:0px}@property --c{syntax:'<color>';inherits:false;initial-value:red}@keyframes grow{to{--w:100px}}div{animation:grow 1s linear;width:var(--w)}span{display:block;width:var(--w)}p{--c:blue;--u:1px;transition:--c 1s linear,--u 1s;color:var(--c)}p.on{--c:lime;--u:9px}</style><div><span></span></div><p>x</p>",
    );
    let find =
        |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
    let (div, span, p) = (find(&doc, "div"), find(&doc, "span"), find(&doc, "p"));
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let styles = computed_styles(&doc);
    motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start,
        false,
        &mut Vec::new(),
    );
    let at = start + Duration::from_millis(500);
    motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        at,
        false,
        &mut Vec::new(),
    );
    assert_eq!(motion.custom[&div].get(Prop::new("--w")), Some("50px"));
    let restyle = |doc: &Document, animated: HashMap<usize, Style>| {
        super::super::compute_styles(
            doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut super::super::StyleCache::default(),
            Some(&super::super::StyleContext {
                animated,
                ..Default::default()
            }),
        )
    };
    let restyled = restyle(&doc, motion.custom.clone());
    assert_eq!(restyled.values[div].get(Prop::WIDTH), Some("50px"));
    assert_eq!(restyled.values[span].get(Prop::WIDTH), Some("50px"));
    assert_eq!(styles.values[div].get(Prop::WIDTH), Some("0px"));
    doc.nodes[p].attrs.insert("class".into(), "on".into());
    let on = computed_styles(&doc);
    motion.update(
        &doc,
        &on,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        at,
        false,
        &mut Vec::new(),
    );
    motion.update(
        &doc,
        &on,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        at + Duration::from_millis(500),
        false,
        &mut Vec::new(),
    );
    let values = &motion.custom[&p];
    assert_eq!(values.get(Prop::new("--c")), Some("#008080ff"));
    assert_eq!(values.get(Prop::new("--u")), None);
    let restyled = restyle(&doc, motion.custom.clone());
    assert_eq!(restyled.values[p].get(Prop::COLOR), Some("#008080ff"));
}

#[test]
fn starting_styles_transition_new_and_shown_elements() {
    let mut doc = Document::parse(
        "<style>p{opacity:1;transition:opacity 1s linear;@starting-style{opacity:0}}</style><p>a</p>",
    );
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut events = Vec::new();
    let styles = computed_styles(&doc);
    assert_eq!(styles.starting[&p].get(Prop::OPACITY), Some("0"));
    motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start,
        false,
        &mut events,
    );
    let (animated, _) = motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start + Duration::from_millis(500),
        false,
        &mut events,
    );
    let value = animated
        .iter()
        .find(|(id, ..)| *id == p)
        .map(|(_, style, _)| style.get(Prop::OPACITY).unwrap_or("").to_owned());
    assert_eq!(value.as_deref(), Some("0.5"), "{events:?}");
    doc.nodes[p]
        .attrs
        .insert("style".into(), "display:none".into());
    motion.update(
        &doc,
        &computed_styles(&doc),
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start + Duration::from_secs(2),
        false,
        &mut events,
    );
    doc.nodes[p].attrs.remove("style");
    motion.update(
        &doc,
        &computed_styles(&doc),
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start + Duration::from_secs(3),
        false,
        &mut events,
    );
    let (animated, _) = motion.update(
        &doc,
        &computed_styles(&doc),
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        start + Duration::from_millis(3250),
        false,
        &mut events,
    );
    let value = animated
        .iter()
        .find(|(id, ..)| *id == p)
        .map(|(_, style, _)| style.get(Prop::OPACITY).unwrap_or("").to_owned());
    assert_eq!(value.as_deref(), Some("0.25"));
}

#[test]
fn lifecycle_events_follow_delays_iterations_and_cancels() {
    let mut doc = Document::parse(
        "<style>@keyframes pulse{to{opacity:0}}div{width:10px;transition:width 1s .5s}div.on{width:20px}p{animation:pulse 1s 3 .25s}</style><div>t</div><p>p</p>",
    );
    let div = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion {
        iterations: true,
        ..Default::default()
    };
    let mut events = Vec::new();
    let mut run = |motion: &mut Motion, doc: &Document, time: f64| {
        events.clear();
        motion.update(
            doc,
            &computed_styles(doc),
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            start + Duration::from_secs_f64(time),
            false,
            &mut events,
        );
        events
            .iter()
            .map(|event| (event.1, event.3))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(&mut motion, &doc, 0.), []);
    doc.nodes[div].attrs.insert("class".into(), "on".into());
    assert_eq!(run(&mut motion, &doc, 0.1), [("transitionrun", 0.)]);
    assert_eq!(run(&mut motion, &doc, 0.3), [("animationstart", 0.)]);
    assert_eq!(run(&mut motion, &doc, 0.7), [("transitionstart", 0.)]);
    assert_eq!(run(&mut motion, &doc, 1.3), [("animationiteration", 1.)]);
    doc.nodes[div].attrs.remove("class");
    let events = run(&mut motion, &doc, 1.4);
    assert!(
        events
            .iter()
            .any(|event| event.0 == "transitioncancel" && (event.1 - 0.8).abs() < 1e-6),
        "{events:?}"
    );
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    doc.nodes[p]
        .attrs
        .insert("style".into(), "animation:none".into());
    assert!(
        run(&mut motion, &doc, 1.5)
            .iter()
            .any(|event| event.0 == "animationcancel")
    );
}

fn frames(html: &str, seconds: f64) -> (usize, Vec<Option<f64>>) {
    let doc = Document::parse(&format!(
        "<style>@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes blink{{50%{{opacity:0}}}}@keyframes grow{{to{{width:10px}}}}@keyframes still{{to{{will-change:transform}}}}@keyframes show{{to{{opacity:1}}}}</style>{html}"
    ));
    let styles = computed_styles(&doc);
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let (mut frame, mut count, mut wakes) = (0, 0, Vec::new());
    while frame < (seconds * 120.).round() as u64 {
        let now = start + Duration::from_nanos(frame * 1_000_000_000 / 120);
        let (_, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            now,
            false,
            &mut Vec::new(),
        );
        count += 1;
        wakes.push(next.map(|next| (next - now).as_secs_f64()));
        let Some(next) = next else { break };
        frame = (frame + 1).max(((next - start).as_nanos() * 120).div_ceil(1_000_000_000) as u64);
    }
    (count, wakes)
}

#[test]
fn animations_wake_the_view_only_when_their_output_can_change() {
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-6);
    let (count, wakes) = frames("<p style='animation:spin 1s linear infinite'></p>", 1.);
    assert!(count == 120 && wakes[0].is_some_and(|wake| wake < 1. / 120.));
    // A slow fade wakes when its opacity moves by 1%, so less often than the display.
    let (count, _) = frames(
        "<p style='animation:breathe 1.6s infinite'></p><style>@keyframes breathe{50%{opacity:.35}}</style>",
        1.6,
    );
    assert!((60..120).contains(&count), "{count} frames");
    // A small box shows a coarser opacity step, so its fade wakes less often.
    let doc = Document::parse(
        "<style>@keyframes breathe{50%{opacity:.35}}</style><p style='animation:breathe 1.6s infinite'></p>",
    );
    let styles = computed_styles(&doc);
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    let wake = |coarse| {
        let effect = &styles.animations[&p][0];
        let mut out = Style::new();
        effect
            .sample(
                0.2,
                false,
                &styles.values[p],
                &Default::default(),
                &mut out,
                coarse,
            )
            .0
            .unwrap()
    };
    assert!(wake(4.) > wake(1.) * 3.);
    let (count, wakes) = frames(
        "<p style='animation:blink 1.2s steps(1) infinite'></p>",
        2.4,
    );
    assert_eq!(count, 4);
    assert!(wakes.iter().all(|&wake| close(wake, 0.6)));
    for hidden in [
        "<p style='visibility:hidden;animation:spin 1s linear infinite'></p>",
        "<div style='opacity:0'><p style='animation:spin 1s linear infinite'></p></div>",
        "<p style='animation:still 1s infinite'></p>",
        "<p style='animation:blink 1s infinite paused'></p>",
        "<p style='opacity:0;animation:spin 1s linear infinite'></p>",
    ] {
        assert_eq!(frames(hidden, 1.), (1, vec![None]), "{hidden}");
    }
    for shown in [
        "<div style='visibility:hidden;animation:spin 1s linear infinite'><p style='visibility:visible'></p></div>",
        "<p style='visibility:hidden;animation:grow 1s infinite'></p>",
        "<p style='opacity:0;animation:show 1s infinite'></p>",
    ] {
        let wake = frames(shown, 0.1).1[0];
        assert!(wake.is_some_and(|wake| wake < 0.1), "{shown}");
    }
    let (_, wakes) = frames(
        "<p style='visibility:hidden;animation:spin 2s linear'></p>",
        3.,
    );
    assert!(close(wakes[0], 2.) && wakes.last() == Some(&None));
    let (_, wakes) = frames("<p style='animation:spin 1s 2s linear'></p>", 0.1);
    assert!(close(wakes[0], 2.));
    let (count, wakes) = frames("<p style='animation:spin 1s linear forwards'></p>", 5.);
    assert_eq!((count, wakes.last()), (121, Some(&None)));
}

#[test]
fn animations_of_unshown_boxes_wait_unless_they_move_the_box() {
    let page = |html: &str| {
        Document::parse(&format!(
            "<style>@keyframes spin{{to{{transform:rotate(360deg)}}}}@keyframes slide{{to{{transform:translateX(9px)}}}}@keyframes blink{{50%{{opacity:0}}}}@keyframes grow{{to{{width:10px}}}}</style>{html}"
        ))
    };
    let start = Instant::now();
    for (html, parked) in [
        ("<p style='animation:spin 1s linear infinite'></p>", true),
        ("<p style='animation:blink 1s infinite'></p>", true),
        ("<p style='animation:slide 1s linear infinite'></p>", false),
        ("<p style='animation:grow 1s infinite'></p>", false),
    ] {
        let doc = page(html);
        let styles = computed_styles(&doc);
        let visible = vec![true; doc.nodes.len()];
        let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        let mut motion = Motion::default();
        let (_, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| false,
            &|_| f32::INFINITY,
            start,
            false,
            &mut Vec::new(),
        );
        assert_eq!(
            (next.is_none(), motion.parked == [p]),
            (parked, parked),
            "{html}"
        );
    }
    let doc = page("<p style='animation:spin 1s linear infinite'></p>");
    let styles = computed_styles(&doc);
    let visible = vec![true; doc.nodes.len()];
    let mut motion = Motion::default();
    motion.update(
        &doc,
        &styles,
        &visible,
        &|_| false,
        &|_| f32::INFINITY,
        start,
        false,
        &mut Vec::new(),
    );
    let later = start + Duration::from_millis(250);
    let (animated, next) = motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        later,
        false,
        &mut Vec::new(),
    );
    assert_eq!(next, Some(later));
    assert_eq!(&animated[0].1["transform"], "rotate(90deg)");
}

#[test]
fn inherited_colors_animate_without_layout() {
    let mut doc = Document::parse(
        "<html><style>body{color:blue;transition:color 1s linear}body.on{color:red}span{transition:background-color 1s linear}span.on{background-color:red}</style><body><p>a <span>b</span></p></body></html>",
    );
    let find =
        |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
    let (body, span) = (find(&doc, "body"), find(&doc, "span"));
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut update = |doc: &Document, time: u64| {
        motion
            .update(
                doc,
                &computed_styles(doc),
                &visible,
                &|_| true,
                &|_| f32::INFINITY,
                start + Duration::from_millis(time),
                false,
                &mut Vec::new(),
            )
            .0
    };
    update(&doc, 0);
    doc.nodes[body].attrs.insert("class".into(), "on".into());
    update(&doc, 0);
    let animated = update(&doc, 500);
    assert_eq!(animated.len(), 5);
    assert!(
        animated
            .iter()
            .all(|(_, style, layout)| !layout && &style["color"] == "#800080ff")
    );
    doc.nodes[span].attrs.insert("class".into(), "on".into());
    update(&doc, 500);
    let animated = update(&doc, 750);
    assert!(
        animated
            .iter()
            .any(|(id, _, layout)| *id == span && *layout)
    );
}

#[test]
fn inherited_transitions_skip_children_that_declare_the_value() {
    let mut doc = Document::parse(
        "<html><style>body{color:blue;transition:color 1s linear}body.on{color:red}p{color:red}</style><body><i>a</i><p>b</p></body></html>",
    );
    let find =
        |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
    let (body, i, p) = (find(&doc, "body"), find(&doc, "i"), find(&doc, "p"));
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut update = |doc: &Document, time: u64| {
        motion
            .update(
                doc,
                &computed_styles(doc),
                &visible,
                &|_| true,
                &|_| f32::INFINITY,
                start + Duration::from_millis(time),
                false,
                &mut Vec::new(),
            )
            .0
    };
    update(&doc, 0);
    doc.nodes[body].attrs.insert("class".into(), "on".into());
    update(&doc, 0);
    let animated = update(&doc, 500);
    let moving = |id: usize| animated.iter().any(|(node, ..)| *node == id);
    assert!(moving(body) && moving(i));
    assert!(
        !moving(p),
        "a paragraph that is always red must not animate"
    );
}

#[test]
fn filled_animations_keep_one_style_while_unchanged() {
    let doc = Document::parse(
        "<style>@keyframes grow{to{width:10px}}p{width:0px;animation:grow 1s forwards}</style><p></p>",
    );
    let styles = computed_styles(&doc);
    let visible = vec![true; doc.nodes.len()];
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut style = |time: f64| {
        let now = start + Duration::from_secs_f64(time);
        let (animated, _) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            now,
            false,
            &mut Vec::new(),
        );
        animated.first().map(|(_, style, _)| style.clone())
    };
    assert!(style(0.).is_none());
    let (half, end, later) = (style(0.5).unwrap(), style(2.).unwrap(), style(3.).unwrap());
    assert!(!Rc::ptr_eq(&half, &end) && Rc::ptr_eq(&end, &later));
    assert_eq!(&later["width"], "10px");
}

#[test]
fn linear_easing_spreads_missing_inputs_and_extrapolates() {
    assert_eq!(easing("linear(0, 1)", 0.3), 0.3);
    assert!((easing("linear(0, 0.25 50%, 1)", 0.25) - 0.125).abs() < 1e-9);
    assert!((easing("linear(0, 0.25 50%, 1)", 0.75) - 0.625).abs() < 1e-9);
    assert!((easing("linear(0, 0.5, 1 40%, 1)", 0.2) - 0.5).abs() < 1e-9);
    assert_eq!(easing("linear(0, 0.5 20% 60%, 1)", 0.4), 0.5);
    assert_eq!(easing("linear(0, 1 50%, 0.2 30%)", 0.5), 0.2);
    assert!((easing("linear(0, 1)", 1.5) - 1.5).abs() < 1e-9);
    assert!((easing("linear(0, 1.2 60%, 1)", 0.6) - 1.2).abs() < 1e-9);
    assert!(Easing::parse("linear(1)").is_none());
}

#[test]
fn script_animations_follow_web_animations_timing() {
    let doc = Document::parse("<div style='opacity:0.5'></div>");
    let styles = computed_styles(&doc);
    let id = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
    let base = styles.values[id].clone();
    let opacity = |animation: &Scripted, local: f64| {
        let mut out = Style::new();
        animation
            .effect
            .sample(local, true, &base, &Default::default(), &mut out, 1.);
        out.get(Prop::OPACITY)
            .map(|value| (value.parse::<f64>().unwrap() * 1e4).round() / 1e4)
    };
    let new = |frames: &[(f64, &str, &str)], timing, direction, fill, easing| {
        let mut animation =
            Scripted::new(1, frames, timing, [direction, fill, easing, "replace"], 0);
        animation.effect.tracks = tracks(&base, &animation.rules, &Default::default(), 0, false);
        animation
    };
    let frames = [(0., "opacity:0", "linear"), (1., "opacity:1", "linear")];
    let alternate = new(
        &frames,
        [0., 0., 1., 3., 0.5],
        "alternate",
        "none",
        "linear",
    );
    assert_eq!(opacity(&alternate, 0.6), Some(0.9));
    assert_eq!(opacity(&alternate, 3.), None);
    let cut = new(&frames, [0.2, -0.5, 1., 1., 0.], "normal", "both", "linear");
    assert_eq!(opacity(&cut, 0.), Some(0.));
    assert_eq!(opacity(&cut, 0.5), Some(0.3));
    assert_eq!(opacity(&cut, 0.7), Some(0.5));
    let spring = new(
        &frames,
        [0., 0., 1., 1., 0.],
        "normal",
        "none",
        "linear(0, 1.2 60%, 1)",
    );
    assert_eq!(opacity(&spring, 0.6), Some(1.2));
    let single = new(
        &[(1., "opacity:1", "linear")],
        [0., 0., 1., 1., 0.],
        "normal",
        "none",
        "linear",
    );
    assert_eq!(opacity(&single, 0.5), Some(0.75));
    let mut motion = Motion::default();
    motion.script(
        id,
        new(&frames, [0., 0., 10., 1., 0.], "normal", "none", "linear"),
        2.,
        Some(1.),
        None,
    );
    let visible = vec![true; doc.nodes.len()];
    let (animated, next) = motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        Instant::now(),
        false,
        &mut Vec::new(),
    );
    let value = animated[0]
        .1
        .get(Prop::OPACITY)
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((0.2..0.21).contains(&value) && next.is_some());
    motion.script(
        id,
        new(&frames, [0., 0., 10., 1., 0.], "normal", "none", "linear"),
        5.,
        None,
        None,
    );
    let (animated, next) = motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        Instant::now(),
        false,
        &mut Vec::new(),
    );
    assert_eq!(
        (animated[0].1.get(Prop::OPACITY), next),
        (Some("0.5"), None)
    );
    assert_eq!(motion.unscript(1).map(|(node, _)| node), Some(id));
    assert!(motion.scripted(id, 0, &styles, u32::MAX).is_none());
}

#[test]
fn composite_adds_and_accumulates_values() {
    assert_eq!(add("opacity", "0.2", "0.4", false).as_deref(), Some("0.6"));
    assert_eq!(
        add("width", "10px", "2em", false).as_deref(),
        Some("calc(10px + 2em)")
    );
    assert_eq!(
        add("transform", "translateX(10px)", "rotate(5deg)", false).as_deref(),
        Some("translateX(10px) rotate(5deg)")
    );
    assert_eq!(
        add(
            "transform",
            "translateX(10px) scale(2)",
            "translateX(5px) scale(1.5)",
            true
        )
        .as_deref(),
        Some("translateX(15px) scale(2.5)")
    );
    assert_eq!(
        add(
            "background-color",
            "rgb(100, 0, 0)",
            "rgb(200, 10, 0)",
            false
        )
        .as_deref(),
        Some("rgba(255, 10, 0, 1)")
    );
    assert_eq!(add("display", "block", "none", false), None);
}

#[test]
fn css_animations_of_elements_and_pseudo_elements_answer_script() {
    let doc = Document::parse(
        "<style>@keyframes fade{from{opacity:0}to{opacity:1}}@keyframes vary{from{--x:a}to{--x:b}}div{animation:fade 1s linear,vary 1s linear -0.6s paused}p::before{content:'x';animation:fade 2s linear -1s paused}</style><div></div><p></p>",
    );
    let styles = computed_styles(&doc);
    let [div, p] =
        ["div", "p"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
    let visible = vec![true; doc.nodes.len()];
    let mut motion = Motion::default();
    let mut events = Vec::new();
    let (animated, _) = motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        Instant::now(),
        false,
        &mut events,
    );
    let own = animated.iter().find(|(id, ..)| *id == div).unwrap();
    assert_eq!(own.1.get(Prop::new("--x")), Some("b"));
    let pair = &motion.pseudos.iter().find(|(id, _)| *id == p).unwrap().1;
    assert_eq!(pair[0].get(Prop::OPACITY), Some("0.5"));
    assert!(animated.iter().any(|(id, ..)| *id == p));
    let list = motion.css(&[div, p], false, &styles);
    assert_eq!(list.len(), 2);
    assert_eq!(
        (list[0][3].as_str(), list[0][5].as_str()),
        (Some("fade"), Some("running"))
    );
    let pseudos = motion.css(&[p], true, &styles);
    assert_eq!(
        (pseudos[0][3].as_str(), pseudos[0][11].as_str()),
        (Some("fade"), Some("::before"))
    );
    let serial = list[0][0].as_u64().unwrap() as u32;
    assert!(motion.css_control(serial, "pause", 0., &styles));
    assert_eq!(
        motion.css(&[div], false, &styles)[0][5].as_str(),
        Some("paused")
    );
    assert!(motion.css_control(serial, "seek", 250., &styles));
    assert_eq!(
        motion.css(&[div], false, &styles)[0][4].as_f64(),
        Some(250.)
    );
    assert!(motion.css_control(serial, "cancel", 0., &styles));
    assert_eq!(motion.css(&[div], false, &styles).len(), 1);
    let mut events = Vec::new();
    motion.update(
        &doc,
        &styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        Instant::now(),
        false,
        &mut events,
    );
    assert!(
        events
            .iter()
            .any(|(id, kind, name, _)| (*id, *kind, name.as_str())
                == (div, "animationcancel", "fade"))
    );
    assert!(!motion.css_control(u32::MAX, "pause", 0., &styles));
}

#[test]
fn timing_functions_preserve_endpoints_and_steps() {
    for name in [
        "ease",
        "ease-in",
        "ease-out",
        "ease-in-out",
        "linear",
        "cubic-bezier(.2,0,.8,1)",
    ] {
        assert_eq!(easing(name, 0.), 0.);
        assert_eq!(easing(name, 1.), 1.);
    }
    assert_eq!(easing("steps(2, jump-end)", 0.49), 0.);
    assert_eq!(easing("steps(2, jump-end)", 0.5), 0.5);
    assert_eq!(easing("steps(2, jump-start)", 0.), 0.5);
    for invalid in [
        "steps()",
        "steps(0)",
        "steps(2.5)",
        "steps(1, jump-none)",
        "cubic-bezier(1.5, 0, 0, 1)",
        "cubic-bezier(0, 0, 1)",
        "linear(0)",
    ] {
        assert!(Easing::parse(invalid).is_none(), "{invalid}");
    }
    for name in [
        "ease-in",
        "steps(3, jump-both)",
        "cubic-bezier(0.1, 2, 0.3, -1)",
    ] {
        assert_eq!(Easing::parse(name).unwrap().to_string(), name);
    }
    assert_eq!(
        easing("linear(0, 0.5 25%, 1)", 0.5),
        easing("linear(0 0%, 0.5 25%, 1 100%)", 0.5)
    );
    let frames = HashMap::from([("pulse".into(), Vec::new())]);
    assert!(
        Animation::parse(
            &Style::from_iter([
                (Prop::new("animation"), "".into()),
                (Prop::new("animation-name"), "pulse".into())
            ]),
            &frames,
            &Default::default(),
            0
        )
        .is_empty()
    );
    assert!(
        Animation::parse(
            &Style::from_iter([
                (Prop::new("animation"), "1s pulse".into()),
                (Prop::new("animation-iteration-count"), "NaN".into())
            ]),
            &frames,
            &Default::default(),
            0
        )
        .is_empty()
    );
    assert_eq!(
        transitions(&Style::from_iter([
            (
                Prop::TRANSITION,
                "opacity .2s ease-in 1s, all .3s allow-discrete".into()
            ),
            (Prop::new("transition-timing-function"), "linear".into())
        ])),
        vec![
            ("opacity".into(), 0.2, 1., Easing::Linear, false),
            ("all".into(), 0.3, 0., Easing::Linear, true)
        ]
    );
}

type Step = (
    HashMap<usize, (Rc<Style>, bool)>,
    Option<Instant>,
    Vec<(usize, &'static str, String, f64)>,
);

fn step(motion: &mut Motion, doc: &Document, styles: &ComputedStyles, at: Instant) -> Step {
    let visible: Vec<bool> = (0..doc.nodes.len())
        .map(|id| styles.values[id].get(Prop::DISPLAY) != Some("none"))
        .collect();
    let mut events = Vec::new();
    let (animated, next) = motion.update(
        doc,
        styles,
        &visible,
        &|_| true,
        &|_| f32::INFINITY,
        at,
        false,
        &mut events,
    );
    let animated = animated
        .into_iter()
        .map(|(id, style, layout)| (id, (style, layout)))
        .collect();
    (animated, next, events)
}

fn node(doc: &Document, tag: &str) -> usize {
    doc.nodes.iter().position(|node| node.tag == tag).unwrap()
}

#[test]
fn times_resolve_math_like_chrome() {
    assert_eq!(time("calc(420ms + 2 * 37ms)"), Some(0.494));
    assert_eq!(time("calc(2 * -91ms)"), Some(-0.182));
    assert_eq!(time("max(1s, 200ms)"), Some(1.));
    assert_eq!(time("calc(2 * 3)"), None);
    assert_eq!(time("calc(4px)"), None);
}

#[test]
fn huge_page_times_never_panic() {
    let mut doc = Document::parse(
        "<style>@keyframes fade{to{opacity:0}}div{animation:fade 1e19s 2}p{animation:fade 1s 1e300s}span{display:block;opacity:1;transition:opacity 1e300s}span.on{opacity:0}</style><div></div><p></p><span></span>",
    );
    let start = Instant::now();
    let mut motion = Motion::default();
    step(&mut motion, &doc, &computed_styles(&doc), start);
    let span = node(&doc, "span");
    doc.nodes[span].attrs.insert("class".into(), "on".into());
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, start);
    let nodes: Vec<usize> = (0..doc.nodes.len()).collect();
    let list = motion.css(&nodes, false, &styles);
    assert_eq!(list.len(), 3);
    for entry in list {
        let serial = entry[0].as_u64().unwrap() as u32;
        for (action, time) in [
            ("seek", 1e300),
            ("pause", 0.),
            ("seek", -1e300),
            ("play", 0.),
            ("seek", f64::MAX),
            ("finish", 0.),
            ("play", 0.),
        ] {
            motion.css_control(serial, action, time, &styles);
        }
    }
    for at in [start, start + Duration::from_secs(1)] {
        step(&mut motion, &doc, &styles, at);
        for id in nodes.iter().copied() {
            motion.next_event(id, &styles, at);
            motion.timeline(id, 0, &styles, &[at], &[]);
        }
    }
    assert_eq!(after(start, 1e19), None);
    assert_eq!(after(start, f64::INFINITY), None);
}

#[test]
fn play_state_changes_keep_the_time_before_them() {
    let mut doc = Document::parse(
        "<style>@keyframes spin{to{transform:rotate(360deg)}}p{animation:spin 10s linear infinite}p.paused{animation-play-state:paused}</style><p></p>",
    );
    let p = node(&doc, "p");
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut turn = |doc: &Document, styles: &ComputedStyles, seconds: f64| {
        let at = start + Duration::from_secs_f64(seconds);
        let (animated, ..) = step(&mut motion, doc, styles, at);
        animated[&p].0.get(Prop::TRANSFORM).unwrap().to_owned()
    };
    let running = computed_styles(&doc);
    turn(&doc, &running, 0.);
    turn(&doc, &running, 0.5);
    doc.nodes[p].attrs.insert("class".into(), "paused".into());
    let paused = computed_styles(&doc);
    assert_eq!(turn(&doc, &paused, 1.), "rotate(36deg)");
    doc.nodes[p].attrs.remove("class");
    assert_eq!(turn(&doc, &running, 11.), "rotate(36deg)");
    assert_eq!(turn(&doc, &running, 11.5), "rotate(54deg)");
}

#[test]
fn iterations_wake_only_listening_pages_and_single_effects_loop() {
    let doc = Document::parse(
        "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 1s infinite}p{animation:pulse .5s 3 alternate}i{animation:pulse 1s 2s}b{animation:pulse 1s,pulse 2s}</style><div></div><p></p><i></i><b></b>",
    );
    let [div, p, i, b] = ["div", "p", "i", "b"].map(|tag| node(&doc, tag));
    let styles = computed_styles(&doc);
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    let kinds = |motion: &mut Motion, seconds: f64| {
        let (.., events) = step(motion, &doc, &styles, at(seconds));
        events
            .into_iter()
            .filter(|event| event.0 == div)
            .map(|event| event.1)
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&mut motion, 0.), ["animationstart"]);
    assert_eq!(kinds(&mut motion, 1.1), [""; 0]);
    assert_eq!(motion.next_event(div, &styles, at(1.1)), None);
    let period = |id: usize, seconds: f64| {
        motion
            .period(id, 0, &styles, at(seconds))
            .map(|(period, phase, left, shape)| {
                let round = |value: f64| (value * 1e6).round() / 1e6;
                (period, round(phase), left, shape.is_some())
            })
    };
    assert_eq!(period(div, 1.1), Some((1., 0.1, f64::INFINITY, true)));
    assert_eq!(period(p, 0.25), Some((1., 0.25, 1.25, true)));
    assert_eq!(period(i, 1.), None);
    assert_eq!(period(b, 0.5), None);
    assert_eq!(period(b, 1.5), Some((2., 1.5, 0.25, false)));
    motion.iterations = true;
    assert_eq!(kinds(&mut motion, 2.2), ["animationiteration"]);
    let next = motion.next_event(div, &styles, at(2.2)).unwrap();
    assert!((next.duration_since(start).as_secs_f64() - 3.).abs() < 1e-6);
}

#[test]
fn endless_animations_without_a_duration_end_at_zero() {
    let doc = Document::parse(
        "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 0s infinite}</style><div></div>",
    );
    let div = node(&doc, "div");
    let styles = computed_styles(&doc);
    let start = Instant::now();
    let mut motion = Motion::default();
    let (.., events) = step(&mut motion, &doc, &styles, start);
    let events: Vec<_> = events
        .into_iter()
        .map(|(id, kind, _, time)| (id, kind, time))
        .collect();
    assert_eq!(
        events,
        [(div, "animationstart", 0.), (div, "animationend", 0.)]
    );
    assert_eq!(motion.next_event(div, &styles, start), None);
}

#[test]
fn late_starts_fire_no_iteration_events() {
    let doc = Document::parse(
        "<style>@keyframes pulse{to{opacity:0}}div{animation:pulse 1s -2.5s 5}p{animation:pulse 1s 1s 5}</style><div></div><p></p>",
    );
    let (div, p) = (node(&doc, "div"), node(&doc, "p"));
    let styles = computed_styles(&doc);
    let start = Instant::now();
    let mut motion = Motion {
        iterations: true,
        ..Default::default()
    };
    let mut events = |seconds: f64| {
        let at = start + Duration::from_secs_f64(seconds);
        let (.., events) = step(&mut motion, &doc, &styles, at);
        let mut events: Vec<_> = events
            .into_iter()
            .map(|(id, kind, _, time)| (id, kind, time))
            .collect();
        events.sort_by_key(|event| event.0);
        events
    };
    assert_eq!(events(0.), [(div, "animationstart", 2.5)]);
    assert_eq!(events(0.6), [(div, "animationiteration", 3.)]);
    assert_eq!(
        events(3.5),
        [(div, "animationend", 5.), (p, "animationstart", 0.)]
    );
    assert_eq!(events(4.2), [(p, "animationiteration", 3.)]);
}

#[test]
fn animations_keep_their_clocks_by_name() {
    let mut doc = Document::parse(
        "<style>@keyframes a{to{opacity:0}}@keyframes b{to{width:10px}}div{width:0px;animation:a 1s,b 2s linear infinite}div.one{animation:b 2s linear infinite}div.two{animation:b 2s linear infinite,b 2s linear infinite}</style><div></div>",
    );
    let div = node(&doc, "div");
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let serials = |motion: &Motion, styles: &ComputedStyles| {
        motion
            .css(&[div], false, styles)
            .iter()
            .map(|entry| entry[0].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    let kinds = |events: &[(usize, &'static str, String, f64)]| {
        events
            .iter()
            .map(|event| (event.1, event.2.clone()))
            .collect::<Vec<_>>()
    };
    let mut motion = Motion::default();
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, at(0.));
    let b = serials(&motion, &styles)[1];
    doc.nodes[div].attrs.insert("class".into(), "one".into());
    let styles = computed_styles(&doc);
    let (animated, _, events) = step(&mut motion, &doc, &styles, at(1.));
    assert_eq!(kinds(&events), [("animationcancel", "a".into())]);
    assert_eq!(serials(&motion, &styles), [b]);
    assert_eq!(animated[&div].0.get(Prop::WIDTH), Some("5px"));
    doc.nodes[div].attrs.insert("class".into(), "two".into());
    let styles = computed_styles(&doc);
    let (.., events) = step(&mut motion, &doc, &styles, at(1.5));
    assert_eq!(kinds(&events), [("animationstart", "b".into())]);
    let list = serials(&motion, &styles);
    assert!(list[0] != b && list[1] == b, "{list:?}");
}

#[test]
fn retargeted_transitions_cancel_before_they_run_again() {
    let mut doc = Document::parse(
        "<style>div{width:0px;transition:width 1s linear}div.a{width:10px}div.b{width:20px}</style><div></div>",
    );
    let div = node(&doc, "div");
    let start = Instant::now();
    let mut motion = Motion::default();
    let mut kinds = |doc: &Document, seconds: f64| {
        let at = start + Duration::from_secs_f64(seconds);
        let (.., events) = step(&mut motion, doc, &computed_styles(doc), at);
        events.into_iter().map(|event| event.1).collect::<Vec<_>>()
    };
    kinds(&doc, 0.);
    doc.nodes[div].attrs.insert("class".into(), "a".into());
    assert_eq!(kinds(&doc, 0.), ["transitionrun", "transitionstart"]);
    doc.nodes[div].attrs.insert("class".into(), "b".into());
    assert_eq!(
        kinds(&doc, 0.5),
        ["transitioncancel", "transitionrun", "transitionstart"]
    );
}

#[test]
fn content_and_font_size_keyframes_animate() {
    let doc = Document::parse(
        "<style>@keyframes dots{33%{content:'..'}66%{content:'...'}}@keyframes grow{to{font-size:32px;line-height:40px}}i::after{content:'.';animation:dots 1s infinite}em{font-size:16px;line-height:20px;animation:grow 1s linear}</style><i></i><em></em>",
    );
    let (i, em) = (node(&doc, "i"), node(&doc, "em"));
    let styles = computed_styles(&doc);
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    let mut sample = |seconds: f64| {
        step(&mut motion, &doc, &styles, at(seconds));
        let pair = &motion.pseudos.iter().find(|(id, _)| *id == i).unwrap().1;
        let sizes = motion.custom.get(&em).map(|values| {
            [Prop::FONT_SIZE, Prop::LINE_HEIGHT].map(|prop| values.get(prop).unwrap().to_owned())
        });
        (pair[1].get(Prop::CONTENT).unwrap().to_owned(), sizes)
    };
    sample(0.);
    assert_eq!(sample(0.4).0, "'..'");
    assert_eq!(sample(0.5).1, Some(["24px".into(), "30px".into()]));
    assert_eq!(sample(0.7).0, "'...'");
    assert_eq!(sample(0.95).0, "'.'");
    let doc = Document::parse(
        "<style>@keyframes dots{33%{content:'..'}66%{content:'...'}}i::after{content:'.';animation:dots 1s infinite}</style><i></i>",
    );
    let mut motion = Motion::default();
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, at(0.));
    let (_, next, _) = step(&mut motion, &doc, &styles, at(0.5));
    let wake = (next.unwrap() - at(0.5)).as_secs_f64();
    assert!(wake > 0.01 && wake < 0.2, "{wake}");
}

#[test]
fn display_keeps_exits_shown_until_they_end() {
    let mut doc = Document::parse(
        "<style>@keyframes out{to{opacity:0;display:none}}b{display:block;animation:out 1s linear forwards}p,s{display:block;opacity:1;transform:translateY(10px) scale(.95)}p{transition:opacity 1s linear,transform 1s linear,display 1s allow-discrete}s{transition:opacity 1s linear}p.off,s.off{display:none;opacity:0;transform:translateY(0)}</style><b></b><p></p><s></s>",
    );
    let [b, p, s] = ["b", "p", "s"].map(|tag| node(&doc, tag));
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, at(0.));
    let (animated, ..) = step(&mut motion, &doc, &styles, at(0.5));
    let (style, layout) = &animated[&b];
    assert_eq!(
        (style.get(Prop::OPACITY), style.get(Prop::DISPLAY), *layout),
        (Some("0.5"), Some("block"), false)
    );
    let (animated, ..) = step(&mut motion, &doc, &styles, at(1.5));
    assert_eq!(animated[&b].0.get(Prop::DISPLAY), Some("none"));
    for id in [p, s] {
        doc.nodes[id].attrs.insert("class".into(), "off".into());
    }
    let styles = computed_styles(&doc);
    let (animated, ..) = step(&mut motion, &doc, &styles, at(2.));
    assert!(!animated.contains_key(&s));
    assert_eq!(animated[&p].0.get(Prop::DISPLAY), Some("block"));
    let (animated, ..) = step(&mut motion, &doc, &styles, at(2.5));
    let style = &animated[&p].0;
    assert_eq!(
        [Prop::DISPLAY, Prop::OPACITY, Prop::TRANSFORM].map(|prop| style.get(prop)),
        [
            Some("block"),
            Some("0.5"),
            Some("translateY(5px) scale(0.975)")
        ]
    );
    let (animated, _, events) = step(&mut motion, &doc, &styles, at(3.));
    assert!(!animated.contains_key(&p));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.1 == "transitionend")
            .count(),
        3
    );
}

#[test]
fn overlay_keeps_leaving_boxes_in_the_top_layer_until_it_ends() {
    let mut doc = Document::parse(
        "<style>dialog{opacity:0;transition:opacity 1s linear,overlay 1s allow-discrete,display 1s allow-discrete}dialog[open]{opacity:1}p{display:block;transition:overlay 1s}</style><dialog open></dialog><p popover></p>",
    );
    let [d, p] = ["dialog", "p"].map(|tag| node(&doc, tag));
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    doc.nodes[d].state = crate::dom::MODAL;
    doc.nodes[p].state = crate::dom::POPOVER;
    let styles = computed_styles(&doc);
    assert!(step(&mut motion, &doc, &styles, at(0.)).0.is_empty());
    doc.nodes[d].attrs.remove("open");
    doc.nodes[d].state = crate::dom::MODAL << 5;
    doc.nodes[p].state = crate::dom::POPOVER << 5;
    let styles = computed_styles(&doc);
    assert_eq!(styles.values[d].get(Prop::OVERLAY), Some("none"));
    let (animated, ..) = step(&mut motion, &doc, &styles, at(1.));
    assert_eq!(animated[&d].0.get(Prop::OVERLAY), Some("auto"));
    assert!(!animated.contains_key(&p));
    let (animated, ..) = step(&mut motion, &doc, &styles, at(1.5));
    let style = &animated[&d].0;
    assert_eq!(
        [Prop::OVERLAY, Prop::DISPLAY, Prop::OPACITY].map(|prop| style.get(prop)),
        [Some("auto"), Some("block"), Some("0.5")]
    );
    let (animated, next, events) = step(&mut motion, &doc, &styles, at(2.));
    assert!(!animated.contains_key(&d) && next.is_none());
    assert!(events.contains(&(d, "transitionend", "overlay".into(), 1.)));
    let mut doc = Document::parse(
        "<style>dialog{transition:overlay 1s allow-discrete,display 1s allow-discrete}</style><dialog></dialog>",
    );
    let d = node(&doc, "dialog");
    let mut motion = Motion::default();
    doc.nodes[d].attrs.insert("open".into(), String::new());
    doc.nodes[d].state = crate::dom::MODAL;
    step(&mut motion, &doc, &computed_styles(&doc), at(0.));
    doc.nodes[d].attrs.remove("open");
    doc.nodes[d].state = crate::dom::MODAL << 5;
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, at(1.));
    let (_, next, _) = step(&mut motion, &doc, &styles, at(1.25));
    assert_eq!(next, Some(at(2.)));
}

#[test]
fn backdrops_transition_from_their_starting_style_and_out() {
    let mut doc = Document::parse(
        "<style>dialog::backdrop{opacity:0;transition:opacity 1s linear}dialog[open]::backdrop{opacity:1}@starting-style{dialog[open]::backdrop{opacity:0}}</style><dialog></dialog>",
    );
    let d = node(&doc, "dialog");
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    step(&mut motion, &doc, &computed_styles(&doc), at(0.));
    doc.nodes[d].attrs.insert("open".into(), String::new());
    doc.nodes[d].state = crate::dom::MODAL;
    let mut opacity = |seconds: f64, doc: &Document| {
        let (_, _, events) = step(&mut motion, doc, &computed_styles(doc), at(seconds));
        let opacity = motion
            .backdrops
            .iter()
            .find(|(id, _)| *id == d)
            .and_then(|(_, style)| style.get(Prop::OPACITY).map(str::to_owned));
        (opacity, events)
    };
    let (value, events) = opacity(1., &doc);
    assert_eq!(value.as_deref(), Some("0"));
    assert!(events.contains(&(d, "transitionrun", "::backdrop\0opacity".into(), 0.)));
    assert_eq!(opacity(1.5, &doc).0.as_deref(), Some("0.5"));
    let (value, events) = opacity(2., &doc);
    assert_eq!(value, None);
    assert!(events.contains(&(d, "transitionend", "::backdrop\0opacity".into(), 1.)));
    doc.nodes[d].attrs.remove("open");
    doc.nodes[d].state = crate::dom::MODAL << 5;
    opacity(3., &doc);
    assert_eq!(opacity(3.25, &doc).0.as_deref(), Some("0.75"));
    doc.nodes[d].state = 0;
    let (value, events) = opacity(3.5, &doc);
    assert_eq!(value, None);
    assert!(events.contains(&(d, "transitioncancel", "::backdrop\0opacity".into(), 0.5)));
}

#[test]
fn script_animations_that_set_display_show_hidden_boxes() {
    let doc = Document::parse(
        "<style>@keyframes show{from,to{display:block}}div{display:none}i{display:block}b{display:none;animation:show 1s}</style><div><i></i></div><b></b>",
    );
    let [div, i, b] = ["div", "i", "b"].map(|tag| node(&doc, tag));
    let styles = computed_styles(&doc);
    let mut visible = vec![false; doc.nodes.len()];
    let mut pending = vec![0];
    while let Some(id) = pending.pop() {
        if styles.values[id].get(Prop::DISPLAY) != Some("none") {
            visible[id] = true;
            pending.extend(&doc.nodes[id].children);
        }
    }
    let mut motion = Motion::default();
    motion.script(
        div,
        Scripted::new(
            1,
            &[
                (0., "display:block;opacity:0", "linear"),
                (1., "display:block;opacity:1", "linear"),
            ],
            [0., 0., 1., 1., 0.],
            ["normal", "none", "linear", "replace"],
            0,
        ),
        0.,
        Some(1.),
        None,
    );
    let start = Instant::now();
    let mut update = |seconds: f64| {
        let mut events = Vec::new();
        let (animated, next) = motion.update(
            &doc,
            &styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            start + Duration::from_secs_f64(seconds),
            false,
            &mut events,
        );
        (animated, motion.painted.take(), next, events)
    };
    let (animated, painted, next, events) = update(0.5);
    let painted = painted.unwrap();
    assert!(painted[div] && painted[i] && !painted[b]);
    let (_, style, layout) = animated.iter().find(|(id, ..)| *id == div).unwrap();
    assert_eq!(style.get(Prop::DISPLAY), Some("block"));
    assert!(*layout && next.is_some() && events.is_empty());
    let (animated, painted, next, events) = update(1.5);
    assert!(animated.is_empty() && painted.is_none() && next.is_none() && events.is_empty());
}

#[test]
fn tailwind_transform_lists_transition_smoothly() {
    let mut doc = Document::parse(
        "<style>@import 'tailwindcss';</style><div class='transition-transform duration-1000 ease-linear rotate-x-12'></div>",
    );
    let div = node(&doc, "div");
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    step(&mut motion, &doc, &computed_styles(&doc), at(0.));
    let mut transform = |class: &str, seconds: [f64; 2]| {
        doc.nodes[div].attrs.insert(
            "class".into(),
            format!("transition-transform duration-1000 ease-linear {class}"),
        );
        let styles = computed_styles(&doc);
        step(&mut motion, &doc, &styles, at(seconds[0]));
        let (animated, ..) = step(&mut motion, &doc, &styles, at(seconds[1]));
        animated[&div].0.get(Prop::TRANSFORM).unwrap().to_owned()
    };
    assert_eq!(
        transform("rotate-x-12 -skew-x-6", [0., 0.5]),
        "rotateX(12deg) skewX(calc(0deg * 0.5 + (6deg * -1) * 0.5))"
    );
    assert_eq!(
        super::super::Lengths::new(16., 800., 600.).math(
            "calc(0deg * 0.5 + (6deg * -1) * 0.5)",
            0.,
            &[]
        ),
        Some((-3., crate::css::Kind::Angle))
    );
    assert!(transform("skew-x-6", [2., 2.5]).starts_with("matrix3d("));
}

#[test]
fn transform_matrices_round_trip() {
    let lerp = Lerp::new(
        "transform",
        "rotate(30deg) skewX(10deg) scale(2, 3) translate(5px, 6px)",
        "skewY(1deg)",
    )
    .unwrap();
    let at = |t: f64| {
        let mut out = String::new();
        lerp.write(t, &mut out);
        out
    };
    assert_eq!(
        at(0.),
        "matrix(1.73205, 1, -1.04189, 2.86257, 2.40892, 22.1754)"
    );
    assert_eq!(at(1.), "matrix(1, 0.0174551, 0, 1, 0, 0)");
}

#[test]
fn exits_keep_their_subtree_painted_until_they_end() {
    let mut doc = Document::parse(
        "<style>@keyframes spin{to{rotate:1turn}}@keyframes out{to{display:none}}div,i,p,s,u,b,em{display:block}div{transition:opacity 1s linear,display 1s allow-discrete}div.off{display:none;opacity:0}i,em{animation:spin 1s linear infinite}p{transition:opacity 1s linear}div.off p{opacity:0}s{transition:display 1s allow-discrete}div.off s{display:none}b.out{animation:out .5s forwards}a{display:block;opacity:1;transition:opacity 1s linear,display 1s allow-discrete}a.off{display:none}@starting-style{a{opacity:0}}</style><p></p><s><u></u></s><div><i></i></div><b><em></em></b><a class=off></a>",
    );
    let [div, i, p, s, u, b, em, a] =
        ["div", "i", "p", "s", "u", "b", "em", "a"].map(|tag| node(&doc, tag));
    let body = doc.nodes[p].parent.unwrap();
    for child in [s, p] {
        doc.nodes[body].children.retain(|&other| other != child);
        doc.nodes[div].children.push(child);
        doc.nodes[child].parent = Some(div);
    }
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    let mut step = |doc: &Document, seconds: f64| {
        let styles = computed_styles(doc);
        let mut visible = vec![false; doc.nodes.len()];
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            if styles.values[id].get(Prop::DISPLAY) != Some("none") {
                visible[id] = true;
                pending.extend(&doc.nodes[id].children);
            }
        }
        let mut events = Vec::new();
        let (animated, next) = motion.update(
            doc,
            &styles,
            &visible,
            &|_| true,
            &|_| f32::INFINITY,
            at(seconds),
            false,
            &mut events,
        );
        let painted = motion.painted.take();
        let shown = painted.clone().unwrap_or(visible);
        let animated: HashMap<usize, Rc<Style>> = animated
            .into_iter()
            .map(|(id, style, _)| (id, style))
            .collect();
        (painted.is_some(), shown, animated, events, next)
    };
    let (changed, shown, ..) = step(&doc, 0.);
    assert!(!changed && shown[div] && !shown[a]);
    doc.nodes[div].attrs.insert("class".into(), "off".into());
    doc.nodes[a].attrs.remove("class");
    for seconds in [0.1, 0.6] {
        let (changed, shown, animated, events, _) = step(&doc, seconds);
        assert!(changed, "{seconds}");
        assert_eq!(
            [div, i, p, s, u, a].map(|id| shown[id]),
            [true; 6],
            "{events:?}"
        );
        assert!(
            !events.iter().any(|event| event.1.ends_with("cancel")),
            "{events:?}"
        );
        assert!(animated[&i].get(Prop::ROTATE).is_some());
        assert_eq!(animated[&s].get(Prop::DISPLAY), Some("block"));
        let opacity = |id: usize| {
            animated[&id]
                .get(Prop::OPACITY)
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        assert!((opacity(p) + seconds - 1.1).abs() < 1e-3, "{}", opacity(p));
        assert!((opacity(a) - seconds + 0.1).abs() < 1e-3, "{}", opacity(a));
    }
    let (changed, shown, animated, events, _) = step(&doc, 1.2);
    assert!(!changed && [div, i, p, s, u].iter().all(|&id| !shown[id]) && shown[a]);
    assert!(!animated.contains_key(&div) && !animated.contains_key(&i));
    assert!(
        events
            .iter()
            .any(|event| event == &(div, "transitionend", "display".into(), 1.))
    );
    doc.nodes[b].attrs.insert("class".into(), "out".into());
    step(&doc, 2.);
    let (changed, shown, animated, _, next) = step(&doc, 2.6);
    assert_eq!(animated[&b].get(Prop::DISPLAY), Some("none"));
    assert!(changed && !shown[b] && !shown[em] && next.is_none());
}

#[test]
fn transition_duration_alone_transitions_all_properties() {
    let mut doc = Document::parse(
        "<style>div{opacity:1;transition-duration:1s;transition-timing-function:linear}div.on{opacity:0}p{opacity:1;transition-delay:-0.5s;transition-duration:1s;transition-timing-function:linear}p.on{opacity:0}</style><div></div><p></p>",
    );
    let [div, p] = ["div", "p"].map(|tag| node(&doc, tag));
    let start = Instant::now();
    let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let mut motion = Motion::default();
    step(&mut motion, &doc, &computed_styles(&doc), at(0.));
    for id in [div, p] {
        doc.nodes[id].attrs.insert("class".into(), "on".into());
    }
    let styles = computed_styles(&doc);
    step(&mut motion, &doc, &styles, at(0.));
    let (animated, ..) = step(&mut motion, &doc, &styles, at(0.25));
    assert_eq!(animated[&div].0.get(Prop::OPACITY), Some("0.75"));
    assert_eq!(animated[&p].0.get(Prop::OPACITY), Some("0.25"));
    assert!(motion.transitions[&div][0].lerp.is_some());
}

#[test]
fn keyframes_interpolate_filters_keywords_shapes_and_steps() {
    let doc = Document::parse(
        "<style>@keyframes k{from{filter:none;backdrop-filter:blur(2px);font-weight:normal;border-top-width:0px;letter-spacing:normal;clip-path:inset(0px);z-index:-1}to{filter:blur(4px) brightness(50%);backdrop-filter:blur(4px) brightness(2);font-weight:bold;border-top-width:thick;letter-spacing:4px;clip-path:inset(10px 20px);z-index:-4}}div{border-top-style:solid;animation:k 1s linear -0.5s paused}</style><div></div>",
    );
    let styles = computed_styles(&doc);
    let div = node(&doc, "div");
    assert!(
        styles.animations[&div][0]
            .tracks
            .iter()
            .all(|(_, frames)| frames[0].lerp.is_some())
    );
    let (animated, ..) = step(&mut Motion::default(), &doc, &styles, Instant::now());
    let style = &animated[&div].0;
    assert_eq!(
        [
            "filter",
            "backdrop-filter",
            "font-weight",
            "border-top-width",
            "letter-spacing",
            "clip-path",
            "z-index"
        ]
        .map(|name| style.get(Prop::new(name))),
        [
            Some("blur(2px) brightness(75%)"),
            Some("blur(3px) brightness(1.5)"),
            Some("550"),
            Some("2.5px"),
            Some("2px"),
            Some("inset(5px 10px 5px 10px)"),
            Some("-2")
        ]
    );
    let base = styles.values[div].clone();
    let mut animation = Scripted::new(
        1,
        &[(0., "opacity:0", "linear"), (1., "opacity:1", "linear")],
        [1., 0., 1., 1., 0.],
        ["normal", "both", "steps(2, jump-start)", "replace"],
        0,
    );
    animation.effect.tracks = tracks(&base, &animation.rules, &Default::default(), 0, false);
    let opacity = |local: f64| {
        let mut out = Style::new();
        animation
            .effect
            .sample(local, true, &base, &Default::default(), &mut out, 1.);
        out.get(Prop::OPACITY).map(str::to_owned)
    };
    assert_eq!(
        [0.5, 1., 1.6].map(opacity),
        ["0", "0.5", "1"].map(|v| Some(v.into()))
    );
}
