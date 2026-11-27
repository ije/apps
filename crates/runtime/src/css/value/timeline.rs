// Values of scroll-driven animations: `animation-timeline`, `animation-range`, `scroll-timeline`, `view-timeline`
// and `timeline-scope`, in Chrome's form.
use crate::css::value::{grammar::syntax_matches, tokens::components};

const RANGES: [&str; 6] = [
    "cover",
    "contain",
    "entry",
    "exit",
    "entry-crossing",
    "exit-crossing",
];
pub(crate) const AXES: [&str; 4] = ["block", "inline", "x", "y"];
// Each shorthand and its longhands.
const SHORTHANDS: [(&str, &[&str]); 3] = [
    (
        "animation-range",
        &["animation-range-start", "animation-range-end"],
    ),
    (
        "scroll-timeline",
        &["scroll-timeline-name", "scroll-timeline-axis"],
    ),
    (
        "view-timeline",
        &[
            "view-timeline-name",
            "view-timeline-axis",
            "view-timeline-inset",
        ],
    ),
];

fn dashed(word: &str) -> bool {
    word.len() > 2 && word.starts_with("--")
}

fn length(word: &str) -> bool {
    syntax_matches("<length-percentage>", word)
}

// One or two insets, `auto` or a length, the second only when it differs.
pub(crate) fn insets(words: &[&str]) -> Option<[String; 2]> {
    match *words {
        [a] | [a, _] if words.iter().all(|word| *word == "auto" || length(word)) => {
            Some([a.into(), words[words.len() - 1].into()])
        }
        _ => None,
    }
}

fn inset_text([a, b]: &[String; 2]) -> String {
    if a == b {
        a.clone()
    } else {
        format!("{a} {b}")
    }
}

const SCROLLERS: [&str; 3] = ["nearest", "root", "self"];

/// A value of `animation-timeline`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Timeline {
    Auto,
    None,
    Named(String),
    /// The scroller (nearest, root or self) and the axis.
    Scroll(u8, u8),
    /// The axis and the insets.
    View(u8, [String; 2]),
}

impl Timeline {
    pub(crate) fn parse(written: &str) -> Option<Self> {
        let written = written.trim();
        if dashed(written) && !written.contains(' ') {
            return Some(Self::Named(written.into()));
        }
        let item = written.to_ascii_lowercase();
        let (name, args) = match item.as_str() {
            "auto" => return Some(Self::Auto),
            "none" => return Some(Self::None),
            item => item.strip_suffix(')')?.split_once('(')?,
        };
        let mut words = components(args, ' ');
        let find =
            |words: &[&str], list: &[&str]| words.iter().position(|word| list.contains(word));
        let mut axis = || {
            let at = find(&words, &AXES)?;
            if at != 0 && at != words.len() - 1 {
                return None;
            }
            let word = words.remove(at);
            AXES.iter().position(|axis| *axis == word)
        };
        let axis = axis().unwrap_or(0) as u8;
        Some(match name {
            "scroll" => {
                let scroller = find(&words, &SCROLLERS).map(|at| words.remove(at));
                let scroller = SCROLLERS.iter().position(|word| Some(*word) == scroller);
                if !words.is_empty() {
                    return None;
                }
                Self::Scroll(scroller.unwrap_or(0) as u8, axis)
            }
            "view" if words.is_empty() => Self::View(axis, ["auto".into(), "auto".into()]),
            "view" => Self::View(axis, insets(&words)?),
            _ => return None,
        })
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Self::Auto => "auto".into(),
            Self::None => "none".into(),
            Self::Named(name) => name.clone(),
            &Self::Scroll(scroller, axis) => {
                let words = [SCROLLERS[scroller as usize], AXES[axis as usize]];
                let words: Vec<_> = words
                    .into_iter()
                    .filter(|word| !matches!(*word, "nearest" | "block"))
                    .collect();
                format!("scroll({})", words.join(" "))
            }
            Self::View(axis, insets) => {
                let mut words = vec![AXES[*axis as usize].to_owned(), inset_text(insets)];
                words.retain(|word| !matches!(word.as_str(), "block" | "auto"));
                format!("view({})", words.join(" "))
            }
        }
    }
}

/// A range start or end: a range name, 0 for none, and an offset, empty for its edge.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edge(pub(crate) u8, pub(crate) String);

impl Edge {
    fn parse(item: &str, end: bool) -> Option<Self> {
        let named = |word| {
            RANGES
                .iter()
                .position(|name| *name == word)
                .map(|at| at as u8 + 1)
        };
        let edge = if end { "100%" } else { "0%" };
        Some(match components(item, ' ')[..] {
            ["normal"] => Self(0, String::new()),
            [word] if length(word) => Self(0, word.into()),
            [word] => Self(named(word)?, String::new()),
            [name, offset] if length(offset) => Self(
                named(name)?,
                if offset == edge {
                    String::new()
                } else {
                    offset.into()
                },
            ),
            _ => return None,
        })
    }

    fn text(&self) -> String {
        match self.0 {
            0 if self.1.is_empty() => "normal".into(),
            0 => self.1.clone(),
            name => format!("{} {}", RANGES[usize::from(name) - 1], self.1)
                .trim_end()
                .into(),
        }
    }
}

