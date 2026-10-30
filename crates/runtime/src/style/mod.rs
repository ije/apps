use crate::dom::{Document, Selector, SelectorCache, SelectorList};
use std::collections::BTreeMap;
use std::{borrow::Cow, collections::HashMap, rc::Rc};

mod animation;
mod color;
pub(crate) mod parser;
mod shorthand;
mod tailwind;
#[cfg(test)]
mod tests;
mod text;
mod value;

pub(crate) use animation::{Animation, Motion};
pub use color::color;
pub(crate) use parser::{FontUnits, Lengths};
use shorthand::expand;
pub(crate) use text::{
    InlineText, cursor, font, inline_text, restyle, selection_colors, spacing, text_transform,
};
pub(crate) use value::{Filter, clip_path, components};
pub use value::{edges, length};

pub type Style = BTreeMap<String, String>;

pub(crate) const DISPLAY_NONE: u8 = 1;
pub(crate) const DISPLAY_CONTENTS: u8 = 2;
pub(crate) const INVISIBLE: u8 = 4;
pub(crate) const SELECT_NONE: u8 = 8;
pub(crate) const SELECT_ALL: u8 = 16;
pub(crate) const SELECT_TEXT: u8 = 32;

fn flags(style: &Style) -> u8 {
    let get = |name| style.get(name).map_or("", String::as_str);
    let flag = |set, value| if set { value } else { 0 };
    flag(get("display") == "none", DISPLAY_NONE)
        | flag(get("display") == "contents", DISPLAY_CONTENTS)
        | flag(
            matches!(get("visibility"), "hidden" | "collapse"),
            INVISIBLE,
        )
        | match get("user-select") {
            "none" => SELECT_NONE,
            "all" => SELECT_ALL,
            "text" => SELECT_TEXT,
            _ => 0,
        }
}

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
button,select{padding:2px 8px;border:1px solid #c4c4c4;border-radius:5px;background-color:#f6f6f6;box-sizing:border-box}
input,textarea{padding:2px 4px;border:1px solid #c4c4c4;border-radius:4px;background-color:#fff}
input[type=checkbox],input[type=radio]{width:13px;height:13px;margin:3px;padding:0}
input[type=radio]{border-radius:50%}
input[type=button],input[type=submit],input[type=reset]{padding:2px 8px;border-radius:5px;background-color:#f6f6f6;box-sizing:border-box}
summary{list-style-position:inside}
apps-shadow-root{display:block}
button:disabled,input:disabled,select:disabled,textarea:disabled{color:#a0a0a0}
input:focus-visible,textarea:focus-visible{outline:3px solid #8fb8f4}
progress,meter{width:10em;height:1em}
iframe{border:2px inset #c0c0c0}
q::before{content:open-quote}
q::after{content:close-quote}
";

// WARNING: keep sorted; it is searched with binary_search.
const INHERITED: &[&str] = &[
    "-moz-osx-font-smoothing",
    "-webkit-font-smoothing",
    "-webkit-text-fill-color",
    "accent-color",
    "app-region",
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
    static UA_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(UA));
    pub(crate) static FONT_NAMES: std::cell::RefCell<(HashMap<String, String>, HashMap<String, render::SharedString>)> = Default::default();
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
    inline: Option<String>,
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    columns: usize,
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
    used: std::cell::Cell<u64>,
    style: Rc<Style>,
    flags: u8,
    lengths: Lengths,
    pseudos: Option<Rc<[Style; 2]>>,
    text_pseudos: Option<Rc<[Option<Style>; 4]>>,
    scrollbars: Option<Rc<[Option<Style>; 5]>>,
    selection: Option<Style>,
    animations: Option<Vec<Animation>>,
    host: Style,
}

/// A bucket key: id, class, tag or attribute name.
type Key = Option<(u8, String)>;
struct StyleScope {
    id: usize,
    used: u64,
    restricted: bool,
    sheets: Vec<Rc<Stylesheet>>,
    rules: Vec<(SelectorList, Style, i32)>,
    /// Pseudo-element kind, selector without it, rule and key.
    pseudos: Vec<(usize, Selector, usize, Key)>,
    /// Element selectors of each rule, by index in the rule's list.
    elements: Vec<Vec<(usize, Key)>>,
    /// Rules by the key one of their selectors requires, per key kind.
    buckets: [HashMap<String, Vec<usize>>; 4],
    universal: Vec<usize>,
    containers: Vec<Option<ContainerQuery>>,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    /// Interaction states the selectors use.
    states: u8,
    matches: SelectorCache,
    nodes: Vec<Option<MatchedRules>>,
}

struct MatchedRules {
    checked: std::cell::Cell<u64>,
    cached: Option<CachedNode>,
    dependencies: Vec<(usize, u64)>,
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    columns: usize,
}

#[derive(Default)]
pub(crate) struct StyleCache {
    viewport: [f32; 2],
    metrics: bool,
    reduced_motion: bool,
    native_theme: u32,
    scopes: Vec<StyleScope>,
    next: usize,
    pass: u64,
    /// Interaction states used by the scopes of the latest pass.
    pub(crate) states: u8,
    /// Parsed `<style>` text by node, with the pass that last used it.
    sheets: HashMap<usize, (u64, Rc<str>, Rc<Stylesheet>)>,
    values: rustc_hash::FxHashMap<StyleKey, Rc<CachedStyle>>,
    containers: HashMap<usize, [f32; 2]>,
    queries: HashMap<(usize, usize, usize, usize), (bool, bool, FontUnits)>,
    queried: bool,
    tailwind: tailwind::Compiler,
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
                |(&(scope, order, index, container), &(inline, matched, font))| {
                    let query = self
                        .scopes
                        .iter()
                        .find(|cached| cached.id == scope)
                        .and_then(|scope| scope.containers.get(order)?.as_ref()?.get(index));
                    query.is_none_or(|(_, _, query)| {
                        sizes.get(&container).is_some_and(|size| {
                            query.matches(
                                size[0],
                                if inline { f32::NAN } else { size[1] },
                                false,
                                0,
                                font,
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

    /// Ends a style pass: drops the scopes and `<style>` parses it did not use.
    pub(crate) fn finish(&mut self) {
        let pass = self.pass;
        let count = self.scopes.len();
        self.scopes.retain(|scope| scope.used == pass);
        if self.scopes.len() < count {
            let live: Vec<_> = self.scopes.iter().map(|scope| scope.id).collect();
            self.values.retain(|key, _| live.contains(&key.scope));
            self.queries.retain(|key, _| live.contains(&key.0));
        }
        self.sheets.retain(|_, (used, ..)| *used == pass);
        if self.values.len() > 4096 {
            self.values.retain(|_, value| value.used.get() == pass);
        }
        self.states = self
            .scopes
            .iter()
            .fold(0, |states, scope| states | scope.states);
        self.pass += 1;
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
    /// `@theme [inline]`: variables and keyframes.
    Theme(bool, Vec<Rule>),
    Variant(String, String),
    Utility(String, String),
    FontFace(Declarations),
    /// `@import "tailwindcss"`, or one of its parts; true when it includes preflight.
    Tailwind(bool),
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
    ("app-region", &["none", "drag", "no-drag"]),
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
    /// Font-relative lengths use `font`: the container's font in container queries.
    fn matches(
        &self,
        width: f32,
        height: f32,
        reduced_motion: bool,
        theme: u32,
        font: FontUnits,
    ) -> Option<bool> {
        let (name, op, value) = match self {
            Self::All => return Some(true),
            Self::Unknown(_) => return None,
            Self::Not(query) => {
                return query
                    .matches(width, height, reduced_motion, theme, font)
                    .map(|v| !v);
            }
            Self::And(queries) | Self::Or(queries) => {
                let and = matches!(self, Self::And(_));
                let mut result = Some(and);
                for query in queries {
                    match query.matches(width, height, reduced_motion, theme, font) {
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
            "width" | "height" => f64::from(
                Lengths {
                    font,
                    ..Lengths::new(16., width, height)
                }
                .resolve(value, f32::NAN)?,
            ),
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
            Self::Theme(_, children) => {
                for rule in children {
                    if matches!(rule, Self::Keyframes(..)) {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
            Self::Variant(..) | Self::Utility(..) | Self::FontFace(_) | Self::Tailwind(_) => {}
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
                            FontUnits::new(16.),
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
            Self::Theme(inline, children) => format!(
                "@theme{}{{{}}}",
                if *inline { " inline" } else { "" },
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Utility(name, body) => format!("@utility {name}{{{body}}}"),
            Self::FontFace(values) => format!("@font-face{{{}}}", values.source()),
            Self::Variant(name, template) => format!("@custom-variant {name} ({template});"),
            Self::Tailwind(true) => "@import 'tailwindcss';".into(),
            Self::Tailwind(false) => "@import 'tailwindcss/utilities';".into(),
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
    pub reduced_motion: bool,
    pub theme: u32,
    /// Loaded font names by the family names pages use.
    pub aliases: HashMap<String, String>,
}

#[derive(Clone, Default)]
pub(crate) struct ComputedStyles {
    pub values: Vec<Rc<Style>>,
    pub flags: Vec<u8>,
    pub lengths: Vec<Lengths>,
    pub pseudos: HashMap<usize, Rc<[Style; 2]>>,
    pub text_pseudos: HashMap<usize, Rc<[Option<Style>; 4]>>,
    /// `::-webkit-scrollbar`, `-track`, `-thumb`, `-corner` and `-thumb:hover`.
    pub scrollbars: HashMap<usize, Rc<[Option<Style>; 5]>>,
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
    text_system: Option<&render::TextSystem>,
    cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
) -> ComputedStyles {
    let mut out = ComputedStyles {
        values: vec![Rc::default(); doc.nodes.len()],
        flags: vec![0; doc.nodes.len()],
        lengths: vec![Lengths::new(16., width, height); doc.nodes.len()],
        user_select: vec![(UserSelect::None, 0); doc.nodes.len()],
        ..Default::default()
    };
    cascade(
        doc,
        [width, height],
        text_system,
        cache,
        style_cache,
        context,
        &mut out,
    );
    out
}

/// Computes the styles of the nodes reachable from node 0 into `out`, leaving other nodes as they are.
/// `cache` shares parsed `<style>` text between nodes.
pub(crate) fn cascade(
    doc: &Document,
    [width, height]: [f32; 2],
    text_system: Option<&render::TextSystem>,
    cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
    out: &mut ComputedStyles,
) {
    let (reduced_motion, native_theme) = context.map_or_else(
        || (reduced_motion(), crate::desktop::native::theme_state()),
        |context| (context.reduced_motion, context.theme),
    );
    let ua = UA_SHEET.with(Clone::clone);
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
            let pass = style_cache.pass;
            sheets.push(match &node.stylesheet {
                Some(sheet) => sheet.clone(),
                None => match style_cache.sheets.get_mut(&id).filter(|(_, text, _)| {
                    let mut rest = &**text;
                    let mut pending = vec![id];
                    while let Some(child) = pending.pop() {
                        let node = &doc.nodes[child];
                        if child != id && node.tag == "#comment" {
                            continue;
                        }
                        match rest.strip_prefix(node.text.as_str()) {
                            Some(tail) if node.stylesheet.is_none() => rest = tail,
                            _ => return false,
                        }
                        pending.extend(node.children.iter().rev());
                    }
                    rest.is_empty()
                }) {
                    Some((used, _, sheet)) => {
                        *used = pass;
                        sheet.clone()
                    }
                    None => {
                        let text = doc.text(id);
                        let (text, sheet) = match cache.get_key_value(text.as_str()) {
                            Some((text, sheet)) => (text.clone(), sheet.clone()),
                            None => {
                                let sheet = Rc::new(Stylesheet::parse(&text));
                                let text: Rc<str> = text.into();
                                cache.insert(text.clone(), sheet.clone());
                                (text, sheet)
                            }
                        };
                        style_cache.sheets.insert(id, (pass, text, sheet.clone()));
                        sheet
                    }
                },
            });
        }
    }
    if style_cache.viewport != [width, height]
        || style_cache.metrics != text_system.is_some()
        || style_cache.reduced_motion != reduced_motion
        || style_cache.native_theme != native_theme
    {
        *style_cache = StyleCache {
            viewport: [width, height],
            metrics: text_system.is_some(),
            reduced_motion,
            native_theme,
            next: style_cache.next,
            pass: style_cache.pass,
            sheets: std::mem::take(&mut style_cache.sheets),
            containers: std::mem::take(&mut style_cache.containers),
            tailwind: std::mem::take(&mut style_cache.tailwind),
            ..Default::default()
        };
    }
    let classes = order
        .iter()
        .filter_map(|&id| doc.nodes[id].attrs.get("class"))
        .flat_map(|classes| classes.split_ascii_whitespace());
    let (tailwind, replaced) = style_cache.tailwind.update(&mut sheets, classes);
    if let Some(replaced) = replaced {
        style_cache.scopes.retain(|scope| {
            !scope
                .sheets
                .iter()
                .any(|sheet| Rc::ptr_eq(sheet, &replaced))
        });
        style_cache.queries.clear();
    }
    let theme = tailwind.then(|| style_cache.tailwind.root.clone());
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
            for rule in &ua.0 {
                rule.collect(None, "", &mut collected);
            }
            for rule in &mut collected.rules {
                rule.2 = i32::MIN + 1;
            }
            collected.rules.extend(author);
            let Collect {
                rules, keyframes, ..
            } = collected;
            let containers = rules.iter().map(|rule| rule.3.clone()).collect();
            let rules: Vec<_> = rules
                .into_iter()
                .map(|(selector, body, layer, _)| (selector.clone(), body.resolve(), layer))
                .collect();
            let mut pseudos = Vec::new();
            let mut buckets: [HashMap<String, Vec<usize>>; 4] = Default::default();
            let mut universal = Vec::new();
            let elements = rules
                .iter()
                .enumerate()
                .map(|(order, (selectors, ..))| {
                    selectors
                        .iter()
                        .enumerate()
                        .filter_map(|(index, selector)| {
                            if let Some((kind, selector)) = selector.without_pseudo_element() {
                                let key = selector.key().map(|(kind, name)| (kind, name.into()));
                                pseudos.push((kind, selector, order, key));
                                return None;
                            }
                            let key: Key = selector.key().map(|(kind, name)| (kind, name.into()));
                            match &key {
                                Some((kind, name)) => buckets[*kind as usize]
                                    .entry(name.clone())
                                    .or_default()
                                    .push(order),
                                None => universal.push(order),
                            }
                            Some((index, key))
                        })
                        .collect()
                })
                .collect();
            let mut states = 0;
            let mut lists: Vec<_> = rules
                .iter()
                .map(|(selectors, ..)| selectors.clone())
                .collect();
            let mut seen = std::collections::HashSet::new();
            while let Some(list) = lists.pop() {
                if seen.insert(Rc::as_ptr(&list)) {
                    for selector in list.iter() {
                        states |= selector.states(&mut lists);
                    }
                }
            }
            style_cache.next += 1;
            style_cache.scopes.push(StyleScope {
                id: style_cache.next,
                used: 0,
                restricted,
                sheets: sheets.clone(),
                rules,
                pseudos,
                elements,
                buckets,
                universal,
                containers,
                keyframes,
                states,
                matches: SelectorCache::default(),
                nodes: Vec::new(),
            });
            style_cache.scopes.len() - 1
        });
    let scoped = &mut style_cache.scopes[scope];
    let scope = scoped.id;
    scoped.used = style_cache.pass;
    let rules = &scoped.rules;
    let matches = &mut scoped.matches;
    matches.update(doc, &order);
    if scoped.nodes.len() < doc.nodes.len() {
        scoped.nodes.resize_with(doc.nodes.len(), || None);
    }
    let mut candidates: Vec<usize> = Vec::new();
    let ComputedStyles {
        values: styles,
        flags: style_flags,
        lengths,
        pseudos,
        text_pseudos,
        scrollbars,
        selections,
        animations,
        host_declarations,
        ..
    } = out;
    let mut root = FontUnits::new(16.);
    let mut font_metrics = HashMap::new();
    for &id in &order {
        if context
            .and_then(|context| context.required.as_ref())
            .is_some_and(|required| !required.contains(&id))
        {
            continue;
        }
        let node = &doc.nodes[id];
        if let Some((style, units)) = context.and_then(|context| context.roots.get(&id)) {
            styles[id] = style.clone();
            style_flags[id] = flags(style);
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
                let (matched, matched_pseudos, columns) = if let Some(cached) =
                    scoped.nodes[id].as_ref()
                    && matches.valid_since(&cached.dependencies, &cached.checked)
                {
                    (cached.rules.clone(), cached.pseudos.clone(), cached.columns)
                } else {
                    matches.dependencies.clear();
                    matches.depend(id, 0);
                    candidates.clear();
                    if !node.tag.starts_with('#') {
                        candidates.extend(&scoped.universal);
                        let classes = node.attrs.get("class").into_iter();
                        for (kind, name) in
                            std::iter::once((2, node.tag.as_str()))
                                .chain(node.attrs.get("id").map(|id| (0, id.as_str())))
                                .chain(classes.flat_map(|c| {
                                    c.split_ascii_whitespace().map(|class| (1, class))
                                }))
                                .chain(node.attrs.keys().map(|name| (3, name.as_str())))
                        {
                            if let Some(list) = scoped.buckets[kind].get(name) {
                                candidates.extend(list);
                            }
                        }
                        candidates.sort_unstable();
                        candidates.dedup();
                    }
                    let has = |key: &Key| match key {
                        Some((0, value)) => node.attrs.get("id") == Some(value),
                        Some((1, value)) => node.attrs.get("class").is_some_and(|classes| {
                            classes.split_ascii_whitespace().any(|class| class == value)
                        }),
                        Some((2, value)) => node.tag == *value,
                        Some((3, name)) => node.attrs.contains_key(name),
                        _ => true,
                    };
                    let matched: Vec<_> = candidates
                        .iter()
                        .filter_map(|&order| {
                            let selectors = &rules[order].0;
                            scoped.elements[order]
                                .iter()
                                .filter(|(index, key)| {
                                    has(key) && selectors[*index].matches_reusing(doc, id, matches)
                                })
                                .map(|(index, _)| selectors[*index].specificity)
                                .max()
                                .map(|specificity| (order, specificity))
                        })
                        .collect();
                    let matched_pseudos: Vec<_> = if node.tag.starts_with('#') {
                        Vec::new()
                    } else {
                        scoped
                            .pseudos
                            .iter()
                            .enumerate()
                            .filter_map(|(index, (_, selector, _, key))| {
                                (has(key) && selector.matches_reusing(doc, id, matches))
                                    .then_some(index)
                            })
                            .collect()
                    };
                    let mut columns = 0;
                    if node.tag == "table" {
                        matches.depend(id, 2);
                        for &child in &node.children {
                            let rows = match doc.nodes[child].tag.as_str() {
                                "tr" => std::slice::from_ref(&child),
                                "thead" | "tbody" | "tfoot" => {
                                    matches.depend(child, 2);
                                    &doc.nodes[child].children[..]
                                }
                                _ => &[],
                            };
                            for &row in rows.iter().filter(|&&row| doc.nodes[row].tag == "tr") {
                                matches.depend(row, 2);
                                columns = columns.max(
                                    doc.nodes[row]
                                        .children
                                        .iter()
                                        .filter(|&&cell| !doc.nodes[cell].tag.starts_with('#'))
                                        .count(),
                                );
                            }
                        }
                    }
                    matches.dependencies.sort_unstable();
                    matches.dependencies.dedup();
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: std::mem::take(&mut matches.dependencies),
                        rules: matched.clone(),
                        pseudos: matched_pseudos.clone(),
                        columns,
                    });
                    (matched, matched_pseudos, columns)
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
                                    let font = lengths[ancestor].font;
                                    let matched =
                                        style_cache.containers.get(&ancestor).is_some_and(|size| {
                                            query.matches(
                                                size[0],
                                                if inline { f32::NAN } else { size[1] },
                                                false,
                                                0,
                                                font,
                                            ) == Some(true)
                                        });
                                    style_cache.queries.insert(
                                        (scope, order, index, ancestor),
                                        (inline, matched, font),
                                    );
                                    return matched;
                                }
                                parent = doc.nodes[ancestor].parent;
                            }
                            false
                        })
                };
                matched.retain(|(order, _)| applies(*order));
                matched_pseudos.retain(|index| applies(scoped.pseudos[*index].2));
                let resolved = StyleKey {
                    scope,
                    tag: node.tag.clone(),
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
                    columns,
                    host: host.cloned(),
                    selection: selection.cloned(),
                };
                let value = style_cache.values.get(&resolved).cloned();
                key = Some(resolved);
                value
            });
        if let Some(cached) = cached {
            cached.used.set(style_cache.pass);
            if let Some(key) = &key
                && let Some(node) = scoped.nodes[id].as_mut()
            {
                node.cached = Some(CachedNode::new(key, cached.clone()));
            }
            styles[id] = cached.style.clone();
            style_flags[id] = cached.flags;
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
            if let Some(values) = &cached.scrollbars {
                scrollbars.insert(id, values.clone());
            }
            if let Some(values) = &cached.selection {
                selections.insert(id, values.clone());
            }
            if let Some(values) = &cached.animations {
                animations.insert(id, values.clone());
            }
            if node.tag == "apps-shadow-root" {
                *host_declarations = cached.host.clone();
            }
            continue;
        }
        let key = key.unwrap();
        let mut style = match &theme {
            Some(theme) if id == 0 => (**theme).clone(),
            _ => Style::new(),
        };
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
            for (key, value) in inline.resolve() {
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
                            !important && *layer == i32::MIN + 1
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
            *host_declarations = cascade
                .iter()
                .map(|(key, (rank, value))| {
                    (
                        key.to_string(),
                        if rank.0 {
                            format!("{value}!important")
                        } else {
                            value.to_string()
                        },
                    )
                })
                .collect();
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
                // Separated borders also keep the spacing between the table's edges and its cells.
                if spacing != "0" {
                    for (side, gap) in [
                        ("top", row),
                        ("right", column),
                        ("bottom", row),
                        ("left", column),
                    ] {
                        let key = format!("padding-{side}");
                        let padding = style.get(&key).map_or("0px", String::as_str);
                        let value = format!("calc({padding} + {gap})");
                        style.insert(key, value);
                    }
                }
            }
            if key.columns > 1 && !style.contains_key("grid-template-columns") {
                style.insert(
                    "grid-template-columns".into(),
                    format!("repeat({},auto)", key.columns),
                );
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
        if let Some(family) = context.and_then(|context| {
            context
                .aliases
                .get(style.get("font-family")?.trim_matches(['\'', '"']))
        }) {
            style.insert("font-family".into(), family.clone());
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
        .filter(|size| size.is_finite() && *size >= 0.)
        // WARNING: glyph rasterization allocates by font size, so huge sizes abort the process.
        .map(|size| size.min(10_000.))
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
                let size = render::px(1.);
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
        let mut pseudo_cascade: [BTreeMap<&str, (CascadeRank, &str)>; 12] = Default::default();
        for &index in &key.pseudos {
            let (pseudo, selector, order, _) = &scoped.pseudos[index];
            let (_, declarations, layer) = &rules[*order];
            let [ids, classes, tags] = selector.specificity;
            for (key, value) in declarations {
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
        let [
            before,
            after,
            selected,
            marker,
            letter,
            line,
            placeholder,
            bar,
            track,
            thumb,
            corner,
            mut hovered,
        ] = pseudo_cascade;
        let derive = |cascade: BTreeMap<&str, (CascadeRank, &str)>| {
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
        };
        if [&marker, &letter, &line, &placeholder]
            .iter()
            .any(|cascade| !cascade.is_empty())
        {
            text_pseudos.insert(
                id,
                Rc::new([marker, letter, line, placeholder].map(&derive)),
            );
        }
        // Scrollbar parts only matter on scroll containers, and on the root and body, which style the page's bar.
        if [&bar, &track, &thumb, &corner, &hovered]
            .iter()
            .any(|cascade| !cascade.is_empty())
            && (matches!(node.tag.as_str(), "html" | "body")
                || ["overflow-x", "overflow-y"].iter().any(|name| {
                    style
                        .get(*name)
                        .is_some_and(|value| matches!(value.as_str(), "auto" | "scroll"))
                }))
        {
            if !hovered.is_empty() {
                for (key, value) in &thumb {
                    hovered.entry(*key).or_insert(*value);
                }
            }
            scrollbars.insert(
                id,
                Rc::new([bar, track, thumb, corner, hovered].map(&derive)),
            );
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
        style_flags[id] = flags(&style);
        let cached = Rc::new(CachedStyle {
            used: std::cell::Cell::new(style_cache.pass),
            style: style.clone(),
            flags: style_flags[id],
            lengths: units,
            pseudos: pseudos.get(&id).cloned(),
            text_pseudos: text_pseudos.get(&id).cloned(),
            scrollbars: scrollbars.get(&id).cloned(),
            selection: selections.get(&id).cloned(),
            animations: animations.get(&id).cloned(),
            host: if node.tag == "apps-shadow-root" {
                host_declarations.clone()
            } else {
                Style::new()
            },
        });
        if let Some(node) = scoped.nodes[id].as_mut() {
            node.cached = Some(CachedNode::new(&key, cached.clone()));
        }
        style_cache.values.insert(key, cached);
        styles[id] = style;
        lengths[id] = units;
    }
    if context.is_some_and(|context| context.required.is_some()) {
        return;
    }
    // Collapsed table borders (CSS 2.1 17.6.2): cells overlap by half their borders, so shared borders straddle
    // the grid lines, and the table keeps half of the outer borders.
    // WARNING: this changes the styles after caching, so cached styles stay unadjusted.
    const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
    for &table in &order {
        if doc.nodes[table].tag != "table"
            || styles[table]
                .get("border-collapse")
                .is_none_or(|value| value != "collapse")
            || styles[table]
                .get("display")
                .is_none_or(|value| value != "grid")
        {
            continue;
        }
        let border = |id: usize, side: &str| {
            if styles[id]
                .get(&format!("border-{side}-style"))
                .is_none_or(|kind| matches!(kind.as_str(), "none" | "hidden"))
            {
                return 0.;
            }
            styles[id]
                .get(&format!("border-{side}-width"))
                .map_or(Some(3.), |value| lengths[id].resolve(value, 0.))
                .unwrap_or(0.)
                .max(0.)
        };
        let rows: Vec<Vec<usize>> = doc.nodes[table]
            .children
            .iter()
            .flat_map(|child| match doc.nodes[*child].tag.as_str() {
                "thead" | "tbody" | "tfoot" => &doc.nodes[*child].children[..],
                _ => std::slice::from_ref(child),
            })
            .filter(|&&row| doc.nodes[row].tag == "tr")
            .map(|&row| {
                doc.nodes[row]
                    .children
                    .iter()
                    .copied()
                    .filter(|&cell| matches!(doc.nodes[cell].tag.as_str(), "td" | "th"))
                    .collect::<Vec<_>>()
            })
            .filter(|cells| !cells.is_empty())
            .collect();
        let edges = SIDES.map(|side| {
            let cells: Vec<usize> = match side {
                "top" => rows.first().cloned().unwrap_or_default(),
                "bottom" => rows.last().cloned().unwrap_or_default(),
                "right" => rows.iter().filter_map(|row| row.last().copied()).collect(),
                _ => rows.iter().filter_map(|row| row.first().copied()).collect(),
            };
            let widest = cells
                .iter()
                .map(|&cell| border(cell, side))
                .fold(0., f32::max);
            (border(table, side), widest)
        });
        let cells: Vec<_> = rows
            .iter()
            .flatten()
            .map(|&cell| (cell, SIDES.map(|side| border(cell, side))))
            .collect();
        let units = lengths[table];
        let style = Rc::make_mut(&mut styles[table]);
        for (side, (own, widest)) in SIDES.iter().zip(edges) {
            if own > widest {
                // The table's border wins and straddles the outer grid line, half outside the box.
                let key = format!("margin-{side}");
                if let Some(margin) = style
                    .get(&key)
                    .map_or(Some(0.), |value| units.resolve(value, f32::NAN))
                {
                    style.insert(key, format!("{}px", margin - own / 2.));
                }
                style.insert(format!("padding-{side}"), "0".into());
            } else {
                // The cells' borders win, and the table keeps half of them as space.
                style.insert(format!("border-{side}-width"), "0".into());
                style.insert(format!("padding-{side}"), format!("{}px", widest / 2.));
            }
        }
        for (cell, widths) in cells {
            let style = Rc::make_mut(&mut styles[cell]);
            for (side, width) in SIDES.iter().zip(widths) {
                style.insert(format!("margin-{side}"), format!("{}px", -width / 2.));
            }
        }
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
                let mut resolved = style
                    .get(name.trim())
                    .map(String::as_str)
                    .unwrap_or(fallback.trim())
                    .to_owned();
                // WARNING: `var()` substitutes tokens, so `var(--a)var(--b)` stays two values.
                let word =
                    |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '%' | '#');
                if value[..start].ends_with(|c: char| word(c) || c == ')')
                    && resolved.starts_with(word)
                {
                    resolved.insert(0, ' ');
                }
                if resolved.ends_with(|c: char| word(c) || c == ')')
                    && value[end + 1..].starts_with(word)
                {
                    resolved.push(' ');
                }
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
    // `light dark` follows the system appearance, like browsers.
    let dark = style.get("color-scheme").is_some_and(|v| {
        v.contains("dark")
            && (!v.contains("light") || crate::desktop::native::theme_state() & 1 != 0)
    });
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

pub fn declarations(source: &str) -> Style {
    Declarations::parse(source).resolve()
}

impl Declarations {
    fn resolve(&self) -> Style {
        let mut result = Style::new();
        for declaration in &self.0 {
            match declaration {
                // WARNING: `@apply` only works with Tailwind, which expands it before the cascade.
                Declaration::Apply(_) => {}
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

impl Stylesheet {
    /// The `font-family` and `src` of each `@font-face` rule.
    pub(crate) fn font_faces(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.0.iter().filter_map(|rule| {
            let Rule::FontFace(values) = rule else {
                return None;
            };
            let values = values.resolve();
            let family = values
                .get("font-family")?
                .trim_matches(['\'', '"'])
                .to_owned();
            Some((family, values.get("src")?.clone()))
        })
    }

    pub(crate) fn matches_media(&self, width: f32, height: f32) -> bool {
        let reduced_motion = reduced_motion();
        let theme = crate::desktop::native::theme_state();
        matches!(self.0.as_slice(), [Rule::Media(queries, _)] if queries.is_empty() || queries.iter().any(|query| {
            query.matches(width, height, reduced_motion, theme, FontUnits::new(16.)) == Some(true)
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
