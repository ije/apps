use crate::html::{Document, Selector, SelectorCache, SelectorList};
use std::collections::BTreeMap;
use std::{borrow::Cow, cmp::Reverse, collections::HashMap, rc::Rc};

#[path = "animation.rs"]
mod animation;
#[path = "color.rs"]
mod colors;
#[path = "css.rs"]
pub(crate) mod css;
pub(crate) use animation::{Animation, Motion};
pub use colors::color;
pub(crate) use css::{FontUnits, Lengths};

pub type Style = BTreeMap<String, String>;

const UA: &str = "
html,body,address,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hr,legend,main,p,pre,search,article,aside,h1,h2,h3,h4,h5,h6,hgroup,nav,section,dd,dl,dt,menu,ol,ul,details,summary,fieldset,optgroup,option,caption,td,th{display:block}
li{display:list-item}
img,svg,canvas,video,audio,iframe,embed,object,input,button,select,textarea,progress,meter{display:inline-block}
head,script,style,title,meta,link,template,noscript,datalist,base,[hidden],input[type=hidden],dialog:not([open]){display:none}
slot{display:contents}
body{margin:8px}
p,dl,ul,ol,menu,pre{margin:1em 0}
blockquote,figure{margin:1em 40px}
ul ul,ul ol,ol ul,ol ol{margin:0}
ul,ol,menu{padding-left:40px}
ul,menu{list-style-type:disc}
ol{list-style-type:decimal}
ul ul,ol ul{list-style-type:circle}
ul ul ul,ol ul ul,ul ol ul,ol ol ul{list-style-type:square}
dd{margin-left:40px}
h1{font-size:2em;margin:.67em 0;font-weight:bold}
h2{font-size:1.5em;margin:.83em 0;font-weight:bold}
h3{font-size:1.17em;margin:1em 0;font-weight:bold}
h4{margin:1.33em 0;font-weight:bold}
h5{font-size:.83em;margin:1.67em 0;font-weight:bold}
h6{font-size:.67em;margin:2.33em 0;font-weight:bold}
b,strong,th{font-weight:bolder}
i,em,cite,var,dfn,address{font-style:italic}
pre,code,kbd,samp,tt{font-family:monospace}
pre,textarea{white-space:pre-wrap}
pre{white-space:pre}
small,sub,sup{font-size:smaller}
sub{vertical-align:sub}
sup{vertical-align:super}
big{font-size:larger}
u,ins{text-decoration:underline}
s,strike,del{text-decoration:line-through}
abbr[title]{text-decoration:underline dotted}
mark{background-color:#ff0;color:#000}
a[href]{color:#0000ee;text-decoration:underline;cursor:pointer}
center,th,caption{text-align:center}
hr{margin:.5em 0;border:1px inset #a0a0a0;color:#a0a0a0}
table{display:grid;width:fit-content;border-spacing:2px;text-indent:0}
thead,tbody,tfoot,tr{display:contents}
table:has(tr>:nth-child(2)){grid-template-columns:repeat(2,auto)}
table:has(tr>:nth-child(3)){grid-template-columns:repeat(3,auto)}
table:has(tr>:nth-child(4)){grid-template-columns:repeat(4,auto)}
table:has(tr>:nth-child(5)){grid-template-columns:repeat(5,auto)}
table:has(tr>:nth-child(6)){grid-template-columns:repeat(6,auto)}
table:has(tr>:nth-child(7)){grid-template-columns:repeat(7,auto)}
table:has(tr>:nth-child(8)){grid-template-columns:repeat(8,auto)}
table:has(tr>:nth-child(9)){grid-template-columns:repeat(9,auto)}
table:has(tr>:nth-child(10)){grid-template-columns:repeat(10,auto)}
table:has(tr>:nth-child(11)){grid-template-columns:repeat(11,auto)}
table:has(tr>:nth-child(12)){grid-template-columns:repeat(12,auto)}
td[colspan='2'],th[colspan='2']{grid-column:span 2}
td[rowspan='2'],th[rowspan='2']{grid-row:span 2}
td[colspan='3'],th[colspan='3']{grid-column:span 3}
td[rowspan='3'],th[rowspan='3']{grid-row:span 3}
td[colspan='4'],th[colspan='4']{grid-column:span 4}
td[rowspan='4'],th[rowspan='4']{grid-row:span 4}
td[colspan='5'],th[colspan='5']{grid-column:span 5}
td[rowspan='5'],th[rowspan='5']{grid-row:span 5}
td[colspan='6'],th[colspan='6']{grid-column:span 6}
td[rowspan='6'],th[rowspan='6']{grid-row:span 6}
caption{grid-column:1/-1}
table[border]{border:1px outset #808080}
table[border] td,table[border] th{border:1px inset #808080}
td,th{padding:1px}
thead,tbody,tfoot,tr{vertical-align:middle}
td,th{vertical-align:inherit}
fieldset{margin:0 2px;padding:.35em .75em .625em;border:2px groove #c0c0c0}
legend{padding:0 2px}
summary{display:list-item;list-style-type:disclosure-closed}
details[open]>summary{list-style-type:disclosure-open}
details:not([open])>:not(summary){display:none}
button,input,select,textarea{font-family:system-ui;font-size:13px;color:#000}
button,select,meter,progress{user-select:none}
button,select{padding:2px 8px;border:1px solid #c4c4c4;border-radius:5px;background-color:#f6f6f6}
input,textarea{padding:2px 4px;border:1px solid #c4c4c4;border-radius:4px;background-color:#fff}
input[type=checkbox],input[type=radio]{width:13px;height:13px;margin:3px;padding:0}
input[type=radio]{border-radius:50%}
input[type=button],input[type=submit],input[type=reset]{padding:2px 8px;border-radius:5px;background-color:#f6f6f6}
summary{list-style-position:inside}
apps-shadow-root{display:block}
button:disabled,input:disabled,select:disabled,textarea:disabled{color:#a0a0a0}
input:focus-visible,textarea:focus-visible{outline:3px solid #8fb8f4}
progress,meter{width:10em;height:1em}
iframe{border:2px inset #c0c0c0}
q::before{content:open-quote}
q::after{content:close-quote}
";

const PREFLIGHT: &str = "
*,::before,::after{box-sizing:border-box;margin:0;padding:0;border:0 solid}
html{line-height:1.5;font-family:ui-sans-serif,system-ui,sans-serif}
h1,h2,h3,h4,h5,h6{font-size:inherit;font-weight:inherit}
a{color:inherit;text-decoration:inherit}
code,kbd,samp,pre{font-family:ui-monospace,monospace;font-size:1em}
small{font-size:80%}
ol,ul,menu{list-style:none}
img,svg,video,canvas,audio,iframe,embed,object{display:block}
img,video{max-width:100%;height:auto}
button,input,select,optgroup,textarea{font:inherit;color:inherit;border-radius:0;background-color:transparent}
hr{height:0;color:inherit;border-top-width:1px}
";

// WARNING: keep sorted; it is searched with binary_search.
const INHERITED: &[&str] = &[
    "-webkit-text-fill-color",
    "accent-color",
    "border-collapse",
    "border-spacing",
    "caret-color",
    "color",
    "color-scheme",
    "cursor",
    "direction",
    "fill",
    "fill-opacity",
    "font-family",
    "font-feature-settings",
    "font-size",
    "font-stretch",
    "font-style",
    "font-variant",
    "font-variant-numeric",
    "font-weight",
    "hyphens",
    "letter-spacing",
    "line-height",
    "list-style-image",
    "list-style-position",
    "list-style-type",
    "overflow-wrap",
    "pointer-events",
    "quotes",
    "stroke",
    "stroke-opacity",
    "stroke-width",
    "tab-size",
    "text-align",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-style",
    "text-decoration-thickness",
    "text-indent",
    "text-shadow",
    "text-transform",
    "text-underline-offset",
    "text-wrap",
    "visibility",
    "white-space",
    "word-break",
    "word-spacing",
];

thread_local! {
    static DEFAULT_SHEETS: [Rc<Stylesheet>; 2] = [UA, PREFLIGHT].map(|source| Rc::new(Stylesheet::parse(source)));
    pub(crate) static FONT_NAMES: std::cell::RefCell<(HashMap<String, String>, HashMap<String, gpui::SharedString>)> = Default::default();
}

#[derive(Clone)]
struct StyleRef(Rc<Style>);

impl PartialEq for StyleRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for StyleRef {}
impl std::hash::Hash for StyleRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Rc::as_ptr(&self.0), state);
    }
}

#[derive(Hash, PartialEq, Eq)]
struct StyleKey {
    scope: usize,
    tag: String,
    classes: Option<String>,
    inline: Option<String>,
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    host: Option<Style>,
    selection: Option<Style>,
}

struct CachedNode {
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    host: Option<Style>,
    selection: Option<Style>,
    value: Rc<CachedStyle>,
}

impl CachedNode {
    fn new(key: &StyleKey, value: Rc<CachedStyle>) -> Self {
        Self {
            parent: key.parent.clone(),
            font: key.font,
            root: key.root,
            host: key.host.clone(),
            selection: key.selection.clone(),
            value,
        }
    }
}

struct CachedStyle {
    style: Rc<Style>,
    lengths: Lengths,
    pseudos: Option<Rc<[Style; 2]>>,
    text_pseudos: Option<Rc<[Option<Style>; 3]>>,
    selection: Option<Style>,
    animations: Option<Vec<Animation>>,
    host: Style,
}

struct StyleScope {
    restricted: bool,
    sheets: Vec<Rc<Stylesheet>>,
    rules: Vec<(SelectorList, Style, i32)>,
    pseudos: Vec<(usize, Selector, usize)>,
    elements: Vec<Vec<usize>>,
    containers: Vec<Option<ContainerQuery>>,
    tailwind: bool,
    theme: Style,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    matches: SelectorCache,
    nodes: Vec<Option<MatchedRules>>,
    // WARNING: the selector memo is keyed by selector address, so variant selectors must live as long as `matches`.
    variants: HashMap<String, Option<Rc<Selector>>>,
}

struct MatchedRules {
    checked: std::cell::Cell<u64>,
    cached: Option<CachedNode>,
    dependencies: Vec<(usize, u64)>,
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    classes: Option<String>,
}

#[derive(Default)]
pub(crate) struct StyleCache {
    viewport: [f32; 2],
    metrics: bool,
    reduced_motion: bool,
    native_theme: u32,
    scopes: Vec<StyleScope>,
    values: HashMap<StyleKey, Rc<CachedStyle>>,
    containers: HashMap<usize, [f32; 2]>,
    queries: HashMap<(usize, usize, usize, usize), (bool, bool)>,
    queried: bool,
}

type ContainerQuery = Rc<[(Option<String>, bool, MediaQuery)]>;

impl StyleCache {
    pub(crate) fn set_containers(&mut self, sizes: HashMap<usize, [f32; 2]>) -> bool {
        if sizes == self.containers {
            return false;
        }
        let changed = self.queried
            && (sizes.len() != self.containers.len()
                || sizes.keys().any(|id| !self.containers.contains_key(id)))
            || self.queries.iter().any(
                |(&(scope, order, index, container), &(inline, matched))| {
                    let query = self
                        .scopes
                        .get(scope)
                        .and_then(|scope| scope.containers.get(order)?.as_ref()?.get(index));
                    query.is_none_or(|(_, _, query)| {
                        sizes.get(&container).is_some_and(|size| {
                            query.matches(
                                size[0],
                                if inline { f32::NAN } else { size[1] },
                                false,
                                0,
                            ) == Some(true)
                        }) != matched
                    })
                },
            );
        self.containers = sizes;
        if changed {
            self.queries.clear();
            for scope in &mut self.scopes {
                for node in scope.nodes.iter_mut().flatten() {
                    node.cached = None;
                }
            }
        }
        changed
    }
}
type CascadeRank = (bool, i32, [usize; 4], usize);

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Declarations(Vec<Declaration>);

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
enum Declaration {
    Property(String, String),
    Apply(Vec<String>),
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Stylesheet(Vec<Rule>);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum Rule {
    Style(SelectorList, Vec<Rule>),
    Media(Vec<MediaQuery>, Vec<Rule>),
    Layer(Vec<String>, Vec<Rule>),
    Supports(String, bool, Vec<Rule>),
    Declarations(Declarations),
    Theme(Declarations),
    Tailwind,
    Keyframes(String, Vec<(f32, Declarations)>),
    Container(Option<String>, bool, MediaQuery, Vec<Rule>),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum MediaQuery {
    All,
    Feature(String, String, String),
    Unknown(String),
    Not(Box<MediaQuery>),
    And(Vec<MediaQuery>),
    Or(Vec<MediaQuery>),
}

/// Properties that `@supports (name: value)` reports as supported. Empty values accept anything.
const SUPPORTED_PROPERTIES: &[(&str, &[&str])] = &[
    ("align-content", &[]),
    ("align-items", &[]),
    ("align-self", &[]),
    ("animation", &[]),
    ("animation-delay", &[]),
    ("animation-direction", &[]),
    ("animation-duration", &[]),
    ("animation-fill-mode", &[]),
    ("animation-iteration-count", &[]),
    ("animation-name", &[]),
    ("animation-play-state", &[]),
    ("animation-timing-function", &[]),
    ("aspect-ratio", &[]),
    ("background", &[]),
    ("background-color", &[]),
    ("background-image", &[]),
    ("background-position", &[]),
    ("background-size", &[]),
    ("border", &[]),
    ("border-block", &[]),
    ("border-bottom", &[]),
    ("border-bottom-left-radius", &[]),
    ("border-bottom-right-radius", &[]),
    ("border-bottom-width", &[]),
    ("border-color", &[]),
    ("border-inline", &[]),
    ("border-left", &[]),
    ("border-left-width", &[]),
    ("border-radius", &[]),
    ("border-right", &[]),
    ("border-right-width", &[]),
    ("border-style", &[]),
    ("border-top", &[]),
    ("border-top-left-radius", &[]),
    ("border-top-right-radius", &[]),
    ("border-top-width", &[]),
    ("border-width", &[]),
    ("bottom", &[]),
    ("box-shadow", &[]),
    ("box-sizing", &["content-box", "border-box"]),
    ("caret-color", &[]),
    ("color", &[]),
    ("color-scheme", &[]),
    ("column-gap", &[]),
    ("container", &[]),
    ("container-name", &[]),
    ("container-type", &["normal", "size", "inline-size"]),
    ("content", &[]),
    ("cursor", &[]),
    ("direction", &["ltr", "rtl"]),
    (
        "display",
        &[
            "none",
            "block",
            "inline",
            "inline-block",
            "flex",
            "inline-flex",
            "grid",
            "inline-grid",
            "contents",
            "list-item",
            "flow-root",
        ],
    ),
    ("fill", &[]),
    ("fill-opacity", &[]),
    ("fill-rule", &[]),
    ("flex", &[]),
    ("flex-basis", &[]),
    ("flex-direction", &[]),
    ("flex-flow", &[]),
    ("flex-grow", &[]),
    ("flex-shrink", &[]),
    ("flex-wrap", &[]),
    ("font", &[]),
    ("font-family", &[]),
    ("font-size", &[]),
    ("font-style", &[]),
    ("font-weight", &[]),
    ("gap", &[]),
    ("grid", &[]),
    ("grid-area", &[]),
    ("grid-auto-columns", &[]),
    ("grid-auto-flow", &[]),
    ("grid-auto-rows", &[]),
    ("grid-column", &[]),
    ("grid-row", &[]),
    ("grid-template", &[]),
    ("grid-template-areas", &[]),
    ("grid-template-columns", &[]),
    ("grid-template-rows", &[]),
    ("height", &[]),
    ("inline-size", &[]),
    ("inset", &[]),
    ("inset-block", &[]),
    ("inset-inline", &[]),
    ("isolation", &["auto", "isolate"]),
    ("justify-content", &[]),
    ("left", &[]),
    ("line-height", &[]),
    ("margin", &[]),
    ("margin-block", &[]),
    ("margin-bottom", &[]),
    ("margin-inline", &[]),
    ("margin-left", &[]),
    ("margin-right", &[]),
    ("margin-top", &[]),
    ("max-height", &[]),
    ("max-width", &[]),
    ("min-height", &[]),
    ("min-width", &[]),
    ("opacity", &[]),
    ("overflow", &[]),
    ("overflow-x", &[]),
    ("overflow-y", &[]),
    ("padding", &[]),
    ("padding-block", &[]),
    ("padding-bottom", &[]),
    ("padding-inline", &[]),
    ("padding-left", &[]),
    ("padding-right", &[]),
    ("padding-top", &[]),
    ("pointer-events", &[]),
    (
        "position",
        &["static", "relative", "absolute", "fixed", "sticky"],
    ),
    ("right", &[]),
    ("row-gap", &[]),
    ("stroke", &[]),
    ("stroke-linecap", &[]),
    ("stroke-linejoin", &[]),
    ("stroke-opacity", &[]),
    ("stroke-width", &[]),
    ("text-align", &[]),
    ("text-shadow", &[]),
    ("top", &[]),
    ("transform", &[]),
    ("transition", &[]),
    ("transition-delay", &[]),
    ("transition-duration", &[]),
    ("transition-property", &[]),
    ("transition-timing-function", &[]),
    ("translate", &[]),
    ("user-select", &[]),
    ("vertical-align", &[]),
    ("visibility", &["visible", "hidden", "collapse"]),
    ("white-space", &[]),
    ("width", &[]),
    ("z-index", &[]),
    ("accent-color", &[]),
    ("all", &[]),
    ("background-clip", &[]),
    ("background-repeat", &[]),
    ("block-size", &[]),
    ("border-block-color", &[]),
    ("border-block-style", &[]),
    ("border-block-width", &[]),
    ("border-bottom-color", &[]),
    ("border-bottom-style", &[]),
    ("border-collapse", &[]),
    ("border-inline-color", &[]),
    ("border-inline-style", &[]),
    ("border-inline-width", &[]),
    ("border-left-color", &[]),
    ("border-left-style", &[]),
    ("border-right-color", &[]),
    ("border-right-style", &[]),
    ("border-spacing", &[]),
    ("border-top-color", &[]),
    ("border-top-style", &[]),
    ("font-variant", &[]),
    ("inset-block-end", &[]),
    ("inset-block-start", &[]),
    ("inset-inline-end", &[]),
    ("inset-inline-start", &[]),
    ("justify-items", &[]),
    ("justify-self", &[]),
    ("line-clamp", &[]),
    ("-webkit-line-clamp", &[]),
    ("list-style", &[]),
    ("list-style-position", &[]),
    ("list-style-type", &[]),
    ("margin-block-end", &[]),
    ("margin-block-start", &[]),
    ("margin-inline-end", &[]),
    ("margin-inline-start", &[]),
    ("max-block-size", &[]),
    ("max-inline-size", &[]),
    ("min-block-size", &[]),
    ("min-inline-size", &[]),
    ("object-fit", &[]),
    ("order", &[]),
    ("outline", &[]),
    ("outline-color", &[]),
    ("outline-offset", &[]),
    ("outline-style", &[]),
    ("outline-width", &[]),
    ("padding-block-end", &[]),
    ("padding-block-start", &[]),
    ("padding-inline-end", &[]),
    ("padding-inline-start", &[]),
    ("place-content", &[]),
    ("place-items", &[]),
    ("place-self", &[]),
    ("text-decoration", &[]),
    ("text-decoration-color", &[]),
    ("text-decoration-line", &[]),
    ("text-decoration-style", &[]),
    ("text-overflow", &[]),
    ("text-transform", &[]),
    ("-webkit-text-fill-color", &[]),
];

pub(crate) fn reduced_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn apps_reduce_motion() -> bool;
        }
        unsafe { apps_reduce_motion() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

impl MediaQuery {
    /// Kleene logic: `None` is unknown and only `Some(true)` matches.
    fn matches(&self, width: f32, height: f32, reduced_motion: bool, theme: u32) -> Option<bool> {
        let (name, op, value) = match self {
            Self::All => return Some(true),
            Self::Unknown(_) => return None,
            Self::Not(query) => {
                return query
                    .matches(width, height, reduced_motion, theme)
                    .map(|v| !v);
            }
            Self::And(queries) | Self::Or(queries) => {
                let and = matches!(self, Self::And(_));
                let mut result = Some(and);
                for query in queries {
                    match query.matches(width, height, reduced_motion, theme) {
                        Some(value) if value != and => return Some(value),
                        None => result = None,
                        _ => {}
                    }
                }
                return result;
            }
            Self::Feature(name, op, value) => (name.as_str(), op.as_str(), value.as_str()),
        };
        let keyword = |values: &[&str], current: &str| match op {
            "" => Some(!matches!(current, "none" | "no-preference")),
            ":" if values.contains(&value) => Some(value == current),
            _ => None,
        };
        let actual = match name {
            "orientation" => {
                return keyword(
                    &["portrait", "landscape"],
                    if height >= width {
                        "portrait"
                    } else {
                        "landscape"
                    },
                );
            }
            "hover" | "any-hover" => return keyword(&["none", "hover"], "hover"),
            "pointer" | "any-pointer" => return keyword(&["none", "coarse", "fine"], "fine"),
            "prefers-color-scheme" => {
                return keyword(
                    &["light", "dark"],
                    if theme & 1 != 0 { "dark" } else { "light" },
                );
            }
            "prefers-contrast" => {
                return keyword(
                    &["no-preference", "more", "less", "custom"],
                    if theme & 2 != 0 {
                        "more"
                    } else {
                        "no-preference"
                    },
                );
            }
            "inverted-colors" => {
                return keyword(
                    &["none", "inverted"],
                    if theme & 4 != 0 { "inverted" } else { "none" },
                );
            }
            "prefers-reduced-transparency" => {
                return keyword(
                    &["no-preference", "reduce"],
                    if theme & 8 != 0 {
                        "reduce"
                    } else {
                        "no-preference"
                    },
                );
            }
            "prefers-reduced-motion" => {
                return keyword(
                    &["no-preference", "reduce"],
                    if reduced_motion {
                        "reduce"
                    } else {
                        "no-preference"
                    },
                );
            }
            "forced-colors" => return keyword(&["none", "active"], "none"),
            "display-mode" => {
                return keyword(
                    &[
                        "browser",
                        "fullscreen",
                        "minimal-ui",
                        "picture-in-picture",
                        "standalone",
                        "window-controls-overlay",
                    ],
                    "standalone",
                );
            }
            "update" => return keyword(&["none", "slow", "fast"], "fast"),
            "scripting" => return keyword(&["none", "initial-only", "enabled"], "enabled"),
            "dynamic-range" | "video-dynamic-range" => {
                return keyword(&["standard", "high"], "standard");
            }
            "color-gamut" => return keyword(&["srgb", "p3", "rec2020"], "srgb"),
            "width" => f64::from(width),
            "height" => f64::from(height),
            "aspect-ratio" => f64::from(width) / f64::from(height),
            "resolution" => 1.,
            "color" => 8.,
            "color-index" | "monochrome" => 0.,
            _ => return None,
        };
        let boundary = match name {
            _ if op.is_empty() => return Some(actual != 0.),
            // NaN base rejects percentages.
            "width" | "height" => {
                f64::from(Lengths::new(16., width, height).resolve(value, f32::NAN)?)
            }
            "aspect-ratio" => {
                let (a, b) = value.split_once('/').unwrap_or((value, "1"));
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                (a >= 0. && b > 0.).then_some(a / b)?
            }
            "resolution" => {
                let number = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                number.parse::<f64>().ok()?
                    / match &value[number.len()..] {
                        "dppx" | "x" => 1.,
                        "dpi" => 96.,
                        "dpcm" => 96. / 2.54,
                        _ => return None,
                    }
            }
            _ => value.parse().ok()?,
        };
        let ordering = actual.partial_cmp(&boundary)?;
        Some(match op {
            ":" | "=" => ordering.is_eq(),
            "<" => ordering.is_lt(),
            "<=" => ordering.is_le(),
            ">" => ordering.is_gt(),
            ">=" => ordering.is_ge(),
            _ => return None,
        })
    }

    fn source(&self) -> String {
        let group = |query: &Self| match query {
            Self::Not(_) | Self::And(_) | Self::Or(_) => format!("({})", query.source()),
            _ => query.source(),
        };
        match self {
            Self::All => "all".into(),
            Self::Unknown(text) => text.clone(),
            Self::Feature(name, op, value) => format!("({name}{op}{value})"),
            Self::Not(query) => format!("not {}", group(query)),
            Self::And(queries) | Self::Or(queries) => {
                queries.iter().map(group).collect::<Vec<_>>().join(
                    if matches!(self, Self::And(_)) {
                        " and "
                    } else {
                        " or "
                    },
                )
            }
        }
    }
}

struct Collect<'a> {
    width: f32,
    height: f32,
    reduced_motion: bool,
    native_theme: u32,
    rules: Vec<(
        &'a SelectorList,
        &'a Declarations,
        i32,
        Option<ContainerQuery>,
    )>,
    container: Option<ContainerQuery>,
    /// Full layer names with their path rank: 10 bits per level, siblings in first-seen order.
    layers: Vec<(String, i32)>,
    tailwind: bool,
    theme: Style,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
}

impl Rule {
    fn bind_parent(&mut self, parent: Option<&SelectorList>) -> anyhow::Result<()> {
        match self {
            Self::Style(selectors, children) => {
                for selector in Rc::make_mut(selectors) {
                    selector.bind_parent(parent)?;
                }
                for child in children {
                    child.bind_parent(Some(selectors))?;
                }
            }
            Self::Media(_, children)
            | Self::Layer(_, children)
            | Self::Supports(_, _, children)
            | Self::Container(.., children) => {
                for child in children {
                    child.bind_parent(parent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn collect<'a>(
        &'a self,
        parent: Option<&'a SelectorList>,
        layer: &str,
        collected: &mut Collect<'a>,
    ) {
        match self {
            Self::Declarations(values) => {
                if let Some(parent) = parent {
                    let rank = collected
                        .layers
                        .iter()
                        .find(|(name, _)| name == layer)
                        .map_or(i32::MAX, |(name, rank)| match name.matches('.').count() {
                            depth @ 0..2 => rank | 1023 << (10 * (1 - depth)),
                            _ => *rank,
                        });
                    collected
                        .rules
                        .push((parent, values, rank, collected.container.clone()));
                }
            }
            Self::Theme(values) => {
                for (key, value) in values.resolve(None, collected.width) {
                    if key.starts_with("--") {
                        collected.theme.insert(key, value);
                    }
                }
            }
            Self::Tailwind => collected.tailwind = true,
            Self::Keyframes(name, frames) => {
                collected.keyframes.insert(name.clone(), frames.clone());
            }
            Self::Style(selectors, children) => {
                for rule in children {
                    rule.collect(Some(selectors), layer, collected);
                }
            }
            Self::Layer(names, children) => {
                let names = if names.is_empty() {
                    vec![format!("__anonymous_{}", collected.layers.len())]
                } else {
                    names.clone()
                };
                for name in names {
                    let name = if layer.is_empty() {
                        name
                    } else {
                        format!("{layer}.{name}")
                    };
                    let mut rank = 0;
                    for (depth, (end, _)) in name
                        .match_indices('.')
                        .chain([(name.len(), "")])
                        .enumerate()
                    {
                        let path = &name[..end];
                        let owner = path.rsplit_once('.').map_or("", |(owner, _)| owner);
                        rank = match collected.layers.iter().find(|(name, _)| name == path) {
                            Some((_, rank)) => *rank,
                            None => {
                                let siblings = collected
                                    .layers
                                    .iter()
                                    .filter(|(name, _)| {
                                        name.rsplit_once('.').map_or("", |(owner, _)| owner)
                                            == owner
                                    })
                                    .count();
                                if depth < 3 {
                                    rank |= (siblings as i32 + 1).min(1022) << (10 * (2 - depth));
                                }
                                collected.layers.push((path.to_owned(), rank));
                                rank
                            }
                        };
                    }
                    for rule in children {
                        rule.collect(parent, &name, collected);
                    }
                }
            }
            Self::Supports(_, supported, children) => {
                if *supported {
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
            Self::Container(name, block, query, children) => {
                let outer = collected.container.clone();
                collected.container = Some(
                    outer
                        .iter()
                        .flat_map(|conditions| conditions.iter().cloned())
                        .chain([(name.clone(), *block, query.clone())])
                        .collect(),
                );
                for rule in children {
                    rule.collect(parent, layer, collected);
                }
                collected.container = outer;
            }
            Self::Media(queries, children) => {
                if queries.is_empty()
                    || queries.iter().any(|query| {
                        query.matches(
                            collected.width,
                            collected.height,
                            collected.reduced_motion,
                            collected.native_theme,
                        ) == Some(true)
                    })
                {
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
        }
    }

    fn source(&self) -> String {
        match self {
            Self::Declarations(values) => values.source(),
            Self::Layer(names, children) => {
                if children.is_empty() {
                    format!("@layer {};", names.join(","))
                } else {
                    format!(
                        "@layer {}{{{}}}",
                        names.join(","),
                        children.iter().map(Self::source).collect::<String>()
                    )
                }
            }
            Self::Supports(condition, _, children) => format!(
                "@supports {condition}{{{}}}",
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Container(name, _, query, children) => format!(
                "@container {}{}{{{}}}",
                name.as_ref()
                    .map_or(String::new(), |name| format!("{name} ")),
                query.source(),
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Theme(values) => format!("@theme{{{}}}", values.source()),
            Self::Tailwind => "@import 'tailwindcss';".into(),
            Self::Keyframes(name, frames) => format!(
                "@keyframes {name}{{{}}}",
                frames
                    .iter()
                    .map(|(at, values)| format!("{}%{{{}}}", at * 100., values.source()))
                    .collect::<String>()
            ),
            Self::Style(selectors, children) => format!(
                "{}{{{}}}",
                selectors
                    .iter()
                    .map(Selector::source)
                    .collect::<Vec<_>>()
                    .join(","),
                children.iter().map(Self::source).collect::<String>(),
            ),
            Self::Media(queries, children) => format!(
                "@media {}{{{}}}",
                queries
                    .iter()
                    .map(MediaQuery::source)
                    .collect::<Vec<_>>()
                    .join(","),
                children.iter().map(Self::source).collect::<String>(),
            ),
        }
    }
}

#[derive(Default)]
pub(crate) struct StyleContext {
    pub hosts: HashMap<usize, Style>,
    pub inherited: HashMap<usize, (Rc<Style>, Lengths)>,
    pub roots: HashMap<usize, (Rc<Style>, Lengths)>,
    pub required: Option<std::collections::HashSet<usize>>,
}

#[derive(Clone)]
pub(crate) struct ComputedStyles {
    pub values: Vec<Rc<Style>>,
    pub lengths: Vec<Lengths>,
    pub pseudos: HashMap<usize, Rc<[Style; 2]>>,
    pub text_pseudos: HashMap<usize, Rc<[Option<Style>; 3]>>,
    pub selections: HashMap<usize, Style>,
    pub user_select: Vec<(UserSelect, usize)>,
    pub animations: HashMap<usize, Vec<Animation>>,
    pub children: HashMap<usize, Vec<usize>>,
    pub host_declarations: Style,
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}

pub fn styles(doc: &Document, width: f32, height: f32) -> Vec<Style> {
    compute_styles(
        doc,
        width,
        height,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values
    .into_iter()
    .map(|style| (*style).clone())
    .collect()
}

pub(crate) fn compute_styles(
    doc: &Document,
    width: f32,
    height: f32,
    text_system: Option<&gpui::TextSystem>,
    cache: &mut HashMap<String, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
) -> ComputedStyles {
    let reduced_motion = reduced_motion();
    let native_theme = crate::browser::theme_state();
    let defaults = DEFAULT_SHEETS.with(Clone::clone);
    if let Some(text) = text_system {
        FONT_NAMES.with(|names| {
            let mut names = names.borrow_mut();
            if names.0.is_empty() {
                names.1.clear();
                names.0 = text
                    .all_font_names()
                    .into_iter()
                    .map(|name| (name.to_ascii_lowercase(), name))
                    .collect();
            }
        });
    }
    let mut collected = Collect {
        width,
        height,
        reduced_motion,
        native_theme,
        rules: Vec::new(),
        container: None,
        layers: Vec::new(),
        tailwind: false,
        theme: Style::new(),
        keyframes: HashMap::new(),
    };
    let mut order = Vec::new();
    let mut sheets = Vec::new();
    let mut pending = vec![0];
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        order.push(id);
        pending.extend(node.children.iter().rev());
        if node.tag == "style" {
            sheets.push(match &node.stylesheet {
                Some(sheet) => sheet.clone(),
                None => {
                    let source = doc.text(id);
                    cache
                        .entry(source.clone())
                        .or_insert_with(|| Rc::new(Stylesheet::parse(&source)))
                        .clone()
                }
            });
        }
    }
    if style_cache.viewport != [width, height]
        || style_cache.metrics != text_system.is_some()
        || style_cache.reduced_motion != reduced_motion
        || style_cache.native_theme != native_theme
        || style_cache.values.len() > 4096
    {
        *style_cache = StyleCache {
            viewport: [width, height],
            metrics: text_system.is_some(),
            reduced_motion,
            native_theme,
            containers: std::mem::take(&mut style_cache.containers),
            ..Default::default()
        };
    }
    let restricted = context.is_some_and(|context| context.required.is_some());
    let scope = style_cache
        .scopes
        .iter()
        .position(|cached| {
            cached.restricted == restricted
                && cached.sheets.len() == sheets.len()
                && cached
                    .sheets
                    .iter()
                    .zip(&sheets)
                    .all(|(a, b)| Rc::ptr_eq(a, b))
        })
        .unwrap_or_else(|| {
            for sheet in &sheets {
                for rule in &sheet.0 {
                    rule.collect(None, "", &mut collected);
                }
            }
            let author = std::mem::take(&mut collected.rules);
            for (index, sheet) in defaults
                .iter()
                .take(1 + usize::from(collected.tailwind))
                .enumerate()
            {
                let start = collected.rules.len();
                for rule in &sheet.0 {
                    rule.collect(None, "", &mut collected);
                }
                for rule in &mut collected.rules[start..] {
                    rule.2 = i32::MIN + 1 + index as i32;
                }
            }
            collected.rules.extend(author);
            let Collect {
                rules,
                tailwind,
                theme,
                keyframes,
                ..
            } = collected;
            let containers = rules.iter().map(|rule| rule.3.clone()).collect();
            let rules: Vec<_> = rules
                .into_iter()
                .map(|(selector, body, layer, _)| {
                    (
                        selector.clone(),
                        body.resolve(tailwind.then_some(&theme), width),
                        layer,
                    )
                })
                .collect();
            let mut pseudos = Vec::new();
            let elements = rules
                .iter()
                .enumerate()
                .map(|(order, (selectors, ..))| {
                    selectors
                        .iter()
                        .enumerate()
                        .filter_map(|(index, selector)| {
                            if let Some((kind, selector)) = selector.without_pseudo_element() {
                                pseudos.push((kind, selector, order));
                                None
                            } else {
                                Some(index)
                            }
                        })
                        .collect()
                })
                .collect();
            style_cache.scopes.push(StyleScope {
                restricted,
                sheets: sheets.clone(),
                rules,
                pseudos,
                elements,
                containers,
                tailwind,
                theme,
                keyframes,
                matches: SelectorCache::default(),
                nodes: Vec::new(),
                variants: HashMap::new(),
            });
            style_cache.scopes.len() - 1
        });
    let scoped = &mut style_cache.scopes[scope];
    let rules = &scoped.rules;
    let tailwind = scoped.tailwind;
    let theme = &scoped.theme;
    let pseudo_rules: Vec<_> = scoped
        .pseudos
        .iter()
        .map(|(kind, selector, order)| {
            let (_, declarations, layer) = &rules[*order];
            (*kind, selector, declarations, *layer, *order)
        })
        .collect();
    scoped.nodes.resize_with(doc.nodes.len(), || None);
    let matches = &mut scoped.matches;
    matches.update(doc, &order);
    let element_selectors: Vec<Vec<_>> = scoped
        .elements
        .iter()
        .zip(rules)
        .map(|(indices, (selectors, ..))| {
            indices
                .iter()
                .map(|&index| (&selectors[index], selectors[index].key()))
                .collect()
        })
        .collect();
    let mut buckets: HashMap<(u8, &str), Vec<usize>> = HashMap::new();
    let mut universal = Vec::new();
    for (index, selectors) in element_selectors.iter().enumerate() {
        for (_, key) in selectors {
            match *key {
                Some(key) => buckets.entry(key).or_default().push(index),
                None => universal.push(index),
            }
        }
    }
    let mut candidates: Vec<usize> = Vec::new();
    let mut utilities = HashMap::new();
    if tailwind {
        for &id in &order {
            if let Some(classes) = doc.nodes[id].attrs.get("class") {
                for class in classes.split_ascii_whitespace() {
                    utilities.entry(class).or_insert_with(|| {
                        let (mut own, mut group, mut rest, mut dark) =
                            (String::new(), String::new(), Vec::new(), false);
                        let mut segments: Vec<&str> = class.split(':').collect();
                        let utility = segments.pop().unwrap_or(class);
                        for variant in segments {
                            let (target, name) = match variant.strip_prefix("group-") {
                                Some(name) => (&mut group, name),
                                None => (&mut own, variant),
                            };
                            let pseudo = match name {
                                "hover" | "focus" | "focus-visible" | "focus-within" | "active"
                                | "visited" | "disabled" | "enabled" | "checked" | "required"
                                | "optional" | "invalid" | "valid" | "placeholder-shown"
                                | "read-only" | "empty" | "first-of-type" | "last-of-type"
                                | "open" | "indeterminate" => format!(":{name}"),
                                "first" | "last" | "only" => format!(":{name}-child"),
                                "odd" | "even" => format!(":nth-child({name})"),
                                "dark" if target.is_empty() && group.is_empty() => {
                                    dark = true;
                                    continue;
                                }
                                _ => {
                                    rest.push(variant);
                                    continue;
                                }
                            };
                            target.push_str(&pseudo);
                        }
                        rest.push(utility);
                        let selector = scoped
                            .variants
                            .entry(class.to_owned())
                            .or_insert_with(|| {
                                (!own.is_empty() || !group.is_empty())
                                    .then(|| {
                                        Selector::parse(
                                            &if group.is_empty() {
                                                format!("*{own}")
                                            } else {
                                                format!(".group{group} *{own}")
                                            },
                                            None,
                                        )
                                        .pop()
                                    })
                                    .flatten()
                                    .map(Rc::new)
                            })
                            .clone();
                        let values = if dark && native_theme & 1 == 0 {
                            None
                        } else {
                            crate::tailwind::utility(&rest.join(":"), width, theme)
                        };
                        (selector, values)
                    });
                }
            }
        }
    }
    let empty = Rc::new(Style::new());
    let mut styles = vec![empty; doc.nodes.len()];
    let mut lengths = vec![Lengths::new(16., width, height); doc.nodes.len()];
    let mut root = FontUnits::new(16.);
    let mut pseudos = HashMap::new();
    let mut text_pseudos = HashMap::new();
    let mut selections: HashMap<usize, Style> = HashMap::new();
    let mut animations = HashMap::new();
    let mut font_metrics = HashMap::new();

    let mut host_declarations = Style::new();
    for id in order {
        if context
            .and_then(|context| context.required.as_ref())
            .is_some_and(|required| !required.contains(&id))
        {
            continue;
        }
        let node = &doc.nodes[id];
        if let Some((style, units)) = context.and_then(|context| context.roots.get(&id)) {
            styles[id] = style.clone();
            lengths[id] = *units;
            continue;
        }
        let inherited = context.and_then(|context| context.inherited.get(&id));
        let parent_style = inherited
            .map(|(style, _)| style)
            .or_else(|| node.parent.map(|parent| &styles[parent]));
        let parent = inherited.map_or_else(
            || node.parent.map_or(FontUnits::new(16.), |p| lengths[p].font),
            |(_, units)| units.font,
        );
        let font_key = [
            parent.em, parent.ch, parent.ex, parent.cap, parent.ic, parent.lh,
        ]
        .map(f32::to_bits);
        let root_font = [root.em, root.ch, root.ex, root.cap, root.ic, root.lh].map(f32::to_bits);
        let host = context.and_then(|context| context.hosts.get(&id));
        let selection = node.parent.and_then(|parent| selections.get(&parent));
        let mut key = None;
        let cached = scoped.nodes[id]
            .as_ref()
            .filter(|node| matches.valid_since(&node.dependencies, &node.checked))
            .and_then(|node| node.cached.as_ref())
            .filter(|cached| {
                cached.parent.as_ref().map(|parent| Rc::as_ptr(&parent.0))
                    == parent_style.map(Rc::as_ptr)
                    && cached.font == font_key
                    && cached.root == root_font
                    && cached.host.as_ref() == host
                    && cached.selection.as_ref() == selection
            })
            .map(|cached| cached.value.clone())
            .or_else(|| {
                let (matched, matched_pseudos, classes) = if let Some(cached) = &scoped.nodes[id]
                    && matches.valid_since(&cached.dependencies, &cached.checked)
                {
                    (
                        cached.rules.clone(),
                        cached.pseudos.clone(),
                        cached.classes.clone(),
                    )
                } else {
                    matches.dependencies.clear();
                    matches.depend(id, 0);
                    candidates.clear();
                    if !node.tag.starts_with('#') {
                        candidates.extend(&universal);
                        let classes = node.attrs.get("class").into_iter();
                        for key in
                            std::iter::once((2, node.tag.as_str()))
                                .chain(node.attrs.get("id").map(|id| (0, id.as_str())))
                                .chain(classes.flat_map(|c| {
                                    c.split_ascii_whitespace().map(|class| (1, class))
                                }))
                                .chain(node.attrs.keys().map(|name| (3, name.as_str())))
                        {
                            if let Some(list) = buckets.get(&key) {
                                candidates.extend(list);
                            }
                        }
                        candidates.sort_unstable();
                        candidates.dedup();
                    }
                    let has = |key: Option<(u8, &str)>| match key {
                        Some((0, value)) => node.attrs.get("id").is_some_and(|id| id == value),
                        Some((1, value)) => node.attrs.get("class").is_some_and(|classes| {
                            classes.split_ascii_whitespace().any(|class| class == value)
                        }),
                        Some((2, value)) => node.tag == value,
                        Some((3, name)) => node.attrs.contains_key(name),
                        _ => true,
                    };
                    let matched: Vec<_> = candidates
                        .iter()
                        .filter_map(|&order| {
                            element_selectors[order]
                                .iter()
                                .filter(|(selector, key)| {
                                    has(*key) && selector.matches_reusing(doc, id, matches)
                                })
                                .map(|(selector, _)| selector.specificity)
                                .max()
                                .map(|specificity| (order, specificity))
                        })
                        .collect();
                    let matched_pseudos: Vec<_> = if node.tag.starts_with('#') {
                        Vec::new()
                    } else {
                        pseudo_rules
                            .iter()
                            .enumerate()
                            .filter_map(|(index, (_, selector, ..))| {
                                (has(selector.key()) && selector.matches_reusing(doc, id, matches))
                                    .then_some(index)
                            })
                            .collect()
                    };
                    let classes =
                        tailwind
                            .then(|| node.attrs.get("class"))
                            .flatten()
                            .map(|classes| {
                                classes
                                    .split_ascii_whitespace()
                                    .filter(|class| match utilities.get(class) {
                                        Some((Some(selector), _)) => {
                                            selector.matches_reusing(doc, id, matches)
                                        }
                                        _ => true,
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            });
                    matches.dependencies.sort_unstable();
                    matches.dependencies.dedup();
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: std::mem::take(&mut matches.dependencies),
                        rules: matched.clone(),
                        pseudos: matched_pseudos.clone(),
                        classes: classes.clone(),
                    });
                    (matched, matched_pseudos, classes)
                };
                let (mut matched, mut matched_pseudos) = (matched, matched_pseudos);
                let mut applies = |order: usize| {
                    let Some(conditions) = &scoped.containers[order] else {
                        return true;
                    };
                    style_cache.queried = true;
                    conditions
                        .iter()
                        .enumerate()
                        .all(|(index, (name, block, query))| {
                            let mut parent = node.parent;
                            while let Some(ancestor) = parent {
                                let style = &styles[ancestor];
                                let kind = style.get("container-type").map_or("", String::as_str);
                                if matches!(kind, "size" | "inline-size")
                                    && (!block || kind == "size")
                                    && name.as_ref().is_none_or(|name| {
                                        style.get("container-name").is_some_and(|names| {
                                            names.split_ascii_whitespace().any(|item| item == name)
                                        })
                                    })
                                {
                                    let inline = kind == "inline-size";
                                    let matched =
                                        style_cache.containers.get(&ancestor).is_some_and(|size| {
                                            query.matches(
                                                size[0],
                                                if inline { f32::NAN } else { size[1] },
                                                false,
                                                0,
                                            ) == Some(true)
                                        });
                                    style_cache
                                        .queries
                                        .insert((scope, order, index, ancestor), (inline, matched));
                                    return matched;
                                }
                                parent = doc.nodes[ancestor].parent;
                            }
                            false
                        })
                };
                matched.retain(|(order, _)| applies(*order));
                matched_pseudos.retain(|index| applies(pseudo_rules[*index].4));
                let resolved = StyleKey {
                    scope,
                    tag: node.tag.clone(),
                    classes,
                    inline: node
                        .inline
                        .as_ref()
                        .map(Declarations::source)
                        .or_else(|| node.attrs.get("style").cloned()),
                    parent: parent_style.cloned().map(StyleRef),
                    font: font_key,
                    root: root_font,
                    rules: matched,
                    pseudos: matched_pseudos,
                    host: host.cloned(),
                    selection: selection.cloned(),
                };
                let value = style_cache.values.get(&resolved).cloned();
                key = Some(resolved);
                value
            });
        if let Some(cached) = cached {
            if let Some(key) = &key {
                scoped.nodes[id].as_mut().unwrap().cached =
                    Some(CachedNode::new(key, cached.clone()));
            }
            styles[id] = cached.style.clone();
            lengths[id] = cached.lengths;
            if id == 0 || node.tag == "html" {
                root = cached.lengths.font;
            }
            if let Some(values) = &cached.pseudos {
                pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.text_pseudos {
                text_pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.selection {
                selections.insert(id, values.clone());
            }
            if let Some(values) = &cached.animations {
                animations.insert(id, values.clone());
            }
            if node.tag == "apps-shadow-root" {
                host_declarations = cached.host.clone();
            }
            continue;
        }
        let key = key.unwrap();
        let mut style = Style::new();
        if id == 0 && tailwind {
            style = theme.clone();
        }
        if let Some(parent) = parent_style {
            let leaf = matches!(node.tag.as_str(), "#text" | "#comment");
            for (key, value) in parent.iter() {
                if key.starts_with("--") && !leaf || INHERITED.binary_search(&key.as_str()).is_ok()
                {
                    style.insert(key.clone(), value.clone());
                }
            }
        }
        let mut cascade: BTreeMap<Cow<str>, (CascadeRank, Cow<str>)> = BTreeMap::new();
        if let Some(classes) = &key.classes {
            let mut matched: Vec<_> = classes
                .split_ascii_whitespace()
                .filter_map(|class| {
                    utilities
                        .get(class)?
                        .1
                        .as_ref()
                        .map(|(breakpoint, values)| (class, *breakpoint, values))
                })
                .collect();
            matched.sort_unstable_by_key(|(class, breakpoint, values)| {
                (*breakpoint, Reverse(values.len()), *class)
            });
            let mut values = Style::new();
            for (_, _, utility) in matched {
                values.extend(
                    utility
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            for (key, value) in Declarations(
                values
                    .into_iter()
                    .map(|(key, value)| Declaration::Property(key, value))
                    .collect(),
            )
            .resolve(None, width)
            {
                cascade.insert(key.into(), ((false, i32::MIN + 3, [0; 4], 0), value.into()));
            }
        }
        for &(order, [ids, classes, tags]) in &key.rules {
            let (_, declarations, layer) = &rules[order];
            for (key, value) in declarations {
                let important = value.trim_end().ends_with("!important");
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    order,
                );
                if cascade
                    .get(key.as_str())
                    .is_none_or(|(old, _)| rank >= *old)
                {
                    cascade.insert(
                        key.as_str().into(),
                        (rank, value.trim_end_matches("!important").trim().into()),
                    );
                }
            }
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (key, value) in inline.resolve(tailwind.then_some(theme), width) {
                let rank = (
                    value.trim_end().ends_with("!important"),
                    i32::MAX,
                    [1, 0, 0, 0],
                    rules.len(),
                );
                if cascade
                    .get(key.as_str())
                    .is_none_or(|(old, _)| rank >= *old)
                {
                    cascade.insert(
                        key.into(),
                        (
                            rank,
                            value
                                .trim_end_matches("!important")
                                .trim()
                                .to_owned()
                                .into(),
                        ),
                    );
                }
            }
        }
        if let Some(declarations) = context.and_then(|context| context.hosts.get(&id)) {
            for (key, value) in declarations {
                let important = value.ends_with("!important");
                if important
                    || cascade
                        .get(key.as_str())
                        .is_none_or(|((important, layer, ..), _)| {
                            !important && *layer <= i32::MIN + 2
                        })
                {
                    cascade.insert(
                        key.as_str().into(),
                        (
                            (important, 0, [0; 4], 0),
                            value.trim_end_matches("!important").trim().into(),
                        ),
                    );
                }
            }
        }
        if node.tag == "apps-shadow-root" {
            for (key, (rank, value)) in &cascade {
                host_declarations.insert(
                    key.to_string(),
                    if rank.0 {
                        format!("{value}!important")
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        if let Some((rank, value)) = cascade.remove("all") {
            for key in style.keys().filter(|key| !key.starts_with("--")) {
                cascade
                    .entry(key.clone().into())
                    .or_insert((rank, value.clone()));
            }
            for (key, entry) in &mut cascade {
                if entry.0 < rank && !key.starts_with("--") {
                    *entry = (rank, value.clone());
                }
            }
        }
        let explicit_size = cascade.contains_key("font-size");
        for (key, (_, value)) in cascade {
            style.insert(key.into_owned(), value.into_owned());
        }
        resolve_style(&mut style, parent_style.map(Rc::as_ref));
        resolve_keywords(&mut style, parent_style.map(Rc::as_ref));
        if style
            .get("background-clip")
            .is_some_and(|value| value.contains("text"))
            && let Some(ink) = style.remove("background-image").and_then(|image| {
                let body = image.split_once('(')?.1.to_owned();
                components(&body, ',').into_iter().find_map(|part| {
                    components(part, ' ')
                        .into_iter()
                        .find(|token| color(token).is_some())
                        .map(str::to_owned)
                })
            })
        {
            style.insert("color".into(), ink.clone());
            style.insert("-webkit-text-fill-color".into(), ink);
        }
        style.entry("color".into()).or_insert("#171717".into());
        if node.tag == "table" && style.get("display").is_some_and(|value| value == "grid") {
            let spacing = if style
                .get("border-collapse")
                .is_some_and(|value| value == "collapse")
            {
                "0".to_owned()
            } else {
                style
                    .get("border-spacing")
                    .cloned()
                    .unwrap_or_else(|| "2px".into())
            };
            let parts = components(&spacing, ' ');
            if let Some(&column) = parts.first() {
                let row = parts.get(1).copied().unwrap_or(column);
                style.insert("column-gap".into(), column.into());
                style.insert("row-gap".into(), row.into());
            }
        }
        let parent_weight = parent_style
            .and_then(|parent| parent.get("font-weight"))
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(400.);
        if let Some(weight) = style.get_mut("font-weight") {
            let value = match weight.as_str() {
                "normal" => 400.,
                "bold" => 700.,
                "bolder" if parent_weight < 350. => 400.,
                "bolder" if parent_weight < 550. => 700.,
                "bolder" => 900f32.max(parent_weight),
                "lighter" if parent_weight < 550. => 100f32.min(parent_weight),
                "lighter" if parent_weight < 750. => 400.,
                "lighter" => 700.,
                value => value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| (1. ..=1000.).contains(v))
                    .unwrap_or(parent_weight),
            };
            *weight = value.to_string();
        }
        let mut units = Lengths {
            font: parent,
            root,
            viewport: [width, height],
            container: None,
        };
        let size = match style.get("font-size").map_or("medium", String::as_str) {
            "xx-small" => Some(9.),
            "x-small" => Some(10.),
            "small" => Some(13.),
            "medium" => Some(16.),
            "large" => Some(18.),
            "x-large" => Some(24.),
            "xx-large" => Some(32.),
            "xxx-large" => Some(48.),
            "larger" => Some(parent.em * 1.2),
            "smaller" => Some(parent.em / 1.2),
            value => units.resolve(value, parent.em),
        }
        .filter(|size| *size >= 0.)
        .unwrap_or(parent.em);
        let monospace = |style: Option<&Style>| {
            style
                .and_then(|style| style.get("font-family"))
                .is_some_and(|family| family.trim().eq_ignore_ascii_case("monospace"))
        };
        let size = if !explicit_size
            && monospace(Some(&style))
            && !monospace(parent_style.map(Rc::as_ref))
        {
            size * 13. / 16.
        } else {
            size
        };
        units.font = FontUnits::new(size);
        if let Some(text) = text_system {
            let font = font(&style);
            let metrics = font_metrics.entry(font.clone()).or_insert_with(|| {
                let id = text.resolve_font(&font);
                let size = gpui::px(1.);
                [
                    text.ch_advance(id, size).map(f32::from).unwrap_or(0.5),
                    f32::from(text.x_height(id, size)),
                    f32::from(text.cap_height(id, size)),
                    text.advance(id, size, '水')
                        .map(|s| f32::from(s.width))
                        .unwrap_or(1.),
                    f32::from(text.ascent(id, size) - text.descent(id, size)),
                ]
            });
            let [ch, ex, cap, ic, lh] = metrics.map(|value| value * size);
            units.font = FontUnits {
                em: size,
                ch,
                ex,
                cap,
                ic,
                lh,
            };
        }
        if let Some(value) = style.get("line-height") {
            let mut line_units = units;
            line_units.font.lh = parent.lh;
            if node.tag == "html" {
                line_units.root = units.font;
                line_units.root.lh = parent.lh;
            }
            if let Some((value, dimensional)) = line_units.parse(value, size)
                && value >= 0.
            {
                units.font.lh = if dimensional { value } else { value * size };
                style.insert(
                    "line-height".into(),
                    if dimensional {
                        format!("{value}px")
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        if id == 0 || node.tag == "html" {
            root = units.font;
        }
        units.root = root;
        style.insert("font-size".into(), format!("{size}px"));
        let mut pseudo_cascade: [BTreeMap<&str, (CascadeRank, &str)>; 6] = Default::default();
        for &index in &key.pseudos {
            let (pseudo, selector, declarations, layer, order) = &pseudo_rules[index];
            let [ids, classes, tags] = selector.specificity;
            for (key, value) in *declarations {
                let important = value.trim_end().ends_with("!important");
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    *order,
                );
                if pseudo_cascade[*pseudo]
                    .get(key.as_str())
                    .is_none_or(|(old, _)| rank >= *old)
                {
                    pseudo_cascade[*pseudo].insert(
                        key.as_str(),
                        (rank, value.trim_end_matches("!important").trim()),
                    );
                }
            }
        }
        let [before, after, selected, marker, letter, line] = pseudo_cascade;
        if [&marker, &letter, &line]
            .iter()
            .any(|cascade| !cascade.is_empty())
        {
            let values = [marker, letter, line].map(|cascade| {
                (!cascade.is_empty()).then(|| {
                    let mut values: Style = style
                        .iter()
                        .filter(|(key, _)| {
                            key.starts_with("--") || INHERITED.binary_search(&key.as_str()).is_ok()
                        })
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    for (key, (_, value)) in cascade {
                        values.insert(key.to_owned(), value.to_owned());
                    }
                    resolve_style(&mut values, Some(&style));
                    resolve_keywords(&mut values, Some(&style));
                    values
                })
            });
            text_pseudos.insert(id, Rc::new(values));
        }
        if !selected.is_empty()
            || node
                .parent
                .is_some_and(|parent| selections.contains_key(&parent))
        {
            let parent = node.parent.and_then(|parent| selections.get(&parent));
            let mut values = parent.cloned().unwrap_or_default();
            values.extend(
                style
                    .iter()
                    .filter(|(key, _)| key.starts_with("--") || key.as_str() == "color-scheme")
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            for (key, (_, value)) in selected {
                if matches!(
                    key,
                    "color" | "background-color" | "-webkit-text-fill-color"
                ) {
                    let value = if matches!(value, "inherit" | "unset") {
                        parent
                            .and_then(|parent| parent.get(key))
                            .cloned()
                            .unwrap_or_else(|| {
                                if key == "background-color" {
                                    "transparent"
                                } else {
                                    "currentColor"
                                }
                                .into()
                            })
                    } else {
                        value.to_owned()
                    };
                    values.insert(key.to_owned(), value);
                }
            }
            resolve_style(&mut values, parent);
            values.retain(|key, value| {
                if !matches!(
                    key.as_str(),
                    "color" | "background-color" | "-webkit-text-fill-color"
                ) {
                    return false;
                }
                if matches!(value.as_str(), "inherit" | "unset") {
                    *value = parent
                        .and_then(|parent| parent.get(key))
                        .cloned()
                        .unwrap_or_else(|| {
                            if key == "background-color" {
                                "transparent"
                            } else {
                                "currentColor"
                            }
                            .into()
                        });
                } else if value == "initial" {
                    *value = if key == "background-color" {
                        "transparent"
                    } else {
                        "currentColor"
                    }
                    .into();
                }
                if value.eq_ignore_ascii_case("currentcolor") || color(value).is_some() {
                    true
                } else if let Some(inherited) = parent.and_then(|parent| parent.get(key)) {
                    *value = inherited.clone();
                    true
                } else {
                    false
                }
            });
            if !values.is_empty() {
                selections.insert(id, values);
            }
        }
        let pseudo_cascade = [before, after];
        if pseudo_cascade
            .iter()
            .any(|values| values.contains_key("content"))
        {
            let values = pseudo_cascade.map(|cascade| {
                let mut values = Style::new();
                for (key, value) in &style {
                    if key.starts_with("--")
                        || matches!(
                            key.as_str(),
                            "color" | "color-scheme" | "font-family" | "font-size" | "line-height"
                        )
                    {
                        values.insert(key.clone(), value.clone());
                    }
                }
                for (key, (_, value)) in cascade {
                    values.insert(key.to_owned(), value.to_owned());
                }
                resolve_style(&mut values, Some(&style));
                resolve_keywords(&mut values, Some(&style));
                values
            });
            pseudos.insert(id, Rc::new(values));
        }
        if style.contains_key("animation") || style.contains_key("animation-name") {
            let effects = Animation::parse(&style, &scoped.keyframes);
            if !effects.is_empty() {
                animations.insert(id, effects);
            }
        }
        let style = Rc::new(style);
        let cached = Rc::new(CachedStyle {
            style: style.clone(),
            lengths: units,
            pseudos: pseudos.get(&id).cloned(),
            text_pseudos: text_pseudos.get(&id).cloned(),
            selection: selections.get(&id).cloned(),
            animations: animations.get(&id).cloned(),
            host: if node.tag == "apps-shadow-root" {
                host_declarations.clone()
            } else {
                Style::new()
            },
        });
        scoped.nodes[id].as_mut().unwrap().cached = Some(CachedNode::new(&key, cached.clone()));
        style_cache.values.insert(key, cached);
        styles[id] = style;
        lengths[id] = units;
    }
    ComputedStyles {
        values: styles,
        lengths,
        pseudos,
        text_pseudos,
        selections,
        user_select: vec![(UserSelect::None, 0); doc.nodes.len()],
        animations,
        children: HashMap::new(),
        host_declarations,
    }
}

fn resolve_style(style: &mut Style, parent: Option<&Style>) {
    style.retain(|key, value| {
        if !key.starts_with("--") || !matches!(value.as_str(), "inherit" | "unset" | "initial") {
            return true;
        }
        match parent
            .and_then(|parent| parent.get(key))
            .filter(|_| value != "initial")
        {
            Some(inherited) => {
                *value = inherited.clone();
                true
            }
            None => false,
        }
    });
    let resolved: Vec<_> = style
        .iter()
        .filter(|(_, value)| value.contains("var("))
        .map(|(key, value)| {
            let mut value = value.clone();
            for _ in 0..64 {
                let Some(start) = value.rfind("var(") else {
                    break;
                };
                let mut depth = 1;
                let Some(end) = value[start + 4..].char_indices().find_map(|(index, c)| {
                    if c == '(' {
                        depth += 1;
                    }
                    if c == ')' {
                        depth -= 1;
                    }
                    (depth == 0).then_some(start + 4 + index)
                }) else {
                    break;
                };
                let (name, fallback) = value[start + 4..end]
                    .split_once(',')
                    .unwrap_or((&value[start + 4..end], ""));
                let resolved = style
                    .get(name.trim())
                    .map(String::as_str)
                    .unwrap_or(fallback.trim())
                    .to_owned();
                value.replace_range(start..end + 1, &resolved);
            }
            (key.clone(), value)
        })
        .collect();
    for (key, value) in resolved {
        if let Some((shorthand, value)) = value
            .strip_prefix('\u{1}')
            .and_then(|value| value.split_once('\u{1}'))
        {
            let mut expanded = Vec::new();
            expand(shorthand, value.trim(), &mut expanded);
            match expanded.into_iter().find(|(name, _)| *name == key) {
                Some((_, value)) => style.insert(key, value),
                None => style.remove(&key),
            };
        } else {
            style.insert(key, value);
        }
    }
    let dark = style
        .get("color-scheme")
        .is_some_and(|v| v == "dark" || v == "only dark");
    for (key, value) in &mut *style {
        if key.starts_with("--") {
            continue;
        }
        while let Some(start) = value.rfind("light-dark(") {
            let mut depth = 1;
            let Some(end) = value[start + 11..].char_indices().find_map(|(i, c)| {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                }
                (depth == 0).then_some(start + 11 + i)
            }) else {
                break;
            };
            let parts = components(&value[start + 11..end], ',');
            if parts.len() != 2 {
                break;
            }
            let selected = parts[usize::from(dark)].trim().to_owned();
            value.replace_range(start..=end, &selected);
        }
    }
    let convert = |value: &str, current: &str| {
        if !value.starts_with("color-mix(") && !value.contains("(from ") {
            return None;
        }
        let (rgb, alpha) = color(&value.to_ascii_lowercase().replace("currentcolor", current))?;
        Some(format!("#{rgb:06x}{alpha:02x}"))
    };
    let inherited = parent
        .and_then(|parent| parent.get("color"))
        .map_or("canvastext", String::as_str);
    if let Some(value) = style
        .get("color")
        .and_then(|value| convert(value, inherited))
    {
        style.insert("color".into(), value);
    }
    let current = style
        .get("color")
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .map_or(inherited, String::as_str);
    let converted: Vec<_> = style
        .iter()
        .filter(|(key, _)| {
            key.as_str() != "color"
                && !key.starts_with("--")
                && (key.ends_with("color") || matches!(key.as_str(), "fill" | "stroke"))
        })
        .filter_map(|(key, value)| Some((key.clone(), convert(value, current)?)))
        .collect();
    style.extend(converted);
}

fn resolve_keywords(style: &mut Style, parent: Option<&Style>) {
    style.retain(|key, value| {
        let inherit = match value.as_str() {
            _ if key.starts_with("--") => return true,
            "inherit" => true,
            "unset" | "revert" | "revert-layer" => INHERITED.binary_search(&key.as_str()).is_ok(),
            "initial" => false,
            _ => return true,
        };
        match parent
            .and_then(|parent| parent.get(key))
            .filter(|_| inherit)
        {
            Some(inherited) => {
                *value = inherited.clone();
                true
            }
            None => false,
        }
    });
    if style
        .get("color")
        .is_some_and(|value| value.eq_ignore_ascii_case("currentcolor"))
    {
        match parent.and_then(|parent| parent.get("color")) {
            Some(inherited) => style.insert("color".into(), inherited.clone()),
            None => style.remove("color"),
        };
    }
    let Some(current) = style.get("color").cloned() else {
        return;
    };
    for (key, value) in &mut *style {
        if key.starts_with("--") || key == "color" || INHERITED.binary_search(&key.as_str()).is_ok()
        {
            continue;
        }
        let mut start = 0;
        while let Some(index) = value.as_bytes()[start..]
            .windows(12)
            .position(|window| window.eq_ignore_ascii_case(b"currentcolor"))
        {
            value.replace_range(start + index..start + index + 12, &current);
            start += index + current.len();
        }
    }
}

pub(crate) fn selection_colors(style: &Style, selection: Option<&Style>) -> (Option<u32>, u32) {
    let normal = style
        .get("color")
        .and_then(|value| color(value))
        .unwrap_or((0, 255));
    let foreground = selection
        .and_then(|style| style.get("color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(normal)
            } else {
                color(value)
            }
        });
    let background = selection
        .and_then(|style| style.get("background-color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .unwrap_or(if foreground.is_some() {
            (0, 0)
        } else {
            (0x4488ff, 85)
        });
    let fill = selection
        .and_then(|style| style.get("-webkit-text-fill-color"))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .or(foreground);
    (
        fill.map(|(color, alpha)| color << 8 | alpha as u32),
        background.0 << 8 | background.1 as u32,
    )
}

#[derive(Clone)]
pub(crate) struct InlineText {
    pub text: Rc<str>,
    pub runs: Vec<gpui::TextRun>,
    pub ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub backgrounds: Vec<(usize, u32)>,
    pub cursors: Vec<(usize, gpui::CursorStyle)>,
    pub shadows: Vec<(f32, f32, f32, u32)>,
    pub first_line: Option<(gpui::TextRun, gpui::TextRun, Option<String>)>,
}

pub(crate) fn restyle(
    runs: &[gpui::TextRun],
    range: std::ops::Range<usize>,
    base: &gpui::TextRun,
    pseudo: &gpui::TextRun,
) -> Vec<gpui::TextRun> {
    let mut restyled = Vec::with_capacity(runs.len() + 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let (a, b) = (range.start.clamp(start, end), range.end.clamp(start, end));
        for (from, to, inside) in [(start, a, false), (a, b, true), (b, end, false)] {
            if from >= to {
                continue;
            }
            let mut piece = run.clone();
            piece.len = to - from;
            if inside {
                if piece.color == base.color {
                    piece.color = pseudo.color;
                }
                if piece.font.family == base.font.family {
                    piece.font.family = pseudo.font.family.clone();
                }
                if piece.font.weight == base.font.weight {
                    piece.font.weight = pseudo.font.weight;
                }
                if piece.font.style == base.font.style {
                    piece.font.style = pseudo.font.style;
                }
                if piece.underline == base.underline {
                    piece.underline = pseudo.underline;
                }
                if piece.strikethrough == base.strikethrough {
                    piece.strikethrough = pseudo.strikethrough;
                }
                if piece.font_size == base.font_size {
                    piece.font_size = pseudo.font_size;
                }
                if piece.letter_spacing == base.letter_spacing {
                    piece.letter_spacing = pseudo.letter_spacing;
                }
                if piece.word_spacing == base.word_spacing {
                    piece.word_spacing = pseudo.word_spacing;
                }
                piece.background_color = piece.background_color.or(pseudo.background_color);
            }
            restyled.push(piece);
        }
        start = end;
    }
    restyled
}

pub(crate) fn text_transform(text: &mut String, range: std::ops::Range<usize>, mode: &str) {
    let Some(source) = text.get(range.clone()) else {
        return;
    };
    let mut word = true;
    let mapped: String = source
        .chars()
        .map(|c| {
            let (mut upper, mut lower) = (c.to_uppercase(), c.to_lowercase());
            let mapped = match mode {
                "uppercase" => upper.next().filter(|_| upper.len() == 0),
                "capitalize" if word => upper.next().filter(|_| upper.len() == 0),
                "lowercase" => lower.next().filter(|_| lower.len() == 0),
                _ => None,
            };
            word = !c.is_alphanumeric();
            mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c)
        })
        .collect();
    text.replace_range(range, &mapped);
}

pub(crate) fn cursor(value: &str) -> Option<gpui::CursorStyle> {
    use gpui::CursorStyle::*;
    Some(match value {
        "default" => Arrow,
        "pointer" => PointingHand,
        "text" => IBeam,
        "vertical-text" => IBeamCursorForVerticalLayout,
        "crosshair" => Crosshair,
        "grab" | "move" | "all-scroll" => OpenHand,
        "grabbing" => ClosedHand,
        "not-allowed" | "no-drop" => OperationNotAllowed,
        "col-resize" => ResizeColumn,
        "row-resize" => ResizeRow,
        "ew-resize" => ResizeLeftRight,
        "ns-resize" => ResizeUpDown,
        "e-resize" => ResizeRight,
        "w-resize" => ResizeLeft,
        "n-resize" => ResizeUp,
        "s-resize" => ResizeDown,
        "nwse-resize" | "nw-resize" | "se-resize" => ResizeUpLeftDownRight,
        "nesw-resize" | "ne-resize" | "sw-resize" => ResizeUpRightDownLeft,
        "alias" => DragLink,
        "copy" => DragCopy,
        "context-menu" => ContextualMenu,
        _ => return Option::None,
    })
}

pub(crate) fn text_run(style: &Style, units: &Lengths, len: usize) -> gpui::TextRun {
    let (ink, alpha) = style
        .get("color")
        .and_then(|value| color(value))
        .unwrap_or((0, 255));
    let ink: gpui::Hsla = gpui::rgba(ink << 8 | alpha as u32).into();
    let lines = style.get("text-decoration-line").map_or("", String::as_str);
    let decoration = style
        .get("text-decoration-color")
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .and_then(|value| color(value))
        .map_or(ink, |(ink, alpha)| {
            gpui::rgba(ink << 8 | alpha as u32).into()
        });
    let thickness = gpui::px(1.);
    gpui::TextRun {
        len,
        font: font(style),
        color: ink,
        background_color: None,
        underline: lines.contains("underline").then(|| gpui::UnderlineStyle {
            thickness,
            color: Some(decoration),
            wavy: style
                .get("text-decoration-style")
                .is_some_and(|value| value == "wavy"),
        }),
        strikethrough: lines
            .contains("line-through")
            .then_some(gpui::StrikethroughStyle {
                thickness,
                color: Some(decoration),
            }),
        letter_spacing: spacing(style, units, "letter-spacing"),
        word_spacing: spacing(style, units, "word-spacing"),
        font_size: None,
    }
}

/// A CSS `letter-spacing` or `word-spacing` value.
pub(crate) fn spacing(style: &Style, units: &Lengths, name: &str) -> gpui::Pixels {
    gpui::px(
        style
            .get(name)
            .filter(|value| *value != "normal")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|value| value.is_finite())
            .unwrap_or(0.),
    )
}

/// Builds one text layout for an element whose content is only text and plain inline elements.
pub(crate) fn inline_text(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
) -> Option<InlineText> {
    let parent = &styles.values[id];
    let own = doc.nodes[id].tag == "#text";
    if !own && doc.nodes[id].children.is_empty()
        || parent.get("display").is_some_and(|value| {
            matches!(
                value.as_str(),
                "grid" | "flex" | "inline-flex" | "inline-grid"
            )
        })
    {
        return None;
    }
    let siblings = doc.nodes[id]
        .parent
        .map_or(&[][..], |parent| &doc.nodes[parent].children[..]);
    let position = siblings
        .iter()
        .position(|&sibling| sibling == id)
        .unwrap_or(0);
    let inline_sibling = |sibling: Option<&usize>| {
        own && sibling.is_some_and(|&sibling| {
            !doc.nodes[sibling].tag.starts_with('#')
                && doc.nodes[sibling].tag != "br"
                && styles.values[sibling]
                    .get("display")
                    .is_none_or(|value| value.starts_with("inline"))
        })
    };
    let trailing = inline_sibling(siblings.get(position + 1));
    let mut text = String::new();
    let mut runs: Vec<gpui::TextRun> = Vec::new();
    let mut ranges = Vec::new();
    let mut backgrounds = Vec::new();
    let mut cursors = Vec::new();
    let mut space = !inline_sibling(
        position
            .checked_sub(1)
            .and_then(|index| siblings.get(index)),
    );
    let mut pending: Vec<_> = if own {
        vec![(id, None, None)]
    } else {
        doc.nodes[id]
            .children
            .iter()
            .rev()
            .map(|&child| (child, None, None))
            .collect()
    };
    while let Some((child, end, background)) = pending.pop() {
        if let Some(index) = end {
            let (_, range): &mut (usize, std::ops::Range<usize>) = &mut ranges[index];
            range.end = text.len();
            continue;
        }
        let node = &doc.nodes[child];
        let style = &styles.values[child];
        if node.tag == "#comment" {
            continue;
        }
        if node.tag == "br" {
            text.push('\n');
            runs.push(text_run(style, &styles.lengths[child], 1));
            space = true;
            continue;
        }
        if node.tag != "#text"
            && (style.get("display").is_some_and(|value| value != "inline")
                || matches!(
                    node.tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "input"
                        | "button"
                        | "select"
                        | "textarea"
                        | "iframe"
                        | "video"
                        | "audio"
                        | "object"
                        | "embed"
                        | "progress"
                        | "meter"
                ))
            || styles.pseudos.contains_key(&child)
            || styles.animations.contains_key(&child)
            || styles.lengths[child].font.em != styles.lengths[id].font.em
            || styles.lengths[child].font.lh != styles.lengths[id].font.lh
            || ["white-space", "visibility", "pointer-events", "text-shadow"]
                .iter()
                .any(|key| style.get(*key) != parent.get(*key))
            || style.iter().any(|(key, value)| {
                !key.starts_with("--")
                    && parent.get(key) != Some(value)
                    && !matches!(
                        value.as_str(),
                        "" | "0"
                            | "0px"
                            | "none"
                            | "normal"
                            | "auto"
                            | "inline"
                            | "baseline"
                            | "static"
                            | "visible"
                            | "border-box"
                            | "content-box"
                            | "transparent"
                            | "currentcolor"
                            | "solid"
                    )
                    && !matches!(
                        key.as_str(),
                        "color"
                            | "font-family"
                            | "font-weight"
                            | "font-style"
                            | "text-decoration-line"
                            | "text-decoration-color"
                            | "text-decoration-style"
                            | "text-decoration-thickness"
                            | "text-underline-offset"
                            | "text-transform"
                            | "color-scheme"
                            | "caret-color"
                            | "cursor"
                            | "fill"
                            | "text-align"
                            | "background-color"
                            | "border-top-left-radius"
                            | "border-top-right-radius"
                            | "border-bottom-right-radius"
                            | "border-bottom-left-radius"
                            | "box-decoration-break"
                            | "user-select"
                            | "-webkit-user-select"
                    )
            })
        {
            return None;
        }
        let index = ranges.len();
        ranges.push((child, text.len()..text.len()));
        if node.tag != "#text"
            && let Some(style) = style
                .get("cursor")
                .filter(|value| Some(*value) != parent.get("cursor"))
                .and_then(|value| cursor(value))
        {
            cursors.push((index, style));
        }
        let inherited_background = background;
        let background = style
            .get("background-color")
            .and_then(|value| colors::color(value))
            .filter(|(_, alpha)| *alpha != 0)
            .or(background);
        if background != inherited_background
            && let Some((color, alpha)) = background
        {
            backgrounds.push((index, color << 8 | alpha as u32));
        }
        pending.push((child, Some(index), background));
        pending.extend(
            node.children
                .iter()
                .rev()
                .map(|&child| (child, None, background)),
        );
        if !node.text.is_empty() {
            let start = text.len();
            let mode = style.get("white-space").map_or("normal", String::as_str);
            let preserve = matches!(mode, "pre" | "pre-wrap" | "break-spaces");
            let transform = style.get("text-transform").map(String::as_str);
            let mut word = true;
            for c in node.text.chars() {
                if c.is_whitespace() && !preserve && !(c == '\n' && mode == "pre-line") {
                    if !space {
                        text.push(' ');
                    }
                    space = true;
                    word = true;
                    continue;
                }
                let mapped = match transform {
                    Some("uppercase") => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("capitalize") if word => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("lowercase") => {
                        let mut mapped = c.to_lowercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    _ => None,
                };
                let c = mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c);
                text.push(c);
                space = c == '\n';
                word = !c.is_alphanumeric();
            }
            if text.len() > start {
                runs.push(text_run(style, &styles.lengths[child], text.len() - start));
            }
        }
    }
    if text.contains(['\t', '\r']) {
        return None;
    }
    if text.ends_with(' ')
        && !trailing
        && !parent
            .get("white-space")
            .is_some_and(|value| matches!(value.as_str(), "pre" | "pre-wrap" | "break-spaces"))
        || text.ends_with('\n')
    {
        text.pop();
        if let Some(run) = runs.last_mut() {
            run.len -= 1;
            if run.len == 0 {
                runs.pop();
            }
        }
        for (_, range) in &mut ranges {
            range.start = range.start.min(text.len());
            range.end = range.end.min(text.len());
        }
    }
    if text.is_empty() {
        return None;
    }
    let pseudo_run = |style: &Style| {
        let units = &styles.lengths[id];
        let mut run = text_run(style, units, 0);
        // `::first-letter` and `::first-line` may change the size inside the line.
        run.font_size = style
            .get("font-size")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| (size - units.font.em).abs() > 0.01)
            .map(gpui::px);
        run.background_color = style
            .get("background-color")
            .and_then(|value| color(value))
            .filter(|(_, alpha)| *alpha != 0)
            .map(|(ink, alpha)| gpui::rgba(ink << 8 | alpha as u32).into());
        run
    };
    let pseudos = styles
        .text_pseudos
        .get(&id)
        .filter(|_| !own && parent.get("display").is_some_and(|value| value != "inline"));
    if let Some(letter) = pseudos.and_then(|pseudos| pseudos[1].as_ref()) {
        let start = text.len() - text.trim_start().len();
        let (mut end, mut found) = (start, false);
        for (index, c) in text[start..].char_indices() {
            if c.is_whitespace() || found && c.is_alphanumeric() {
                break;
            }
            found |= c.is_alphanumeric();
            end = start + index + c.len_utf8();
        }
        if found {
            if let Some(mode) = letter
                .get("text-transform")
                .filter(|mode| Some(*mode) != parent.get("text-transform"))
            {
                text_transform(&mut text, start..end, mode);
            }
            runs = restyle(
                &runs,
                start..end,
                &text_run(parent, &styles.lengths[id], 0),
                &pseudo_run(letter),
            );
        }
    }
    let ink = parent
        .get("color")
        .and_then(|value| color(value))
        .map_or(0xff, |(ink, alpha)| ink << 8 | alpha as u32);
    let shadows = parent
        .get("text-shadow")
        .filter(|value| *value != "none")
        .map_or_else(Vec::new, |value| {
            components(value, ',')
                .into_iter()
                .rev()
                .filter_map(|shadow| {
                    let (mut ink, mut lengths) = (ink, Vec::new());
                    for part in components(shadow, ' ') {
                        match color(part) {
                            Some((color, alpha)) => ink = color << 8 | alpha as u32,
                            None if part.eq_ignore_ascii_case("currentcolor") => {}
                            None => lengths.push(styles.lengths[id].resolve(part, 0.)?),
                        }
                    }
                    match lengths[..] {
                        [x, y] => Some((x, y, 0., ink)),
                        [x, y, blur] => Some((x, y, blur.max(0.), ink)),
                        _ => None,
                    }
                })
                .collect()
        });
    Some(InlineText {
        text: text.into(),
        runs,
        ranges,
        backgrounds,
        cursors,
        shadows,
        first_line: pseudos.and_then(|pseudos| pseudos[2].as_ref()).map(|line| {
            (
                text_run(parent, &styles.lengths[id], 0),
                pseudo_run(line),
                line.get("text-transform")
                    .filter(|mode| Some(*mode) != parent.get("text-transform"))
                    .cloned(),
            )
        }),
    })
}

pub(crate) fn font(style: &Style) -> gpui::Font {
    let families = style.get("font-family").map_or("system-ui", String::as_str);
    let family = FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some(family) = names.1.get(families) {
            return family.clone();
        }
        let family: gpui::SharedString = components(families, ',')
            .into_iter()
            .find_map(|family| {
                let family = family.trim().trim_matches(['\'', '"']);
                let lower = family.to_ascii_lowercase();
                let generic: &[&str] = match lower.as_str() {
                    "system-ui" | "-apple-system" | "blinkmacsystemfont" | "ui-sans-serif"
                    | "sans-serif" | "ui-rounded" => &[".SystemUIFont"],
                    "monospace" | "ui-monospace" => &[
                        "Menlo",
                        "SF Mono",
                        "Consolas",
                        "DejaVu Sans Mono",
                        "Liberation Mono",
                        "Courier New",
                    ],
                    "serif" | "ui-serif" => &[
                        "Times",
                        "Times New Roman",
                        "DejaVu Serif",
                        "Liberation Serif",
                    ],
                    "cursive" => &["Apple Chancery", "Comic Sans MS"],
                    "fantasy" => &["Papyrus", "Impact"],
                    _ => {
                        return if names.0.is_empty() {
                            Some(family.to_owned())
                        } else {
                            names.0.get(&lower).cloned()
                        };
                    }
                };
                generic
                    .iter()
                    .find(|name| {
                        name.starts_with('.') || names.0.contains_key(&name.to_ascii_lowercase())
                    })
                    .or(generic.first().filter(|_| names.0.is_empty()))
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| ".SystemUIFont".into())
            .into();
        if !names.0.is_empty() {
            names.1.insert(families.to_owned(), family.clone());
        }
        family
    });
    let mut font = gpui::font(family);
    let weight = match style.get("font-weight").map(String::as_str) {
        Some("bold") => 700.,
        Some(value) => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(400.),
        None => 400.,
    };
    font.weight = gpui::FontWeight(weight.clamp(1., 1000.));
    font.style = match style.get("font-style").map(String::as_str) {
        Some("italic") => gpui::FontStyle::Italic,
        Some(value) if value.starts_with("oblique") => gpui::FontStyle::Oblique,
        _ => gpui::FontStyle::Normal,
    };
    font
}

pub fn declarations(source: &str) -> Style {
    Declarations::parse(source).resolve(None, 0.)
}

impl Declarations {
    fn resolve(&self, theme: Option<&Style>, width: f32) -> Style {
        let mut result = Style::new();
        for declaration in &self.0 {
            match declaration {
                Declaration::Apply(classes) => {
                    if let Some(theme) = theme {
                        let mut applied: Vec<_> = classes
                            .iter()
                            .map(String::as_str)
                            .filter_map(|class| {
                                crate::tailwind::utility(class, width, theme)
                                    .map(|(breakpoint, values)| (class, breakpoint, values))
                            })
                            .collect();
                        applied.sort_unstable_by_key(|(class, breakpoint, values)| {
                            (*breakpoint, Reverse(values.len()), *class)
                        });
                        for (_, _, values) in applied {
                            for (key, value) in values {
                                if result
                                    .get(&key)
                                    .is_none_or(|v: &String| !v.ends_with("!important"))
                                {
                                    result.insert(key, value);
                                }
                            }
                        }
                    }
                }
                Declaration::Property(key, value) => {
                    let important = value.ends_with("!important");
                    let plain = value.trim_end_matches("!important").trim();
                    let mut expanded = Vec::new();
                    if plain.contains("var(") && expand(key, "initial", &mut expanded) {
                        for (_, value) in &mut expanded {
                            *value = format!("\u{1}{key}\u{1}{plain}");
                        }
                    } else if !expand(key, plain, &mut expanded) {
                        expanded.push((key.clone(), plain.to_owned()));
                    }
                    for (key, value) in expanded {
                        if important || result.get(&key).is_none_or(|v| !v.ends_with("!important"))
                        {
                            result.insert(
                                key,
                                if important {
                                    format!("{value} !important")
                                } else {
                                    value
                                },
                            );
                        }
                    }
                }
            }
        }
        result
    }

    pub(crate) fn source(&self) -> String {
        self.0
            .iter()
            .map(|declaration| match declaration {
                Declaration::Property(key, value) => format!("{key}:{value};"),
                Declaration::Apply(classes) => format!("@apply {};", classes.join(" ")),
            })
            .collect()
    }
}

/// Expands a shorthand or logical property into physical longhands. Returns false for other properties.
fn expand(key: &str, value: &str, out: &mut Vec<(String, String)>) -> bool {
    let alias = match key {
        "inline-size" => "width",
        "block-size" => "height",
        "min-inline-size" => "min-width",
        "max-inline-size" => "max-width",
        "min-block-size" => "min-height",
        "max-block-size" => "max-height",
        "overflow-inline" => "overflow-x",
        "overflow-block" => "overflow-y",
        "word-wrap" => "overflow-wrap",
        "grid-row-gap" => "row-gap",
        "-webkit-background-clip" => "background-clip",
        "grid-column-gap" => "column-gap",
        "border-start-start-radius" => "border-top-left-radius",
        "border-start-end-radius" => "border-top-right-radius",
        "border-end-start-radius" => "border-bottom-left-radius",
        "border-end-end-radius" => "border-bottom-right-radius",
        _ => "",
    };
    if !alias.is_empty() {
        out.push((alias.into(), value.into()));
        return true;
    }
    if let Some((property, rest)) = key
        .split_once("-inline-")
        .or_else(|| key.split_once("-block-"))
        && matches!(
            property,
            "margin" | "padding" | "inset" | "border" | "scroll-margin" | "scroll-padding"
        )
        && let Some((side, rest)) = rest
            .split_once('-')
            .map_or(Some((rest, "")), Some)
            .filter(|(side, _)| matches!(*side, "start" | "end"))
    {
        let side = match (key.contains("-inline-"), side) {
            (true, "start") => "left",
            (true, _) => "right",
            (false, "start") => "top",
            _ => "bottom",
        };
        let physical = match (property, rest) {
            ("inset", _) => side.to_owned(),
            (_, "") => format!("{property}-{side}"),
            _ => format!("{property}-{side}-{rest}"),
        };
        if !expand(&physical, value, out) {
            out.push((physical, value.into()));
        }
        return true;
    }
    const BOX: [&str; 4] = ["top", "right", "bottom", "left"];
    let (names, kind): (Vec<String>, u8) = match key {
        "margin" | "padding" | "scroll-margin" | "scroll-padding" => {
            (BOX.map(|side| format!("{key}-{side}")).into(), 0)
        }
        "inset" => (BOX.map(String::from).into(), 0),
        "border-width" | "border-style" | "border-color" => {
            let suffix = &key[7..];
            (BOX.map(|side| format!("border-{side}-{suffix}")).into(), 0)
        }
        "border-radius" => (
            ["top-left", "top-right", "bottom-right", "bottom-left"]
                .map(|corner| format!("border-{corner}-radius"))
                .into(),
            1,
        ),
        "margin-inline"
        | "padding-inline"
        | "margin-block"
        | "padding-block"
        | "inset-inline"
        | "inset-block"
        | "border-inline-width"
        | "border-inline-style"
        | "border-inline-color"
        | "border-block-width"
        | "border-block-style"
        | "border-block-color" => {
            let (property, rest) = key
                .split_once(if key.contains("-inline") {
                    "-inline"
                } else {
                    "-block"
                })
                .unwrap_or((key, ""));
            let sides = if key.contains("-inline") {
                ["left", "right"]
            } else {
                ["top", "bottom"]
            };
            (
                sides
                    .map(|side| match property {
                        "inset" => side.to_owned(),
                        "border" => format!("border-{side}{rest}"),
                        _ => format!("{property}-{side}"),
                    })
                    .into(),
                2,
            )
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left"
        | "border-inline" | "border-block" | "outline" => {
            let sides: &[&str] = match key {
                "border-top" => &["top"],
                "border-right" => &["right"],
                "border-bottom" => &["bottom"],
                "border-left" => &["left"],
                "border-inline" => &["left", "right"],
                "border-block" => &["top", "bottom"],
                "outline" => &[""],
                _ => &BOX,
            };
            let mut names = Vec::new();
            for side in sides {
                for part in ["width", "style", "color"] {
                    names.push(if side.is_empty() {
                        format!("outline-{part}")
                    } else {
                        format!("border-{side}-{part}")
                    });
                }
            }
            (names, 3)
        }
        "background" => (
            ["color", "image", "position", "size", "repeat", "clip"]
                .map(|part| format!("background-{part}"))
                .into(),
            4,
        ),
        "font" => (
            [
                "font-style",
                "font-variant",
                "font-weight",
                "font-stretch",
                "font-size",
                "line-height",
                "font-family",
            ]
            .map(String::from)
            .into(),
            5,
        ),
        "flex" => (
            ["flex-grow", "flex-shrink", "flex-basis"]
                .map(String::from)
                .into(),
            6,
        ),
        "flex-flow" => (["flex-direction", "flex-wrap"].map(String::from).into(), 7),
        "place-items" | "place-content" | "place-self" => {
            let suffix = &key[6..];
            (
                [format!("align-{suffix}"), format!("justify-{suffix}")].into(),
                2,
            )
        }
        "gap" | "grid-gap" => (["row-gap", "column-gap"].map(String::from).into(), 2),
        "overflow" => (["overflow-x", "overflow-y"].map(String::from).into(), 2),
        "text-decoration" => (
            ["line", "style", "color", "thickness"]
                .map(|part| format!("text-decoration-{part}"))
                .into(),
            8,
        ),
        "list-style" => (
            ["list-style-type", "list-style-position", "list-style-image"]
                .map(String::from)
                .into(),
            9,
        ),
        "container" => (
            ["container-name", "container-type"]
                .map(String::from)
                .into(),
            10,
        ),
        _ => return false,
    };
    if matches!(
        value,
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) {
        out.extend(names.into_iter().map(|name| (name, value.into())));
        return true;
    }
    let parts = components(value, ' ');
    let Some(&first) = parts.first() else {
        return true;
    };
    let length = |part: &str| {
        part.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
            || ["calc(", "min(", "max(", "clamp(", "var("]
                .iter()
                .any(|f| part.starts_with(f))
    };
    let mut values: Vec<String> = match kind {
        0 | 1 => {
            let parts = if kind == 1 {
                let end = parts
                    .iter()
                    .position(|part| *part == "/")
                    .unwrap_or(parts.len());
                &parts[..end.max(1)]
            } else {
                &parts[..]
            };
            let side = |index: usize, fallback: usize| {
                let part = parts.get(index).or(parts.get(fallback)).unwrap_or(&first);
                part.split_once('/')
                    .filter(|_| kind == 1 && !part.contains('('))
                    .map_or(*part, |(part, _)| part)
                    .to_owned()
            };
            vec![side(0, 0), side(1, 0), side(2, 0), side(3, 1)]
        }
        2 => vec![first.into(), parts.get(1).unwrap_or(&first).to_string()],
        3 => {
            let (mut width, mut style, mut color) = (
                "medium".to_owned(),
                "none".to_owned(),
                "currentcolor".to_owned(),
            );
            for &part in &parts {
                if matches!(
                    part,
                    "none"
                        | "hidden"
                        | "dotted"
                        | "dashed"
                        | "solid"
                        | "double"
                        | "groove"
                        | "ridge"
                        | "inset"
                        | "outset"
                        | "auto"
                ) {
                    style = part.into();
                } else if matches!(part, "thin" | "medium" | "thick")
                    || length(part) && !part.starts_with("var(")
                {
                    width = part.into();
                } else {
                    color = part.into();
                }
            }
            width = match width.as_str() {
                "thin" => "1px".into(),
                "medium" => "3px".into(),
                "thick" => "5px".into(),
                _ => width,
            };
            names
                .chunks(3)
                .flat_map(|_| [width.clone(), style.clone(), color.clone()])
                .collect()
        }
        4 => {
            let layers = components(value, ',');
            let mut result = vec![Vec::new(); 6];
            let mut color = "transparent".to_owned();
            for (index, layer) in layers.iter().enumerate() {
                let (mut image, mut position, mut size, mut repeat, mut boxes) = (
                    "none".to_owned(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                );
                let mut after_slash = false;
                for part in components(layer, ' ') {
                    if part == "/" {
                        after_slash = true;
                    } else if let Some((before, after)) =
                        part.split_once('/').filter(|_| !part.contains('('))
                    {
                        position.push(before.to_owned());
                        size.push(after.to_owned());
                        after_slash = true;
                    } else if part.starts_with("url(")
                        || part.contains("gradient(")
                        || part.starts_with("image-set(")
                        || part == "none"
                    {
                        image = part.into();
                    } else if matches!(
                        part,
                        "repeat" | "repeat-x" | "repeat-y" | "no-repeat" | "space" | "round"
                    ) {
                        repeat.push(part);
                    } else if matches!(part, "border-box" | "padding-box" | "content-box" | "text")
                    {
                        boxes.push(part);
                    } else if matches!(part, "scroll" | "fixed" | "local") {
                        continue;
                    } else if after_slash
                        && (length(part) || matches!(part, "auto" | "cover" | "contain"))
                    {
                        size.push(part.into());
                    } else if length(part)
                        || matches!(part, "left" | "right" | "top" | "bottom" | "center")
                    {
                        position.push(part.into());
                    } else if index + 1 == layers.len() {
                        color = part.into();
                    }
                }
                let join = |values: Vec<String>, fallback: &str| {
                    if values.is_empty() {
                        fallback.to_owned()
                    } else {
                        values.join(" ")
                    }
                };
                result[1].push(image);
                result[2].push(join(position, "0% 0%"));
                result[3].push(join(size, "auto"));
                result[4].push(join(
                    repeat.into_iter().map(String::from).collect(),
                    "repeat",
                ));
                result[5].push(boxes.last().unwrap_or(&"border-box").to_string());
            }
            result[0].push(color);
            result.into_iter().map(|values| values.join(", ")).collect()
        }
        5 => {
            if matches!(
                first,
                "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar"
            ) {
                vec![
                    "normal".into(),
                    "normal".into(),
                    "normal".into(),
                    "normal".into(),
                    "13px".into(),
                    "normal".into(),
                    "system-ui".into(),
                ]
            } else {
                let mut values = vec!["normal".to_owned(); 6];
                let mut index = 0;
                while let Some(&part) = parts.get(index) {
                    match part {
                        "italic" => values[0] = part.into(),
                        _ if part.starts_with("oblique") => values[0] = part.into(),
                        "small-caps" => values[1] = part.into(),
                        "bold" | "bolder" | "lighter" => values[2] = part.into(),
                        _ if part.ends_with("condensed") || part.ends_with("expanded") => {
                            values[3] = part.into()
                        }
                        "normal" => {}
                        _ if part.parse::<f32>().is_ok() => values[2] = part.into(),
                        _ => break,
                    }
                    index += 1;
                }
                let Some(size) = parts.get(index) else {
                    return true;
                };
                let (size, mut line) = size
                    .split_once('/')
                    .map_or((*size, None), |(size, line)| (size, Some(line.to_owned())));
                index += 1;
                if line.as_deref() == Some("") || parts.get(index) == Some(&"/") {
                    index += usize::from(parts.get(index) == Some(&"/"));
                    line = parts.get(index).map(|part| part.to_string());
                    index += 1;
                } else if let Some(rest) = parts.get(index).and_then(|part| part.strip_prefix('/'))
                {
                    line = Some(rest.to_owned());
                    index += 1;
                }
                let family = parts.get(index..).unwrap_or_default().join(" ");
                if family.is_empty() {
                    return true;
                }
                values[4] = size.into();
                values[5] = line.unwrap_or_else(|| "normal".into());
                values.push(family);
                values
            }
        }
        6 => match parts.as_slice() {
            ["none"] => vec!["0".into(), "0".into(), "auto".into()],
            ["auto"] => vec!["1".into(), "1".into(), "auto".into()],
            [grow] if grow.parse::<f32>().is_ok() => {
                vec![grow.to_string(), "1".into(), "0%".into()]
            }
            [basis] => vec!["1".into(), "1".into(), basis.to_string()],
            [grow, shrink] if shrink.parse::<f32>().is_ok() => {
                vec![grow.to_string(), shrink.to_string(), "0%".into()]
            }
            [grow, basis] => vec![grow.to_string(), "1".into(), basis.to_string()],
            [grow, shrink, basis, ..] => {
                vec![grow.to_string(), shrink.to_string(), basis.to_string()]
            }
            [] => return true,
        },
        7 => {
            let mut values = vec!["row".to_owned(), "nowrap".to_owned()];
            for &part in &parts {
                values[usize::from(part.contains("wrap"))] = part.into();
            }
            values
        }
        8 => {
            let (mut lines, mut values) = (
                Vec::new(),
                vec![
                    String::new(),
                    "solid".into(),
                    "currentcolor".into(),
                    "auto".into(),
                ],
            );
            for &part in &parts {
                if matches!(
                    part,
                    "none" | "underline" | "overline" | "line-through" | "blink"
                ) {
                    lines.push(part);
                } else if matches!(part, "solid" | "double" | "dotted" | "dashed" | "wavy") {
                    values[1] = part.into();
                } else if length(part) || matches!(part, "auto" | "from-font") {
                    values[3] = part.into();
                } else {
                    values[2] = part.into();
                }
            }
            values[0] = if lines.is_empty() {
                "none".into()
            } else {
                lines.join(" ")
            };
            values
        }
        9 => {
            let mut values = vec!["disc".to_owned(), "outside".to_owned(), "none".to_owned()];
            for &part in &parts {
                match part {
                    "inside" | "outside" => values[1] = part.into(),
                    _ if part.starts_with("url(") => values[2] = part.into(),
                    "none" => values[0] = part.into(),
                    _ => values[0] = part.into(),
                }
            }
            values
        }
        _ => {
            let (name, kind) = value
                .split_once('/')
                .map_or((value, "normal"), |(name, kind)| (name.trim(), kind.trim()));
            vec![name.into(), kind.into()]
        }
    };
    values.truncate(names.len());
    out.extend(names.into_iter().zip(values));
    true
}

impl Stylesheet {
    pub(crate) fn matches_media(&self, width: f32, height: f32) -> bool {
        let reduced_motion = reduced_motion();
        let theme = crate::browser::theme_state();
        matches!(self.0.as_slice(), [Rule::Media(queries, _)] if queries.is_empty() || queries.iter().any(|query| {
            query.matches(width, height, reduced_motion, theme) == Some(true)
        }))
    }

    pub(crate) fn bind_parents(&mut self) -> anyhow::Result<()> {
        for rule in &mut self.0 {
            rule.bind_parent(None)?;
        }
        Ok(())
    }

    pub(crate) fn source(&self) -> String {
        self.0.iter().map(Rule::source).collect()
    }
}

pub fn edges(style: &Style, name: &str, base: f32, font: f32, vw: f32, vh: f32) -> [f32; 4] {
    Lengths::new(font, vw, vh).edges(style, name, base)
}

impl Lengths {
    pub fn edges(&self, style: &Style, name: &str, base: f32) -> [f32; 4] {
        let values: Vec<_> = components(style.get(name).map_or("0", String::as_str), ' ')
            .into_iter()
            .map(|v| self.resolve(v, base).unwrap_or(0.))
            .collect();
        let first = values.first().copied().unwrap_or(0.);
        let mut result = [
            first,
            values.get(1).copied().unwrap_or(first),
            values.get(2).copied().unwrap_or(first),
            values.get(3).or(values.get(1)).copied().unwrap_or(first),
        ];
        for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
            if let Some(value) = style.get(&format!("{name}-{side}")) {
                result[i] = self.resolve(value, base).unwrap_or(0.);
            }
        }
        result
    }
}

pub fn length(value: &str, base: f32, font: f32, vw: f32, vh: f32) -> f32 {
    Lengths::new(font, vw, vh)
        .parse(value, base)
        .map(|(value, _)| value)
        .unwrap_or(0.)
}

pub(crate) fn components(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut input = cssparser::Parser::new(value);
    let mut start = input.position();
    loop {
        let position = input.position();
        let Ok(token) = input.next_including_whitespace() else {
            break;
        };
        if matches!(token, cssparser::Token::Comma) && separator == ','
            || matches!(token, cssparser::Token::WhiteSpace(_)) && separator == ' '
        {
            let part = input.slice(start..position).trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = input.position();
        } else if matches!(
            token,
            cssparser::Token::Function(_)
                | cssparser::Token::ParenthesisBlock
                | cssparser::Token::SquareBracketBlock
                | cssparser::Token::CurlyBracketBlock
        ) && input
            .parse_nested_block(|input| {
                input
                    .expect_no_error_token()
                    .map_err(cssparser::ParseError::<()>::from)
            })
            .is_err()
        {
            return Vec::new();
        }
    }
    let part = input.slice_from(start).trim();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

impl Lengths {
    pub fn shadows(&self, value: &str, inset: bool) -> Vec<gpui::BoxShadow> {
        let mut shadows = Vec::new();
        if value == "none" {
            return shadows;
        }
        for shadow in components(value, ',').into_iter().rev() {
            let mut lengths = Vec::new();
            let mut ink = (0, 255);
            let parts = components(shadow, ' ');
            if parts.contains(&"inset") != inset {
                continue;
            }
            for part in parts.into_iter().filter(|part| *part != "inset") {
                if let Some(color) = color(part) {
                    ink = color;
                } else if let Some(value) = self.resolve(part, self.viewport[0]) {
                    lengths.push(value);
                } else {
                    return Vec::new();
                }
            }
            if !(2..=4).contains(&lengths.len()) {
                return Vec::new();
            }
            shadows.push(gpui::BoxShadow {
                color: gpui::rgba((ink.0 << 8) | u32::from(ink.1)).into(),
                offset: gpui::point(gpui::px(lengths[0]), gpui::px(lengths[1])),
                // WARNING: gpui uses the blur radius as the Gaussian sigma, which is half the CSS blur radius.
                blur_radius: gpui::px(lengths.get(2).copied().unwrap_or(0.).max(0.) / 2.),
                spread_radius: gpui::px(lengths.get(3).copied().unwrap_or(0.)),
                inset,
            });
        }
        shadows
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Filter(pub [[f32; 4]; 3]);

impl Filter {
    pub fn parse(value: &str, units: &Lengths) -> (Option<Self>, f32, Vec<gpui::BoxShadow>) {
        let identity = Self([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        let (mut matrix, mut opacity, mut shadows) =
            (identity, 1f32, Vec::<gpui::BoxShadow>::new());
        for part in components(value, ' ') {
            let Some((name, args)) = part.strip_suffix(')').and_then(|part| part.split_once('('))
            else {
                return (None, 1., Vec::new());
            };
            let args = args.trim();
            let amount = if args.is_empty() {
                Some(1.)
            } else if let Some(percent) = args.strip_suffix('%') {
                percent.trim().parse::<f32>().ok().map(|value| value / 100.)
            } else {
                units
                    .math(args, 0., &[])
                    .filter(|(_, kind)| *kind == css::Kind::Number)
                    .map(|(value, _)| value as f32)
            }
            .filter(|value| value.is_finite() && *value >= 0.);
            let blend = |rows: [[f32; 3]; 3], amount: f32| {
                Self(std::array::from_fn(|i| {
                    let mut row = [0.; 4];
                    for (j, value) in row.iter_mut().take(3).enumerate() {
                        *value = rows[i][j] + amount * (f32::from(i == j) - rows[i][j]);
                    }
                    row
                }))
            };
            let diagonal = |scale: f32, offset: f32| {
                Self(std::array::from_fn(|i| {
                    let mut row = [0., 0., 0., offset];
                    row[i] = scale;
                    row
                }))
            };
            let step = match (name.to_ascii_lowercase().as_str(), amount) {
                ("grayscale", Some(amount)) => {
                    blend([[0.2126, 0.7152, 0.0722]; 3], 1. - amount.min(1.))
                }
                ("sepia", Some(amount)) => blend(
                    [
                        [0.393, 0.769, 0.189],
                        [0.349, 0.686, 0.168],
                        [0.272, 0.534, 0.131],
                    ],
                    1. - amount.min(1.),
                ),
                ("saturate", Some(amount)) => blend([[0.213, 0.715, 0.072]; 3], amount),
                ("invert", Some(amount)) => diagonal(1. - 2. * amount.min(1.), amount.min(1.)),
                ("brightness", Some(amount)) => diagonal(amount, 0.),
                ("contrast", Some(amount)) => diagonal(amount, 0.5 - 0.5 * amount),
                ("opacity", Some(amount)) => {
                    opacity *= amount.min(1.);
                    continue;
                }
                ("hue-rotate", _) => {
                    let Some((angle, _)) = (if args.is_empty() {
                        Some((0., css::Kind::Angle))
                    } else {
                        units.math(args, 0., &[]).filter(|(value, kind)| {
                            *kind == css::Kind::Angle || *kind == css::Kind::Number && *value == 0.
                        })
                    }) else {
                        return (None, 1., Vec::new());
                    };
                    let (sin, cos) = (angle as f32).to_radians().sin_cos();
                    Self([
                        [
                            0.213 + cos * 0.787 - sin * 0.213,
                            0.715 - cos * 0.715 - sin * 0.715,
                            0.072 - cos * 0.072 + sin * 0.928,
                            0.,
                        ],
                        [
                            0.213 - cos * 0.213 + sin * 0.143,
                            0.715 + cos * 0.285 + sin * 0.140,
                            0.072 - cos * 0.072 - sin * 0.283,
                            0.,
                        ],
                        [
                            0.213 - cos * 0.213 - sin * 0.787,
                            0.715 - cos * 0.715 + sin * 0.715,
                            0.072 + cos * 0.928 + sin * 0.072,
                            0.,
                        ],
                    ])
                }
                ("drop-shadow", _) => {
                    let found = units.shadows(args, false);
                    if found.is_empty() {
                        return (None, 1., Vec::new());
                    }
                    shadows.extend(found);
                    continue;
                }
                ("blur" | "url", _) => continue,
                _ => return (None, 1., Vec::new()),
            };
            for shadow in &mut shadows {
                shadow.color = step.hsla(shadow.color);
            }
            matrix = matrix.then(step);
        }
        ((matrix != identity).then_some(matrix), opacity, shadows)
    }

    pub fn then(self, outer: Self) -> Self {
        Self(std::array::from_fn(|i| {
            let mut row = [0., 0., 0., outer.0[i][3]];
            for (j, value) in row.iter_mut().enumerate() {
                *value += (0..3).map(|k| outer.0[i][k] * self.0[k][j]).sum::<f32>();
            }
            row
        }))
    }

    fn rgb(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.0
            .map(|row| (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + row[3]).clamp(0., 1.))
    }

    #[cfg(test)]
    pub fn color(&self, (ink, alpha): (u32, u8)) -> (u32, u8) {
        let [r, g, b] = self
            .rgb([16, 8, 0].map(|shift| ((ink >> shift) & 255) as f32 / 255.))
            .map(|value| (value * 255.).round() as u32);
        (r << 16 | g << 8 | b, alpha)
    }

    pub fn hsla(&self, color: gpui::Hsla) -> gpui::Hsla {
        let color = color.to_rgb();
        let [r, g, b] = self.rgb([color.r, color.g, color.b]);
        gpui::Rgba {
            r,
            g,
            b,
            a: color.a,
        }
        .into()
    }
}

pub(crate) fn clip_path(
    value: &str,
    units: &Lengths,
    size: [f32; 2],
) -> Option<([f32; 4], [f32; 4])> {
    let [width, height] = size;
    let (name, args) = components(value, ' ')
        .into_iter()
        .find_map(|part| part.strip_suffix(')')?.split_once('('))?;
    let tokens = components(args, ' ');
    let split = tokens
        .iter()
        .position(|token| matches!(*token, "round" | "at"))
        .unwrap_or(tokens.len());
    let (head, tail) = (
        &tokens[..split],
        tokens.get(split + 1..).unwrap_or_default(),
    );
    let edges = |values: &[&str]| -> Option<[f32; 4]> {
        let values = match values {
            [a] => [*a; 4],
            [a, b] => [*a, *b, *a, *b],
            [a, b, c] => [*a, *b, *c, *b],
            [a, b, c, d] => [*a, *b, *c, *d],
            _ => return None,
        };
        let mut result = [0.; 4];
        for (side, value) in values.into_iter().enumerate() {
            result[side] = units.resolve(value, if side % 2 == 0 { height } else { width })?;
        }
        Some(result)
    };
    let radii = || {
        let corners = tail.split(|token| *token == "/").next().unwrap_or_default();
        edges(corners)
            .map(|[a, b, c, d]| [a, b, c, d].map(|value| value.max(0.)))
            .unwrap_or_default()
    };
    let rect = |x: f32, y: f32, w: f32, h: f32| [x, y, w.max(0.), h.max(0.)];
    match name.to_ascii_lowercase().as_str() {
        "inset" => {
            let [top, right, bottom, left] = edges(head)?;
            Some((
                rect(left, top, width - left - right, height - top - bottom),
                radii(),
            ))
        }
        "rect" => {
            let [top, right, bottom, left] = <[&str; 4]>::try_from(head).ok()?;
            let edge = |token: &str, base: f32, auto: f32| {
                if token == "auto" {
                    Some(auto)
                } else {
                    units.resolve(token, base)
                }
            };
            let [top, bottom] = [edge(top, height, 0.)?, edge(bottom, height, height)?];
            let [left, right] = [edge(left, width, 0.)?, edge(right, width, width)?];
            Some((rect(left, top, right - left, bottom - top), radii()))
        }
        "xywh" => {
            let [x, y, w, h] = <[&str; 4]>::try_from(head).ok()?;
            let [x, w] = [x, w].map(|value| units.resolve(value, width));
            let [y, h] = [y, h].map(|value| units.resolve(value, height));
            Some((rect(x?, y?, w?, h?), radii()))
        }
        "circle" | "ellipse" => {
            let (x, y) = match tail {
                [only] if matches!(*only, "top" | "bottom") => ("center", *only),
                [only] => (*only, "center"),
                [a, b, ..] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => {
                    (*b, *a)
                }
                [a, b, ..] => (*a, *b),
                [] => ("center", "center"),
            };
            let [x, y] = [(x, width), (y, height)].map(|(token, extent)| match token {
                "left" | "top" => Some(0.),
                "center" => Some(extent / 2.),
                "right" | "bottom" => Some(extent),
                value => units.resolve(value, extent),
            });
            let (x, y) = (x?, y?);
            let sides = [x.abs(), (width - x).abs(), y.abs(), (height - y).abs()];
            let radius = |token: Option<&&str>, near: f32, far: f32, base: f32| match token {
                None | Some(&"closest-side") => Some(near),
                Some(&"farthest-side") => Some(far),
                Some(value) => units.resolve(value, base).map(|value| value.max(0.)),
            };
            let (rx, ry) = if name.eq_ignore_ascii_case("circle") {
                let r = radius(
                    head.first(),
                    sides.into_iter().fold(f32::MAX, f32::min),
                    sides.into_iter().fold(0., f32::max),
                    width.hypot(height) / std::f32::consts::SQRT_2,
                )?;
                (r, r)
            } else {
                (
                    radius(
                        head.first(),
                        sides[0].min(sides[1]),
                        sides[0].max(sides[1]),
                        width,
                    )?,
                    radius(
                        head.get(1),
                        sides[2].min(sides[3]),
                        sides[2].max(sides[3]),
                        height,
                    )?,
                )
            };
            Some((rect(x - rx, y - ry, 2. * rx, 2. * ry), [rx.min(ry); 4]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_queries_parse_and_follow_container_sizes() {
        let css = "@container card (min-width: 400px) { .item { color: red } }
            @container (orientation: landscape) { .item { width: 1px } }
            @container style(--x: 1) { .item { height: 2px } }
            @container card not (width > 100px) { .item { z-index: 3 } }
            @container (300px < inline-size <= 600px) or (width > 2000px) { .item { order: 5 } }";
        let sources: Vec<_> = Stylesheet::parse(css)
            .0
            .iter()
            .map(|rule| match rule {
                Rule::Container(name, block, query, children) => {
                    assert_eq!(children.len(), 1);
                    (name.clone(), *block, query.source().trim().to_owned())
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            sources,
            [
                (Some("card".into()), false, "(width>=400px)".into()),
                (None, true, "(aspect-ratio>1)".into()),
                (None, false, "style(--x: 1)".into()),
                (Some("card".into()), false, "not (width>100px)".into()),
                (
                    None,
                    false,
                    "((width>300px) and (width<=600px)) or (width>2000px)".into()
                ),
            ]
        );
        let doc = Document::parse(&format!(
            "<style>{css}</style><div id=a style='container:card / inline-size'><p class=item></p><div id=b style='container-type:size'><p class=item></p></div></div>"
        ));
        let id = |name: &str| {
            doc.nodes
                .iter()
                .position(|node| node.attrs.get("id").is_some_and(|id| id == name))
                .unwrap()
        };
        let [a, b] = [id("a"), id("b")];
        let [outer, inner] = [doc.nodes[a].children[0], doc.nodes[b].children[0]];
        let mut cache = StyleCache::default();
        let compute = |cache: &mut StyleCache| {
            compute_styles(&doc, 800., 600., None, &mut HashMap::new(), cache, None).values
        };
        assert!(compute(&mut cache)[outer].get("color") != Some(&"red".to_owned()));
        assert!(cache.set_containers(HashMap::from([(a, [500., 50.]), (b, [200., 100.])])));
        let values = compute(&mut cache);
        assert_eq!(values[outer]["color"], "red");
        assert_eq!(values[outer]["order"], "5");
        assert!(!values[outer].contains_key("width") && !values[outer].contains_key("z-index"));
        assert_eq!(values[inner]["color"], "red");
        assert_eq!(values[inner]["width"], "1px");
        assert!(!values[inner].contains_key("order") && !values[inner].contains_key("height"));
        assert!(!cache.set_containers(HashMap::from([(a, [450., 50.]), (b, [200., 100.])])));
        assert!(cache.set_containers(HashMap::from([(a, [90., 50.]), (b, [200., 100.])])));
        let values = compute(&mut cache);
        assert!(values[outer].get("color") != Some(&"red".to_owned()));
        assert_eq!(values[outer]["z-index"], "3");
    }

    #[test]
    fn custom_property_initial_uses_the_var_fallback() {
        let doc = Document::parse(
            "<style>section{--ink:#f00;--background:#fff}.corner{background-color:var(--ink,var(--background))}#reset{--ink:initial}#inherit{--ink:unset}</style><section><div id=reset class=corner><span class=corner></span></div><div id=inherit class=corner></div></section>",
        );
        let styles = styles(&doc, 800., 600.);
        assert_eq!(styles[element(&doc, "#reset")]["background-color"], "#fff");
        assert_eq!(styles[element(&doc, "span")]["background-color"], "#fff");
        assert!(!styles[element(&doc, "#reset")].contains_key("--ink"));
        assert_eq!(
            styles[element(&doc, "#inherit")]["background-color"],
            "#f00"
        );
    }

    #[test]
    fn selector_dependencies_follow_tree_and_attribute_changes() {
        let mut doc = Document::parse(
            "<style>.on span{color:red}i+span{font-weight:700}span:nth-child(2){background-color:yellow}div:has(.mark) span{color:blue}span:empty::before{content:'empty'}</style><div><i>head</i><span id=a>alpha</span><span id=b></span></div>",
        );
        let parent = element(&doc, "div");
        let a = element(&doc, "#a");
        let b = element(&doc, "#b");
        let sibling = element(&doc, "i");
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        for step in 0..45 {
            match step {
                1 => {
                    doc.nodes[parent].attrs.insert("class".into(), "on".into());
                }
                2 => {
                    doc.nodes[b].attrs.insert("class".into(), "mark".into());
                }
                3 => {
                    doc.nodes[parent].children.swap(0, 1);
                }
                4 => {
                    doc.nodes[b].text = "content".into();
                }
                5 => {
                    doc.nodes[sibling].tag = "#text".into();
                }
                6 => {
                    doc.nodes[b].attrs.remove("class");
                    doc.nodes[a].attrs.insert("class".into(), "mark".into());
                }
                7 => {
                    doc.nodes[a].inline = Some(Declarations::parse("color:orange"));
                }
                8 => {
                    doc.nodes[parent].children.retain(|&id| id != a);
                    doc.nodes[b].children.push(a);
                    doc.nodes[a].parent = Some(b);
                }
                _ => {
                    doc.nodes[b].attrs.insert("title".into(), step.to_string());
                }
            }
            let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
            let fresh = compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "mutation {step}");
            assert_eq!(cached.pseudos, fresh.pseudos, "mutation {step}");
        }
    }

    #[test]
    fn inline_runs_keep_node_ranges_and_reject_independent_boxes() {
        let mut doc = Document::parse(
            "<pre style='white-space:pre;cursor:text'><span style='color:red;cursor:text'>A😀</span><span style='background-color:#ffee00'><b>B</b>C</span>\n</pre>",
        );
        let id = element(&doc, "pre");
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let inline = inline_text(&doc, &computed, id).unwrap();
        assert_eq!(&*inline.text, "A😀BC");
        assert_eq!(inline.backgrounds.len(), 1);
        assert_eq!(inline.backgrounds[0].1, 0xffee00ff);
        assert_eq!(
            inline.runs.iter().map(|run| run.len).sum::<usize>(),
            inline.text.len()
        );
        for (id, range) in &inline.ranges {
            assert_eq!(
                &inline.text[range.clone()],
                doc.text(*id).trim_end_matches('\n')
            );
        }
        let span = element(&doc, "span");
        doc.nodes[span].inline = None;
        doc.nodes[span]
            .attrs
            .insert("style".into(), "padding:4px".into());
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert!(inline_text(&doc, &computed, id).is_none());
    }

    #[test]
    fn rounded_inline_highlights_keep_one_text_layout_when_tokens_split() {
        let mut expected = None;
        for content in [
            "<span>  state: </span><span style='background-color:#abcdef;border-radius:3px;box-decoration-break:clone'>ReviewState | 'all'</span>",
            "<span>  </span><span>state</span><span>:</span><span> </span><span style='background-color:#abcdef;border-radius:3px;box-decoration-break:clone'><span>ReviewState</span><span> </span><span>|</span><span> </span><span>'all'</span></span>",
        ] {
            let doc = Document::parse(&format!("<pre style='white-space:pre'>{content}</pre>"));
            let id = element(&doc, "pre");
            let computed = compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut HashMap::new(),
                &mut StyleCache::default(),
                None,
            );
            let inline = inline_text(&doc, &computed, id).unwrap();
            let actual = (
                inline.text.clone(),
                inline
                    .backgrounds
                    .iter()
                    .map(|&(index, color)| (inline.ranges[index].1.clone(), color))
                    .collect::<Vec<_>>(),
            );
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                assert_eq!(&*actual.0, "  state: ReviewState | 'all'");
                assert_eq!(actual.1, vec![(9..28, 0xabcdefff)]);
                expected = Some(actual);
            }
        }
    }

    #[test]
    fn text_pseudo_elements_restyle_runs_and_parse_shadows() {
        let doc = Document::parse(
            "<style>p{color:#111111;text-shadow:1px 2px red,0 0 3px}p::first-letter{color:#ff0000;text-transform:uppercase}p::first-line{font-weight:700;color:#0000ff}li::marker{content:'-';color:blue}b{color:#00ff00}</style><p>\"hi <b>there</b> <span style='text-shadow:inherit'>x</span></p><ul><li>item</li></ul>",
        );
        let (p, li) = (element(&doc, "p"), element(&doc, "li"));
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let marker = computed.text_pseudos[&li][0].as_ref().unwrap();
        assert_eq!(marker["color"], "blue");
        assert!(marker.contains_key("content") && marker.contains_key("font-size"));
        assert!(computed.text_pseudos[&p][0].is_none());
        let inline = inline_text(&doc, &computed, p).unwrap();
        assert_eq!(&*inline.text, "\"Hi there x");
        assert_eq!(
            inline.runs.iter().map(|run| run.len).sum::<usize>(),
            inline.text.len()
        );
        let ink = |color: u32| gpui::Hsla::from(gpui::rgba(color));
        assert_eq!(
            (inline.runs[0].len, inline.runs[0].color),
            (2, ink(0xff0000ff))
        );
        assert_eq!(inline.runs[1].color, ink(0x111111ff));
        assert_eq!(
            inline.shadows,
            vec![(0., 0., 3., 0x111111ff), (1., 2., 0., 0xff0000ff)]
        );
        let (base, pseudo, mode) = inline.first_line.as_ref().unwrap();
        assert_eq!(mode, &None);
        let runs = restyle(&inline.runs, 0..inline.text.len(), base, pseudo);
        let colors: Vec<_> = runs.iter().map(|run| run.color).collect();
        assert_eq!(
            colors[..3],
            [ink(0xff0000ff), ink(0x0000ffff), ink(0x00ff00ff)]
        );
        assert!(
            runs.iter()
                .all(|run| run.font.weight == gpui::FontWeight(700.))
        );
        let doc = Document::parse(
            "<p style='text-shadow:1px 1px red'>a <span style='text-shadow:none'>b</span></p><table><tr><td>a</td><td style='vertical-align:top'>b</td></tr></table>",
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert!(inline_text(&doc, &computed, element(&doc, "p")).is_none());
        assert_eq!(
            computed.values[element(&doc, "td")]["vertical-align"],
            "middle"
        );
        assert_eq!(
            computed.values[element(&doc, "td+td")]["vertical-align"],
            "top"
        );
        let mut text = "hello wörld ß".to_owned();
        text_transform(&mut text, 0..15, "capitalize");
        assert_eq!(text, "Hello Wörld ß");
        text_transform(&mut text, 8..9, "uppercase");
        assert_eq!(text, "Hello Wörld ß");
    }

    #[test]
    fn cached_styles_follow_selectors_inheritance_and_stylesheets() {
        let mut doc = Document::parse(
            "<style>div{--ink:red}span{color:var(--ink)}div:has(b) span{color:blue}span::selection{background-color:var(--ink)}</style><div><span>one</span><span>two</span></div>",
        );
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        let parent = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let children: Vec<_> = doc
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(id, node)| (node.tag == "span").then_some(id))
            .collect();
        let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert!(Rc::ptr_eq(
            &first.values[children[0]],
            &second.values[children[0]]
        ));
        assert!(Rc::ptr_eq(
            &second.values[children[0]],
            &second.values[children[1]]
        ));
        doc.nodes[parent]
            .attrs
            .insert("style".into(), "--ink:green".into());
        let inherited = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(inherited.values[children[0]]["color"], "green");
        assert_eq!(
            inherited.selections[&children[0]]["background-color"],
            "green"
        );
        doc.nodes[children[1]].tag = "b".into();
        let matched = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(matched.values[children[0]]["color"], "blue");
        let sheet = doc
            .nodes
            .iter()
            .position(|node| node.tag == "style")
            .unwrap();
        doc.nodes[sheet].text = "span{color:purple}".into();
        doc.nodes[sheet].stylesheet = None;
        let changed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(changed.values[children[0]]["color"], "purple");
    }

    #[test]
    fn selection_colors_follow_highlight_inheritance() {
        let doc = Document::parse(
            r#"<style>
            div { color: black; background: yellow; --ink: red }
            div::selection { color: white; background: blue; font-size: 80px }
            span { color: green; --ink: purple }
            #inherit::selection { color: inherit; background-color: unset }
            #current::selection { color: currentColor; background-color: currentColor }
            #variable::selection { color: var(--ink); background: transparent }
            #initial::selection { color: initial; background-color: initial }
            </style><div><span id=inherit>inherit</span><span id=current>current</span><span id=variable>variable</span><span id=initial>initial</span></div>"#,
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        for (selector, expected) in [
            ("#inherit", (Some(0xffffffff), 0x0000ffff)),
            ("#current", (Some(0x008000ff), 0x008000ff)),
            ("#variable", (Some(0x800080ff), 0x00000000)),
            ("#initial", (Some(0x008000ff), 0x00000000)),
        ] {
            let id = element(&doc, selector);
            assert_eq!(
                selection_colors(&computed.values[id], computed.selections.get(&id)),
                expected,
                "{selector}"
            );
            assert!(!computed.selections[&id].contains_key("font-size"));
            for child in &doc.nodes[id].children {
                assert_eq!(
                    selection_colors(&computed.values[*child], computed.selections.get(child)),
                    expected,
                    "{selector} text"
                );
            }
        }
    }

    #[test]
    fn selection_colors_do_not_copy_element_backgrounds() {
        for (rule, expected) in [
            ("color: red", (Some(0xff0000ff), 0)),
            ("background: blue", (None, 0x0000ffff)),
            ("color: transparent; background: transparent", (Some(0), 0)),
            (
                "-webkit-text-fill-color: yellow; color: red",
                (Some(0xffff00ff), 0),
            ),
            (
                "font-size: 80px; background-image: linear-gradient(red, blue)",
                (None, 0x4488ff55),
            ),
        ] {
            let doc = Document::parse(&format!(
                "<style>div{{color:green;background:yellow}}div::selection{{{rule}}}</style><div>text</div>"
            ));
            let computed = compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut HashMap::new(),
                &mut StyleCache::default(),
                None,
            );
            let id = element(&doc, "div");
            assert_eq!(
                selection_colors(&computed.values[id], computed.selections.get(&id)),
                expected,
                "{rule}"
            );
        }
    }

    #[test]
    fn editor_media_caret_and_selection_styles() {
        let doc = Document::parse(
            "<style>div{caret-color:red}::selection{background-color:#0000}@media(width>=480px){div{caret-color:#0000}}@media(prefers-reduced-motion:no-preference){div{animation:1s blink}}</style><div><span>text</span></div>",
        );
        let id = doc
            .nodes
            .iter()
            .position(|node| node.text == "text")
            .unwrap();
        let narrow = compute_styles(
            &doc,
            479.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let wide = compute_styles(
            &doc,
            480.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(narrow.values[id]["caret-color"], "red");
        assert_eq!(wide.values[id]["caret-color"], "#0000");
        assert_eq!(wide.selections[&id]["background-color"], "#0000");
        let div = doc.nodes[id]
            .parent
            .and_then(|parent| doc.nodes[parent].parent)
            .unwrap();
        assert_eq!(
            wide.values[div].contains_key("animation"),
            !reduced_motion()
        );
        let motion = |value: &str| {
            MediaQuery::Feature("prefers-reduced-motion".into(), ":".into(), value.into())
                .matches(800., 600., true, 0)
        };
        assert_eq!(motion("reduce"), Some(true));
        assert_eq!(motion("no-preference"), Some(false));
    }

    #[test]
    fn layers_supports_and_generated_content_keep_the_cascade() {
        let doc = Document::parse(
            r#"<style>
            @layer base, theme;
            @layer theme { .item { color: blue; background-color: blue !important; } }
            @layer base { #item { color: red; background-color: red !important; } .item::before { content: ""; width: 4px; background-color: red; } }
            @supports (display:grid) { .item { display:grid; } }
            @supports (width:1cqi) { .item { width:100cqi; } }
            @media (pointer:fine) { .item { cursor:pointer; } }
        </style><div id="item" class="item"></div>"#,
        );
        let id = element(&doc, "#item");
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(computed.values[id]["color"], "blue");
        assert_eq!(computed.values[id]["background-color"], "red");
        assert_eq!(computed.values[id]["display"], "grid");
        assert_eq!(computed.values[id]["width"], "100cqi");
        assert_eq!(computed.values[id]["cursor"], "pointer");
        assert!(!computed.values[id].contains_key("content"));
        assert_eq!(computed.pseudos[&id][0]["background-color"], "red");
        assert_eq!(computed.pseudos[&id][0]["width"], "4px");
    }

    #[test]
    fn nested_layers_media_and_supports_keep_cascade_order() {
        let doc = Document::parse(
            r#"<style>
            @layer reset, base;
            @layer base { .a { color: red; } @layer inner { .a { color: blue; background-color: blue !important; } } .a { background-color: red !important; } }
            @layer reset { .a { width: 1px !important; caret-color: green; } }
            .a { width: 2px !important; }
            @layer base { @media (min-width: 1px) { .a { height: 3px; } } @supports (display: grid) { .a { padding-top: 4px; } } }
            @layer one { .a { margin-top: 1px; } }
            @layer two { .a { margin-top: 2px; } }
            @layer one.sub { .a { margin-top: 3px; } }
            .a { @media (width > 1px) { min-width: 5px; } @supports not (display: nonsense) { max-width: 6px; } @layer late { caret-color: red; } }
        </style><div class="a"></div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let id = element(&doc, ".a");
        for (key, expected) in [
            ("color", "red"),
            ("background-color", "blue"),
            ("width", "1px"),
            ("height", "3px"),
            ("padding-top", "4px"),
            ("margin-top", "2px"),
            ("min-width", "5px"),
            ("max-width", "6px"),
            ("caret-color", "red"),
        ] {
            assert_eq!(computed[id][key], expected, "{key}");
        }
    }

    #[test]
    fn logical_borders_and_theme_colors_resolve_variables() {
        let doc = Document::parse(
            r#"<style>
            :host { --edge:2px solid #fff; --bg:#fff; --ink:light-dark(#f00,#0f0); color-scheme:dark; }
            .item { border-right:var(--edge); border-left:1px solid var(--bg); padding-inline:1ch 2ch; margin-block:8px; color:var(--ink); background-color:color-mix(in lab, white 50%, black); }
            .item span { background-color:light-dark(inherit,inherit); }
        </style><apps-shadow-root><div class="item"><span>text</span></div></apps-shadow-root>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let item = element(&doc, ".item");
        assert_eq!(computed[item]["border-right-width"], "2px");
        assert_eq!(computed[item]["border-left-width"], "1px");
        assert_eq!(computed[item]["border-right-color"], "#fff");
        assert_eq!(computed[item]["border-left-color"], "#fff");
        assert_eq!(computed[item]["border-left-style"], "solid");
        assert_eq!(computed[item]["padding-left"], "1ch");
        assert_eq!(computed[item]["padding-right"], "2ch");
        assert_eq!(computed[item]["margin-bottom"], "8px");
        assert_eq!(computed[item]["color"], "#0f0");
        assert_eq!(
            color(&computed[item]["background-color"]),
            Some((0x777777, 255))
        );
        assert_eq!(
            computed[element(&doc, "span")]["background-color"],
            computed[item]["background-color"]
        );
        assert_eq!(color("rgb(from #d52c36 r g b / .15)"), Some((0xd52c36, 38)));
    }

    #[test]
    fn relative_and_mixed_colors_resolve_with_current_color() {
        let doc = Document::parse(
            r#"<style>
            .item { color: hsl(from red h s l); border-color: color-mix(in srgb, currentColor 50%, transparent); background-color: oklch(from CurrentColor l c h / 0.5); }
            .item span { color: color-mix(in srgb, currentColor, white); fill: lab(from currentcolor l a b); --mix-color: color-mix(in srgb, red, blue); }
        </style><div class="item"><span>text</span></div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let item = element(&doc, ".item");
        let span = element(&doc, "span");
        assert_eq!(computed[item]["color"], "#ff0000ff");
        assert_eq!(computed[item]["border-top-color"], "#ff000080");
        assert_eq!(computed[item]["background-color"], "#ff000080");
        assert_eq!(computed[span]["color"], "#ff8080ff");
        assert_eq!(computed[span]["fill"], "#ff8080ff");
        assert_eq!(
            computed[span]["--mix-color"],
            "color-mix(in srgb, red, blue)"
        );
    }

    #[test]
    fn declaration_tokens_preserve_strings_blocks_and_priority() {
        let style = declarations(
            r#"/* leading ; } */ COLOR: red ! /* priority */ IMPORTANT;
            color: blue; p\61 dding: 2px /* gap */ 4px;
            --Case: { key: [a;b]; nested: fn(";}"); };
            --case: "/* text */;escaped\"}";
            --tokens: one/**/two;
            content: "!important"; width: calc(10px /* math */ + 2px);"#,
        );
        assert_eq!(style["color"], "red !important");
        assert_eq!(style["padding-top"], "2px");
        assert_eq!(style["padding-right"], "4px");
        assert_eq!(style["--Case"], r#"{ key: [a;b]; nested: fn(";}"); }"#);
        assert_eq!(style["--case"], r#""/* text */;escaped\"}""#);
        assert_eq!(style["--tokens"], "one/**/two");
        assert_eq!(style["content"], r#""!important""#);
        assert_eq!(length(&style["width"], 0., 16., 800., 600.), 12.);
    }

    #[test]
    fn invalid_declarations_recover_at_the_next_property() {
        let style = declarations(
            "color: red; broken; width: ]; height: 7px; content: \"bad\n; \
             padding: 3px; background: url(bad\"url); margin: 4px; \
             @unknown { color: blue; } border-width: 2px; \
             color: green !important extra; opacity:; --empty:;",
        );
        assert_eq!(style["color"], "red");
        assert_eq!(style["height"], "7px");
        assert_eq!(style["padding-left"], "3px");
        assert_eq!(style["margin-bottom"], "4px");
        assert_eq!(style["border-top-width"], "2px");
        assert_eq!(style["--empty"], "");
        for key in ["width", "content", "background", "opacity"] {
            assert!(!style.contains_key(key), "{key}");
        }
    }

    #[test]
    fn tokenized_at_rules_keep_scope_and_accept_css_escapes() {
        let doc = Document::parse(
            r#"<style>
                @IMPORT url(tailwindcss);
                @THEME { --color-brand: #123456; }
                .box/**/.active {
                    @APPLY bg-brand p-[calc(1px_+_2px)];
                    COLOR: red !/**/IMPORTANT;
                    @MEDIA SCREEN/**/and (MIN-WIDTH: 500px) {
                        width: 10px;
                        > span { height: 20px; }
                    }
                    color: blue;
                }
                @unknown { .box { width: 999px; } }
                .box/**/span { height: 999px; }
                @m\65 dia (max-width: 499px) { .box { width: 5px; } }
            </style><div class="box active"><span></span></div>"#,
        );
        for (width, expected) in [(400., "5px"), (800., "10px")] {
            let computed = styles(&doc, width, 600.);
            let style = &computed[element(&doc, ".box")];
            assert_eq!(style["color"], "red");
            assert_eq!(style["width"], expected);
            assert_eq!(style["background-color"], "#123456");
            assert_eq!(edges(style, "padding", width, 16., width, 600.), [3.; 4]);
            assert_eq!(
                computed[element(&doc, "span")].contains_key("height"),
                width >= 500.
            );
        }
    }

    #[test]
    fn relative_units_follow_root_font_and_inherited_line_height() {
        let doc = Document::parse(
            r#"<html style="font-size:20px;line-height:1.5"><style>
            section { font-size:2em; line-height:150%; width:10rem; padding:calc(1lh / 2) 2ch; }
            #absolute { font-size:.5em; height:2lh; }
            #number { font-size:1rem; line-height:2; }
            #number span { font-size:.5em; height:1lh; }
            #relative { line-height:2lh; margin:1rlh; }
        </style><section><div id="absolute"></div><div id="number"><span></span></div>
        <div id="relative"></div></section></html>"#,
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let section = element(&doc, "section");
        let absolute = element(&doc, "#absolute");
        let number = element(&doc, "#number");
        let child = element(&doc, "span");
        let relative = element(&doc, "#relative");
        assert_eq!(computed.lengths[section].font.em, 40.);
        assert_eq!(computed.lengths[section].font.lh, 60.);
        assert_eq!(
            computed.lengths[section].resolve(&computed.values[section]["width"], 800.),
            Some(200.)
        );
        assert_eq!(
            computed.lengths[section].edges(&computed.values[section], "padding", 800.),
            [30., 40., 30., 40.]
        );
        assert_eq!(computed.lengths[absolute].font.em, 20.);
        assert_eq!(computed.lengths[absolute].font.lh, 60.);
        assert_eq!(computed.values[absolute]["line-height"], "60px");
        assert_eq!(computed.lengths[number].font.lh, 40.);
        assert_eq!(computed.lengths[child].font.em, 10.);
        assert_eq!(computed.lengths[child].font.lh, 20.);
        assert_eq!(computed.values[child]["line-height"], "2");
        assert_eq!(computed.lengths[relative].font.lh, 120.);
        assert_eq!(
            computed.lengths[relative].edges(&computed.values[relative], "margin", 800.),
            [30.; 4]
        );
    }

    #[test]
    fn root_units_resolve_without_circular_font_metrics() {
        let doc = Document::parse(
            "<html style='font-size:2rem;line-height:2rem'><div style='font-size:.5rem;line-height:calc(1 + .5);width:2rlh'></div></html>",
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let root = element(&doc, "html");
        let child = element(&doc, "div");
        assert_eq!(computed.lengths[root].font.em, 32.);
        assert_eq!(computed.lengths[root].font.lh, 64.);
        assert_eq!(computed.lengths[child].font.em, 16.);
        assert_eq!(computed.lengths[child].font.lh, 24.);
        assert_eq!(computed.lengths[child].resolve("2rlh", 0.), Some(128.));
        let doc = Document::parse(
            "<html style='line-height:2rlh'><div style='font-size:1lh'></div></html>",
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert!((computed.lengths[element(&doc, "div")].font.em - 38.4).abs() < 0.001);
    }

    #[test]
    fn math_values_work_in_variables_shorthands_shadows_and_utilities() {
        let doc = Document::parse(
            r#"<html style="font-size:20px"><style>
            @import 'tailwindcss';
            .box { --space:calc(1rem + 2px); padding:var(--space) min(4vw, 30px);
                box-shadow:0 1lh clamp(2px, 1vw, 10px) #000; }
        </style><div class="box h-[calc(100dvh_-_2rem)] w-[min(50vw,40ch)]"></div></html>"#,
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let id = element(&doc, ".box");
        let style = &computed.values[id];
        let units = computed.lengths[id];
        assert_eq!(units.edges(style, "padding", 800.), [22., 30., 22., 30.]);
        assert_eq!(units.resolve(&style["height"], 600.), Some(560.));
        assert_eq!(units.resolve(&style["width"], 800.), Some(400.));
        let shadows = units.shadows(&style["box-shadow"], false);
        assert_eq!(shadows[0].offset.y, gpui::px(30.));
        assert_eq!(shadows[0].blur_radius, gpui::px(4.));
    }

    #[test]
    fn media_queries_use_viewport_and_initial_font_units() {
        let doc = Document::parse(
            r#"<html style="font-size:32px"><style>
            @media (min-width:calc(40em + 1px)) and (min-height:300px) { .box { width:1px; } }
            @media print, (max-height:min(300px, 50vw)) { .box { height:2px; } }
            @media (max-width:invalid) { .box { padding:99px; } }
        </style><div class="box"></div></html>"#,
        );
        let id = element(&doc, ".box");
        for (width, height, matches_width, matches_height) in [
            (640., 600., false, false),
            (641., 600., true, false),
            (800., 200., false, true),
            (800., 600., true, false),
        ] {
            let computed = styles(&doc, width, height);
            assert_eq!(computed[id].contains_key("width"), matches_width);
            assert_eq!(computed[id].contains_key("height"), matches_height);
            assert!(!computed[id].contains_key("padding"));
        }
    }

    #[test]
    fn theme_colors_and_apply_reach_native_paint() {
        let doc = Document::parse(
            r#"<html><style>
            @import "tailwindcss";
            .select2-dropdown { @apply rounded-b-lg shadow-md; }
            @theme { --color-mint-500: oklch(0.72 0.11 178); }
        </style><div class="select2-dropdown bg-mint-500 text-mint-500 border-mint-500"></div></html>"#,
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        for property in ["background-color", "color", "border-top-color"] {
            assert_eq!(style[property], "oklch(0.72 0.11 178)");
            assert!(color(&style[property]).is_some());
        }
        assert_eq!(style["border-bottom-left-radius"], "8px");
        assert_eq!(style["border-bottom-right-radius"], "8px");
        assert!(!style.contains_key("border-top-left-radius"));
        let paint = Lengths::new(16., 800., 600.).shadows(&style["box-shadow"], false);
        assert_eq!(paint.len(), 2);
        assert_eq!(paint[1].offset, gpui::point(gpui::px(0.), gpui::px(4.)));
        assert_eq!(paint[1].blur_radius, gpui::px(3.));
        assert_eq!(paint[1].spread_radius, gpui::px(-1.));
        assert!((paint[1].color.a - 0.1).abs() < 0.003);
    }

    #[test]
    fn theme_tokens_apply_order_and_scoped_overrides() {
        let doc = Document::parse(
            r#"<html><style>
            @import "tailwindcss";
            @theme {
                --color-mint-500: oklch(0.72 0.11 178);
                --color-accent: var(--color-mint-500);
                --spacing: 5px;
                --spacing-card: 23px;
                --radius-lg: 10px;
                --text-title: 2rem;
                --font-body: Helvetica;
                --font-weight-heavy: 850;
                --shadow-card: 0 2px 4px rgba(0, 0, 0, 0.25);
                --breakpoint-wide: 50rem;
            }
            .box {
                @apply rounded-b-lg bg-accent p-4;
                padding-left: 3px;
                @apply pl-2;
                color: blue !important;
                @apply text-mint-500;
                &.selected { --color-accent: #123456; }
            }
        </style><div class="box selected w-card -mt-card -mb-2 text-title font-body font-heavy shadow-card wide:w-8"></div></html>"#,
        );
        let style = styles(&doc, 799., 600.).pop().unwrap();
        assert_eq!(style["background-color"], "#123456");
        assert_eq!(style["color"], "blue");
        assert_eq!(style["border-bottom-left-radius"], "10px");
        assert_eq!(style["font-size"], "32px");
        assert_eq!(style["font-family"], "Helvetica");
        assert_eq!(style["font-weight"], "850");
        assert_eq!(
            edges(&style, "padding", 799., 32., 799., 600.),
            [20., 20., 20., 10.]
        );
        assert_eq!(
            edges(&style, "margin", 799., 32., 799., 600.),
            [-23., 0., -10., 0.]
        );
        assert_eq!(length(&style["width"], 799., 32., 799., 600.), 23.);
        assert_eq!(
            Lengths::new(32., 799., 600.)
                .shadows(&style["box-shadow"], false)
                .len(),
            1
        );
        let wide = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(length(&wide["width"], 800., 32., 800., 600.), 40.);
    }

    #[test]
    fn oklch_and_rgb_colors_have_expected_channels_and_alpha() {
        assert_eq!(color("oklch(0 0 0)"), Some((0, 255)));
        assert_eq!(color("oklch(100% 0 0 / 50%)"), Some((0xffffff, 128)));
        assert_eq!(
            color("oklch(0.62795536 0.25768331 29.233885)"),
            Some((0xff0000, 255))
        );
        assert_eq!(color("rgb(100% 0% 0% / 25%)"), Some((0xff0000, 64)));
        assert_eq!(color("rgba(0, 128, 255, 0.5)"), Some((0x0080ff, 128)));
        assert_eq!(color("oklch(NaN 0 0)"), None);
        assert_eq!(color("oklch(0.5 0)"), None);
        let doc = Document::parse(
            "<div style='color: var(--missing, oklch(0 0 0)); background: var(--missing, var(--fallback, #123456))'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(style["color"], "oklch(0 0 0)");
        assert_eq!(style["background-color"], "#123456");
    }

    #[test]
    fn shadows_split_inset_and_outer_lists() {
        let units = Lengths::new(16., 800., 600.);
        let value = "inset 0 0 0 2px red, 0 4px 8px blue, inset 1px 2px 6px green";
        let outer = units.shadows(value, false);
        let inset = units.shadows(value, true);
        assert_eq!(outer.len(), 1);
        assert_eq!(outer[0].blur_radius, gpui::px(4.));
        assert_eq!(inset.len(), 2);
        assert_eq!(inset[0].offset, gpui::point(gpui::px(1.), gpui::px(2.)));
        assert_eq!(inset[1].spread_radius, gpui::px(2.));
    }

    #[test]
    fn filters_fold_into_one_color_matrix() {
        let units = Lengths::new(16., 800., 600.);
        let parse = |value: &str| Filter::parse(value, &units);
        let (gray, opacity, shadows) = parse("grayscale(100%) opacity(50%) blur(2px)");
        let gray = gray.unwrap();
        assert!(gray.0.iter().all(|row| row[..3] == gray.0[0][..3]));
        assert_eq!(opacity, 0.5);
        assert!(shadows.is_empty());
        let (ink, alpha) = gray.color((0xff0000, 128));
        assert_eq!(
            (ink >> 16, ink >> 8 & 255, ink & 255, alpha),
            (54, 54, 54, 128)
        );
        assert_eq!(
            parse("invert(1)").0.unwrap().color((0x00ff80, 255)),
            (0xff007f, 255)
        );
        assert_eq!(
            parse("brightness(2) contrast(0)")
                .0
                .unwrap()
                .color((0x123456, 255)),
            (0x808080, 255)
        );
        assert_eq!(parse("hue-rotate(0deg) saturate(1)").0, None);
        assert_eq!(parse("grayscale(-1)").0, None);
        assert_eq!(parse("sepia(1) unknown(2)").0, None);
        let (matrix, _, shadows) = parse("drop-shadow(2px 3px 4px #ff0000) invert(1)");
        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].offset, gpui::point(gpui::px(2.), gpui::px(3.)));
        assert_eq!(gpui::Rgba::from(shadows[0].color), gpui::rgba(0x00ffffff));
        let twice = matrix.unwrap().then(matrix.unwrap());
        assert!(
            twice
                .0
                .iter()
                .zip([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]])
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4))
        );
    }

    #[test]
    fn clip_paths_resolve_to_rects_and_radii() {
        let units = Lengths::new(16., 800., 600.);
        let clip = |value: &str| clip_path(value, &units, [100., 60.]);
        assert_eq!(
            clip("inset(10px 20% round 4px 8px)"),
            Some(([20., 10., 60., 40.], [4., 8., 4., 8.]))
        );
        assert_eq!(clip("circle()"), Some(([20., 0., 60., 60.], [30.; 4])));
        assert!(
            (clip("circle(50%)").unwrap().1[0] - 100f32.hypot(60.) / 2. / std::f32::consts::SQRT_2)
                .abs()
                < 1e-3
        );
        assert_eq!(
            clip("circle(10px at left top)"),
            Some(([-10., -10., 20., 20.], [10.; 4]))
        );
        assert_eq!(clip("ellipse()"), Some(([0., 0., 100., 60.], [30.; 4])));
        assert_eq!(
            clip("rect(auto 80px 50% 10px)"),
            Some(([10., 0., 70., 30.], [0.; 4]))
        );
        assert_eq!(
            clip("xywh(5px 6px 50% 10px round 3px) border-box"),
            Some(([5., 6., 50., 10.], [3.; 4]))
        );
        assert_eq!(clip("polygon(0 0, 100% 0, 0 100%)"), None);
        assert_eq!(clip("url(#mask)"), None);
    }

    fn element(doc: &Document, selector: &str) -> usize {
        (0..doc.nodes.len())
            .find(|&id| doc.matches(id, selector))
            .unwrap()
    }

    #[test]
    fn nested_selectors_and_combinators() {
        let doc = Document::parse(
            r#"<style>
                .card, .panel {
                    padding: 10px;
                    &.active { border-radius: 8px; }
                    h2, p {
                        margin: 0;
                        span { font-weight: 700; }
                    }
                    > p { background: red; }
                    & > h2 + p { width: 100px; }
                    h2 ~ p { height: 20px; }
                }
            </style><main>
                <section class="card active"><h2>Title</h2>text<p id="direct"><span>Text</span></p>
                    <div><p id="deep">Deep</p></div><p id="sibling">Last</p>
                </section>
                <section class="panel"><p id="panel">Panel</p></section>
                <p id="outside">Outside</p>
            </main>"#,
        );
        let computed = styles(&doc, 800., 600.);
        for (selector, key, value) in [
            (".card", "padding-left", "10px"),
            (".card", "border-bottom-left-radius", "8px"),
            ("#direct", "margin-top", "0"),
            ("#direct", "background-color", "red"),
            ("#direct", "width", "100px"),
            ("#direct", "height", "20px"),
            ("#sibling", "height", "20px"),
            ("#deep", "margin-top", "0"),
            ("#panel", "background-color", "red"),
            ("span", "font-weight", "700"),
        ] {
            let id = element(&doc, selector);
            assert_eq!(computed[id][key], value, "{selector} {key}");
        }
        for (selector, key) in [
            ("#deep", "background-color"),
            ("#deep", "height"),
            ("#sibling", "width"),
            ("#outside", "background-color"),
        ] {
            let id = element(&doc, selector);
            assert!(!computed[id].contains_key(key), "{selector} {key}");
        }
        assert_eq!(computed[element(&doc, "#outside")]["margin-top"], "1em");
    }

    #[test]
    fn nesting_keeps_parent_selector_context() {
        let doc = Document::parse(
            r#"<style>
                .outer .inner {
                    .context & { padding: 7px; }
                    div& { width: 10px; }
                    && { height: 20px; }
                }
                .a, .b {
                    & + & { margin: 3px; }
                    + & { border-radius: 4px; }
                }
            </style><main class="outer">
                <section class="context"><div class="inner" id="inside"></div></section>
                <div class="inner" id="outside"></div>
                <section class="inner" id="section"></section>
                <div class="a"></div>text<div class="b" id="mixed"></div>
            </main>"#,
        );
        let computed = styles(&doc, 800., 600.);
        for (selector, key, value) in [
            ("#inside", "padding-left", "7px"),
            ("#inside", "width", "10px"),
            ("#inside", "height", "20px"),
            ("#outside", "width", "10px"),
            ("#mixed", "margin-left", "3px"),
            ("#mixed", "border-top-right-radius", "4px"),
        ] {
            let id = element(&doc, selector);
            assert_eq!(computed[id][key], value, "{selector} {key}");
        }
        for (selector, key) in [("#outside", "padding"), ("#section", "width")] {
            let id = element(&doc, selector);
            assert!(!computed[id].contains_key(key), "{selector} {key}");
        }
    }

    #[test]
    fn nested_cascade_uses_parent_list_specificity_and_source_order() {
        let doc = Document::parse(
            r#"<style>
                .card {
                    color: red;
                    & { color: green; padding: 1px !important; }
                    color: blue;
                    padding: 2px;
                    & { padding: 3px !important; }
                    margin: 4px;
                }
                .card, #unused {
                    & { width: 10px; }
                    width: 20px;
                    & { max-width: 11px; }
                    max-width: 21px;
                    .child, span { height: 30px; }
                    && { border-radius: 5px; }
                }
                #card { width: 40px; border-radius: 6px; }
                .card .child { height: 50px; }
            </style><div id="card" class="card" style="width:60px;padding:9px">
                <span class="child"></span>
            </div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let card = element(&doc, "#card");
        assert_eq!(computed[card]["color"], "blue");
        assert_eq!(computed[card]["padding-left"], "3px");
        assert_eq!(computed[card]["margin-left"], "4px");
        assert_eq!(computed[card]["width"], "60px");
        assert_eq!(computed[card]["max-width"], "11px");
        assert_eq!(computed[card]["border-top-left-radius"], "5px");
        let child = element(&doc, ".child");
        assert_eq!(computed[child]["height"], "30px");
    }

    #[test]
    fn deep_nesting_shares_parent_selectors() {
        let source = format!(
            "<html><style>html {{ color: blue; }} & {{ color: red; }} .card {{{}&.active {{ width: 1px; }}{}}}</style><div class='card active'></div></html>",
            "&& {".repeat(80),
            "}".repeat(80),
        );
        let doc = Document::parse(&source);
        let computed = styles(&doc, 800., 600.);
        assert_eq!(computed[element(&doc, "html")]["color"], "blue");
        assert_eq!(computed[element(&doc, ".card")]["width"], "1px");
    }

    #[test]
    fn nested_media_preserves_scope_and_order() {
        let doc = Document::parse(
            r#"<style>
                @media (min-width: 300px) {
                    .card {
                        width: 10px;
                        @media (max-width: 600px) {
                            width: 20px;
                            > .child { height: 30px; }
                            @media (min-width: 400px) { padding: 5px; }
                        }
                        height: 40px;
                        @supports (display: unsupported) { width: 99px; & { height: 99px; } }
                        margin: 6px;
                    }
                }
            </style><div class="card"><div class="child"></div></div><div id="outside"></div>"#,
        );
        let card = element(&doc, ".card");
        let child = element(&doc, ".child");
        let outside = element(&doc, "#outside");
        for (width, expected, padding, child_height) in [
            (250., None, None, None),
            (350., Some("20px"), None, Some("30px")),
            (500., Some("20px"), Some("5px"), Some("30px")),
            (800., Some("10px"), None, None),
        ] {
            let computed = styles(&doc, width, 600.);
            assert_eq!(computed[card].get("width").map(String::as_str), expected);
            assert_eq!(
                computed[card].get("padding-left").map(String::as_str),
                padding
            );
            assert_eq!(
                computed[child].get("height").map(String::as_str),
                child_height
            );
            assert!(!computed[outside].contains_key("width"));
            if expected.is_some() {
                assert_eq!(computed[card]["height"], "40px");
                assert_eq!(computed[card]["margin-left"], "6px");
            }
        }
    }

    #[test]
    fn nested_parser_handles_values_comments_and_invalid_rules() {
        let doc = Document::parse(
            r#"<style>
                .card {
                    --quoted: "escaped\"}; /* text */";
                    --block: { one: [a;b]; two: { three: four; }; };
                    background-image: url(data:image/svg+xml;a={b});
                    /* } ; { */
                    &/* comment */.active { padding: 4px; }
                    &:unsupported { width: 99px; & { height: 99px; } }
                    &suffix { margin: 99px; }
                    > { color: red; }
                    color: blue;
                    .child { height: 10px }
                    width: 20px
                }
                .after { width: 30px; }
            </style><div class="card active"><div class="child"></div></div><div class="after"></div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let card = element(&doc, ".card");
        assert_eq!(computed[card]["--quoted"], r#""escaped\"}; /* text */""#);
        assert_eq!(
            computed[card]["--block"],
            "{ one: [a;b]; two: { three: four; }; }"
        );
        assert_eq!(
            computed[card]["background-image"],
            "url(data:image/svg+xml;a={b})"
        );
        assert_eq!(computed[card]["padding-left"], "4px");
        assert_eq!(computed[card]["color"], "blue");
        assert_eq!(computed[card]["width"], "20px");
        assert!(!computed[card].contains_key("height"));
        assert!(!computed[card].contains_key("margin"));
        let child = element(&doc, ".child");
        assert_eq!(computed[child]["height"], "10px");
        let after = element(&doc, ".after");
        assert_eq!(computed[after]["width"], "30px");
    }

    #[test]
    fn tailwind_import_is_opt_in_and_preserves_following_rules() {
        for import in [
            "",
            "/* @import \"tailwindcss\"; */",
            "p { content: '@import \"tailwindcss\";' }",
        ] {
            let doc = Document::parse(&format!(
                "<style>{import} .box {{ color: blue }}</style><div class='box h-4 w-4 flex'></div>"
            ));
            let style = styles(&doc, 800., 600.).pop().unwrap();
            assert!(!style.contains_key("height"));
            assert_eq!(style["display"], "block");
            assert_eq!(style["color"], "blue");
        }
        for import in [
            "@import \"tailwindcss\";",
            "@import 'tailwindcss';",
            "@import url('tailwindcss');",
        ] {
            let doc = Document::parse(&format!(
                "<style>{import} .box {{ color: blue }}</style><div class='box h-4 w-4 flex'></div>"
            ));
            let style = styles(&doc, 800., 600.).pop().unwrap();
            assert_eq!(length(&style["height"], 0., 16., 800., 600.), 16.);
            assert_eq!(length(&style["width"], 0., 16., 800., 600.), 16.);
            assert_eq!(style["display"], "flex");
            assert_eq!(style["color"], "blue");
        }
    }

    #[test]
    fn tailwind_classes_have_stable_order_and_respond_to_width() {
        for classes in [
            "p-8 px-4 pl-2 gap-4 gap-x-2 size-8 w-4 sm:w-8 md:w-12 lg:w-16",
            "lg:w-16 md:w-12 sm:w-8 w-4 size-8 gap-x-2 gap-4 pl-2 px-4 p-8",
        ] {
            let doc = Document::parse(&format!(
                "<style>@import 'tailwindcss';</style><div class='{classes}'></div>"
            ));
            for (width, expected) in [
                (639., "1rem"),
                (640., "2rem"),
                (768., "3rem"),
                (1024., "4rem"),
            ] {
                let style = styles(&doc, width, 600.).pop().unwrap();
                assert_eq!(style["width"], expected);
                assert_eq!(style["height"], "2rem");
                assert_eq!(
                    edges(&style, "padding", width, 16., width, 600.),
                    [32., 16., 32., 8.]
                );
                assert_eq!(style["row-gap"], "1rem");
                assert_eq!(style["column-gap"], "0.5rem");
            }
        }
    }

    #[test]
    fn css_and_inline_shorthands_override_utilities() {
        let doc = Document::parse(
            "<style>@import 'tailwindcss'; div { padding: 2px 3px; gap: 4px; background: blue; width: 20px !important; } .box { padding-left: 5px; }</style><div class='box p-8 bg-white gap-8 w-4' style='padding: 6px; gap: 7px 8px; background: black; width: 30px'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(edges(&style, "padding", 800., 16., 800., 600.), [6.; 4]);
        assert_eq!(style["background-color"], "black");
        assert_eq!(style["row-gap"], "7px");
        assert_eq!(style["column-gap"], "8px");
        assert_eq!(style["width"], "20px");
        let style = declarations(
            "padding: 1px !important; padding: 2px; padding-left: 3px; padding-right: 4px !important",
        );
        assert_eq!(style["padding-left"], "1px !important");
        assert_eq!(style["padding-right"], "4px !important");
        let doc = Document::parse(
            "<html><style>@import 'tailwindcss'; :root { --space: 2px 4px; } div { padding: var(--space); gap: var(--missing, 5px 6px); margin: var(--space); margin-left: 9px; }</style><div class='p-8 gap-8 m-8'></div></html>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(
            edges(&style, "padding", 800., 16., 800., 600.),
            [2., 4., 2., 4.]
        );
        assert_eq!(
            edges(&style, "margin", 800., 16., 800., 600.),
            [2., 4., 2., 9.]
        );
        assert_eq!(style["row-gap"], "5px");
        assert_eq!(style["column-gap"], "6px");
    }

    #[test]
    fn tailwind_sizes_colors_and_unknown_classes() {
        let doc = Document::parse(
            "<style>@import 'tailwindcss';</style><div class='grid grid-cols-2 min-h-screen max-w-full w-1/2 h-[30px] -mt-2 mx-auto border-2 border-[#123456] rounded-xl bg-[#abcdef] text-lg font-semibold hover:hidden w-NaN w-1/0 p--4 custom'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(style["display"], "grid");
        assert_eq!(style["grid-template-columns"], "repeat(2,1fr)");
        assert_eq!(style["min-height"], "100vh");
        assert_eq!(style["max-width"], "100%");
        assert_eq!(style["width"], "50%");
        assert_eq!(style["height"], "30px");
        assert_eq!(style["margin-top"], "-0.5rem");
        assert_eq!(style["margin-left"], "auto");
        assert_eq!(style["margin-right"], "auto");
        assert_eq!(style["border-left-width"], "2px");
        assert_eq!(style["border-bottom-color"], "#123456");
        assert_eq!(style["border-bottom-left-radius"], "12px");
        assert_eq!(style["background-color"], "#abcdef");
        assert_eq!(style["font-size"], "18px");
        assert_eq!(style["font-weight"], "600");
        assert_eq!(style["padding-top"], "0");
    }

    #[test]
    fn tailwind_state_variants_follow_interaction_states() {
        let mut doc = Document::parse(
            "<style>@import 'tailwindcss';</style><div class='group bg-[#111111] hover:bg-[#222222] md:hover:text-[#333333]'><span class='first:p-[3px] group-hover:font-bold hover:dark:font-thin'>x</span><b class='first:p-[3px]'>y</b></div>",
        );
        let card = element(&doc, "div");
        let span = element(&doc, "span");
        let bold = element(&doc, "b");
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        for hovered in [false, true, false] {
            doc.nodes[card].state = if hovered { crate::html::HOVER } else { 0 };
            let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
            let fresh = compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "hovered {hovered}");
            let style = &cached.values[card];
            assert_eq!(
                style["background-color"],
                if hovered { "#222222" } else { "#111111" }
            );
            assert_eq!(
                style.get("color").map(String::as_str),
                Some(if hovered { "#333333" } else { "#171717" })
            );
            assert_eq!(
                cached.values[span].get("font-weight").map(String::as_str),
                hovered.then_some("700")
            );
            assert_eq!(cached.values[span]["padding-top"], "3px");
            assert_eq!(cached.values[bold]["padding-top"], "0");
        }
    }

    #[test]
    fn cascade_variables_media_and_specificity() {
        let doc = Document::parse(
            "<html><head><style>:root{--ink:#123456}.box{color:red} #one{color:var(--ink)!important} .box{display:grid;grid-template-columns:repeat(2,1fr)} @media(max-width:500px){.box{grid-template-columns:1fr}}</style></head><body><div class='box' id='one' style='color:blue'>A</div></body></html>",
        );
        let id = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|s| s == "one"))
            .unwrap();
        let wide = styles(&doc, 800., 600.);
        assert_eq!(wide[id]["color"], "#123456");
        assert_eq!(wide[id]["grid-template-columns"], "repeat(2,1fr)");
        let narrow = styles(&doc, 400., 600.);
        assert_eq!(narrow[id]["grid-template-columns"], "1fr");
    }
}