/// A scroll-driven effect: the node that its timeline is read from (its own when None), the timeline, its range,
/// whether its duration is `auto`, and for a script animation its start in percent of the timeline (the start of
/// the range when None), its held time and its rate.
#[derive(Debug)]
pub(crate) struct Scroll {
    pub(crate) at: Option<usize>,
    pub(crate) timeline: Timeline,
    pub(crate) range: [Edge; 2],
    pub(crate) auto: bool,
    pub(crate) start: Option<f64>,
    pub(crate) hold: Option<f64>,
    pub(crate) rate: f64,
}

impl Scroll {
    // From the texts of a timeline and its range.
    pub(crate) fn new(at: Option<usize>, [timeline, start, end]: [&str; 3]) -> Self {
        let edge = |text: &str, end| {
            Edge::parse(&text.trim().to_ascii_lowercase(), end).unwrap_or(Edge(0, String::new()))
        };
        Self {
            at,
            timeline: Timeline::parse(timeline).unwrap_or(Timeline::None),
            range: [edge(start, false), edge(end, true)],
            auto: false,
            start: None,
            hold: None,
            rate: 1.,
        }
    }

    // From the texts of a timeline and its range that a script gives.
    pub(crate) fn of(at: Option<usize>, texts: &[serde_json::Value]) -> Self {
        let text = |at: usize| {
            texts
                .get(at)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("normal")
        };
        Self::new(at, [text(0), text(1), text(2)])
    }

    pub(crate) fn texts(&self) -> [String; 3] {
        [
            self.timeline.text(),
            self.range[0].text(),
            self.range[1].text(),
        ]
    }
}

// The values of the longhands for each item of a value.
pub(crate) fn items(name: &str, value: &str) -> Option<Vec<Vec<String>>> {
    if !name.contains("timeline") && !name.starts_with("animation-range") {
        return None;
    }
    let lower = value.trim().to_ascii_lowercase();
    let list = components(&lower, ',');
    let written = components(value.trim(), ',');
    if list.len() != written.len() {
        return None;
    }
    fn words(item: &str) -> Vec<&str> {
        components(item, ' ')
    }
    let name_of = |word: &str| (word == "none" || dashed(word)).then(|| word.to_owned());
    let mut out = Vec::with_capacity(list.len());
    for (&item, written) in list.iter().zip(written) {
        // Names keep their case.
        let (item, written) = (item.trim(), written.trim());
        out.push(match name {
            "animation-timeline" => vec![Timeline::parse(written)?.text()],
            "animation-range-start" | "animation-range-end" => {
                vec![Edge::parse(item, name.ends_with("end"))?.text()]
            }
            "scroll-timeline-name" | "view-timeline-name" => {
                vec![name_of(written).filter(|_| !written.contains(' '))?]
            }
            "timeline-scope" => vec![
                (dashed(written) && !written.contains(' ') || item == "none" && list.len() == 1)
                    .then(|| written.to_owned())?,
            ],
            "scroll-timeline-axis" | "view-timeline-axis" => {
                vec![AXES.contains(&item).then(|| item.to_owned())?]
            }
            "view-timeline-inset" => vec![inset_text(&insets(&words(item))?)],
            "animation-range" => {
                let parts = words(item);
                // The start takes a name and the length after it, or a length.
                let split = match parts[..] {
                    [first, second, ..] if RANGES.contains(&first) && length(second) => 2,
                    _ => 1,
                };
                let split = split.min(parts.len());
                let start = Edge::parse(&parts[..split].join(" "), false)?;
                let end = match &parts[split..] {
                    [] if start.0 > 0 => Edge(start.0, String::new()),
                    [] => Edge(0, String::new()),
                    rest => Edge::parse(&rest.join(" "), true)?,
                };
                vec![start.text(), end.text()]
            }
            "scroll-timeline" | "view-timeline" => {
                let written = components(written, ' ');
                let mut parts = words(item);
                let named = name_of(written.first()?)?;
                parts.remove(0);
                let axis = parts
                    .iter()
                    .position(|word| AXES.contains(word))
                    .filter(|&at| at == 0 || at == parts.len() - 1)
                    .map_or("block", |at| parts.remove(at))
                    .to_owned();
                match name {
                    "scroll-timeline" if parts.is_empty() => vec![named, axis],
                    "view-timeline" => {
                        let inset = match parts[..] {
                            [] => "auto".to_owned(),
                            _ => inset_text(&insets(&parts)?),
                        };
                        vec![named, axis, inset]
                    }
                    _ => return None,
                }
            }
            _ => return None,
        });
    }
    (!out.is_empty()).then_some(out)
}

// Chrome's text of a value, or None when the value is invalid or not of these properties.
pub(crate) fn specified(name: &str, value: &str) -> Option<String> {
    let items = items(name, value)?;
    Some(
        items
            .iter()
            .map(|parts| match (name, &parts[..]) {
                ("animation-range", [start, end]) => {
                    let named = start.split(' ').next().filter(|word| RANGES.contains(word));
                    match named.map_or(end == "normal" || end == "100%", |named| end == named) {
                        true => start.clone(),
                        false => format!("{start} {end}"),
                    }
                }
                (_, [only]) => only.clone(),
                (_, [named, axis, rest @ ..]) => {
                    let mut out = vec![named.as_str()];
                    out.extend(Some(axis.as_str()).filter(|axis| *axis != "block"));
                    out.extend(
                        rest.iter()
                            .map(String::as_str)
                            .filter(|inset| *inset != "auto"),
                    );
                    out.join(" ")
                }
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

// The longhands of a shorthand with their lists.
pub(crate) fn expand(key: &str, value: &str, out: &mut Vec<(String, String)>) -> bool {
    let Some((_, longhands)) = SHORTHANDS.iter().find(|(name, _)| *name == key) else {
        return false;
    };
    if matches!(
        value.trim(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) {
        out.extend(
            longhands
                .iter()
                .map(|name| (name.to_string(), value.trim().into())),
        );
    } else if let Some(items) = items(key, value) {
        for (index, name) in longhands.iter().enumerate() {
            let list: Vec<_> = items.iter().map(|parts| parts[index].as_str()).collect();
            out.push((name.to_string(), list.join(", ")));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_take_chromes_form() {
        for (name, value, text) in [
            (
                "animation-timeline",
                "scroll(nearest block)",
                Some("scroll()"),
            ),
            (
                "animation-timeline",
                "scroll(y root)",
                Some("scroll(root y)"),
            ),
            ("animation-timeline", "scroll( Y )", Some("scroll(y)")),
            ("animation-timeline", "scroll(root root)", None),
            ("animation-timeline", "view(auto auto)", Some("view()")),
            ("animation-timeline", "view(10px 10px)", Some("view(10px)")),
            ("animation-timeline", "view(20px block)", Some("view(20px)")),
            (
                "animation-timeline",
                "view(block 10% 20px)",
                Some("view(10% 20px)"),
            ),
            ("animation-timeline", "view(1px 2px 3px)", None),
            (
                "animation-timeline",
                "--Foo, scroll(), none",
                Some("--Foo, scroll(), none"),
            ),
            ("animation-timeline", "foo", None),
            ("animation-range-start", "cover 0%", Some("cover")),
            ("animation-range-start", "entry 100%", Some("entry 100%")),
            ("animation-range-start", "cover normal", None),
            ("animation-range-end", "cover 100%", Some("cover")),
            ("animation-range", "entry 0% entry 100%", Some("entry")),
            ("animation-range", "50% 100%", Some("50%")),
            (
                "animation-range",
                "cover 50% normal",
                Some("cover 50% normal"),
            ),
            (
                "animation-range",
                "entry 10% exit 90%",
                Some("entry 10% exit 90%"),
            ),
            ("animation-range", "auto", None),
            ("scroll-timeline", "--a block", Some("--a")),
            ("scroll-timeline", "--a, --b y", Some("--a, --b y")),
            ("scroll-timeline", "x", None),
            ("view-timeline", "--a x 10px 20px", Some("--a x 10px 20px")),
            ("view-timeline", "--a inline auto", Some("--a inline")),
            ("view-timeline-inset", "10px 10px", Some("10px")),
            ("timeline-scope", "--a, none", None),
            ("timeline-scope", "none", Some("none")),
        ] {
            assert_eq!(specified(name, value).as_deref(), text, "{name}: {value}");
        }
        let mut out = Vec::new();
        expand("animation-range", "entry, cover 10% 90%", &mut out);
        assert_eq!(
            out,
            [
                ("animation-range-start".into(), "entry, cover 10%".into()),
                ("animation-range-end".into(), "entry, 90%".into())
            ]
        );
    }

    #[test]
    fn progress_maps_to_local_time() {
        let scripted = crate::style::animation::timing::Scripted::new(
            1,
            &[],
            [0., 0., 1., 2., 0.],
            ["normal", "none", "linear", "replace"],
            0,
        );
        let mut effect = scripted.effect;
        let mut scroll = Scroll::new(None, ["scroll()", "normal", "normal"]);
        effect.scroll = Some(std::rc::Rc::new(Scroll::new(
            None,
            ["scroll()", "normal", "normal"],
        )));
        let at = |time: f64, end: f64| [0., end, time, 0., 100.];
        assert_eq!(effect.scrolled(at(50., 0.)), 1.);
        assert_eq!(effect.scrolled(at(-50., 0.)), -1.);
        // The end of the timeline stays in the active phase.
        assert!(effect.scrolled(at(100., 1.)) < 2.);
        assert_eq!(effect.scrolled(at(100., 0.)), 2.);
        assert_eq!(effect.scrolled([f64::INFINITY, 0., 0., 0., 0.]), 0.);
        // A paused script animation holds its time.
        (scroll.hold, scroll.rate, scroll.start) = (Some(25.), 1., None);
        effect.scroll = Some(std::rc::Rc::new(scroll));
        assert_eq!(effect.scrolled(at(80., 0.)), 0.5);
        effect.iterations = f64::INFINITY;
        assert!(effect.scrolled(at(0.5, 0.)).is_nan());
    }
}
