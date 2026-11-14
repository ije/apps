use crate::dom::{Document, Selector, SelectorCache, SelectorList};
use bbtree::unsync::BBTreeMap;
use rustc_hash::FxHashMap;
use std::{borrow::Cow, collections::HashMap, rc::Rc};

mod animation;
mod chunked;
mod color;
pub(crate) mod parser;
mod properties;
mod shorthand;
mod tailwind;
#[cfg(test)]
mod tests;
mod text;
mod value;
pub(crate) use chunked::Chunked;

pub(crate) use animation::{Animation, Easing, Motion, Scripted};
pub use color::{color, rgba};
pub(crate) use parser::{DROPPED, FontUnits, Lengths, NodeLengths};
use shorthand::{BORDER_IMAGE, BORDER_IMAGE_INITIAL, expand};
pub(crate) use text::{
    InlineText, LineHeight, cursor, font, holds_block_box, inline_text, line_height, makes_no_line,
    restyle, selection_colors, spacing, text_run, text_transform, word_break,
};
pub use value::length;
pub(crate) use value::{
    BORDERS, Filter, MARGIN, PADDING, area_cells, clip_path, components, number, write_number,
};

pub use properties::Style;
pub(crate) use properties::{Prop, Vars};

pub(crate) const DISPLAY_NONE: u16 = 1;
pub(crate) const DISPLAY_CONTENTS: u16 = 2;
pub(crate) const INVISIBLE: u16 = 4;
pub(crate) const SELECT_NONE: u16 = 8;
pub(crate) const SELECT_ALL: u16 = 16;
pub(crate) const SELECT_TEXT: u16 = 32;
const COUNTED: u16 = 64;
pub(crate) const AGENT_DISPLAY: u16 = 128;
// Table parts lay out as grids (tables), `contents` (row groups, rows) and blocks (cells, captions). Their role,
// the index of their display here, is in the high byte of the style flags. Rows and row groups outside a table
// make a table of their own, like the anonymous table around them in browsers. The last two roles are blocks whose
// children are all table parts, so they lay out as the anonymous table around those parts.
pub(crate) const TABLE_DISPLAYS: [&str; 15] = [
    "",
    "table",
    "inline-table",
    "block",
    "table-row",
    "table-row-group",
    "table-header-group",
    "table-footer-group",
    "table-cell",
    "table-caption",
    "table-row",
    "table-row-group",
    "table-header-group",
    "block",
    "inline-block",
];
pub(crate) const ROW: u16 = 4;
pub(crate) const HEADER: u16 = 6;
pub(crate) const FOOTER: u16 = 7;
pub(crate) const CELL: u16 = 8;
pub(crate) const CAPTION: u16 = 9;

pub(crate) fn role(styles: &ComputedStyles, id: usize) -> u16 {
    styles.flags.get(id).map_or(0, |flags| flags >> 8)
}

pub(crate) fn table_box(role: u16) -> bool {
    matches!(role, 1..=3 | 10..)
}

const UA: &str = "
html,body,address,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hr,legend,main,p,pre,search,article,aside,h1,h2,h3,h4,h5,h6,hgroup,nav,section,dd,dl,dt,menu,ol,ul,details,summary,fieldset,optgroup,option{display:block}
li{display:list-item}
img,svg,canvas,video,audio,iframe,embed,object,input,button,select,textarea,progress,meter{display:inline-block}
canvas[content=drawable i]{-canvas-content:drawable}
canvas[content=drawable i] [drawable]{isolation:isolate;contain:layout}
:-internal-sf-symbol{display:inline-block;width:1em;height:1em;vertical-align:middle;flex-shrink:0}
head,script,style,title,meta,link,template,noscript,datalist,base,param,[hidden],input[type=hidden],dialog:not([open]){display:none}
audio:not([controls]),embed:not([src]):not([type]){display:none!important}
audio{width:300px;height:54px}
video{object-fit:contain}
terminal{display:inline-block;width:80ch;height:24lh;overflow:hidden;font:13px ui-monospace,Menlo,Consolas,monospace;cursor:text;user-select:none}
slot{display:contents}
[dir=rtl i]{direction:rtl}
[dir=ltr i]{direction:ltr}
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
a[href]{color:LinkText;text-decoration:underline;cursor:pointer}
center,caption{text-align:center}
th{text-align:-internal-center}
hr{margin:.5em auto;border:1px inset;color:gray;overflow:hidden}
table{display:table;box-sizing:border-box;border-spacing:2px;text-indent:0}
thead{display:table-header-group}
tbody{display:table-row-group}
tfoot{display:table-footer-group}
tr{display:table-row}
td,th{display:table-cell}
caption{display:table-caption}
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
table[border]{border:1px outset}
table[border]:not([border='0']) td,table[border]:not([border='0']) th{border:1px inset}
td,th{padding:1px}
thead,tbody,tfoot,tr{vertical-align:middle}
td,th{vertical-align:inherit}
fieldset{margin:0 2px;padding:.35em .75em .625em;border:2px groove light-dark(#efefef,#6b6b6b)}
legend{padding:0 2px}
summary{display:list-item;list-style-type:disclosure-closed}
details[open]>summary{list-style-type:disclosure-open}
details:not([open])>:not(summary){display:none}
button,input,select,textarea{margin:0;font-family:Arial;font-size:13.333333px;font-weight:normal;font-style:normal;line-height:normal;letter-spacing:normal;word-spacing:normal;text-transform:none;text-indent:0;text-shadow:none;text-align:start;color:light-dark(#000,#fff)}
button,select,meter,progress{user-select:none}
label,button,select,input[type=button i],input[type=submit i],input[type=reset i],input[type=checkbox i],input[type=radio i],input[type=range i],input[type=color i],input[type=file i],input[type=ns-button i],input[type=ns-switch i],input[type=ns-range i],input[type=ns-color i]{cursor:default}
svg:not(:root){overflow:hidden}
input{padding:1px 2px;border:2px inset light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
textarea{font-family:monospace;padding:2px;overflow:auto;border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
select{border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b);box-sizing:border-box;white-space:pre}
button,input[type=button i],input[type=submit i],input[type=reset i],input[type=ns-button i]{padding:1px 6px;border:2px outset light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box;text-align:center}
input[type=checkbox i],input[type=radio i],input[type=ns-switch i]{margin:3px 3px 3px 4px;padding:0;border:0;background-color:transparent;box-sizing:border-box}
input[type=radio i]{margin:3px 3px 0 5px}
input[type=range i],input[type=ns-range i]{margin:2px;padding:0;border:0;background-color:transparent;color:light-dark(#101010,#fff)}
input[type=color i],input[type=ns-color i]{width:50px;height:27px;border:1px solid light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box}
input[type=file i]{padding:0;border:0;background-color:transparent}
input[type=file i]::file-selector-button{margin:0 4px 0 0;padding:2px 7px;border:1px solid light-dark(#767676,#858585);border-radius:2px;background-color:light-dark(#efefef,#6b6b6b)}
input[type=search i],input[type=ns-search i]{box-sizing:border-box}
summary{list-style-position:inside}
button:disabled,input[type=button i]:disabled,input[type=submit i]:disabled,input[type=reset i]:disabled,input[type=ns-button i]:disabled{color:light-dark(rgba(16,16,16,.3),rgba(255,255,255,.3));background-color:light-dark(rgba(239,239,239,.3),rgba(19,1,1,.3));border-color:light-dark(rgba(118,118,118,.3),rgba(195,195,195,.3))}
input:disabled,textarea:disabled{color:light-dark(#545454,#aaa)}
input:disabled:not([type=checkbox i],[type=radio i],[type=range i],[type=color i],[type=file i],[type=button i],[type=submit i],[type=reset i],[type=ns-button i],[type=ns-switch i],[type=ns-range i],[type=ns-color i]),textarea:disabled{background-color:light-dark(rgba(239,239,239,.3),rgba(59,59,59,.3));border-color:rgba(118,118,118,.3)}
input:-internal-native-control{margin:0;padding:0!important;border:0!important;background:none!important;font-family:system-ui;font-size:13px;vertical-align:middle;box-sizing:border-box}
input[type=ns-color i]:-internal-native-control{width:auto;height:auto}
select:disabled{opacity:.7;color:light-dark(#808080,#aaa);border-color:rgba(118,118,118,.3)}
input:focus-visible,textarea:focus-visible{outline:3px solid #8fb8f4}
progress,meter{width:10em;height:1em;vertical-align:-.2em;box-sizing:border-box}
meter{width:5em}
iframe{border:2px inset}
q::before{content:open-quote}
q::after{content:close-quote}
noembed,noframes{display:none}
select:not([multiple]):not([size]) :is(option:not(:checked),hr){display:none}
select[multiple] option,select[size]:not([size='0']):not([size='1']) option{padding:0 2px 1px;min-height:1.2em;line-height:normal!important}
select[multiple] option:checked,select[size]:not([size='0']):not([size='1']) option:checked{background-color:light-dark(#cecece,#545454)}
select[multiple],select[size]:not([size='0']):not([size='1']){overflow-y:auto;vertical-align:text-bottom}
dialog{position:absolute;left:0;right:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:1em;background-color:#fff;color:#000}
dialog:-internal-dialog-in-top-layer{position:fixed;top:0;bottom:0;max-width:calc(100% - 6px - 2em);max-height:calc(100% - 6px - 2em);overflow:auto;user-select:text;visibility:visible}
dialog::backdrop{background:rgba(0,0,0,.1)}
[popover]{position:fixed;inset:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:.25em;overflow:auto;color:CanvasText;background-color:Canvas}
[popover]:not(:popover-open):not(dialog[open]){display:none}
dialog:popover-open{display:block}
[popover]:popover-open::backdrop{pointer-events:none!important;background-color:transparent}
";

const FULLSCREEN_UA: &str = "
:not(:root):fullscreen{position:fixed!important;inset:0!important;margin:0!important;box-sizing:border-box!important;min-width:0!important;max-width:none!important;min-height:0!important;max-height:none!important;width:100%!important;height:100%!important;transform:none!important;object-fit:contain}
iframe:fullscreen{border:none!important;padding:0!important}
:not(:root):fullscreen::backdrop{background:black}
";

pub(crate) const FONT_FORMATS: [&str; 5] = ["collection", "opentype", "truetype", "woff", "woff2"];
pub(crate) const FONT_TECHS: [&str; 2] = ["features-opentype", "variations"];

thread_local! {
    static UA_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(UA));
    static FULLSCREEN_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(&format!("{UA}{FULLSCREEN_UA}")));
    #[cfg(target_os = "macos")]
    static GLASS_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(include_str!("tailwind/liquid-glass.css")));
    // Retain font-family values used as pointer keys to prevent address reuse.
    pub(crate) static FONT_NAMES: std::cell::RefCell<FontNames> = Default::default();
}

type FontNames = (
    HashMap<String, String>,
    FxHashMap<*const u8, (Option<Rc<str>>, renderer::SharedString)>,
);

struct StyleRef<T: ?Sized = Style>(Rc<T>);

impl<T: ?Sized> Clone for StyleRef<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: ?Sized> PartialEq for StyleRef<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl<T: ?Sized> Eq for StyleRef<T> {}
impl<T: ?Sized> std::hash::Hash for StyleRef<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Rc::as_ptr(&self.0) as *const (), state);
    }
}

pub(crate) const SVG: [Prop; 43] = [
    Prop::CLIP_PATH,
    Prop::CLIP_RULE,
    Prop::COLOR,
    Prop::DISPLAY,
    Prop::FILL,
    Prop::FILL_OPACITY,
    Prop::FILL_RULE,
    Prop::FILTER,
    Prop::FLOOD_COLOR,
    Prop::FLOOD_OPACITY,
    Prop::FONT_FAMILY,
    Prop::FONT_SIZE,
    Prop::FONT_STYLE,
    Prop::FONT_WEIGHT,
    Prop::IMAGE_RENDERING,
    Prop::ISOLATION,
    Prop::LETTER_SPACING,
    Prop::LIGHTING_COLOR,
    Prop::MASK,
    Prop::MASK_TYPE,
    Prop::MIX_BLEND_MODE,
    Prop::OPACITY,
    Prop::PAINT_ORDER,
    Prop::SHAPE_RENDERING,
    Prop::STOP_COLOR,
    Prop::STOP_OPACITY,
    Prop::STROKE,
    Prop::STROKE_DASHARRAY,
    Prop::STROKE_DASHOFFSET,
    Prop::STROKE_LINECAP,
    Prop::STROKE_LINEJOIN,
    Prop::STROKE_MITERLIMIT,
    Prop::STROKE_OPACITY,
    Prop::STROKE_WIDTH,
    Prop::TEXT_ANCHOR,
    Prop::TEXT_DECORATION,
    Prop::TEXT_RENDERING,
    Prop::TRANSFORM,
    Prop::TRANSFORM_ORIGIN,
    Prop::VECTOR_EFFECT,
    Prop::VISIBILITY,
    Prop::WORD_SPACING,
    Prop::FONT_STRETCH,
];

pub(crate) fn presentation(name: &str) -> Option<Prop> {
    Prop::find(name).filter(|prop| {
        SVG.contains(prop) && !matches!(*prop, Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN)
    })
}

fn svg_transform(value: &str) -> Option<String> {
    let mut result = Vec::new();
    for part in value.split_inclusive(')') {
        let part = part.trim_matches(|c: char| c == ',' || c.is_ascii_whitespace());
        if part.is_empty() {
            continue;
        }
        let (name, args) = part.strip_suffix(')')?.split_once('(')?;
        let args = args
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|arg| !arg.is_empty())
            .map(|arg| arg.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        result.push(match (name.trim(), args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => format!("matrix({a}, {b}, {c}, {d}, {e}, {f})"),
            ("translate", [x]) => format!("translate({x}px)"),
            ("translate", [x, y]) => format!("translate({x}px, {y}px)"),
            ("scale", [x]) => format!("scale({x})"),
            ("scale", [x, y]) => format!("scale({x}, {y})"),
            ("rotate", [a]) => format!("rotate({a}deg)"),
            ("rotate", [a, x, y]) => {
                format!(
                    "translate({x}px, {y}px) rotate({a}deg) translate({}px, {}px)",
                    -x, -y
                )
            }
            ("skewX", [a]) => format!("skewX({a}deg)"),
            ("skewY", [a]) => format!("skewY({a}deg)"),
            _ => return None,
        });
    }
    Some(if result.is_empty() {
        "none".into()
    } else {
        result.join(" ")
    })
}

fn var(vars: &Vars, prop: Prop) -> Option<&Rc<str>> {
    vars.binary_search_by_key(&prop, |entry| entry.0)
        .ok()
        .map(|index| &vars[index].1)
}

fn exported_parts(
    hosted: Vec<Passed>,
    own: Option<&Shadowed>,
    outer: Option<&Shadowed>,
    names: Option<&String>,
    old: Option<&Shadowed>,
) -> Option<Shadowed> {
    let mut list = hosted;
    list.extend(own.into_iter().flat_map(|own| own.iter().cloned()));
    if let (Some(outer), Some(names)) = (outer, names) {
        let pairs: Vec<(&str, &str)> = names
            .split(',')
            .filter_map(|item| {
                let (inner, name) = item.split_once(':').unwrap_or((item, item));
                let (inner, name) = (inner.trim(), name.trim());
                (!inner.is_empty() && !name.is_empty()).then_some((inner, name))
            })
            .collect();
        for passed in outer.iter().filter(|passed| passed.rule.context > 0) {
            let mut inner = vec![String::new()];
            for name in passed.name.split_ascii_whitespace() {
                inner = inner
                    .iter()
                    .flat_map(|done| {
                        pairs
                            .iter()
                            .filter(move |pair| pair.1 == name)
                            .map(move |pair| format!("{done} {}", pair.0))
                    })
                    .take(64)
                    .collect();
            }
            list.extend(
                inner
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .map(|name| {
                        let mut passed = passed.clone();
                        passed.name = name.trim_start().into();
                        passed.rule.context = passed.rule.context.saturating_add(1);
                        passed
                    }),
            );
        }
    }
    (!list.is_empty()).then(|| match old {
        Some(old) if **old == list => old.clone(),
        _ => Rc::new(list),
    })
}

// The next `var(` outside quoted strings.
fn next_var(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut index = 0;
    loop {
        index += memchr::memchr3(b'v', b'"', b'\'', bytes.get(index..)?)?;
        match bytes[index] {
            b'v' if bytes[index..].starts_with(b"var(") => return Some(index),
            b'v' => index += 1,
            quote => {
                index += 1;
                while let Some(&byte) = bytes.get(index)
                    && byte != quote
                {
                    index += if byte == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
        }
    }
}

fn references(value: &str, names: &mut Option<Vec<Prop>>, missing: &mut Vec<Box<str>>) {
    let mut rest = value;
    while let Some(list) = names.as_mut()
        && let Some(start) = next_var(rest)
    {
        rest = &rest[start + 4..];
        let name = rest[..rest.find([',', ')']).unwrap_or(rest.len())].trim();
        match Prop::find(name) {
            Some(prop) => list.push(prop),
            None if !missing.iter().any(|old| **old == *name) => missing.push(name.into()),
            None => {}
        }
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Inputs {
    parent: Option<(StyleRef, bool)>,
    units: [u32; 14],
}

impl Inputs {
    fn new(font: FontUnits, root: FontUnits) -> Self {
        let mut units = [0; 14];
        units[..7].copy_from_slice(&font.bits());
        units[7..].copy_from_slice(&root.bits());
        Self {
            parent: None,
            units,
        }
    }

    fn hash(&self) -> u64 {
        self.units.iter().fold(0, |hash: u64, &bits| {
            (hash.rotate_left(5) ^ u64::from(bits)).wrapping_mul(0x517c_c1b7_2722_0a95)
        })
    }
}

#[derive(PartialEq, Eq)]
struct Container {
    id: usize,
    size: bool,
    names: Option<Rc<str>>,
    // Queries in `em` depend on the font size.
    font: Option<Rc<str>>,
    up: Option<Rc<Container>>,
}

impl Container {
    fn of(id: usize, style: &Style, up: Option<Rc<Container>>) -> Option<Self> {
        let kind = style.get(Prop::CONTAINER_TYPE)?;
        matches!(kind, "size" | "inline-size").then(|| Self {
            id,
            size: kind == "size",
            names: style.value(Prop::CONTAINER_NAME).cloned(),
            font: style.value(Prop::FONT_SIZE).cloned(),
            up,
        })
    }
}

fn query_containers(
    conditions: &[(Option<String>, bool, MediaQuery)],
    chain: Option<&Container>,
    (scope, order): (usize, usize),
    sizes: &FxHashMap<usize, [f32; 2]>,
    lengths: impl Fn(usize) -> Lengths,
    queries: &mut QueryCache,
) -> bool {
    conditions
        .iter()
        .enumerate()
        .all(|(index, (name, block, query))| {
            let mut link = chain;
            while let Some(container) = link {
                if (!block || container.size)
                    && name.as_ref().is_none_or(|name| {
                        container.names.as_ref().is_some_and(|names| {
                            names.split_ascii_whitespace().any(|item| item == name)
                        })
                    })
                {
                    let lengths = lengths(container.id);
                    let matched = sizes.get(&container.id).is_some_and(|size| {
                        query.matches(
                            size[0],
                            if container.size { size[1] } else { f32::NAN },
                            false,
                            0,
                            lengths,
                        ) == Some(true)
                    });
                    queries.insert(
                        (scope, order, index, container.id),
                        (!container.size, matched, lengths),
                    );
                    return matched;
                }
                link = container.up.as_deref();
            }
            false
        })
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct ShadowRule {
    kind: usize,
    context: i8,
    layer: Layer,
    specificity: [usize; 3],
    order: u128,
    declarations: Rc<[Declared]>,
    query: Option<(usize, usize, bool)>,
}

const ELEMENT: usize = usize::MAX;
// Pseudo-element kinds of `Selector::without_pseudo_element`.
const SLOTTED: usize = 12;
const PART: usize = 13;
const STARTING: usize = 14;

#[derive(Clone)]
pub(crate) struct Passed {
    name: Rc<str>,
    compound: SelectorList,
    conditions: Option<(usize, usize, ContainerQuery)>,
    rule: ShadowRule,
}

impl PartialEq for Passed {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && Rc::ptr_eq(&self.compound, &other.compound)
            && self.rule == other.rule
            && match (&self.conditions, &other.conditions) {
                (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1 && Rc::ptr_eq(&a.2, &b.2),
                (a, b) => a.is_none() && b.is_none(),
            }
    }
}

#[derive(Hash, PartialEq, Eq)]
struct StyleKey {
    scope: usize,
    tag: u32,
    inline: Option<u64>,
    hints: Vec<(Prop, Rc<str>)>,
    inputs: Inputs,
    rules: StyleRef<[(usize, [usize; 3])]>,
    pseudos: Rc<[usize]>,
    shadow: Option<Rc<[ShadowRule]>>,
    selection: Option<StyleRef>,
}

#[derive(PartialEq)]
struct RareInputs {
    shadow: Option<Rc<[ShadowRule]>>,
    selection: Option<StyleRef>,
}

struct CachedNode {
    parent: Option<StyleRef>,
    units: u64,
    inherited: Option<Vars>,
    vars: Vars,
    containers: Option<Option<Rc<Container>>>,
    rare: Option<Box<RareInputs>>,
    value: Rc<CachedStyle>,
}

impl CachedNode {
    fn new(
        key: &StyleKey,
        parent: Option<&Rc<Style>>,
        value: Rc<CachedStyle>,
        inherited: Option<&Vars>,
        vars: &Vars,
        containers: Option<Option<Rc<Container>>>,
    ) -> Self {
        let rare = (key.shadow.is_some() || key.selection.is_some()).then(|| {
            Box::new(RareInputs {
                shadow: key.shadow.clone(),
                selection: key.selection.clone(),
            })
        });
        Self {
            parent: parent.cloned().map(StyleRef),
            units: key.inputs.hash(),
            inherited: inherited.cloned(),
            vars: vars.clone(),
            containers,
            rare,
            value,
        }
    }

    fn same_rare(&self, shadow: Option<&Rc<[ShadowRule]>>, selection: Option<&Rc<Style>>) -> bool {
        match &self.rare {
            None => shadow.is_none() && selection.is_none(),
            Some(rare) => {
                rare.shadow.as_ref() == shadow
                    && match (&rare.selection, selection) {
                        (Some(old), Some(new)) => Rc::ptr_eq(&old.0, new),
                        (old, new) => old.is_none() && new.is_none(),
                    }
            }
        }
    }
}

type Read = (Prop, Option<Rc<str>>);

struct CachedStyle {
    used: std::cell::Cell<u64>,
    inline: Option<Box<str>>,
    style: Rc<Style>,
    vars: std::cell::RefCell<(Option<Vars>, Vars)>,
    reads: Option<Box<[Read]>>,
    own: Box<[Read]>,
    missing: Box<[Box<str>]>,
    flags: u16,
    lengths: Lengths,
    pseudos: Option<Rc<[Style; 2]>>,
    text_pseudos: Option<Rc<[Option<Style>; 4]>>,
    backdrop: Option<Rc<Style>>,
    backdrop_start: Option<Rc<Style>>,
    scrollbars: Option<Rc<[Option<Style>; 5]>>,
    selection: Option<Rc<Style>>,
    animations: Option<Rc<[Animation]>>,
    important: Option<Rc<[Prop]>>,
    shadowed: [Option<Shadowed>; 2],
    starting: Option<Rc<Style>>,
    viewport: bool,
    keyword: bool,
}

type Key = Option<(u8, String)>;

struct Share {
    from: usize,
    rules: Rc<[(usize, [usize; 3])]>,
    pseudos: Rc<[usize]>,
    dependencies: Box<[crate::dom::Dependency]>,
    style: Option<u32>,
}
const PSEUDO: usize = 1 << (usize::BITS - 1);
const SELECTOR: usize = 32;
#[derive(Default)]
struct StyleScope {
    id: usize,
    used: u64,
    fullscreen: bool,
    sheets: Vec<Rc<Stylesheet>>,
    rules: Vec<(SelectorList, Rc<[Declared]>, Layer)>,
    layers: Vec<(String, Vec<u32>)>,
    pseudos: Vec<(usize, Selector, usize, Key)>,
    arguments: FxHashMap<usize, (Rc<str>, SelectorList, usize)>,
    elements: Vec<Vec<(usize, Key)>>,
    buckets: [FxHashMap<String, Vec<usize>>; 4],
    universal: Vec<usize>,
    containers: Vec<Option<ContainerQuery>>,
    blocks: Vec<Rc<MediaBlock>>,
    medias: Vec<Option<MediaStack>>,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    properties: Registry,
    states: u16,
    nodes: Vec<Option<MatchedRules>>,
    lists: RuleLists,
    keys: Vec<u128>,
    block: Option<u64>,
    sequence: u64,
}

// While rules are collected, `rank` is the index of the layer in `Collect.layers`, not its rank.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Layer {
    origin: u8,
    rank: u32,
}

const DELETED: u8 = u8::MAX;
const UNLAYERED: u32 = u32::MAX;

type Collected<'a> = (
    &'a SelectorList,
    &'a Declarations,
    Layer,
    Option<ContainerQuery>,
    Option<MediaStack>,
);

impl StyleScope {
    fn push(
        &mut self,
        rules: Vec<Collected>,
        tailwind: Option<&FxHashMap<usize, u64>>,
        attributes: &mut crate::dom::Named,
    ) {
        let mut lists = Vec::new();
        for (selectors, body, layer, container, media) in rules {
            let order = self.rules.len();
            let key = tailwind.and_then(|keys| keys.get(&(Rc::as_ptr(selectors) as usize)));
            self.keys.push(match key {
                Some(key) => {
                    u128::from(*self.block.get_or_insert(self.sequence)) << 64 | u128::from(*key)
                }
                None => {
                    self.sequence += 1;
                    u128::from(self.sequence) << 64
                }
            });
            let elements = selectors
                .iter()
                .enumerate()
                .filter_map(|(index, selector)| {
                    if let Some((kind, mut pseudo)) = selector.without_pseudo_element() {
                        if matches!(kind, SLOTTED | PART) {
                            let (target, classes) = selector.shadow_target()?;
                            let argument = selector.pseudo_argument().unwrap_or_default().trim();
                            let compound = Rc::new(if kind == SLOTTED {
                                Selector::parse(argument, None)
                            } else {
                                vec![classes]
                            });
                            if kind == SLOTTED
                                && let Some(own) =
                                    compound.iter().map(|selector| selector.specificity).max()
                            {
                                for (value, own) in pseudo.specificity.iter_mut().zip(own) {
                                    *value = value.saturating_add(own);
                                }
                            }
                            lists.push(compound.clone());
                            self.arguments
                                .insert(self.pseudos.len(), (argument.into(), compound, target));
                        }
                        let key: Key = pseudo.key().map(|(kind, name)| (kind, name.into()));
                        let index = PSEUDO | self.pseudos.len();
                        match &key {
                            Some((kind, name)) => self.buckets[*kind as usize]
                                .entry(name.clone())
                                .or_default()
                                .push(index),
                            None => self.universal.push(index),
                        }
                        self.pseudos.push((kind, pseudo, order, key));
                        return None;
                    }
                    let key: Key = selector.key().map(|(kind, name)| (kind, name.into()));
                    let entry = order << SELECTOR | index;
                    match &key {
                        Some((kind, name)) => self.buckets[*kind as usize]
                            .entry(name.clone())
                            .or_default()
                            .push(entry),
                        None => self.universal.push(entry),
                    }
                    Some((index, key))
                })
                .collect();
            self.elements.push(elements);
            self.containers.push(container);
            self.medias.push(media);
            self.rules
                .push((selectors.clone(), body.resolved(false).into(), layer));
            lists.push(selectors.clone());
        }
        // Parent selectors of nested rules are queued once each, so `& &` chains stay linear.
        let (mut seen, mut parents, mut subject) =
            (std::collections::HashSet::new(), Vec::new(), true);
        while let Some(list) = lists.pop().or_else(|| {
            subject = false;
            parents.pop()
        }) {
            if seen.insert((Rc::as_ptr(&list), subject)) {
                for selector in list.iter() {
                    self.states |= selector.states(&mut parents);
                    selector.attributes(attributes, subject);
                }
            }
        }
    }

    // Add the rules of new Tailwind chunks. `None` means that the scope cannot take them. `Some(true)` means
    // that only the nodes marked dirty can match them.
    fn insert(
        &mut self,
        update: &tailwind::Update,
        rules: &[Collected],
        blocks: &[Rc<MediaBlock>],
        doc: &Document,
        matches: &mut SelectorCache,
    ) -> Option<bool> {
        let generated = |key: &u128| Some((key >> 64) as u64) == self.block && *key as u64 != 0;
        let layer = self.rules[self.keys.iter().position(generated)?].2;
        if update
            .replaced
            .iter()
            .any(|old| self.sheets.iter().any(|sheet| Rc::ptr_eq(sheet, old)))
        {
            return None;
        }
        for (old, new) in &update.added {
            let index = self
                .sheets
                .iter()
                .position(|sheet| Rc::ptr_eq(sheet, old))?;
            self.sheets.splice(index..=index, new.iter().cloned());
        }
        let start = self.rules.len();
        let rules = rules
            .iter()
            .map(|(selectors, body, _, container, media)| {
                (*selectors, *body, layer, container.clone(), media.clone())
            })
            .collect();
        self.push(rules, Some(update.keys), &mut matches.attributes);
        self.blocks.extend(blocks.iter().cloned());
        Some(self.rematch(start, doc, &mut matches.dirty))
    }

    // Forget the matched rules of the nodes that the rules from `start` can match, and mark them dirty. False
    // means that the rules can also reach other nodes, so only a full pass finds them all.
    fn rematch(&mut self, start: usize, doc: &Document, dirty: &mut [bool]) -> bool {
        let added = self.pseudos.iter().filter(|pseudo| pseudo.2 >= start);
        let passed = added
            .clone()
            .any(|pseudo| matches!(pseudo.0, SLOTTED | PART));
        let keys = self.elements[start..].iter().flatten().map(|(_, key)| key);
        let mut names: [rustc_hash::FxHashSet<&str>; 4] = Default::default();
        for key in keys.chain(added.map(|pseudo| &pseudo.3)) {
            match key {
                Some((kind, name)) => names[*kind as usize].insert(name),
                None => {
                    self.nodes.clear();
                    return false;
                }
            };
        }
        let [ids, classes, tags, attributes] = &names;
        for (id, node) in doc.nodes.iter().enumerate() {
            let class = |name: &String| {
                name.split_ascii_whitespace()
                    .any(|class| classes.contains(class))
            };
            if (tags.contains(node.tag.as_str())
                || !attributes.is_empty()
                    && node
                        .attrs
                        .keys()
                        .any(|name| attributes.contains(name.as_str()))
                || !classes.is_empty() && node.attrs.get("class").is_some_and(class)
                || !ids.is_empty()
                    && node
                        .attrs
                        .get("id")
                        .is_some_and(|id| ids.contains(id.as_str())))
                && let Some(matched) = self.nodes.get_mut(id)
            {
                *matched = None;
                if let Some(dirty) = dirty.get_mut(id) {
                    *dirty = true;
                }
            }
        }
        !passed
    }
}

type RuleLists = rustc_hash::FxHashSet<Rc<[(usize, [usize; 3])]>>;

fn intern(lists: &mut RuleLists, list: &[(usize, [usize; 3])]) -> Rc<[(usize, [usize; 3])]> {
    if let Some(old) = lists.get(list) {
        return old.clone();
    }
    if lists.len() > 65536 {
        lists.clear();
    }
    let list: Rc<[_]> = list.into();
    lists.insert(list.clone());
    list
}

struct MatchedRules {
    checked: std::cell::Cell<u64>,
    cached: Option<CachedNode>,
    dependencies: Box<[crate::dom::Dependency]>,
    rules: Rc<[(usize, [usize; 3])]>,
    pseudos: Rc<[usize]>,
}

type Derived = HashMap<
    *const Stylesheet,
    (
        std::rc::Weak<Stylesheet>,
        Rc<Stylesheet>,
        Vec<usize>,
        Vec<usize>,
    ),
>;

fn derive<R, E>(
    derived: &mut Derived,
    sheet: &mut Rc<Stylesheet>,
    top: Option<(usize, bool)>,
    change: impl FnOnce(&mut Stylesheet) -> Result<R, E>,
) -> Result<R, E> {
    let before = Rc::as_ptr(sheet);
    let base = (Rc::strong_count(sheet) > 1).then(|| sheet.clone());
    let result = change(Rc::make_mut(sheet));
    let previous = derived.remove(&before);
    if top.is_some() || result.is_err() {
        let Some((base, mut inserted, mut removed)) = base
            .map(|base| (base, Vec::new(), Vec::new()))
            .or_else(|| previous.map(|(_, base, inserted, removed)| (base, inserted, removed)))
        else {
            return result;
        };
        match top.filter(|_| result.is_ok()) {
            Some((index, true)) => {
                for at in &mut inserted {
                    *at += usize::from(*at >= index);
                }
                inserted.push(index);
            }
            Some((index, false)) => {
                match inserted.iter().position(|&at| at == index) {
                    Some(position) => {
                        inserted.swap_remove(position);
                    }
                    None => {
                        let (mut old, mut current) = (0, 0);
                        while removed.contains(&old)
                            || inserted.contains(&current)
                            || current != index
                        {
                            if removed.contains(&old) {
                                old += 1;
                            } else if inserted.contains(&current) {
                                current += 1;
                            } else {
                                old += 1;
                                current += 1;
                            }
                        }
                        removed.push(old);
                    }
                }
                for at in &mut inserted {
                    *at -= usize::from(*at > index);
                }
            }
            None => {}
        }
        derived.insert(
            Rc::as_ptr(sheet),
            (Rc::downgrade(sheet), base, inserted, removed),
        );
    }
    result
}

#[derive(Default)]
pub(crate) struct StyleCache {
    pub(crate) environment: Environment,
    scopes: Vec<StyleScope>,
    next: usize,
    pass: u64,
    pub(crate) states: u16,
    sheets: HashMap<usize, Sheet>,
    cssom: HashMap<usize, Cssom>,
    pub(crate) constructed: FxHashMap<u32, Rc<Stylesheet>>,
    derived: Derived,
    pub(crate) adopted: FxHashMap<usize, Vec<u32>>,
    values: rustc_hash::FxHashMap<StyleKey, Vec<Rc<CachedStyle>>>,
    // Values keys hold tag names by these numbers, which are never freed: a page uses few tag names.
    tags: FxHashMap<String, u32>,
    heritage: FxHashMap<*const Style, (Rc<Style>, Rc<Style>)>,
    inherited: rustc_hash::FxHashSet<Rc<Style>>,
    containers: FxHashMap<usize, [f32; 2]>,
    sized: std::collections::HashSet<usize>,
    queries: QueryCache,
    pub(crate) units: bool,
    changed: Vec<u64>,
    stamp: u64,
    tables: FxHashMap<usize, CollapsedTable>,
    // Parents of table parts that are outside a table, so they can hold an anonymous table.
    orphans: Vec<usize>,
    tailwind: tailwind::Compiler,
    // Retain source lists used as pointer keys to prevent address reuse.
    inheritable: FxHashMap<*const Vec<(Prop, Rc<str>)>, Inheritable>,
    pub(crate) scripted: Registry,
    walk: Option<Rc<TreeWalk>>,
    shadows: Rc<Shadows>,
    matches: SelectorCache,
    // An incremental pass starts from the last stored output, so `full` and `styled` must always describe that
    // output.
    full: bool,
    styled: Option<Styled>,
    unshared: bool,
    #[cfg(debug_assertions)]
    pub(crate) incremental: u64,
}

// An incremental pass may start only from a stored pass that styled with the same environment.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Environment {
    viewport: [f32; 2],
    pub(crate) metrics: Option<f32>,
    reduced_motion: bool,
    native_theme: u32,
    registry: Rc<Registry>,
    scheme: Option<Rc<str>>,
}

// `scopes` must stay sorted by root, and a tree that the stored pass styled must use the same scope again.
struct Styled {
    nodes: Rc<Vec<(u32, u32)>>,
    serial: u64,
    tree: Rc<Vec<u32>>,
    trees: Rc<Vec<(usize, usize)>>,
    environment: Environment,
    scopes: Vec<(usize, usize, usize)>,
}

type ContainerQuery = Rc<[(Option<String>, bool, MediaQuery)]>;
type MediaStack = Rc<[Rc<MediaBlock>]>;

struct MediaBlock {
    queries: Vec<MediaQuery>,
    matched: std::cell::Cell<bool>,
    fixed: bool,
}
#[derive(Clone)]
struct Cssom {
    used: u64,
    text: Rc<str>,
    bundled: bool,
    disabled: bool,
    sheet: Rc<Stylesheet>,
}
type Sheet = (u64, Option<u64>, Rc<str>, Rc<Stylesheet>);
type Inheritable = (Vars, Option<Vars>);
type QueryCache = HashMap<(usize, usize, usize, usize), (bool, bool, Lengths)>;
type CellKey = (StyleRef, [(u32, Option<[Option<Rc<str>>; 2]>); 4]);

#[derive(PartialEq)]
struct Line {
    row: (usize, StyleRef),
    group: Option<(usize, StyleRef)>,
    cells: Vec<(usize, StyleRef, usize)>,
}

#[derive(Default)]
struct CollapsedTable {
    pass: u64,
    size: [f32; 2],
    table: Option<(StyleRef, [u32; 4], Rc<Style>)>,
    lines: Vec<Line>,
    adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])>,
    styles: FxHashMap<CellKey, Rc<Style>>,
}

// Compiles Tailwind for a page that a system web view shows, like the developer tools: `css`, and the
// classes among the words of `text`.
pub(crate) fn tailwind(css: &str, text: &str) -> String {
    let stop = |c: char| c.is_whitespace() || "\"'`<>=;,&".contains(c);
    let words: Vec<_> = text.split(stop).filter(|word| !word.is_empty()).collect();
    let doc = Document::parse(&format!("<p class=\"{}\">", words.join(" ")));
    let order: Vec<_> = (0..doc.nodes.len() as u32).map(|id| (id, 0)).collect();
    let mut sheets = vec![Rc::new(Stylesheet::parse(css))];
    tailwind::Compiler::default().update(&mut sheets, &doc, &order);
    // A web view does not know the rules that only configure Tailwind.
    let plain = |rule: &&Rc<Rule>| {
        !matches!(
            ***rule,
            Rule::Tailwind(_) | Rule::Theme(..) | Rule::Variant(..) | Rule::Utility(..)
        )
    };
    let rules = sheets.iter().flat_map(|sheet| &sheet.0).filter(plain);
    rules.map(|rule| rule.source()).collect()
}

impl StyleCache {
    fn walk(&mut self, doc: &Document, shadows: &Rc<Shadows>) -> Rc<TreeWalk> {
        let walk = self.walk.take().and_then(|walk| Rc::try_unwrap(walk).ok());
        let mut walk = walk.unwrap_or_default();
        let serial = walk.walk.serial;
        // This skip relies on a new shadow map whenever the tree or its shadow roots change.
        if !(walk.built.0 == doc.nodes.tree() && Rc::ptr_eq(&walk.built.1, shadows))
            && walk.walk.update(doc, &shadows.composed, None)
        {
            let special = |id: usize| {
                matches!(
                    doc.nodes[id].tag.as_str(),
                    "meta" | "style" | "link" | "html" | "table"
                )
            };
            // A patched walk keeps the lists when no node in them, and no node for them, moved or changed.
            // `tree` then stays as it is: without shadow trees, every node is in the first tree.
            let patched = shadows.hosts.is_empty()
                && walk.trees.len() == 1
                && walk.walk.since(serial).is_some_and(|touched| {
                    let mut listed = walk.special[0].iter().chain(&walk.tables);
                    touched.len() <= 256
                        && !listed.any(|&id| touched.contains(&(id as u32)))
                        && touched
                            .iter()
                            .all(|&id| !(walk.walk.contains(id as usize) && special(id as usize)))
                });
            if !patched || cfg!(debug_assertions) {
                let mut tree = vec![u32::MAX; doc.nodes.len()];
                let mut trees = vec![(0, usize::MAX)];
                let mut lists = vec![Vec::new()];
                let mut tables = Vec::new();
                for &(id, _) in walk.walk.nodes.iter() {
                    let id = id as usize;
                    let node = &doc.nodes[id];
                    let index = match node.parent {
                        Some(parent) => tree[parent].min(trees.len() as u32 - 1) as usize,
                        None => match shadows.hosts.get(&id) {
                            Some(&host) => {
                                trees.push((id, host));
                                lists.push(Vec::new());
                                trees.len() - 1
                            }
                            None => 0,
                        },
                    };
                    tree[id] = index as u32;
                    match node.tag.as_str() {
                        "table" => tables.push(id),
                        _ if special(id) => lists[index].push(id),
                        _ => {}
                    }
                }
                assert!(
                    !patched
                        || (&trees, &lists, &tables) == (&*walk.trees, &walk.special, &walk.tables),
                    "patched style walk lists differ"
                );
                if !patched {
                    walk.tree = Rc::new(tree);
                    walk.trees = Rc::new(trees);
                    (walk.special, walk.tables) = (lists, tables);
                }
            }
        }
        walk.built = (doc.nodes.tree(), shadows.clone());
        walk.used.set(self.pass);
        let walk = Rc::new(walk);
        self.walk = Some(walk.clone());
        walk
    }

    pub(crate) fn document_walk(&self) -> Option<&crate::dom::Walk> {
        self.walk.as_ref().map(|walk| &walk.walk)
    }

    pub(crate) fn forget(&mut self, gone: &[usize]) {
        for &id in gone {
            for scope in &mut self.scopes {
                if let Some(node) = scope.nodes.get_mut(id) {
                    *node = None;
                }
            }
            self.matches.forget(id);
        }
    }

    pub(crate) fn trim(&mut self, len: usize) {
        self.values = Default::default();
        self.walk = None;
        for scope in &mut self.scopes {
            scope.nodes.truncate(len);
            scope.nodes.shrink_to_fit();
            scope.lists = Default::default();
        }
        self.matches.trim(len);
    }

    fn uncache(&mut self) {
        self.values.clear();
        for scope in &mut self.scopes {
            for node in scope.nodes.iter_mut().flatten() {
                node.cached = None;
            }
        }
    }

    pub(crate) fn set_containers(
        &mut self,
        sizes: &FxHashMap<usize, [f32; 2]>,
        doc: &Document,
    ) -> bool {
        self.units = false;
        if *sizes == self.containers {
            return false;
        }
        if self
            .sized
            .iter()
            .any(|id| sizes.get(id) != self.containers.get(id))
        {
            self.units = true;
            self.full = true;
            self.sized.clear();
            self.queries.clear();
            self.uncache();
            self.containers = sizes.clone();
            return true;
        }
        let mut flipped = Vec::new();
        for (&(scope, order, index, container), &(inline, matched, lengths)) in &self.queries {
            let query = self
                .scopes
                .iter()
                .find(|cached| cached.id == scope)
                .and_then(|scope| scope.containers.get(order)?.as_ref()?.get(index));
            if query.is_none_or(|(_, _, query)| {
                sizes.get(&container).is_some_and(|size| {
                    query.matches(
                        size[0],
                        if inline { f32::NAN } else { size[1] },
                        false,
                        0,
                        lengths,
                    ) == Some(true)
                }) != matched
            }) {
                flipped.push(container);
            }
        }
        self.containers = sizes.clone();
        if flipped.is_empty() {
            return false;
        }
        self.queries
            .retain(|&(_, _, _, container), _| !flipped.contains(&container));
        let mut pending = flipped;
        while let Some(id) = pending.pop() {
            let Some(node) = doc.nodes.get(id) else {
                continue;
            };
            pending.extend(self.shadows.composed.get(&id).unwrap_or(&node.children));
            for scope in &mut self.scopes {
                if let Some(Some(node)) = scope.nodes.get_mut(id) {
                    node.cached = None;
                }
            }
            self.matches.mark(id, false);
        }
        true
    }

    pub(crate) fn font_faces(&self) -> Vec<(String, String, Option<String>)> {
        let mut seen = std::collections::HashSet::new();
        self.scopes
            .iter()
            .flat_map(|scope| &scope.sheets)
            .filter(|sheet| seen.insert(Rc::as_ptr(sheet)))
            .flat_map(|sheet| sheet.font_faces())
            .collect()
    }

    pub(crate) fn sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    ) -> Option<Rc<Stylesheet>> {
        let node = &doc.nodes[id];
        let pass = self.pass;
        let bundled = node.stylesheet.as_ref().or(match node.children[..] {
            [text] => doc.nodes[text].stylesheet.as_ref(),
            _ => None,
        });
        let same = |text: &str| {
            let mut rest = text;
            let mut pending = vec![id];
            while let Some(child) = pending.pop() {
                let node = &doc.nodes[child];
                if child != id && node.tag == "#comment" {
                    continue;
                }
                match rest.strip_prefix(node.text.as_str()) {
                    Some(tail) if child == id || node.stylesheet.is_none() => rest = tail,
                    _ => return false,
                }
                pending.extend(node.children.iter().rev());
            }
            rest.is_empty()
        };
        if let Some(edited) = self.cssom.get_mut(&id).filter(|edited| {
            edited.bundled == bundled.is_some() && (bundled.is_some() || same(&edited.text))
        }) {
            edited.used = pass;
            return (!edited.disabled).then(|| edited.sheet.clone());
        }
        self.cssom.remove(&id);
        if let Some(sheet) = bundled {
            return Some(sheet.clone());
        }
        let stamp = node
            .children
            .iter()
            .try_fold(doc.nodes.revision(id), |stamp, &child| {
                doc.nodes[child]
                    .children
                    .is_empty()
                    .then(|| stamp.max(doc.nodes.revision(child)))
            });
        if let Some((used, old, text, sheet)) = self.sheets.get_mut(&id)
            && (stamp.is_some() && *old == stamp || same(text))
        {
            *used = pass;
            *old = stamp;
            return Some(sheet.clone());
        }
        let text = doc.text(id);
        let (text, sheet) = match cache.get_key_value(text.as_str()) {
            Some((text, sheet)) => (text.clone(), sheet.clone()),
            None => {
                let previous = self.sheets.get(&id).filter(|(_, _, old, _)| {
                    let (mut depth, mut quote, mut comment, mut escaped) = (0i32, 0u8, None, false);
                    let bytes = old.as_bytes();
                    for (index, &byte) in bytes.iter().enumerate() {
                        match byte {
                            _ if escaped => escaped = false,
                            b'\\' => escaped = true,
                            b'/' if comment.is_some_and(|open| {
                                index > open + 1 && bytes[index - 1] == b'*'
                            }) =>
                            {
                                comment = None
                            }
                            _ if comment.is_some() => {}
                            b'*' if quote == 0 && index > 0 && bytes[index - 1] == b'/' => {
                                comment = Some(index)
                            }
                            b'"' | b'\'' if quote == 0 => quote = byte,
                            _ if quote == byte => quote = 0,
                            b'{' if quote == 0 => depth += 1,
                            b'}' if quote == 0 => depth -= 1,
                            _ => {}
                        }
                    }
                    text.starts_with(&**old)
                        && depth == 0
                        && quote == 0
                        && comment.is_none()
                        && !escaped
                        && old.trim_end().ends_with(['}', ';'])
                });
                let sheet = match previous {
                    Some((_, _, old, base)) => {
                        let mut rules = base.0.clone();
                        let start = rules.len();
                        rules.extend(Stylesheet::parse(&text[old.len()..]).0);
                        let sheet = Rc::new(Stylesheet(rules));
                        self.derived.insert(
                            Rc::as_ptr(&sheet),
                            (
                                Rc::downgrade(&sheet),
                                base.clone(),
                                (start..sheet.0.len()).collect(),
                                Vec::new(),
                            ),
                        );
                        sheet
                    }
                    None => Rc::new(Stylesheet::parse(&text)),
                };
                let text: Rc<str> = text.into();
                cache.insert(text.clone(), sheet.clone());
                (text, sheet)
            }
        };
        self.sheets.insert(id, (pass, stamp, text, sheet.clone()));
        Some(sheet)
    }

    pub(crate) fn edit_constructed(
        &mut self,
        id: u32,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        match self.constructed.get_mut(&id) {
            Some(sheet) => derive(&mut self.derived, sheet, top, change),
            None => Err("InvalidStateError"),
        }
    }

    pub(crate) fn registry(&self) -> Rc<Registry> {
        self.environment.registry.clone()
    }

    pub(crate) fn edit_sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet, &mut bool) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        let current = self.sheet(doc, id, cache);
        let (mut sheet, mut disabled) = self.cssom.remove(&id).map_or_else(
            || (current.unwrap_or_default(), false),
            |edited| (edited.sheet, edited.disabled),
        );
        let result = derive(&mut self.derived, &mut sheet, top, |sheet| {
            change(sheet, &mut disabled)
        });
        let mut text = String::new();
        let mut pending = vec![id];
        while let Some(child) = pending.pop() {
            let node = &doc.nodes[child];
            if child == id || node.tag != "#comment" {
                text.push_str(&node.text);
                pending.extend(node.children.iter().rev());
            }
        }
        let bundled = doc.nodes[id].stylesheet.is_some()
            || matches!(doc.nodes[id].children[..], [child] if doc.nodes[child].stylesheet.is_some());
        self.cssom.insert(
            id,
            Cssom {
                used: self.pass,
                text: text.into(),
                bundled,
                disabled,
                sheet,
            },
        );
        result
    }

    pub(crate) fn finish(&mut self) {
        let pass = self.pass;
        let count = self.scopes.len();
        self.scopes.retain(|scope| scope.used == pass);
        if self.scopes.len() < count {
            let live: rustc_hash::FxHashSet<_> = self.scopes.iter().map(|scope| scope.id).collect();
            self.values.retain(|key, _| live.contains(&key.scope));
            self.queries.retain(|key, _| live.contains(&key.0));
            self.matches.forget_selectors();
        }
        self.sheets.retain(|_, (used, ..)| *used == pass);
        self.tables.retain(|_, table| table.pass == pass);
        if self
            .walk
            .as_ref()
            .is_some_and(|walk| walk.used.get() != pass)
        {
            self.walk = None;
        }
        // A filtered list is also the source of its own entry, so dropping one entry can free another.
        loop {
            let count = self.inheritable.len();
            self.inheritable
                .retain(|_, (from, _)| Rc::strong_count(from) > 1);
            if self.inheritable.len() == count {
                break;
            }
        }
        self.derived.clear();
        self.heritage
            .retain(|_, (style, _)| Rc::strong_count(style) > 1);
        self.inherited.retain(|style| Rc::strong_count(style) > 1);
        self.cssom.retain(|_, edited| edited.used >= pass);
        // Incremental passes don't visit clean nodes, so values that nodes still hold stay.
        if self.values.len() > 4096 {
            self.values.retain(|_, values| {
                values.retain(|value| value.used.get() == pass || Rc::strong_count(value) > 1);
                !values.is_empty()
            });
        }
        self.states = self
            .scopes
            .iter()
            .fold(0, |states, scope| states | scope.states);
        self.pass += 1;
    }
}
// Origin values are in precedence order and important ones rank `IMPORTANT - origin`, so don't reorder them.
const AGENT: u8 = 0;
const USER: u8 = 1;
const HINT: u8 = 2;
const AUTHOR: u8 = 3;
const ANIMATION: u8 = 4;
const IMPORTANT: u8 = 8;

// The derived `Ord` compares fields in declaration order, which must stay the cascade's order.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct CascadeRank {
    level: u8,
    context: i8,
    inline: bool,
    layer: u32,
    specificity: [u32; 3],
    order: u128,
    position: u16,
}

impl CascadeRank {
    fn new(origin: u8, important: bool, layer: u32, specificity: [usize; 3], order: u128) -> Self {
        Self {
            level: if important {
                IMPORTANT - origin
            } else {
                origin
            },
            context: 0,
            inline: false,
            layer: if important { !layer } else { layer },
            specificity: specificity.map(|value| value.min(u32::MAX as usize) as u32),
            order,
            position: 0,
        }
    }

    fn rule(
        layer: Layer,
        important: bool,
        specificity: [usize; 3],
        order: u128,
        position: u16,
    ) -> Self {
        Self {
            position,
            ..Self::new(layer.origin, important, layer.rank, specificity, order)
        }
    }

    fn important(&self) -> bool {
        self.level > ANIMATION
    }

    fn source(&self) -> (u8, u32) {
        let origin = if self.important() {
            IMPORTANT - self.level
        } else {
            self.level
        };
        (
            origin.min(HINT),
            if self.important() {
                !self.layer
            } else {
                self.layer
            },
        )
    }
}

// The last field is the position of the declaration in its block, which breaks ties between properties.
type Declared = (Prop, Rc<str>, bool, u16);

#[derive(Clone, Debug, Default, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Declarations(Vec<Declaration>);

#[derive(Clone, Debug, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
enum Declaration {
    Property(String, String),
    Apply(Vec<String>),
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Stylesheet(Vec<Rc<Rule>>);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum Rule {
    Style(SelectorList, Vec<Rule>),
    Media(Vec<MediaQuery>, Vec<Rule>),
    Layer(Vec<String>, Vec<Rule>),
    Supports(String, bool, Vec<Rule>),
    Declarations(Declarations),
    Theme(bool, Vec<Rule>),
    Variant(String, String),
    Utility(String, String),
    FontFace(Declarations),
    Tailwind(Option<bool>),
    Keyframes(String, Vec<(f32, Declarations)>),
    Container(Option<String>, bool, MediaQuery, Vec<Rule>),
    Property(String, String, bool, Option<String>),
    Starting(Vec<Rule>),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum MediaQuery {
    All,
    Feature(String, String, String),
    Unknown(String),
    Not(Box<MediaQuery>),
    And(Vec<MediaQuery>),
    Or(Vec<MediaQuery>),
    Typed(String, Box<MediaQuery>),
}

fn grammar(name: &str) -> Option<&'static str> {
    match name {
        "-webkit-text-decoration" => Some("*"),
        _ => Prop::find(name)?.grammar(),
    }
}

pub(crate) fn supports(name: &str, value: &str) -> Option<bool> {
    let value = value.trim();
    if name.starts_with("--") {
        return Some(true);
    }
    let lower = value.to_ascii_lowercase();
    if let Some(index) = name
        .strip_prefix("border-image")
        .map(|longhand| {
            (
                longhand,
                BORDER_IMAGE.iter().position(|name| name[12..] == *longhand),
            )
        })
        .filter(|(longhand, index)| longhand.is_empty() || index.is_some())
        .map(|(_, index)| index)
    {
        if lower.contains("var(")
            || matches!(
                lower.as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            )
        {
            return Some(true);
        }
        // A longhand is valid when the shorthand takes it in its place and sets nothing else.
        let text = match index {
            Some(2) => format!("100% / {value}"),
            Some(3) => format!("100% / / {value}"),
            _ => value.to_owned(),
        };
        return Some(shorthand::border_image(&text).is_some_and(|parts| {
            index.is_none()
                || (0..5).all(|other| {
                    Some(other) == index
                        || (other == 1 && matches!(index, Some(2 | 3)))
                        || parts[other] == BORDER_IMAGE_INITIAL[other]
                })
        }));
    }
    let grammar = grammar(name)?;
    Some(
        !value.is_empty()
            && (lower.contains("var(")
                || matches!(
                    lower.as_str(),
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer"
                )
                || syntax_matches(grammar, &lower)),
    )
}

// Readers parse `<integer>` and `<number>` values as plain numbers, so math in them folds when it is stored.
fn fold(name: &str, value: &str) -> Option<String> {
    if !value.ends_with(')') {
        return None;
    }
    let grammar = grammar(name)?;
    let integer = grammar.contains("integer>");
    if !integer && !grammar.contains("number>") {
        return None;
    }
    let (mut value, kind) = Lengths::new(16., 100., 100.).math(value, f32::NAN, &[])?;
    if grammar.contains("<non-negative-") {
        value = value.max(0.);
    }
    (kind == parser::Kind::Number).then(|| {
        if integer {
            (value.round() as i64).to_string()
        } else {
            (value as f32 + 0.).to_string()
        }
    })
}

// Keep sorted.
const INITIAL: &[(&str, &str)] = &[
    ("-macos-glass", "none"),
    ("-macos-glass-interactive", "false"),
    ("-macos-glass-tint-color", "transparent"),
    ("-terminal-black", "auto"),
    ("-terminal-blue", "auto"),
    ("-terminal-bright-black", "auto"),
    ("-terminal-bright-blue", "auto"),
    ("-terminal-bright-cyan", "auto"),
    ("-terminal-bright-green", "auto"),
    ("-terminal-bright-magenta", "auto"),
    ("-terminal-bright-red", "auto"),
    ("-terminal-bright-white", "auto"),
    ("-terminal-bright-yellow", "auto"),
    ("-terminal-cyan", "auto"),
    ("-terminal-green", "auto"),
    ("-terminal-magenta", "auto"),
    ("-terminal-red", "auto"),
    ("-terminal-white", "auto"),
    ("-terminal-yellow", "auto"),
    ("accent-color", "auto"),
    ("align-content", "normal"),
    ("align-items", "normal"),
    ("align-self", "auto"),
    ("animation-delay", "0s"),
    ("animation-direction", "normal"),
    ("animation-duration", "0s"),
    ("animation-fill-mode", "none"),
    ("animation-iteration-count", "1"),
    ("animation-name", "none"),
    ("animation-play-state", "running"),
    ("animation-timing-function", "ease"),
    ("app-region", "none"),
    ("appearance", "none"),
    ("aspect-ratio", "auto"),
    ("backdrop-filter", "none"),
    ("backface-visibility", "visible"),
    ("background-attachment", "scroll"),
    ("background-blend-mode", "normal"),
    ("background-clip", "border-box"),
    ("background-color", "transparent"),
    ("background-image", "none"),
    ("background-origin", "padding-box"),
    ("background-position", "0% 0%"),
    ("background-repeat", "repeat"),
    ("background-size", "auto"),
    ("border-bottom-color", "currentcolor"),
    ("border-bottom-left-radius", "0px"),
    ("border-bottom-right-radius", "0px"),
    ("border-bottom-style", "none"),
    ("border-bottom-width", "3px"),
    ("border-collapse", "separate"),
    ("border-image-outset", "0"),
    ("border-image-repeat", "stretch"),
    ("border-image-slice", "100%"),
    ("border-image-source", "none"),
    ("border-image-width", "1"),
    ("border-left-color", "currentcolor"),
    ("border-left-style", "none"),
    ("border-left-width", "3px"),
    ("border-right-color", "currentcolor"),
    ("border-right-style", "none"),
    ("border-right-width", "3px"),
    ("border-spacing", "0px 0px"),
    ("border-top-color", "currentcolor"),
    ("border-top-left-radius", "0px"),
    ("border-top-right-radius", "0px"),
    ("border-top-style", "none"),
    ("border-top-width", "3px"),
    ("bottom", "auto"),
    ("box-shadow", "none"),
    ("box-sizing", "content-box"),
    ("caption-side", "top"),
    ("caret-color", "auto"),
    ("clear", "none"),
    ("clip-path", "none"),
    ("color", "canvastext"),
    ("color-scheme", "normal"),
    ("column-count", "auto"),
    ("column-gap", "normal"),
    ("column-rule-color", "currentcolor"),
    ("column-rule-style", "none"),
    ("column-rule-width", "3px"),
    ("column-width", "auto"),
    ("contain", "none"),
    ("container-name", "none"),
    ("container-type", "normal"),
    ("content", "normal"),
    ("counter-increment", "none"),
    ("counter-reset", "none"),
    ("cursor", "auto"),
    ("direction", "ltr"),
    ("display", "inline"),
    ("empty-cells", "show"),
    ("field-sizing", "fixed"),
    ("fill", "black"),
    ("fill-opacity", "1"),
    ("filter", "none"),
    ("flex-basis", "auto"),
    ("flex-direction", "row"),
    ("flex-grow", "0"),
    ("flex-shrink", "1"),
    ("flex-wrap", "nowrap"),
    ("float", "none"),
    ("font-family", "system-ui"),
    ("font-size", "16px"),
    ("font-stretch", "100%"),
    ("font-style", "normal"),
    ("font-weight", "400"),
    ("grid-auto-columns", "auto"),
    ("grid-auto-flow", "row"),
    ("grid-auto-rows", "auto"),
    ("grid-column-end", "auto"),
    ("grid-column-start", "auto"),
    ("grid-row-end", "auto"),
    ("grid-row-start", "auto"),
    ("grid-template-areas", "none"),
    ("grid-template-columns", "none"),
    ("grid-template-rows", "none"),
    ("height", "auto"),
    ("hyphens", "manual"),
    ("image-rendering", "auto"),
    ("isolation", "auto"),
    ("justify-content", "normal"),
    ("justify-items", "normal"),
    ("justify-self", "auto"),
    ("left", "auto"),
    ("letter-spacing", "normal"),
    ("line-height", "normal"),
    ("list-style-image", "none"),
    ("list-style-position", "outside"),
    ("list-style-type", "disc"),
    ("margin-bottom", "0px"),
    ("margin-left", "0px"),
    ("margin-right", "0px"),
    ("margin-top", "0px"),
    ("mask-image", "none"),
    ("max-height", "none"),
    ("max-width", "none"),
    ("min-height", "auto"),
    ("min-width", "auto"),
    ("mix-blend-mode", "normal"),
    ("object-fit", "fill"),
    ("object-position", "50% 50%"),
    ("opacity", "1"),
    ("order", "0"),
    ("outline-color", "currentcolor"),
    ("outline-offset", "0px"),
    ("outline-style", "none"),
    ("outline-width", "3px"),
    ("overflow-wrap", "normal"),
    ("overflow-x", "visible"),
    ("overflow-y", "visible"),
    ("overlay", "none"),
    ("overscroll-behavior-x", "auto"),
    ("overscroll-behavior-y", "auto"),
    ("padding-bottom", "0px"),
    ("padding-left", "0px"),
    ("padding-right", "0px"),
    ("padding-top", "0px"),
    ("perspective", "none"),
    ("perspective-origin", "50% 50%"),
    ("pointer-events", "auto"),
    ("position", "static"),
    ("quotes", "auto"),
    ("resize", "none"),
    ("right", "auto"),
    ("rotate", "none"),
    ("row-gap", "normal"),
    ("scale", "none"),
    ("scroll-behavior", "auto"),
    ("scrollbar-color", "auto"),
    ("scrollbar-gutter", "auto"),
    ("scrollbar-width", "auto"),
    ("stroke", "none"),
    ("stroke-width", "1px"),
    ("tab-size", "8"),
    ("table-layout", "auto"),
    ("text-align", "start"),
    ("text-box", "normal"),
    ("text-box-edge", "auto"),
    ("text-box-trim", "none"),
    ("text-decoration-color", "currentcolor"),
    ("text-decoration-line", "none"),
    ("text-decoration-style", "solid"),
    ("text-decoration-thickness", "auto"),
    ("text-indent", "0px"),
    ("text-overflow", "clip"),
    ("text-rendering", "auto"),
    ("text-shadow", "none"),
    ("text-size-adjust", "auto"),
    ("text-transform", "none"),
    ("text-underline-offset", "auto"),
    ("text-wrap", "wrap"),
    ("top", "auto"),
    ("touch-action", "auto"),
    ("transform", "none"),
    ("transform-origin", "50% 50%"),
    ("transform-style", "flat"),
    ("transition-behavior", "normal"),
    ("transition-delay", "0s"),
    ("transition-duration", "0s"),
    ("transition-property", "all"),
    ("transition-timing-function", "ease"),
    ("translate", "none"),
    ("unicode-bidi", "normal"),
    ("user-select", "auto"),
    ("vertical-align", "baseline"),
    ("view-transition-name", "none"),
    ("visibility", "visible"),
    ("white-space", "normal"),
    ("width", "auto"),
    ("will-change", "auto"),
    ("word-break", "normal"),
    ("word-spacing", "0px"),
    ("writing-mode", "horizontal-tb"),
    ("z-index", "auto"),
    ("zoom", "1"),
];

const SHORTHANDS: &[(&str, &[&str])] = &[
    (
        "border-color",
        &[
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
        ],
    ),
    (
        "border-style",
        &[
            "border-top-style",
            "border-right-style",
            "border-bottom-style",
            "border-left-style",
        ],
    ),
    (
        "border-width",
        &[
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ],
    ),
    ("inset", &["top", "right", "bottom", "left"]),
    (
        "margin",
        &["margin-top", "margin-right", "margin-bottom", "margin-left"],
    ),
    (
        "padding",
        &[
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ],
    ),
    ("flex", &["flex-grow", "flex-shrink", "flex-basis"]),
    ("gap", &["row-gap", "column-gap"]),
    ("overflow", &["overflow-x", "overflow-y"]),
    (
        "overscroll-behavior",
        &["overscroll-behavior-x", "overscroll-behavior-y"],
    ),
    ("place-content", &["align-content", "justify-content"]),
    ("place-items", &["align-items", "justify-items"]),
    ("place-self", &["align-self", "justify-self"]),
    (
        "border-top",
        &["border-top-width", "border-top-style", "border-top-color"],
    ),
    (
        "border-right",
        &[
            "border-right-width",
            "border-right-style",
            "border-right-color",
        ],
    ),
    (
        "border-bottom",
        &[
            "border-bottom-width",
            "border-bottom-style",
            "border-bottom-color",
        ],
    ),
    (
        "border-left",
        &[
            "border-left-width",
            "border-left-style",
            "border-left-color",
        ],
    ),
    (
        "outline",
        &["outline-width", "outline-style", "outline-color"],
    ),
    (
        "text-decoration",
        &[
            "text-decoration-line",
            "text-decoration-style",
            "text-decoration-color",
        ],
    ),
    (
        "list-style",
        &["list-style-position", "list-style-image", "list-style-type"],
    ),
];

pub(crate) fn resolved(
    style: &Style,
    units: &Lengths,
    name: &str,
    size: Option<[f32; 2]>,
    container: &dyn Fn() -> [f32; 2],
    registry: &Registry,
) -> String {
    let cell = std::cell::OnceCell::new();
    let container = &|| *cell.get_or_init(container);
    let name = &*physical_name(style, name);
    if name.starts_with("--") {
        return Prop::find(name)
            .and_then(|prop| {
                style
                    .value(prop)
                    .or_else(|| registry.get(&prop)?.2.as_ref())
            })
            .map_or_else(String::new, |value| value.to_string());
    }
    let get = |name: &str| resolved(style, units, name, size, container, registry);
    // The item count comes from the longhand that defines it, because not every shorthand is stored.
    let list = |shorthand: &str, key: &str, defining: &str| {
        let parts = |key: &str| -> Vec<String> {
            components(&get(&format!("{shorthand}-{key}")), ',')
                .into_iter()
                .map(|part| part.trim().to_owned())
                .collect()
        };
        let count = parts(defining).len().max(1);
        let parts = parts(key);
        (0..count)
            .map(|index| {
                parts
                    .get(index % parts.len().max(1))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    };
    match name {
        "font" => {
            let mut parts = Vec::new();
            for (longhand, initial) in [
                ("font-style", "normal"),
                ("font-variant", "normal"),
                ("font-weight", "400"),
                ("font-stretch", "100%"),
            ] {
                let value = get(longhand);
                if value != initial && value != "normal" {
                    parts.push(value);
                }
            }
            let line = get("line-height");
            parts.push(if line == "normal" {
                get("font-size")
            } else {
                format!("{} / {line}", get("font-size"))
            });
            parts.push(get("font-family"));
            return parts.join(" ");
        }
        "background" => {
            let images = list("background", "image", "image");
            let layers: Vec<String> = (0..images.len())
                .map(|index| {
                    let pick = |key: &str| list("background", key, "image").swap_remove(index);
                    let mut layer = format!(
                        "{} {} {} {} / {} {} {}",
                        images[index],
                        pick("repeat"),
                        pick("attachment"),
                        pick("position"),
                        pick("size"),
                        pick("origin"),
                        pick("clip")
                    );
                    if index + 1 == images.len() {
                        layer = format!("{} {layer}", get("background-color"));
                    }
                    layer
                })
                .collect();
            return layers.join(", ");
        }
        "transition" | "animation" => {
            let defining = if name == "transition" {
                "property"
            } else {
                "name"
            };
            let keys: &[(&str, &str)] = if name == "transition" {
                &[
                    ("property", "all"),
                    ("duration", "0s"),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("behavior", "normal"),
                ]
            } else {
                &[
                    ("duration", "0s"),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("iteration-count", "1"),
                    ("direction", "normal"),
                    ("fill-mode", "none"),
                    ("play-state", "running"),
                    ("name", "none"),
                ]
            };
            let columns: Vec<Vec<String>> = keys
                .iter()
                .map(|(key, _)| list(name, key, defining))
                .collect();
            let count = columns.iter().map(Vec::len).max().unwrap_or(1);
            let delay = keys
                .iter()
                .position(|(key, _)| *key == "delay")
                .unwrap_or(0);
            return (0..count)
                .map(|index| {
                    let at = |column: &Vec<String>| column.get(index).cloned().unwrap_or_default();
                    // Like Chrome, a delay needs its duration, and an item of only initial values keeps its name.
                    let kept = keys
                        .iter()
                        .zip(&columns)
                        .filter_map(|((key, initial), values)| {
                            let value = at(values);
                            (value != *initial || *key == "duration" && at(&columns[delay]) != "0s")
                                .then_some(value)
                        })
                        .collect::<Vec<_>>();
                    if kept.is_empty() {
                        at(&columns[if name == "transition" {
                            0
                        } else {
                            keys.len() - 1
                        }])
                    } else {
                        kept.join(" ")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
        }
        "border-radius" => {
            let corners: Vec<Vec<String>> = [
                "border-top-left-radius",
                "border-top-right-radius",
                "border-bottom-right-radius",
                "border-bottom-left-radius",
            ]
            .iter()
            .map(|corner| {
                components(&get(corner), ' ')
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
            let axis = |index: usize| {
                shortest(std::array::from_fn(|corner| {
                    let corner = &corners[corner];
                    corner
                        .get(index)
                        .or(corner.first())
                        .map_or("0px", String::as_str)
                }))
            };
            let (horizontal, vertical) = (axis(0), axis(1));
            return if horizontal == vertical {
                horizontal
            } else {
                format!("{horizontal} / {vertical}")
            };
        }
        "outline" => {
            return ["outline-color", "outline-style", "outline-width"]
                .map(get)
                .join(" ");
        }
        "text-decoration" => {
            let mut parts = vec![get("text-decoration-line")];
            for (longhand, initial) in [
                ("text-decoration-thickness", "auto"),
                ("text-decoration-style", "solid"),
            ] {
                let value = get(longhand);
                if value != initial {
                    parts.push(value);
                }
            }
            // The cascade can store `currentcolor` as the color it stands for, so equal colors count as it.
            let color = get("text-decoration-color");
            if color != get("color") {
                parts.push(color);
            }
            return parts.join(" ");
        }
        "border-image" => {
            let [source, slice, width, outset, repeat] = BORDER_IMAGE.map(get);
            return if source == "none" {
                source
            } else {
                format!("{source} {slice} / {width} / {outset} {repeat}")
            };
        }
        "border-block" | "border-inline" => {
            let [start, end] = if name == "border-block" {
                ["border-top", "border-bottom"]
            } else {
                ["border-left", "border-right"]
            }
            .map(get);
            return if start == end { start } else { String::new() };
        }
        "margin-block" | "margin-inline" | "padding-block" | "padding-inline" | "inset-block"
        | "inset-inline" => {
            let mut sides = Vec::new();
            expand(name, "0 1", &mut sides);
            let [start, end] = [0, sides.len().saturating_sub(1)].map(|index| {
                sides
                    .get(index)
                    .map_or_else(String::new, |(side, _)| get(side))
            });
            return if start == end {
                start
            } else {
                format!("{start} {end}")
            };
        }
        "border" => {
            let sides = ["border-top", "border-right", "border-bottom", "border-left"].map(get);
            return if sides.iter().all(|side| *side == sides[0]) {
                sides[0].clone()
            } else {
                String::new()
            };
        }
        "flex-flow" => {
            let (direction, wrap) = (get("flex-direction"), get("flex-wrap"));
            return if wrap == "nowrap" {
                direction
            } else {
                format!("{direction} {wrap}")
            };
        }
        "columns" => {
            let (width, count) = (get("column-width"), get("column-count"));
            return match (width.as_str(), count.as_str()) {
                ("auto", "auto") => "auto".into(),
                ("auto", _) => count,
                (_, "auto") => width,
                _ => format!("{width} {count}"),
            };
        }
        "grid-row" | "grid-column" => {
            let start = get(&format!("{name}-start"));
            let end = get(&format!("{name}-end"));
            return if end == "auto" {
                start
            } else {
                format!("{start} / {end}")
            };
        }
        "grid-area" => {
            let mut parts: Vec<String> = [
                "grid-row-start",
                "grid-column-start",
                "grid-row-end",
                "grid-column-end",
            ]
            .iter()
            .map(|longhand| get(longhand))
            .collect();
            while parts.len() > 1 && parts.last().is_some_and(|part| part == "auto") {
                parts.pop();
            }
            return parts.join(" / ");
        }
        "grid-template" => {
            let (rows, columns, areas) = (
                get("grid-template-rows"),
                get("grid-template-columns"),
                get("grid-template-areas"),
            );
            if areas == "none" {
                return format!("{rows} / {columns}");
            }
            let names: Vec<&str> = areas
                .split('"')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect();
            let sizes = components(&rows, ' ');
            let lines: Vec<String> = names
                .iter()
                .enumerate()
                .map(|(index, row)| match sizes.get(index) {
                    Some(size) if *size != "auto" => format!("\"{row}\" {size}"),
                    _ => format!("\"{row}\""),
                })
                .collect();
            return format!("{} / {columns}", lines.join(" "));
        }
        "grid" => {
            return [
                "grid-template-rows",
                "grid-template-columns",
                "grid-template-areas",
                "grid-auto-flow",
                "grid-auto-rows",
                "grid-auto-columns",
            ]
            .map(get)
            .join(" / ");
        }
        _ => {}
    }
    if let Some((_, longhands)) = SHORTHANDS.iter().find(|(shorthand, _)| *shorthand == name) {
        let values: Vec<_> = longhands
            .iter()
            .map(|longhand| resolved(style, units, longhand, size, container, registry))
            .collect();
        return match values.as_slice() {
            [top, right, bottom, left] => shortest([top, right, bottom, left].map(String::as_str)),
            [a, b] if a == b => a.clone(),
            _ => values.join(" "),
        };
    }
    let Some(prop) = Prop::find(name) else {
        return String::new();
    };
    let initial = INITIAL
        .binary_search_by(|(property, _)| (*property).cmp(name))
        .ok()
        .map(|index| INITIAL[index].1);
    let shorthand = |shorthand: Prop, key: &str| {
        let list = components(style.get(shorthand)?, ',');
        let parts: Vec<_> = list
            .iter()
            .map(|item| {
                let mut times = 0;
                let parts = components(item, ' ');
                let found = parts.into_iter().find(|part| {
                    let time = part.ends_with('s')
                        && part.trim_end_matches(['m', 's']).parse::<f32>().is_ok();
                    times += usize::from(time);
                    match key {
                        "duration" => time && times == 1,
                        "delay" => time && times == 2,
                        "iteration-count" => *part == "infinite" || part.parse::<f32>().is_ok(),
                        "direction" => matches!(
                            *part,
                            "normal" | "reverse" | "alternate" | "alternate-reverse"
                        ),
                        "fill-mode" => matches!(*part, "forwards" | "backwards" | "both"),
                        "play-state" => matches!(*part, "paused" | "running"),
                        "behavior" => matches!(*part, "allow-discrete"),
                        "timing-function" => {
                            matches!(
                                *part,
                                "linear"
                                    | "ease"
                                    | "ease-in"
                                    | "ease-out"
                                    | "ease-in-out"
                                    | "step-start"
                                    | "step-end"
                            ) || part.starts_with("cubic-bezier(")
                                || part.starts_with("steps(")
                                || part.starts_with("linear(")
                        }
                        _ => {
                            !time
                                && part.parse::<f32>().is_err()
                                && !matches!(
                                    *part,
                                    "infinite"
                                        | "normal"
                                        | "reverse"
                                        | "alternate"
                                        | "alternate-reverse"
                                        | "forwards"
                                        | "backwards"
                                        | "both"
                                        | "none"
                                        | "paused"
                                        | "running"
                                        | "linear"
                                        | "ease"
                                        | "ease-in"
                                        | "ease-out"
                                        | "ease-in-out"
                                        | "step-start"
                                        | "step-end"
                                        | "allow-discrete"
                                )
                                && !part.contains('(')
                        }
                    }
                });
                found.map_or_else(
                    || initial.unwrap_or_default().to_owned(),
                    |part| part.to_owned(),
                )
            })
            .collect();
        Some(parts.join(", "))
    };
    let value = match style.get(prop) {
        Some(value) => value.to_owned(),
        None => match name.split_once('-') {
            Some(("animation", key)) => shorthand(Prop::ANIMATION, key),
            Some(("transition", "property")) if style.get(Prop::TRANSITION) == Some("none") => {
                Some("none".to_owned())
            }
            Some(("transition", key)) => shorthand(Prop::TRANSITION, key),
            _ => None,
        }
        .unwrap_or_else(|| initial.unwrap_or_default().to_owned()),
    };
    let number = |value: f32| self::value::number(f64::from(value));
    let px = |value: f32| format!("{}px", number(value));
    let vertical = matches!(
        name,
        "top" | "bottom" | "height" | "min-height" | "max-height"
    );
    let length = |value: &str| {
        let base = match value.contains('%') {
            // A box that is not rendered has no used values, so its percentages stay.
            true if size.is_none() => return value.to_owned(),
            true => container()[usize::from(vertical)],
            false => 0.,
        };
        units
            .resolve(value, base)
            .map_or_else(|| value.to_owned(), px)
    };
    let current = style.get(Prop::COLOR).unwrap_or("canvastext");
    let paint = |value: &str| {
        let value = if value.eq_ignore_ascii_case("currentcolor") {
            current
        } else {
            value
        };
        match color::serialize(value) {
            Some(text) => text,
            None => value.to_owned(),
        }
    };
    let none = |prop: Prop| {
        style
            .get(prop)
            .is_none_or(|value| matches!(value, "none" | "hidden"))
    };
    match name {
        "border-top-width" if none(Prop::BORDER_TOP_STYLE) => "0px".into(),
        "border-right-width" if none(Prop::BORDER_RIGHT_STYLE) => "0px".into(),
        "border-bottom-width" if none(Prop::BORDER_BOTTOM_STYLE) => "0px".into(),
        "border-left-width" if none(Prop::BORDER_LEFT_STYLE) => "0px".into(),
        "column-rule-width" if none(Prop::COLUMN_RULE_STYLE) => "0px".into(),
        "width" | "height"
            if size.is_some()
                && !style
                    .get(Prop::DISPLAY)
                    .is_some_and(|display| display == "inline") =>
        {
            let [width, height] = size.unwrap_or_default();
            let border_box = style.get(Prop::BOX_SIZING) == Some("border-box");
            let [padding, border] = [
                PADDING,
                [
                    Prop::BORDER_TOP_WIDTH,
                    Prop::BORDER_RIGHT_WIDTH,
                    Prop::BORDER_BOTTOM_WIDTH,
                    Prop::BORDER_LEFT_WIDTH,
                ],
            ]
            .map(|props| units.edges(style, props, container()[0]));
            let border = std::array::from_fn::<f32, 4, _>(|side| {
                if none(BORDERS[side][1]) {
                    0.
                } else {
                    border[side]
                }
            });
            px(if border_box {
                if name == "width" { width } else { height }
            } else if name == "width" {
                width - padding[1] - padding[3] - border[1] - border[3]
            } else {
                height - padding[0] - padding[2] - border[0] - border[2]
            }
            .max(0.))
        }
        "line-height" if value != "normal" => match units.parse(&value, units.font.em) {
            Some((number, false)) => px(number * units.font.em),
            Some((length, true)) => px(length),
            None => value,
        },
        "grid-template-areas" if value != "none" => value
            .split(['"', '\''])
            .map(str::trim)
            .filter(|row| !row.is_empty())
            .map(|row| format!("\"{}\"", area_cells(row).join(" ")))
            .collect::<Vec<_>>()
            .join(" "),
        "font-family" => components(&value, ',')
            .into_iter()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(", "),
        _ if name.ends_with("duration") || name.ends_with("delay") => components(&value, ',')
            .into_iter()
            .map(|time| {
                let time = time.trim();
                match time
                    .strip_suffix("ms")
                    .map(|ms| (ms, 1000.))
                    .or_else(|| time.strip_suffix('s').map(|s| (s, 1.)))
                {
                    Some((number, scale)) => number
                        .parse::<f32>()
                        .map_or_else(|_| time.to_owned(), |number| format!("{}s", number / scale)),
                    None => time.to_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        "opacity" | "fill-opacity" | "stroke-opacity" | "flood-opacity" => {
            match value.strip_suffix('%') {
                Some(percent) => percent
                    .parse::<f32>()
                    .map_or(value.clone(), |percent| number(percent / 100.)),
                None => value,
            }
        }
        "transform" if value != "none" => {
            let [width, height] = size.unwrap_or_default();
            let mut flat = style.clone();
            flat.insert(Prop::TRANSFORM_ORIGIN, "0 0");
            let bounds = renderer::Rect::new(
                renderer::point(renderer::px(0.), renderer::px(0.)),
                renderer::size(renderer::px(width), renderer::px(height)),
            );
            match crate::render::effects::transform(&flat, units, bounds, None) {
                Some(m)
                    if [m[2], m[3], m[6], m[7], m[8], m[9], m[11], m[14]] == [0.; 8]
                        && m[10] == 1.
                        && m[15] == 1. =>
                {
                    format!(
                        "matrix({}, {}, {}, {}, {}, {})",
                        number(m[0]),
                        number(m[1]),
                        number(m[4]),
                        number(m[5]),
                        number(m[12]),
                        number(m[13])
                    )
                }
                Some(m) => format!("matrix3d({})", m.map(number).join(", ")),
                None if value.contains('(') => "matrix(1, 0, 0, 1, 0, 0)".into(),
                None => value,
            }
        }
        "box-shadow" | "text-shadow" if value != "none" => components(&value, ',')
            .into_iter()
            .map(|shadow| {
                let (mut ink, mut lengths, mut inset) = (current.to_owned(), Vec::new(), false);
                for part in components(shadow, ' ') {
                    if part == "inset" {
                        inset = true;
                    } else if let Some(size) = units.resolve(part, f32::NAN) {
                        lengths.push(px(size));
                    } else {
                        ink = part.to_owned();
                    }
                }
                lengths.resize(if name == "box-shadow" { 4 } else { 3 }, "0px".into());
                let mut text = format!("{} {}", paint(&ink), lengths.join(" "));
                if inset {
                    text.push_str(" inset");
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", "),
        "flex-basis" if value.ends_with('%') => value,
        "overflow-x" | "overflow-y" if matches!(value.as_str(), "visible" | "clip") => {
            let other = if name == "overflow-x" {
                Prop::OVERFLOW_Y
            } else {
                Prop::OVERFLOW_X
            };
            match style.get(other) {
                Some(other) if !matches!(other, "visible" | "clip") => {
                    if value == "visible" { "auto" } else { "hidden" }.to_owned()
                }
                _ => value,
            }
        }
        "background-position" | "object-position" | "mask-position" => components(&value, ',')
            .into_iter()
            .map(|layer| {
                let mut parts: Vec<&str> = components(layer, ' ');
                if parts.len() == 1 {
                    parts.insert(usize::from(!matches!(parts[0], "top" | "bottom")), "center");
                }
                if parts.len() == 2
                    && (matches!(parts[0], "top" | "bottom")
                        || matches!(parts[1], "left" | "right"))
                {
                    parts.swap(0, 1);
                }
                parts
                    .into_iter()
                    .map(|part| match part {
                        "left" | "top" => "0%".to_owned(),
                        "center" => "50%".to_owned(),
                        "right" | "bottom" => "100%".to_owned(),
                        part if part.ends_with('%') => part.to_owned(),
                        part => units
                            .resolve(part, f32::NAN)
                            .map_or_else(|| part.to_owned(), px),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(", "),
        "clip-path" | "shape-outside" if value.starts_with("inset(") && value.ends_with(')') => {
            let body = &value[6..value.len() - 1];
            let (sides, round) = body
                .split_once(" round ")
                .map_or((body, None), |(sides, round)| (sides, Some(round.trim())));
            let sides = match components(sides, ' ')[..] {
                [a] => [a; 4],
                [a, b] => [a, b, a, b],
                [a, b, c] => [a, b, c, b],
                [a, b, c, d] => [a, b, c, d],
                _ => return value,
            };
            let mut result = format!("inset({}", shortest(sides));
            if let Some(round) = round {
                result.push_str(" round ");
                result.push_str(round);
            }
            result.push(')');
            result
        }
        "transform-origin" | "perspective-origin" => {
            let [width, height] = size.unwrap_or_default();
            let mut parts = components(&value, ' ');
            if parts.len() == 1 {
                parts.insert(usize::from(!matches!(parts[0], "top" | "bottom")), "center");
            }
            if parts.len() > 1
                && (matches!(parts[0], "top" | "bottom") || matches!(parts[1], "left" | "right"))
            {
                parts.swap(0, 1);
            }
            parts
                .into_iter()
                .enumerate()
                .map(|(index, part)| {
                    let part = match part {
                        "left" | "top" => "0%",
                        "center" => "50%",
                        "right" | "bottom" => "100%",
                        part => part,
                    };
                    if size.is_none() && part.contains('%') {
                        return part.to_owned();
                    }
                    units
                        .resolve(part, if index == 0 { width } else { height })
                        .map_or_else(|| part.to_owned(), |value| px((value * 64.).round() / 64.))
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        "border-image-slice"
        | "border-image-width"
        | "border-image-outset"
        | "border-image-repeat" => specified(name, &value)
            .split(' ')
            .map(|part| {
                if part.starts_with(|c: char| c.is_ascii_digit() || c == '.')
                    && part.ends_with(|c: char| c.is_ascii_alphabetic())
                {
                    length(part)
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        "caret-color" if value == "auto" => paint(current),
        "font-stretch" => match value.as_str() {
            "ultra-condensed" => "50%",
            "extra-condensed" => "62.5%",
            "condensed" => "75%",
            "semi-condensed" => "87.5%",
            "normal" => "100%",
            "semi-expanded" => "112.5%",
            "expanded" => "125%",
            "extra-expanded" => "150%",
            "ultra-expanded" => "200%",
            _ => &value,
        }
        .to_owned(),
        _ if name.ends_with("color")
            || name.starts_with("-terminal-")
            || matches!(name, "fill" | "stroke") =>
        {
            if value == "auto" {
                value
            } else {
                paint(&value)
            }
        }
        _ if name.starts_with("margin-")
            || name.starts_with("padding-")
            || name.ends_with("-width") && name.starts_with("border-")
            || name.ends_with("-radius")
            || matches!(
                name,
                "top"
                    | "right"
                    | "bottom"
                    | "left"
                    | "width"
                    | "height"
                    | "min-width"
                    | "min-height"
                    | "max-width"
                    | "max-height"
                    | "font-size"
                    | "letter-spacing"
                    | "word-spacing"
                    | "text-indent"
                    | "outline-offset"
                    | "outline-width"
                    | "row-gap"
                    | "column-gap"
                    | "border-spacing"
                    | "perspective"
                    | "flex-basis"
                    | "text-underline-offset"
                    | "text-decoration-thickness"
                    | "column-width"
                    | "column-rule-width"
            ) =>
        {
            let used = name.starts_with("margin-")
                || name.starts_with("padding-")
                || matches!(
                    name,
                    "top" | "right" | "bottom" | "left" | "width" | "height"
                );
            let mut parts = components(&value, ' ')
                .into_iter()
                .map(|part| {
                    if !used && part.ends_with('%') {
                        part.to_owned()
                    } else if used && size.is_some() {
                        let text = length(part);
                        // Layout boxes have sizes in 1/64 px, like Chrome's, so their used values round to it.
                        match text
                            .strip_suffix("px")
                            .and_then(|number| number.parse::<f32>().ok())
                        {
                            Some(number) => px((number * 64.).round() / 64.),
                            None => text,
                        }
                    } else {
                        length(part)
                    }
                })
                .collect::<Vec<_>>();
            if name == "border-spacing" && parts.len() == 2 && parts[0] == parts[1] {
                parts.pop();
            }
            parts.join(" ")
        }
        _ => value,
    }
}

// The physical property that a logical property name maps to, as the cascade maps it.
pub(crate) fn physical_name<'a>(style: &Style, name: &'a str) -> Cow<'a, str> {
    if let Some((_, ltr, rtl)) = LOGICAL.iter().find(|(logical, ..)| logical.name() == name) {
        return Cow::Borrowed(
            if style.get(Prop::DIRECTION) == Some("rtl") {
                rtl
            } else {
                ltr
            }
            .name(),
        );
    }
    let mut out = Vec::new();
    if (name.contains("block") || name.contains("inline"))
        && expand(name, "0", &mut out)
        && let [(physical, _)] = &out[..]
        && physical != name
    {
        return Cow::Owned(physical_name(style, physical).into_owned());
    }
    Cow::Borrowed(name)
}

pub(crate) fn shortest(sides: [&str; 4]) -> String {
    let count = if sides[3] != sides[1] {
        4
    } else if sides[2] != sides[0] {
        3
    } else if sides[1] != sides[0] {
        2
    } else {
        1
    };
    sides[..count].join(" ")
}

// Chrome's text for a declared value: lowercase keywords, units and functions, short numbers, `0px` for
// lengths, colors as `rgb()`, strings in double quotes and `, ` between items. Values with `var()` stay as
// written, like in Chrome.
pub(crate) fn specified(name: &str, value: &str) -> String {
    let value = value.trim();
    if name.starts_with("--")
        || ["var(", "env(", "attr("]
            .iter()
            .any(|function| value.contains(function))
    {
        return value.to_owned();
    }
    let grammar = grammar(name).unwrap_or("*");
    let length =
        grammar.contains("length") && !grammar.contains("number>") && !grammar.contains("integer>")
            || name.ends_with("-shadow")
            || name.ends_with("-origin")
            || name.ends_with("-position");
    // Names that authors choose keep their case.
    let named = matches!(
        name,
        "font" | "font-family" | "animation" | "animation-name" | "list-style" | "list-style-type"
    ) || name.starts_with("grid")
        || name.starts_with("counter-")
        || name.starts_with("container")
        || name.ends_with("-name");
    let mut out = String::with_capacity(value.len());
    canonical(value, length, named, &mut out);
    let numeric =
        |part: &str| part.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'));
    let layers = |out: &str, layer: &dyn Fn(Vec<&str>) -> String| {
        components(out, ',')
            .into_iter()
            .map(|item| layer(components(item.trim(), ' ')))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match name {
        "font-family" => layers(&out, &|parts| match parts[..] {
            // Chrome quotes names of several words and unquotes names that are one identifier.
            [quoted]
                if quoted.len() > 2
                    && quoted.starts_with('"')
                    && quoted[1..quoted.len() - 1].bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
                    })
                    && !quoted[1..].starts_with(|c: char| c.is_ascii_digit() || c == '-')
                    && !matches!(
                        &*quoted[1..quoted.len() - 1].to_ascii_lowercase(),
                        "serif"
                            | "sans-serif"
                            | "monospace"
                            | "cursive"
                            | "fantasy"
                            | "system-ui"
                            | "math"
                            | "emoji"
                            | "fangsong"
                            | "ui-serif"
                            | "ui-sans-serif"
                            | "ui-monospace"
                            | "ui-rounded"
                            | "inherit"
                            | "initial"
                            | "unset"
                            | "default"
                            | "revert"
                    ) =>
            {
                quoted[1..quoted.len() - 1].to_owned()
            }
            [first, ..] if parts.len() > 1 && !first.starts_with('"') => {
                format!("\"{}\"", parts.join(" "))
            }
            _ => parts.join(" "),
        }),
        "aspect-ratio" => match components(&out, ' ')[..] {
            [ratio] if numeric(ratio) => format!("{ratio} / 1"),
            _ => out,
        },
        "transition" => layers(&out, &|mut parts| {
            let times: Vec<String> = parts
                .iter()
                .filter(|part| numeric(part))
                .map(|part| part.to_string())
                .collect();
            if times.iter().all(|time| time == "0s") {
                parts.retain(|part| !numeric(part));
            } else if times.get(1).is_some_and(|delay| delay == "0s") {
                let delay = parts.iter().rposition(|part| *part == "0s");
                parts.remove(delay.unwrap_or_default());
            }
            parts.retain(|part| !matches!(*part, "ease" | "normal"));
            if parts.len() > 1 {
                parts.retain(|part| *part != "all");
            }
            if parts.is_empty() {
                "all".into()
            } else {
                parts.join(" ")
            }
        }),
        // Chrome lists every part of each animation, in this order.
        "animation" if out != "none" => layers(&out, &|parts| {
            let mut slots: [Option<&str>; 8] = [None; 8];
            let mut times = 0;
            for part in parts {
                let slot = match part {
                    _ if numeric(part) && part.ends_with('s') => {
                        times += 1;
                        if times == 1 { 0 } else { 2 }
                    }
                    "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out" | "step-start"
                    | "step-end" => 1,
                    _ if part.starts_with("cubic-bezier(")
                        || part.starts_with("steps(")
                        || part.starts_with("linear(") =>
                    {
                        1
                    }
                    "infinite" => 3,
                    _ if numeric(part) => 3,
                    "normal" | "reverse" | "alternate" | "alternate-reverse"
                        if slots[4].is_none() =>
                    {
                        4
                    }
                    "forwards" | "backwards" | "both" | "none" if slots[5].is_none() => 5,
                    "running" | "paused" if slots[6].is_none() => 6,
                    _ => 7,
                };
                slots[slot] = Some(part);
            }
            slots
                .iter()
                .zip(["0s", "ease", "0s", "1", "normal", "none", "running", "none"])
                .map(|(value, initial)| value.unwrap_or(initial))
                .collect::<Vec<_>>()
                .join(" ")
        }),
        "columns" => match components(&out, ' ')[..] {
            [count, width] if count.parse::<u32>().is_ok() => format!("{width} {count}"),
            _ => out,
        },
        "translate" | "scale" => {
            let mut parts = components(&out, ' ');
            let zero = if name == "scale" { "1" } else { "0px" };
            if parts.len() == 3 && parts[2] == zero {
                parts.pop();
            }
            if parts.len() == 2
                && (if name == "scale" {
                    parts[1] == parts[0]
                } else {
                    parts[1] == zero
                })
            {
                parts.pop();
            }
            parts.join(" ")
        }
        "background-position"
        | "object-position"
        | "mask-position"
        | "transform-origin"
        | "perspective-origin" => layers(&out, &|parts| match parts[..] {
            [part @ ("top" | "bottom")] => format!("center {part}"),
            [part] => format!("{part} center"),
            _ => parts.join(" "),
        }),
        "counter-reset" | "counter-set" | "counter-increment" if out != "none" => {
            let parts = components(&out, ' ');
            let mut text = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                text.push(part.to_string());
                if !numeric(part) && parts.get(index + 1).is_none_or(|next| !numeric(next)) {
                    text.push(
                        if name == "counter-increment" {
                            "1"
                        } else {
                            "0"
                        }
                        .into(),
                    );
                }
            }
            text.join(" ")
        }
        "border-image-slice"
        | "border-image-width"
        | "border-image-outset"
        | "border-image-repeat" => {
            let mut parts = components(&out, ' ');
            let fill = parts
                .iter()
                .position(|part| *part == "fill")
                .map(|at| parts.remove(at));
            let sides = match parts.len() {
                0 => return out,
                1 => [0; 4],
                2 => [0, 1, 0, 1],
                3 => [0, 1, 2, 1],
                _ => [0, 1, 2, 3],
            };
            let mut text = shortest(sides.map(|side| parts[side]));
            if let Some(fill) = fill {
                text.push(' ');
                text.push_str(fill);
            }
            text
        }
        "opacity" | "fill-opacity" | "stroke-opacity" | "flood-opacity" | "stop-opacity" => {
            match out
                .strip_suffix('%')
                .and_then(|percent| percent.parse::<f64>().ok())
            {
                Some(percent) => number(percent / 100.),
                None => out,
            }
        }
        "box-shadow" | "text-shadow" if out != "none" => layers(&out, &|mut parts| {
            if let Some(color) = parts
                .iter()
                .position(|part| !numeric(part) && *part != "inset")
            {
                let color = parts.remove(color);
                parts.insert(0, color);
            }
            parts.join(" ")
        }),
        _ => out,
    }
}

fn canonical(value: &str, length: bool, named: bool, out: &mut String) {
    for (index, item) in components(value, ',').into_iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        for (index, part) in components(item.trim(), ' ').into_iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let lower = part.to_ascii_lowercase();
            let first = part.as_bytes()[0];
            if first == b'"' || first == b'\'' {
                let text = cssparser::Parser::new(part)
                    .expect_string()
                    .map_or_else(|_| part.to_owned(), |text| text.to_string());
                out.push('"');
                out.push_str(&text.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            } else if let Some((function, args)) =
                part.strip_suffix(')').and_then(|part| part.split_once('('))
            {
                let function = function.to_ascii_lowercase();
                match function.as_str() {
                    "url" => {
                        let url = args.trim();
                        let url = cssparser::Parser::new(url)
                            .expect_string()
                            .map_or_else(|_| url.to_owned(), |text| text.to_string());
                        out.push_str("url(\"");
                        out.push_str(&url.replace('\\', "\\\\").replace('"', "\\\""));
                        out.push_str("\")");
                    }
                    "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" => {
                        match color::serialize(part) {
                            Some(color) => out.push_str(&color),
                            None => out.push_str(&lower),
                        }
                    }
                    "drop-shadow" => {
                        let mut parts = String::new();
                        canonical(args, true, named, &mut parts);
                        let mut parts = components(&parts, ' ');
                        if let Some(color) = parts.iter().position(|part| {
                            !part.starts_with(|c: char| {
                                c.is_ascii_digit() || matches!(c, '-' | '+' | '.')
                            })
                        }) {
                            let color = parts.remove(color);
                            parts.insert(0, color);
                        }
                        out.push_str("drop-shadow(");
                        out.push_str(&parts.join(" "));
                        out.push(')');
                    }
                    _ => {
                        // Math of absolute lengths folds into one length, like `calc(1px + 2px)` to `calc(3px)`.
                        let folded = (function == "calc"
                            && !lower
                                .replace("calc", "")
                                .replace("px", "")
                                .bytes()
                                .any(|byte| byte.is_ascii_alphabetic() || byte == b'%'))
                        .then(|| Lengths::new(16., 100., 100.).math(&lower, f32::NAN, &[]))
                        .flatten()
                        .filter(|(value, _)| value.is_finite());
                        // Transform functions keep their camel case.
                        let camel = [
                            "translateX",
                            "translateY",
                            "translateZ",
                            "translate3d",
                            "scaleX",
                            "scaleY",
                            "scaleZ",
                            "scale3d",
                            "rotateX",
                            "rotateY",
                            "rotateZ",
                            "rotate3d",
                            "skewX",
                            "skewY",
                            "matrix3d",
                        ];
                        out.push_str(
                            camel
                                .iter()
                                .find(|name| name.eq_ignore_ascii_case(&function))
                                .unwrap_or(&&*function),
                        );
                        out.push('(');
                        match folded {
                            Some((value, kind)) => {
                                write_number(value, out);
                                if kind == parser::Kind::Length {
                                    out.push_str("px");
                                }
                            }
                            None => {
                                let start = out.len();
                                canonical(args, function.starts_with("translate"), named, out);
                                // The default direction of a linear gradient is left out.
                                if function.ends_with("linear-gradient") {
                                    for default in ["to bottom, ", "180deg, "] {
                                        if out[start..].starts_with(default) {
                                            out.replace_range(start..start + default.len(), "");
                                        }
                                    }
                                }
                            }
                        }
                        out.push(')');
                    }
                }
            } else if first == b'#' && !named {
                out.push_str(&color::serialize(part).unwrap_or(lower));
            } else if part.len() > 1 && part.contains('/') && !part.contains(['(', '"', '\'']) {
                for (index, side) in part.split('/').enumerate() {
                    if index > 0 {
                        out.push_str(" / ");
                    }
                    canonical(side.trim(), length, named, out);
                }
            } else if first.is_ascii_digit()
                || matches!(first, b'+' | b'-' | b'.')
                    && part
                        .as_bytes()
                        .get(1)
                        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
            {
                let end = part
                    .char_indices()
                    .find(|&(index, c)| {
                        !(c.is_ascii_digit()
                            || c == '.'
                            || (c == '+' || c == '-')
                                && (index == 0
                                    || matches!(part.as_bytes()[index - 1], b'e' | b'E'))
                            || matches!(c, 'e' | 'E')
                                && part[index + 1..].starts_with(|c: char| {
                                    c.is_ascii_digit() || c == '+' || c == '-'
                                }))
                    })
                    .map_or(part.len(), |(index, _)| index);
                match part[..end].parse::<f64>() {
                    Ok(number) => {
                        write_number(number, out);
                        let unit = &lower[end..];
                        out.push_str(if unit.is_empty() && number == 0. && length {
                            "px"
                        } else {
                            unit
                        });
                    }
                    Err(_) => out.push_str(part),
                }
            } else if named {
                out.push_str(part);
            } else {
                out.push_str(&lower);
            }
        }
    }
}

pub(crate) fn declare(source: &str) -> Vec<[String; 3]> {
    let mut result = Vec::new();
    for declaration in Declarations::parse(source).0 {
        let Declaration::Property(name, value) = declaration else {
            continue;
        };
        let (value, important) = match value.strip_suffix("!important") {
            Some(value) => (value.trim_end(), "important"),
            None => (value.as_str(), ""),
        };
        let known = name.starts_with("--") || Prop::find(&name).is_some_and(Prop::is_known);
        if !known || supports(&name, value) == Some(false) {
            continue;
        }
        let mut expanded = Vec::new();
        // Scripts read these shorthands back by name, and `style` has no serializer for them.
        if value.contains("var(")
            || matches!(
                name.as_str(),
                "columns"
                    | "column-rule"
                    | "grid-row"
                    | "grid-column"
                    | "grid-area"
                    | "grid-template"
                    | "grid"
                    | "mask"
                    | "-webkit-mask"
            )
            || !expand(&name, value, &mut expanded)
        {
            expanded = vec![(name.clone(), value.to_owned())];
        } else if name == "border"
            && !matches!(
                value,
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            )
        {
            // Like Chrome, scripts read the initial value that `border` resets `border-image-source` to.
            if let Some((_, value)) = expanded.get_mut(12) {
                *value = BORDER_IMAGE_INITIAL[0].into();
            }
        }
        result.extend(
            expanded
                .into_iter()
                .map(|(name, value)| [name, value, important.to_owned()]),
        );
    }
    result
}

impl ComputedStyles {
    pub(crate) fn blended(&self) -> bool {
        self.blended.get().unwrap_or_else(|| {
            let blended = self.values.iter().any(|style| {
                style
                    .get(Prop::MIX_BLEND_MODE)
                    .is_some_and(|value| value != "normal")
            });
            self.blended.set(Some(blended));
            blended
        })
    }
}

pub(crate) fn computed_names() -> impl Iterator<Item = &'static str> {
    INITIAL.iter().map(|(name, _)| *name)
}

pub(crate) fn counter_text(kind: &str, number: i64) -> String {
    match kind {
        "none" => String::new(),
        "disc" => "•".into(),
        "circle" => "◦".into(),
        "square" => "▪".into(),
        "decimal-leading-zero" => format!("{number:02}"),
        "lower-alpha" | "lower-latin" | "upper-alpha" | "upper-latin" if number > 0 => {
            let mut letters = Vec::new();
            let mut rest = number;
            while rest > 0 {
                rest -= 1;
                letters.push((b'a' + (rest % 26) as u8) as char);
                rest /= 26;
            }
            let letters: String = letters.into_iter().rev().collect();
            if kind.starts_with("upper") {
                letters.to_ascii_uppercase()
            } else {
                letters
            }
        }
        "lower-roman" | "upper-roman" if (1..4000).contains(&number) => {
            let mut rest = number;
            let mut roman = String::new();
            for (value, digits) in [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ] {
                while rest >= value {
                    roman.push_str(digits);
                    rest -= value;
                }
            }
            if kind == "upper-roman" {
                roman.to_ascii_uppercase()
            } else {
                roman
            }
        }
        "lower-greek" | "hiragana" | "katakana" if number > 0 => {
            let digits: Vec<char> = match kind {
                "lower-greek" => "αβγδεζηθικλμνξοπρστυφχψω",
                "hiragana" => "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわゐゑをん",
                _ => "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヰヱヲン",
            }
            .chars()
            .collect();
            let mut letters = Vec::new();
            let mut rest = number;
            while rest > 0 {
                rest -= 1;
                letters.push(digits[(rest % digits.len() as i64) as usize]);
                rest /= digits.len() as i64;
            }
            letters.into_iter().rev().collect()
        }
        "cjk-decimal" if number >= 0 => number
            .to_string()
            .chars()
            .map(|digit| {
                "〇一二三四五六七八九"
                    .chars()
                    .nth(digit as usize - '0' as usize)
                    .unwrap_or(digit)
            })
            .collect(),
        "cjk-ideographic" | "trad-chinese-informal" | "simp-chinese-informal"
            if (1..10000).contains(&number) =>
        {
            let digits: Vec<char> = "零一二三四五六七八九".chars().collect();
            let mut text = String::new();
            let mut zero = false;
            for (unit, name) in [(1000, "千"), (100, "百"), (10, "十"), (1, "")] {
                let digit = (number / unit % 10) as usize;
                if digit == 0 {
                    zero = !text.is_empty();
                    continue;
                }
                if std::mem::take(&mut zero) {
                    text.push('零');
                }
                if !(unit == 10 && digit == 1 && number < 20) {
                    text.push(digits[digit]);
                }
                text.push_str(name);
            }
            text
        }
        "armenian" | "upper-armenian" | "lower-armenian" | "georgian" | "hebrew"
            if (1..10000).contains(&number) =>
        {
            let symbols: &[(i64, &str)] = match kind {
                "georgian" => &[
                    (9000, "ჰ"),
                    (8000, "ჯ"),
                    (7000, "ჴ"),
                    (6000, "ხ"),
                    (5000, "ჭ"),
                    (4000, "წ"),
                    (3000, "ძ"),
                    (2000, "ც"),
                    (1000, "ჩ"),
                    (900, "შ"),
                    (800, "ყ"),
                    (700, "ღ"),
                    (600, "ქ"),
                    (500, "ფ"),
                    (400, "ჳ"),
                    (300, "ტ"),
                    (200, "ს"),
                    (100, "რ"),
                    (90, "ჟ"),
                    (80, "პ"),
                    (70, "ო"),
                    (60, "ჲ"),
                    (50, "ნ"),
                    (40, "მ"),
                    (30, "ლ"),
                    (20, "კ"),
                    (10, "ი"),
                    (9, "თ"),
                    (8, "ჱ"),
                    (7, "ზ"),
                    (6, "ვ"),
                    (5, "ე"),
                    (4, "დ"),
                    (3, "გ"),
                    (2, "ბ"),
                    (1, "ა"),
                ],
                "hebrew" => &[
                    (400, "ת"),
                    (300, "ש"),
                    (200, "ר"),
                    (100, "ק"),
                    (90, "צ"),
                    (80, "פ"),
                    (70, "ע"),
                    (60, "ס"),
                    (50, "נ"),
                    (40, "מ"),
                    (30, "ל"),
                    (20, "כ"),
                    (19, "יט"),
                    (18, "יח"),
                    (17, "יז"),
                    (16, "טז"),
                    (15, "טו"),
                    (10, "י"),
                    (9, "ט"),
                    (8, "ח"),
                    (7, "ז"),
                    (6, "ו"),
                    (5, "ה"),
                    (4, "ד"),
                    (3, "ג"),
                    (2, "ב"),
                    (1, "א"),
                ],
                _ => &[
                    (9000, "Ք"),
                    (8000, "Փ"),
                    (7000, "Ւ"),
                    (6000, "Ց"),
                    (5000, "Ր"),
                    (4000, "Տ"),
                    (3000, "Վ"),
                    (2000, "Ս"),
                    (1000, "Ռ"),
                    (900, "Ջ"),
                    (800, "Պ"),
                    (700, "Չ"),
                    (600, "Ո"),
                    (500, "Շ"),
                    (400, "Ն"),
                    (300, "Յ"),
                    (200, "Մ"),
                    (100, "Ճ"),
                    (90, "Ղ"),
                    (80, "Ձ"),
                    (70, "Հ"),
                    (60, "Կ"),
                    (50, "Ծ"),
                    (40, "Խ"),
                    (30, "Լ"),
                    (20, "Ի"),
                    (10, "Ժ"),
                    (9, "Թ"),
                    (8, "Ը"),
                    (7, "Է"),
                    (6, "Զ"),
                    (5, "Ե"),
                    (4, "Դ"),
                    (3, "Գ"),
                    (2, "Բ"),
                    (1, "Ա"),
                ],
            };
            let mut rest = number;
            let mut text = String::new();
            for &(value, symbol) in symbols {
                while rest >= value {
                    text.push_str(symbol);
                    rest -= value;
                }
            }
            if kind == "lower-armenian" {
                text.to_lowercase()
            } else {
                text
            }
        }
        _ => number.to_string(),
    }
}

pub(crate) fn supports_condition(condition: &str) -> bool {
    let evaluate = |source: String| match Stylesheet::parse(&source).0.as_slice() {
        [rule] => match **rule {
            Rule::Supports(_, supported, _) => Some(supported),
            _ => None,
        },
        _ => None,
    };
    evaluate(format!("@supports {condition}{{}}"))
        .or_else(|| evaluate(format!("@supports ({condition}){{}}")))
        .unwrap_or(false)
}

pub(crate) fn reduced_motion() -> bool {
    crate::desktop::native::theme_state() & 32 != 0
}

impl MediaQuery {
    fn list(queries: &[Self], [width, height]: [f32; 2], reduced_motion: bool, theme: u32) -> bool {
        queries.is_empty()
            || queries.iter().any(|query| {
                let lengths = Lengths::new(16., width, height);
                query.matches(width, height, reduced_motion, theme, lengths) == Some(true)
            })
    }

    fn matches(
        &self,
        width: f32,
        height: f32,
        reduced_motion: bool,
        theme: u32,
        lengths: Lengths,
    ) -> Option<bool> {
        let (name, op, value) = match self {
            Self::All => return Some(true),
            Self::Unknown(_) => return None,
            Self::Not(query) => {
                return query
                    .matches(width, height, reduced_motion, theme, lengths)
                    .map(|v| !v);
            }
            Self::And(queries) | Self::Or(queries) => {
                let and = matches!(self, Self::And(_));
                let mut result = Some(and);
                for query in queries {
                    match query.matches(width, height, reduced_motion, theme, lengths) {
                        Some(value) if value != and => return Some(value),
                        None => result = None,
                        _ => {}
                    }
                }
                return result;
            }
            Self::Typed(medium, query) => {
                let not = medium.starts_with("not ");
                if !matches!(medium.rsplit(' ').next(), Some("all" | "screen")) {
                    return Some(not);
                }
                return query
                    .matches(width, height, reduced_motion, theme, lengths)
                    .map(|v| v != not);
            }
            Self::Feature(name, op, value) => (
                name.as_str(),
                match op.as_str() {
                    "min" => ">=",
                    "max" => "<=",
                    op => op,
                },
                value.as_str(),
            ),
        };
        let keyword = |values: &[&str], current: &str| match op {
            "" => Some(!matches!(current, "none" | "no-preference")),
            ":" if values.contains(&value) => Some(value == current),
            _ => None,
        };
        let actual = match name {
            "-macos-glass" => return op.is_empty().then(renderer::supports_glass),
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
            "color-gamut" => {
                #[cfg(target_os = "macos")]
                let p3 = {
                    unsafe extern "C" {
                        fn apps_display_p3() -> bool;
                    }
                    theme & 64 != 0 || theme >> 16 != 0 && unsafe { apps_display_p3() }
                };
                #[cfg(not(target_os = "macos"))]
                let p3 = theme & 64 != 0;
                return match (op, value) {
                    (":", "srgb") => Some(true),
                    (":", "p3") => Some(p3),
                    (":", "rec2020") => Some(false),
                    ("", _) => Some(true),
                    _ => None,
                };
            }
            "width" => f64::from(width),
            "height" => f64::from(height),
            "aspect-ratio" => f64::from(width) / f64::from(height),
            "resolution" | "-webkit-device-pixel-ratio" | "device-pixel-ratio" => {
                f64::from(theme >> 16).max(100.) / 100.
            }
            "color" => 8.,
            "color-index" | "monochrome" => 0.,
            _ => return None,
        };
        let boundary = match name {
            _ if op.is_empty() => return Some(actual != 0.),
            "width" | "height" => f64::from(lengths.resolve(value, f32::NAN)?),
            "aspect-ratio" => {
                let (a, b) = value.split_once('/').unwrap_or((value, "1"));
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                (a >= 0. && b > 0.).then_some(a / b)?
            }
            "-webkit-device-pixel-ratio" | "device-pixel-ratio" => value.parse().ok()?,
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
            Self::Feature(name, op, value) => match op.as_str() {
                "" => format!("({name})"),
                ":" => format!("({name}: {value})"),
                "min" | "max" => format!("({op}-{name}: {value})"),
                _ => format!("({name} {op} {value})"),
            },
            Self::Typed(medium, query) => match **query {
                Self::All => medium.clone(),
                _ => format!("{medium} and {}", group(query)),
            },
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
    rules: Vec<Collected<'a>>,
    container: Option<ContainerQuery>,
    media: Option<MediaStack>,
    blocks: Vec<Rc<MediaBlock>>,
    layers: Vec<(String, Vec<u32>)>,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    properties: Registry,
    // The layer of each `@property` (true) and `@keyframes` (false) rule that wins its name.
    layered: HashMap<(bool, String), Option<usize>>,
}

impl Collect<'_> {
    fn media(&self, queries: &[MediaQuery]) -> bool {
        MediaQuery::list(
            queries,
            [self.width, self.height],
            self.reduced_motion,
            self.native_theme,
        )
    }

    // A new layer moves the ranks of others, so this runs once, after every sheet is collected.
    fn rank(&mut self) {
        let mut sorted: Vec<usize> = (0..self.layers.len()).collect();
        sorted.sort_by(|&a, &b| {
            let path = |index: usize| self.layers[index].1.iter().copied().chain([u32::MAX]);
            path(a).cmp(path(b))
        });
        let mut ranks = vec![0; self.layers.len()];
        for (rank, index) in (0..).zip(sorted) {
            ranks[index] = rank;
        }
        for rule in &mut self.rules {
            rule.2.rank = ranks
                .get(rule.2.rank as usize)
                .copied()
                .unwrap_or(UNLAYERED);
        }
    }
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
            | Self::Starting(children)
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
        layer: Option<usize>,
        collected: &mut Collect<'a>,
    ) {
        match self {
            Self::Declarations(values) => {
                if let Some(parent) = parent {
                    let rank = layer.map_or(UNLAYERED, |index| index as u32);
                    collected.rules.push((
                        parent,
                        values,
                        Layer {
                            origin: AUTHOR,
                            rank,
                        },
                        collected.container.clone(),
                        collected.media.clone(),
                    ));
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
            Self::Property(name, ..) | Self::Keyframes(name, _) => {
                let path = |index: Option<usize>| {
                    index
                        .map_or(&[][..], |index| &collected.layers[index].1[..])
                        .iter()
                        .copied()
                        .chain([u32::MAX])
                };
                let key = (matches!(self, Self::Property(..)), name.clone());
                if collected
                    .layered
                    .get(&key)
                    .is_some_and(|&old| path(old).gt(path(layer)))
                {
                    return;
                }
                collected.layered.insert(key, layer);
                match self {
                    Self::Property(name, syntax, inherits, initial) => {
                        collected.properties.insert(
                            Prop::new(name),
                            (
                                syntax.as_str().into(),
                                *inherits,
                                initial.as_deref().map(Rc::from),
                            ),
                        );
                    }
                    Self::Keyframes(name, frames) => {
                        collected.keyframes.insert(name.clone(), frames.clone());
                    }
                    _ => {}
                }
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
                    let name = match layer {
                        Some(outer) => format!("{}.{name}", collected.layers[outer].0),
                        None => name,
                    };
                    let (mut route, mut index) = (Vec::new(), 0);
                    for (end, _) in name.match_indices('.').chain([(name.len(), "")]) {
                        let path = &name[..end];
                        let owner = path.rsplit_once('.').map_or("", |(owner, _)| owner);
                        let found = collected.layers.iter().position(|(name, _)| name == path);
                        index = found.unwrap_or(collected.layers.len());
                        route = match found {
                            Some(found) => collected.layers[found].1.clone(),
                            None => {
                                let siblings = collected
                                    .layers
                                    .iter()
                                    .filter(|(name, _)| {
                                        name.rsplit_once('.').map_or("", |(owner, _)| owner)
                                            == owner
                                    })
                                    .count();
                                route.push(siblings as u32);
                                collected.layers.push((path.to_owned(), route.clone()));
                                route
                            }
                        };
                    }
                    for rule in children {
                        rule.collect(parent, Some(index), collected);
                    }
                }
            }
            Self::Starting(children) => {
                for rule in children {
                    rule.collect(parent, layer, collected);
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
                let matched = collected.media(queries);
                let fixed = !children.iter().all(Self::plain);
                let block = Rc::new(MediaBlock {
                    queries: queries.clone(),
                    matched: matched.into(),
                    fixed,
                });
                collected.blocks.push(block.clone());
                if matched || !fixed {
                    let outer = collected.media.clone();
                    if !fixed {
                        collected.media = Some(
                            outer
                                .iter()
                                .flat_map(|stack| stack.iter().cloned())
                                .chain([block])
                                .collect(),
                        );
                    }
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                    collected.media = outer;
                }
            }
        }
    }

    fn plain(&self) -> bool {
        match self {
            Self::Layer(..) | Self::Keyframes(..) | Self::Property(..) | Self::Theme(..) => false,
            Self::Style(_, children)
            | Self::Media(_, children)
            | Self::Supports(_, _, children)
            | Self::Starting(children)
            | Self::Container(.., children) => children.iter().all(Self::plain),
            _ => true,
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
            Self::Starting(children) => format!(
                "@starting-style{{{}}}",
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
            Self::Variant(name, body) if body.starts_with('{') => format!("@variant {name}{body}"),
            Self::Variant(name, body) if body.contains("@slot") => {
                format!("@custom-variant {name}{{{body}}}")
            }
            Self::Variant(name, template) => format!("@custom-variant {name} ({template});"),
            Self::Property(name, syntax, inherits, initial) => format!(
                "@property {name}{{syntax:{};inherits:{inherits};{}}}",
                serde_json::to_string(syntax).unwrap_or_default(),
                initial
                    .as_ref()
                    .map_or(String::new(), |value| format!("initial-value:{value}"))
            ),
            Self::Tailwind(Some(true)) => "@import 'tailwindcss';".into(),
            Self::Tailwind(Some(false)) => "@import 'tailwindcss/utilities';".into(),
            Self::Tailwind(None) => "@reference 'tailwindcss';".into(),
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
    pub shadows: Rc<Shadows>,
    pub top_layer: bool,
    pub animated: HashMap<usize, Style>,
    pub reduced_motion: bool,
    pub theme: u32,
    pub scale: f32,
    pub aliases: HashMap<String, String>,
    pub inserted: Vec<(Rc<Stylesheet>, bool)>,
    pub previous: Option<Rc<ComputedStyles>>,
}

#[derive(Default)]
pub(crate) struct Shadows {
    pub roots: FxHashMap<usize, usize>,
    pub hosts: Rc<FxHashMap<usize, usize>>,
    pub composed: FxHashMap<usize, Vec<usize>>,
}

#[derive(Default)]
struct TreeWalk {
    walk: crate::dom::Walk,
    built: (u64, Rc<Shadows>),
    used: std::cell::Cell<u64>,
    tree: Rc<Vec<u32>>,
    trees: Rc<Vec<(usize, usize)>>,
    special: Vec<Vec<usize>>,
    tables: Vec<usize>,
}

thread_local! {
    static EMPTY: (Rc<Style>, Vars) = Default::default();
}

pub(crate) fn empty() -> (Rc<Style>, Vars) {
    EMPTY.with(Clone::clone)
}

#[derive(Clone, Default)]
pub(crate) struct ComputedStyles {
    blended: std::cell::Cell<Option<bool>>,
    pub values: Chunked<Rc<Style>>,
    pub vars: Chunked<Vars>,
    pub flags: Chunked<u16>,
    pub lengths: NodeLengths,
    pub pseudos: FxHashMap<usize, Rc<[Style; 2]>>,
    pub text_pseudos: FxHashMap<usize, Rc<[Option<Style>; 4]>>,
    pub backdrops: FxHashMap<usize, Rc<Style>>,
    pub backdrop_starts: FxHashMap<usize, Rc<Style>>,
    pub scrollbars: FxHashMap<usize, Rc<[Option<Style>; 5]>>,
    pub selections: FxHashMap<usize, Rc<Style>>,
    pub user_select: Chunked<(UserSelect, usize)>,
    pub animations: FxHashMap<usize, Rc<[Animation]>>,
    // Properties whose winning declaration is important, so animations don't change them.
    pub important: FxHashMap<usize, Rc<[Prop]>>,
    pub children: FxHashMap<usize, Vec<usize>>,
    pub slotted: FxHashMap<usize, Shadowed>,
    pub parts: FxHashMap<usize, Shadowed>,
    pub exported: FxHashMap<usize, Shadowed>,
    pub starting: FxHashMap<usize, Rc<Style>>,
    pub list_items: FxHashMap<usize, i64>,
    // The declared `content` of `::before` and `::after` where it has a counter. Their styles hold the text of
    // the last pass.
    counting: FxHashMap<usize, [Option<Rc<str>>; 2]>,
    pub registry: Rc<Registry>,
    pub authored: FxHashMap<usize, Rc<Style>>,
    // The own flags of the nodes that an anonymous table changed. Their own styles are in `authored`.
    anonymous: FxHashMap<usize, u16>,
    containers: Chunked<Option<Rc<Container>>>,
    counted: bool,
}

pub(crate) type Shadowed = Rc<Vec<Passed>>;

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}

pub fn styles(doc: &Document, width: f32, height: f32) -> Vec<BBTreeMap<String, String>> {
    let out = compute_styles(
        doc,
        width,
        height,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    out.values
        .iter()
        .zip(out.vars.iter())
        .map(|(style, vars)| {
            style
                .own()
                .chain(vars.iter().map(|(prop, value)| (*prop, value)))
                .map(|(prop, value)| (prop.name().to_owned(), value.to_string()))
                .collect()
        })
        .collect()
}

pub(crate) fn font_lists(doc: &Document) -> Vec<Vec<String>> {
    let mut lists = Vec::new();
    let mut pending: Vec<&Rule> = doc
        .nodes
        .iter()
        .filter_map(|node| node.stylesheet.as_deref())
        .flat_map(|sheet| sheet.0.iter().map(|rule| &**rule))
        .collect();
    while let Some(rule) = pending.pop() {
        match rule {
            Rule::Style(_, rules)
            | Rule::Media(_, rules)
            | Rule::Layer(_, rules)
            | Rule::Supports(_, _, rules)
            | Rule::Theme(_, rules)
            | Rule::Container(_, _, _, rules)
            | Rule::Starting(rules) => pending.extend(rules),
            Rule::Declarations(values) => {
                for declaration in &values.0 {
                    if let Declaration::Property(name, value) = declaration
                        && (name == "font-family" || name.starts_with("--font"))
                    {
                        lists.push(
                            components(value, ',')
                                .iter()
                                .map(|family| family.trim().trim_matches(['\'', '"']).to_owned())
                                .collect(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    lists
}

pub(crate) fn font_names(system: &renderer::TextSystem, aliases: &HashMap<String, String>) {
    FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if names.0.is_empty() {
            names.1.clear();
            names.0 = system
                .families()
                .into_iter()
                .map(|name| (name.to_ascii_lowercase(), name))
                .chain(
                    aliases
                        .iter()
                        .map(|(alias, name)| (alias.to_ascii_lowercase(), name.clone())),
                )
                .collect();
        }
    });
}

pub(crate) fn compute_styles(
    doc: &Document,
    width: f32,
    height: f32,
    text_system: Option<&renderer::TextSystem>,
    cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
) -> ComputedStyles {
    let mut out = ComputedStyles::default();
    let (reduced_motion, native_theme) = context.map_or_else(
        || (reduced_motion(), crate::desktop::native::theme_state()),
        |context| (context.reduced_motion, context.theme),
    );
    if let Some(text) = text_system {
        font_names(
            text,
            context.map_or(&HashMap::new(), |context| &context.aliases),
        );
    }
    let blank = || Collect {
        width,
        height,
        reduced_motion,
        native_theme,
        rules: Vec::new(),
        container: None,
        media: None,
        blocks: Vec::new(),
        layers: Vec::new(),
        keyframes: HashMap::new(),
        properties: Registry::default(),
        layered: HashMap::new(),
    };
    let shadows = context.map_or_else(Default::default, |context| context.shadows.clone());
    style_cache.shadows = shadows.clone();
    let walk = style_cache.walk(doc, &shadows);
    let order = walk.walk.nodes.as_slice();
    let stored = context.is_some_and(|context| context.animated.is_empty());
    let previous = context
        .and_then(|context| context.previous.as_deref())
        .filter(|previous| {
            previous.values.len() <= doc.nodes.len() && previous.lengths.list_len() <= 4096
        });
    let mut fast = previous.is_some() && !style_cache.full;
    // This scan relies on the page setting `top_layer` whenever a node is fullscreen.
    let fullscreen = context.is_none_or(|context| context.top_layer)
        && order
            .iter()
            .any(|&(id, _)| doc.nodes[id as usize].state & crate::dom::FULLSCREEN != 0);
    let inserted = context.map_or(&[][..], |context| &context.inserted[..]);
    let user = |sheet: &Rc<Stylesheet>| {
        inserted
            .iter()
            .any(|(inserted, user)| *user && Rc::ptr_eq(inserted, sheet))
    };
    let scale = context.map_or(1., |context| context.scale);
    let metrics = text_system.map(|_| scale);
    let old = &mut style_cache.environment;
    if old.viewport != [width, height]
        || old.metrics != metrics
        || old.reduced_motion != reduced_motion
        || old.native_theme != native_theme
    {
        fast = false;
        style_cache.full = true;
        let resized = old.metrics == metrics
            && old.reduced_motion == reduced_motion
            && old.native_theme == native_theme;
        old.viewport = [width, height];
        old.metrics = metrics;
        old.reduced_motion = reduced_motion;
        old.native_theme = native_theme;
        style_cache.values.retain(|_, values| {
            values.retain(|value| resized && !value.viewport);
            !values.is_empty()
        });
        if !resized {
            style_cache.queries.clear();
        }
        // Scopes can share `@media` blocks, so set new results only after every scope has checked them.
        let probe = blank();
        let mut flipped: Vec<Rc<MediaBlock>> = Vec::new();
        for block in style_cache.scopes.iter().flat_map(|scope| &scope.blocks) {
            if probe.media(&block.queries) != block.matched.get()
                && !flipped.iter().any(|old| Rc::ptr_eq(old, block))
            {
                flipped.push(block.clone());
            }
        }
        let hit = |stack: &Option<MediaStack>| {
            stack.as_ref().is_some_and(|stack| {
                stack
                    .iter()
                    .any(|block| flipped.iter().any(|old| Rc::ptr_eq(old, block)))
            })
        };
        let count = style_cache.scopes.len();
        style_cache.scopes.retain_mut(|scope| {
            if scope
                .blocks
                .iter()
                .any(|block| block.fixed && flipped.iter().any(|old| Rc::ptr_eq(old, block)))
            {
                return false;
            }
            let StyleScope {
                nodes,
                medias,
                pseudos,
                ..
            } = scope;
            for node in nodes.iter_mut().flatten() {
                if !resized
                    || node
                        .cached
                        .as_ref()
                        .is_some_and(|cached| cached.value.viewport)
                    || !flipped.is_empty()
                        && (node.rules.iter().any(|(order, _)| hit(&medias[*order]))
                            || node
                                .pseudos
                                .iter()
                                .any(|&index| hit(&medias[pseudos[index].2])))
                {
                    node.cached = None;
                }
            }
            true
        });
        for block in &flipped {
            block.matched.set(!block.matched.get());
        }
        if style_cache.scopes.len() < count {
            style_cache.matches.forget_selectors();
        }
        let live: rustc_hash::FxHashSet<_> =
            style_cache.scopes.iter().map(|scope| scope.id).collect();
        style_cache.queries.retain(|key, _| live.contains(&key.0));
    }
    let mut scheme = None;
    let mut theme = None;
    let mut chosen = Vec::with_capacity(walk.trees.len());
    let mut lists: FxHashMap<Vec<usize>, usize> = FxHashMap::default();
    let (mut sheets, mut list) = (Vec::new(), Vec::new());
    for (tree, &(root, _)) in walk.trees.iter().enumerate() {
        sheets.clear();
        #[cfg(target_os = "macos")]
        sheets.push(GLASS_SHEET.with(Clone::clone));
        for &id in &walk.special[tree] {
            let node = &doc.nodes[id];
            if node.tag == "meta"
                && tree == 0
                && node
                    .attrs
                    .get("name")
                    .is_some_and(|name| name.eq_ignore_ascii_case("color-scheme"))
            {
                scheme = node
                    .attrs
                    .get("content")
                    .map(|content| Rc::<str>::from(content.trim()));
            }
            if node.tag != "style"
                && !(node.tag == "link"
                    && node.stylesheet.is_some()
                    && node.attrs.get("rel").is_some_and(|rel| {
                        rel.split_ascii_whitespace()
                            .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    }))
            {
                continue;
            }
            let mut ancestor = node.parent;
            while let Some(parent) = ancestor {
                if matches!(doc.nodes[parent].tag.as_str(), "template" | "noscript") {
                    break;
                }
                ancestor = doc.nodes[parent].parent;
            }
            if ancestor.is_some() || node.attrs.contains_key("disabled") {
                continue;
            }
            if let Some(media) = node
                .attrs
                .get("media")
                .filter(|media| !media.trim().is_empty())
            {
                let source = format!("@media {media}{{}}");
                let query = match cache.get(source.as_str()) {
                    Some(sheet) => sheet.clone(),
                    None => {
                        let sheet = Rc::new(Stylesheet::parse(&source));
                        cache.insert(source.into(), sheet.clone());
                        sheet
                    }
                };
                if !query.matches_media(width, height, native_theme) {
                    continue;
                }
            }
            sheets.extend(style_cache.sheet(doc, id, cache));
        }
        if let Some(adopted) = style_cache.adopted.get(&root) {
            sheets.extend(
                adopted
                    .iter()
                    .filter_map(|id| style_cache.constructed.get(id).cloned()),
            );
        }
        if tree == 0 {
            sheets.extend(inserted.iter().map(|(sheet, _)| sheet.clone()));
        }
        list.clear();
        list.extend(sheets.iter().map(|sheet| Rc::as_ptr(sheet) as usize));
        if let Some(&scope) = lists.get(list.as_slice()) {
            chosen.push(scope);
            continue;
        }
        let mut collected = blank();
        let tailwind = style_cache.tailwind.update(&mut sheets, doc, order);
        if let Some(update) = &tailwind
            && !(update.replaced.is_empty() && update.added.is_empty())
        {
            let mut added = blank();
            for rule in &update.rules {
                rule.collect(None, None, &mut added);
            }
            let old: Vec<_> = update
                .replaced
                .iter()
                .chain(update.added.iter().map(|(old, _)| old))
                .collect();
            let count = style_cache.scopes.len();
            let (matches, styled) = (&mut style_cache.matches, &mut style_cache.styled);
            let plain = added.keyframes.is_empty() && added.properties.is_empty();
            style_cache.scopes.retain_mut(|scope| {
                if lists.values().any(|&id| id == scope.id)
                    || !scope
                        .sheets
                        .iter()
                        .any(|sheet| old.iter().any(|old| Rc::ptr_eq(sheet, old)))
                {
                    return true;
                }
                let local = scope.insert(update, &added.rules, &added.blocks, doc, matches);
                // The dirty marks stand for the new rules in the last stored pass.
                match styled.as_mut().filter(|_| plain && local == Some(true)) {
                    Some(styled) => {
                        for old in styled.scopes.iter_mut().filter(|old| old.1 == scope.id) {
                            old.2 = scope.rules.len();
                        }
                    }
                    None => fast = false,
                }
                local.is_some()
            });
            if style_cache.scopes.len() < count {
                style_cache.matches.forget_selectors();
            }
            if !update.replaced.is_empty() {
                style_cache.queries.clear();
            }
        }
        if tree == 0 {
            theme = tailwind.as_ref().map(|update| update.root.clone());
        }
        // Collect built-in sheets last so author sheets define layer order. Keep built-in rules first within
        // each layer.
        let builtin = usize::from(cfg!(target_os = "macos"));
        let sources = (builtin..sheets.len()).chain(0..builtin);
        let exact = style_cache.scopes.iter().position(|cached| {
            cached.fullscreen == fullscreen
                && cached.sheets.len() == sheets.len()
                && cached
                    .sheets
                    .iter()
                    .zip(&sheets)
                    .all(|(a, b)| Rc::ptr_eq(a, b))
        });
        type Fresh = (Vec<Vec<bool>>, Vec<(Rc<Stylesheet>, usize)>);
        let fresh = |old: &[Rc<Stylesheet>]| -> Option<Fresh> {
            let mut next = 0;
            let mut fresh = Vec::new();
            let mut gone = Vec::new();
            for sheet in &sheets {
                let mut flags = vec![false; sheet.0.len()];
                match old.get(next) {
                    Some(previous) if Rc::ptr_eq(previous, sheet) => next += 1,
                    Some(previous)
                        if style_cache.derived.get(&Rc::as_ptr(sheet)).is_some_and(
                            |(weak, base, ..)| {
                                weak.as_ptr() == Rc::as_ptr(sheet) && Rc::ptr_eq(base, previous)
                            },
                        ) =>
                    {
                        next += 1;
                        let (_, base, inserted, removed) = &style_cache.derived[&Rc::as_ptr(sheet)];
                        for &index in inserted {
                            *flags.get_mut(index)? = true;
                        }
                        gone.extend(removed.iter().map(|&index| (base.clone(), index)));
                    }
                    _ if old.iter().any(|previous| Rc::ptr_eq(previous, sheet)) => return None,
                    _ => flags.fill(true),
                }
                fresh.push(flags);
            }
            (next == old.len()).then_some((fresh, gone))
        };
        let incremental = exact
            .is_none()
            .then(|| {
                style_cache
                    .scopes
                    .iter()
                    .enumerate()
                    .find_map(|(index, cached)| {
                        (lists.values().all(|&id| id != cached.id)
                            && cached.fullscreen == fullscreen)
                            .then(|| fresh(&cached.sheets))
                            .flatten()
                            .map(|fresh| (index, fresh))
                    })
            })
            .flatten();
        let scope = match (exact, incremental) {
            (Some(scope), _) => scope,
            (None, Some((index, (fresh, gone)))) => {
                let mut new = Vec::new();
                for index in sources.clone() {
                    let (sheet, flags) = (&sheets[index], &fresh[index]);
                    let first = collected.rules.len();
                    for (rule, &flag) in sheet.0.iter().zip(flags) {
                        let start = collected.rules.len();
                        rule.collect(None, None, &mut collected);
                        new.extend(std::iter::repeat_n(flag, collected.rules.len() - start));
                    }
                    if builtin != 0 && index == 0 {
                        let count = collected.rules.len() - first;
                        collected.rules.rotate_right(count);
                        new.rotate_right(count);
                    }
                    if user(sheet) {
                        for rule in &mut collected.rules[first..] {
                            rule.2.origin = USER;
                        }
                    }
                }
                collected.rank();
                let mut removed = blank();
                for (sheet, index) in &gone {
                    if let Some(rule) = sheet.0.get(*index) {
                        rule.collect(None, None, &mut removed);
                    }
                }
                let cached = &mut style_cache.scopes[index];
                let deleted: Vec<usize> = (0..cached.rules.len())
                    .filter(|&order| {
                        removed
                            .rules
                            .iter()
                            .any(|rule| Rc::ptr_eq(rule.0, &cached.rules[order].0))
                    })
                    .collect();
                // Preserve deleted rule positions with DELETED origin and no declarations or keys.
                let mut author: Vec<usize> = (0..cached.rules.len())
                    .filter(|&order| {
                        !matches!(cached.rules[order].2.origin, AGENT | DELETED)
                            && !deleted.contains(&order)
                    })
                    .collect();
                author.sort_by_key(|&order| cached.keys[order]);
                let old = new.iter().filter(|flag| !**flag).count();
                let same = old == author.len()
                    && collected.layers == cached.layers
                    && collected
                        .rules
                        .iter()
                        .zip(&new)
                        .filter(|(_, flag)| !**flag)
                        .zip(&author)
                        .all(|(((selectors, ..), _), &order)| {
                            Rc::ptr_eq(selectors, &cached.rules[order].0)
                        });
                let mut room = same;
                let mut keys = Vec::new();
                let mut inserted = Vec::new();
                let mut pending = Vec::new();
                if same {
                    let generated = |key: u128| {
                        cached
                            .block
                            .is_some_and(|block| (key >> 64) as u64 == block && key as u64 != 0)
                    };
                    let mut olds = author.iter().copied();
                    let mut previous = cached
                        .keys
                        .iter()
                        .zip(&cached.rules)
                        .filter(|(_, rule)| rule.2.origin == AGENT)
                        .map(|(key, _)| *key)
                        .max()
                        .unwrap_or(0);
                    for (rule, &flag) in collected.rules.iter().zip(&new) {
                        if flag {
                            pending.push(rule.clone());
                            continue;
                        }
                        let next = cached.keys[olds.next().unwrap_or_default()];
                        if !pending.is_empty() {
                            let step =
                                ((next - previous) / (pending.len() as u128 + 1)).min(1 << 20);
                            room &= step > 0 && !generated(previous) && !generated(next);
                            for (index, rule) in pending.drain(..).enumerate() {
                                keys.push(previous + step * (index as u128 + 1));
                                inserted.push(rule);
                            }
                        }
                        previous = next;
                    }
                }
                if room {
                    let start = cached.rules.len();
                    cached.push(inserted, None, &mut style_cache.matches.attributes);
                    for (order, key) in (start..).zip(keys) {
                        cached.keys[order] = key;
                    }
                    cached.push(pending, None, &mut style_cache.matches.attributes);
                    let local = cached.rematch(start, doc, &mut style_cache.matches.dirty);
                    let Collect {
                        keyframes,
                        properties,
                        blocks,
                        ..
                    } = std::mem::replace(&mut collected, blank());
                    let plain = local
                        && deleted.is_empty()
                        && cached.keyframes == keyframes
                        && cached.properties == properties;
                    // The dirty marks stand for the new rules in the last stored pass.
                    match style_cache.styled.as_mut().filter(|_| plain) {
                        Some(styled) => {
                            for old in styled.scopes.iter_mut().filter(|old| old.1 == cached.id) {
                                old.2 = cached.rules.len();
                            }
                        }
                        None => fast = false,
                    }
                    if !deleted.is_empty() {
                        for &order in &deleted {
                            cached.rules[order].1 = Rc::new([]);
                            cached.rules[order].2.origin = DELETED;
                            cached.elements[order].clear();
                        }
                        let stale = |rules: &[(usize, [usize; 3])], pseudos: &[usize]| {
                            rules.iter().any(|(order, _)| deleted.contains(order))
                                || pseudos
                                    .iter()
                                    .any(|&index| deleted.contains(&cached.pseudos[index].2))
                        };
                        for matched in &mut cached.nodes {
                            if matched
                                .as_ref()
                                .is_some_and(|matched| stale(&matched.rules, &matched.pseudos))
                            {
                                *matched = None;
                            }
                        }
                        style_cache.values.retain(|key, _| {
                            key.scope != cached.id || !stale(&key.rules.0, &key.pseudos)
                        });
                    }
                    if cached.keyframes != keyframes {
                        let animated = |value: &CachedStyle| {
                            value.style.contains(Prop::ANIMATION)
                                || value.style.contains(Prop::ANIMATION_NAME)
                        };
                        for node in cached.nodes.iter_mut().flatten() {
                            if node
                                .cached
                                .as_ref()
                                .is_some_and(|node| animated(&node.value))
                            {
                                node.cached = None;
                            }
                        }
                        style_cache.values.retain(|key, values| {
                            key.scope != cached.id || {
                                values.retain(|value| !animated(value));
                                !values.is_empty()
                            }
                        });
                    }
                    cached.sheets = sheets.clone();
                    let mut seen = std::collections::HashSet::new();
                    cached.blocks = cached
                        .medias
                        .iter()
                        .flatten()
                        .flat_map(|stack| stack.iter())
                        .chain(blocks.iter().filter(|block| block.fixed))
                        .filter(|block| seen.insert(Rc::as_ptr(block)))
                        .cloned()
                        .collect();
                    cached.keyframes = keyframes;
                    cached.properties = properties;
                    index
                } else {
                    collected.rules.clear();
                    collected.layers.clear();
                    collected.keyframes.clear();
                    collected.properties.clear();
                    collected.layered.clear();
                    collected.blocks.clear();
                    usize::MAX
                }
            }
            (None, None) => usize::MAX,
        };
        let scope = if scope != usize::MAX {
            scope
        } else {
            for index in sources {
                let sheet = &sheets[index];
                let first = collected.rules.len();
                for rule in &sheet.0 {
                    rule.collect(None, None, &mut collected);
                }
                if builtin != 0 && index == 0 {
                    let count = collected.rules.len() - first;
                    collected.rules.rotate_right(count);
                }
                if user(sheet) {
                    for rule in &mut collected.rules[first..] {
                        rule.2.origin = USER;
                    }
                }
            }
            let author = std::mem::take(&mut collected.rules);
            let ua = if fullscreen {
                &FULLSCREEN_SHEET
            } else {
                &UA_SHEET
            }
            .with(Clone::clone);
            for rule in &ua.0 {
                rule.collect(None, None, &mut collected);
            }
            for rule in &mut collected.rules {
                rule.2.origin = AGENT;
            }
            collected.rules.extend(author);
            collected.rank();
            let Collect {
                rules,
                keyframes,
                properties,
                layers,
                blocks,
                ..
            } = collected;
            style_cache.next += 1;
            let mut scope = StyleScope {
                id: style_cache.next,
                fullscreen,
                sheets: sheets.clone(),
                layers,
                blocks,
                keyframes,
                properties,
                ..Default::default()
            };
            scope.push(
                rules,
                tailwind.as_ref().map(|update| update.keys),
                &mut style_cache.matches.attributes,
            );
            style_cache.scopes.push(scope);
            style_cache.scopes.len() - 1
        };
        let scoped = &mut style_cache.scopes[scope];
        scoped.used = style_cache.pass;
        if scoped.nodes.len() < doc.nodes.len() {
            scoped.nodes.resize_with(doc.nodes.len(), || None);
        }
        lists.insert(list.clone(), scoped.id);
        chosen.push(scoped.id);
    }
    let indices: FxHashMap<usize, usize> = style_cache
        .scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| (scope.id, index))
        .collect();
    let scopes: Vec<usize> = chosen
        .iter()
        .map(|id| indices.get(id).copied().unwrap_or_default())
        .collect();
    {
        let mut registry = style_cache.scopes[scopes[0]].properties.clone();
        registry.extend(
            style_cache
                .scripted
                .iter()
                .map(|(&prop, value)| (prop, value.clone())),
        );
        if registry != *style_cache.environment.registry {
            style_cache.environment.registry = Rc::new(registry);
            style_cache.inheritable.clear();
            style_cache.uncache();
        }
    }
    if scheme != style_cache.environment.scheme {
        style_cache.environment.scheme = scheme.clone();
        style_cache.uncache();
    }
    let environment = style_cache.environment.clone();
    let registry = environment.registry.clone();
    let mut used: Vec<(usize, usize, usize)> = walk
        .trees
        .iter()
        .zip(&scopes)
        .map(|(&(root, _), &index)| {
            let scope = &style_cache.scopes[index];
            (root, scope.id, scope.rules.len())
        })
        .collect();
    used.sort_unstable();
    // Reuse previous only with the same viewport, registry and scopes. Unstored passes can change these too.
    fast &= style_cache.styled.as_ref().is_some_and(|old| {
        old.environment == environment
            && used.iter().all(|scope| {
                match old.scopes.binary_search_by_key(&scope.0, |old| old.0) {
                    Ok(index) => old.scopes[index] == *scope,
                    Err(_) => true,
                }
            })
    });
    out.registry = registry.clone();
    let count = doc.nodes.len();
    match previous.filter(|_| fast) {
        Some(previous) => {
            out.values.clone_from(&previous.values);
            out.vars.clone_from(&previous.vars);
            out.flags.clone_from(&previous.flags);
            out.lengths.clone_from(&previous.lengths);
            out.lengths.grow(count);
            out.pseudos.clone_from(&previous.pseudos);
            out.text_pseudos.clone_from(&previous.text_pseudos);
            out.backdrops.clone_from(&previous.backdrops);
            out.backdrop_starts.clone_from(&previous.backdrop_starts);
            out.counting.clone_from(&previous.counting);
            out.scrollbars.clone_from(&previous.scrollbars);
            out.selections.clone_from(&previous.selections);
            out.animations.clone_from(&previous.animations);
            out.important.clone_from(&previous.important);
            out.starting.clone_from(&previous.starting);
            out.containers.clone_from(&previous.containers);
            out.slotted.clone_from(&previous.slotted);
            out.parts.clone_from(&previous.parts);
            out.exported.clone_from(&previous.exported);
        }
        None => out.lengths.reset(count, Lengths::new(16., width, height)),
    }
    let (style, vars) = empty();
    out.values.resize(count, style);
    out.vars.resize(count, vars);
    out.flags.resize(count, 0);
    out.containers.resize(count, None);
    let mut orphans = std::mem::take(&mut style_cache.orphans);
    match previous.filter(|_| fast) {
        // The cascade must read the own styles of the nodes that the last anonymous tables changed.
        Some(previous) => {
            for (&id, &flags) in &previous.anonymous {
                if let Some(own) = previous.authored.get(&id).filter(|_| id < count) {
                    out.values[id] = own.clone();
                    out.flags[id] = flags;
                }
            }
        }
        None => orphans.clear(),
    }
    let mut inheritable = std::mem::take(&mut style_cache.inheritable);
    let mut local: Vec<Prop> = registry
        .iter()
        .filter(|(_, (_, inherits, _))| !inherits)
        .map(|(&prop, _)| prop)
        .collect();
    local.sort_unstable();
    let mut inherit = |vars: &Vars| -> Vars {
        if local.is_empty() {
            return vars.clone();
        }
        inheritable
            .entry(Rc::as_ptr(vars))
            .or_insert_with(|| {
                // Both lists are in order. Most lists hold no local property, so they are not copied.
                let (mut rest, mut kept) = (&local[..], None::<Vec<_>>);
                for (index, entry) in vars.iter().enumerate() {
                    while rest.first().is_some_and(|&prop| prop < entry.0) {
                        rest = &rest[1..];
                    }
                    if rest.first() == Some(&entry.0) {
                        kept.get_or_insert_with(|| vars[..index].to_vec());
                    } else if let Some(kept) = &mut kept {
                        kept.push(entry.clone());
                    }
                }
                (vars.clone(), kept.map(Rc::new))
            })
            .1
            .clone()
            .unwrap_or_else(|| vars.clone())
    };
    let (unstyled, empty) = empty();
    let matches = &mut style_cache.matches;
    matches.set_hosts(&shadows.hosts);
    matches.update(doc, &walk.walk);
    let mut candidates: Vec<usize> = Vec::new();
    let mut shares: FxHashMap<(usize, [u32; 7]), Share> = FxHashMap::default();
    let no_pseudos: Rc<[usize]> = Rc::new([]);
    let unshared = style_cache.unshared;
    let ComputedStyles {
        values: styles,
        vars: custom,
        flags: style_flags,
        lengths,
        pseudos,
        text_pseudos,
        backdrops,
        backdrop_starts,
        scrollbars,
        selections,
        animations,
        important,
        slotted,
        parts,
        exported,
        starting: starts,
        list_items,
        counting,
        authored,
        anonymous,
        containers,
        counted,
        ..
    } = &mut out;
    macro_rules! unstyle {
        ($id:expr) => {
            pseudos.remove(&$id);
            text_pseudos.remove(&$id);
            backdrops.remove(&$id);
            backdrop_starts.remove(&$id);
            counting.remove(&$id);
            scrollbars.remove(&$id);
            selections.remove(&$id);
            animations.remove(&$id);
            important.remove(&$id);
            starts.remove(&$id);
            slotted.remove(&$id);
            parts.remove(&$id);
            exported.remove(&$id);
        };
    }
    const CHANGED: u8 = 1;
    const DEEP: u8 = 2;
    let mut changed = std::mem::take(&mut style_cache.changed);
    style_cache.stamp += 1;
    let stamp = style_cache.stamp << 8;
    macro_rules! changed {
        ($id:expr) => {
            match changed[$id] ^ stamp {
                value if value < 256 => value as u8,
                _ => 0,
            }
        };
    }
    let mut counters = false;
    let html: Vec<usize> = walk.special[0]
        .iter()
        .copied()
        .filter(|&id| doc.nodes[id].tag == "html")
        .collect();
    if fast {
        if changed.len() < doc.nodes.len() {
            changed.resize(doc.nodes.len(), 0);
        }
        if let Some(old) = style_cache
            .styled
            .as_ref()
            .filter(|old| !Rc::ptr_eq(&old.nodes, &walk.walk.nodes))
        {
            const GONE: u32 = u32::MAX - 1;
            const KEPT: u32 = u32::MAX - 2;
            let root = |tree: &[u32], trees: &[(usize, usize)], id: usize| {
                tree.get(id)
                    .and_then(|&index| trees.get(index as usize))
                    .map(|tree| tree.0)
            };
            macro_rules! leave {
                ($id:expr) => {
                    styles[$id] = unstyled.clone();
                    custom[$id] = empty.clone();
                    style_flags[$id] = 0;
                    lengths.unset($id);
                    containers[$id] = None;
                    unstyle!($id);
                };
            }
            // Without shadow trees, only the nodes that a patched walk touched can have another place.
            let touched = walk
                .walk
                .since(old.serial)
                .filter(|_| shadows.hosts.is_empty() && old.trees.len() == 1);
            let mut placed = Vec::new();
            if touched.is_none() || cfg!(debug_assertions) {
                placed.resize(doc.nodes.len(), GONE);
                for &(id, up) in old.nodes.iter() {
                    if let Some(slot) = placed.get_mut(id as usize) {
                        *slot = up;
                    }
                }
            }
            match touched {
                Some(touched) => {
                    // A node with another parent is dirty by its snapshot. A node that joins has no style.
                    for &id in touched {
                        let id = id as usize;
                        if !walk.walk.contains(id) {
                            leave!(id);
                        } else if Rc::ptr_eq(&styles[id], &unstyled) {
                            matches.dirty[id] = true;
                        }
                    }
                    #[cfg(debug_assertions)]
                    for (nodes, kept) in [(order, true), (&old.nodes[..], false)] {
                        for &(id, up) in nodes {
                            let slot = &mut placed[id as usize];
                            assert!(
                                *slot == if kept { up } else { KEPT } || touched.contains(&id),
                                "node {id} moved outside the patched walk"
                            );
                            assert!(
                                !kept
                                    || *slot == up
                                    || matches.dirty[id as usize]
                                    || doc.nodes[id as usize].parent != Some(up as usize),
                                "node {id} moved without a dirty mark"
                            );
                            *slot = if kept { KEPT } else { *slot };
                        }
                    }
                }
                None => {
                    for &(id, up) in order {
                        let id = id as usize;
                        if placed[id] != up
                            || root(&old.tree, &old.trees, id) != root(&walk.tree, &walk.trees, id)
                        {
                            matches.dirty[id] = true;
                            if let Some(&host) = shadows.hosts.get(&id) {
                                matches.dirty[host] = true;
                            }
                        }
                        placed[id] = KEPT;
                    }
                    for &(id, _) in old.nodes.iter() {
                        let id = id as usize;
                        if placed.get(id).is_some_and(|&slot| slot != KEPT) {
                            leave!(id);
                        }
                    }
                }
            }
        }
    }
    let mut all = false;
    let mut root = FontUnits::new(16.);
    let mut font_metrics = HashMap::new();
    let mut passed_on = FxHashMap::default();
    let mut merges = FxHashMap::default();
    let last = context.and_then(|context| context.previous.as_deref());
    let address =
        |list: Option<&Option<Rc<Container>>>| list.map(|list| list.as_ref().map(Rc::as_ptr));
    let mut hosted: Vec<Passed> = Vec::new();
    let mut texts = FxHashMap::<_, (Rc<Style>, Vars, u16, Lengths, Option<Rc<Style>>)>::default();
    macro_rules! settle {
        (@changed $id:expr, $up:expr) => {
            let (id, up): (usize, Option<usize>) = ($id, $up);
            if shadows.roots.contains_key(&id) {
                let list = exported_parts(
                    std::mem::take(&mut hosted),
                    parts.get(&id),
                    exported.get(&walk.trees[walk.tree[id] as usize].1),
                    doc.nodes[id].attrs.get("exportparts"),
                    last.and_then(|last| last.exported.get(&id)),
                );
                match list {
                    Some(list) => exported.insert(id, list),
                    None => exported.remove(&id),
                };
            }
            if let Some(previous) = previous.filter(|_| fast) {
                let font = previous.lengths.get(id).map(|old| old.font);
                let pointer = |list: Option<&Shadowed>| list.map(Rc::as_ptr);
                changed[id] = stamp | u64::from(u8::from(
                    !(previous
                        .authored
                        .get(&id)
                        .or(previous.values.get(id))
                        .is_some_and(|old| Rc::ptr_eq(old, &styles[id]))
                        && previous
                            .vars
                            .get(id)
                            .is_some_and(|old| Rc::ptr_eq(old, &custom[id]))
                        && font == Some(lengths[id].font)
                        && address(previous.containers.get(id)) == address(containers.get(id))
                        && previous.selections.get(&id) == selections.get(&id)
                        && pointer(previous.slotted.get(&id)) == pointer(slotted.get(&id))),
                ) * CHANGED
                    | up.map_or(0, |up| changed!(up) & DEEP)
                    | u8::from(pointer(previous.exported.get(&id)) != pointer(exported.get(&id)))
                        * DEEP);
                all |= (id == 0 || html.contains(&id)) && font != Some(root);
            }
            counters |= style_flags[id] & COUNTED != 0;
            if matches!(style_flags[id] >> 8, CELL | CAPTION | 10..=12)
                && let Some(up) = up.filter(|&up| style_flags[up] >> 8 == 0)
            {
                orphans.push(up);
            }
        };
        ($id:expr, $up:expr, $seen:expr) => {
            let id = $id;
            let link = match Container::of(id, &styles[id], $seen.clone()) {
                Some(link) => match last.and_then(|last| last.containers.get(id)?.as_ref()) {
                    Some(old) if **old == link => Some(old.clone()),
                    _ => Some(Rc::new(link)),
                },
                None => $seen,
            };
            containers.set(id, link, |a, b| address(Some(a)) == address(Some(b)));
            settle!(@changed id, $up);
        };
    }
    // A pass with animated values must leave the dirty marks, as the next stored pass starts again from the
    // last stored one.
    let mut moving = Vec::new();
    if fast && !stored {
        moving.resize(doc.nodes.len(), false);
        for &id in context
            .into_iter()
            .flat_map(|context| context.animated.keys())
        {
            if let Some(slot) = moving.get_mut(id) {
                *slot = true;
            }
        }
    }
    // A fast pass visits only the dirty and moving nodes and the nodes below a changed one. The document and
    // the root elements are in the list because they set the root font size.
    let mut seeds: Vec<u32> = Vec::new();
    if fast {
        let dirty = matches.dirty.chunks(64).enumerate();
        let dirty = dirty.filter(|(_, chunk)| chunk.contains(&true));
        let dirty = dirty.flat_map(|(index, chunk)| {
            let set = chunk.iter().enumerate().filter(|(_, dirty)| **dirty);
            set.map(move |(bit, _)| index * 64 + bit)
        });
        let animated = context.filter(|_| !stored);
        let animated = animated
            .into_iter()
            .flat_map(|context| context.animated.keys().copied());
        let roots = std::iter::once(0).chain(html.iter().copied());
        let places = roots.chain(dirty).chain(animated);
        seeds = places.filter_map(|id| walk.walk.place(id)).collect();
        seeds.sort_unstable();
    }
    let (mut next, mut seed, mut scan) = (0, 0, 0);
    let mut visited: Option<(usize, usize)> = None;
    loop {
        if let Some((at, id)) = visited.take()
            && changed!(id) != 0
        {
            scan = scan.max(at + walk.walk.span(id));
        }
        let mut reached = true;
        if fast && !all && next >= scan {
            while seeds.get(seed).is_some_and(|&at| (at as usize) < next) {
                seed += 1;
            }
            // Debug builds visit every node, to check that the skipped ones need no new style.
            match seeds.get(seed) {
                _ if cfg!(debug_assertions) => reached = seeds.get(seed) == Some(&(next as u32)),
                Some(&at) => next = at as usize,
                None => break,
            }
        }
        let Some(&(id, up)) = order.get(next) else {
            break;
        };
        let at = next;
        next += 1;
        let id = id as usize;
        if fast {
            if !all
                && !matches.dirty[id]
                && moving.get(id) != Some(&true)
                && (up == u32::MAX || changed!(up as usize) == 0)
            {
                if id == 0 || html.contains(&id) {
                    root = lengths[id].font;
                }
                continue;
            }
            assert!(
                reached || all,
                "node {id} needs a style outside the dirty list"
            );
            visited = Some((at, id));
            if stored {
                matches.dirty[id] = false;
            }
            unstyle!(id);
        }
        let up = (up != u32::MAX).then_some(up as usize);
        // A patched walk without shadow trees leaves `tree` as it was.
        let tree = match walk.trees.len() {
            1 => 0,
            _ => walk.tree[id] as usize,
        };
        let node = &doc.nodes[id];
        if tree != 0 && walk.trees[tree].0 == id {
            let host = walk.trees[tree].1;
            styles.set(id, styles[host].clone(), Rc::ptr_eq);
            custom.set(id, custom[host].clone(), Rc::ptr_eq);
            style_flags.set(id, style_flags[host], u16::eq);
            lengths.set(id, lengths[host]);
            containers.set(id, containers[host].clone(), |a, b| {
                address(Some(a)) == address(Some(b))
            });
            if let Some(selection) = selections.get(&host).cloned() {
                selections.insert(id, selection);
            }
            settle!(@changed id, up);
            continue;
        }
        let seen = up.and_then(|up| containers[up].clone());
        let parent_style = up.map(|up| &styles[up]);
        // Children of a `display: contents` box are items of the nearest ancestor that has a box.
        let parent_role = up.map_or(0, |up| style_flags[up] >> 8);
        let (item, table_item) = {
            let (mut at, mut display) = (
                up,
                parent_style.and_then(|parent| parent.get(Prop::DISPLAY)),
            );
            while display == Some("contents") {
                at = at
                    .and_then(|at| walk.walk.parent(at))
                    .filter(|&at| at != u32::MAX)
                    .map(|at| at as usize);
                display = at.and_then(|at| styles[at].get(Prop::DISPLAY));
            }
            (
                matches!(
                    display,
                    Some("flex" | "inline-flex" | "grid" | "inline-grid")
                ) || at.is_some_and(|at| drawable_canvas(&doc.nodes[at], &styles[at])),
                at.is_some_and(|at| table_box(style_flags[at] >> 8)),
            )
        };
        let parent_vars = up
            .map(|up| &custom[up])
            .filter(|_| !matches!(node.tag.as_str(), "#text" | "#comment"));
        let parent = up.map_or(FontUnits::new(16.), |up| lengths[up].font);
        let inputs = Inputs::new(parent, root);
        let mut waiting: Vec<(usize, usize, ContainerQuery)> = Vec::new();
        let mut shadow: Vec<ShadowRule> = Vec::new();
        let mut contained =
            |rule: &mut ShadowRule, key: (usize, usize), conditions: &ContainerQuery| {
                let applies = query_containers(
                    conditions,
                    seen.as_deref(),
                    key,
                    &style_cache.containers,
                    |container| lengths[container],
                    &mut style_cache.queries,
                );
                if matches!(rule.kind, ELEMENT | STARTING) {
                    return applies;
                }
                waiting.push((key.0, key.1, conditions.clone()));
                rule.query = Some((key.0, key.1, applies));
                true
            };
        hosted.clear();
        if let Some(&root) = shadows.roots.get(&id)
            && let Some(&index) = walk
                .tree
                .get(root)
                .and_then(|&tree| scopes.get(tree as usize))
        {
            let inner = &mut style_cache.scopes[index];
            if !inner.nodes[root]
                .as_ref()
                .is_some_and(|matched| matches.valid_since(&matched.dependencies, &matched.checked))
            {
                matches.dependencies.clear();
                matches.depend(root, 0);
                let mut matched: Vec<(usize, [usize; 3])> = Vec::new();
                for &entry in inner.universal.iter().filter(|&&entry| entry < PSEUDO) {
                    let (order, index) = (entry >> SELECTOR, entry & ((1 << SELECTOR) - 1));
                    let selector = &inner.rules[order].0[index];
                    if selector.matches_cached(doc, root, None, matches) {
                        match matched.last_mut() {
                            Some(last) if last.0 == order => {
                                last.1 = last.1.max(selector.specificity)
                            }
                            _ => matched.push((order, selector.specificity)),
                        }
                    }
                }
                let pseudos = inner
                    .universal
                    .iter()
                    .filter(|&&index| index >= PSEUDO)
                    .map(|&index| index & !PSEUDO)
                    .filter(|&index| {
                        let (kind, selector, _, key) = &inner.pseudos[index];
                        key.is_none()
                            && *kind != 12
                            && selector.matches_cached(doc, root, None, matches)
                    })
                    .collect();
                matches.dependencies.sort_unstable();
                matches.dependencies.dedup();
                matches.register(id);
                inner.nodes[root] = Some(MatchedRules {
                    checked: std::cell::Cell::new(matches.epoch),
                    cached: None,
                    dependencies: matches.dependencies.as_slice().into(),
                    rules: matched.into(),
                    pseudos,
                });
            }
            let inner = &*inner;
            for (kind, order, specificity, pseudo) in inner.nodes[root].iter().flat_map(|matched| {
                matched
                    .rules
                    .iter()
                    .map(|&(order, specificity)| (ELEMENT, order, specificity, 0))
                    .chain(matched.pseudos.iter().map(|&index| {
                        let (kind, selector, order, _) = &inner.pseudos[index];
                        (*kind, *order, selector.specificity, index)
                    }))
            }) {
                if inner.medias[order]
                    .as_ref()
                    .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                {
                    continue;
                }
                let (_, declarations, layer) = &inner.rules[order];
                let mut rule = ShadowRule {
                    kind,
                    context: -1,
                    layer: *layer,
                    specificity,
                    order: inner.keys[order],
                    declarations: declarations.clone(),
                    query: None,
                };
                if kind == PART {
                    if let Some((name, compound, target)) = inner.arguments.get(&pseudo) {
                        rule.kind = *target;
                        rule.context = 0;
                        hosted.push(Passed {
                            name: name.clone(),
                            compound: compound.clone(),
                            conditions: inner.containers[order]
                                .clone()
                                .map(|conditions| (inner.id, order, conditions)),
                            rule,
                        });
                    }
                    continue;
                }
                if let Some(conditions) = &inner.containers[order]
                    && !contained(&mut rule, (inner.id, order), conditions)
                {
                    continue;
                }
                shadow.push(rule);
            }
        }
        let selection = up.and_then(|up| selections.get(&up));
        let assigned = up
            .filter(|&up| node.parent != Some(up))
            .and_then(|up| slotted.get(&up));
        let names = (tree != 0).then(|| node.attrs.get("part")).flatten();
        let named = names.and_then(|_| exported.get(&walk.trees[tree].1));
        if assigned.is_some() || named.is_some() {
            matches.dependencies.clear();
            for passed in assigned.into_iter().flat_map(|list| list.iter()).chain(
                named
                    .into_iter()
                    .flat_map(|list| list.iter())
                    .filter(|passed| {
                        names.is_some_and(|names| {
                            !passed.name.is_empty()
                                && passed.name.split_ascii_whitespace().all(|name| {
                                    names.split_ascii_whitespace().any(|part| part == name)
                                })
                        })
                    }),
            ) {
                if !passed
                    .compound
                    .iter()
                    .any(|selector| selector.matches_cached(doc, id, None, matches))
                {
                    continue;
                }
                let mut rule = passed.rule.clone();
                if let Some((scope, order, conditions)) = &passed.conditions
                    && !contained(&mut rule, (*scope, *order), conditions)
                {
                    continue;
                }
                shadow.push(rule);
            }
            matches.dependencies.sort_unstable();
            matches.dependencies.dedup();
            matches.register(id);
        }
        let shadow: Option<Rc<[ShadowRule]>> = (!shadow.is_empty()).then(|| shadow.into());
        let animated = context.and_then(|context| context.animated.get(&id));
        let text = (matches!(node.tag.as_str(), "#text" | "#comment")
            && shadow.is_none()
            && animated.is_none()
            && !unshared)
            .then(|| {
                (
                    scopes[tree],
                    node.tag == "#comment",
                    parent_style.map(Rc::as_ptr),
                    inputs.units,
                    selection.map(Rc::as_ptr),
                )
            });
        if let Some((style, vars, flags, units, selected)) =
            text.as_ref().and_then(|text| texts.get(text))
        {
            styles.set(id, style.clone(), Rc::ptr_eq);
            custom.set(id, vars.clone(), Rc::ptr_eq);
            style_flags.set(id, *flags, u16::eq);
            lengths.set(id, *units);
            if let Some(selected) = selected {
                selections.insert(id, selected.clone());
            }
            settle!(id, up, seen);
            continue;
        }
        let scoped = &mut style_cache.scopes[scopes[tree]];
        let (rules, keys, scope) = (&scoped.rules, &scoped.keys, scoped.id);
        let mut key = None;
        let mut passed = None;
        let mut written = None;
        let mut moved = false;
        let mut contained = false;
        let mut reuse =
            |value: &CachedStyle, old: Option<&Vars>, vars: &Vars| match (old, parent_vars) {
                (None, None) => Some((vars.clone(), false)),
                (Some(old), Some(new)) if Rc::ptr_eq(old, new) => Some((vars.clone(), false)),
                (Some(_), Some(new)) => {
                    let reads = value.reads.as_ref()?;
                    (value
                        .missing
                        .iter()
                        .all(|name| Prop::find(name).is_none_or(|prop| var(new, prop).is_none()))
                        && reads
                            .iter()
                            .all(|(prop, value)| var(new, *prop) == value.as_ref()))
                    .then(|| {
                        if value.own.is_empty() {
                            return (inherit(new), true);
                        }
                        // Nodes with one style and one parent list must share the new list, or the
                        // nodes below them cannot share their styles.
                        let key = (Rc::as_ptr(new), value.own.as_ptr());
                        let merge = || {
                            let vars = inherit(new);
                            if value
                                .own
                                .iter()
                                .all(|(prop, own)| var(&vars, *prop) == own.as_ref())
                            {
                                return vars;
                            }
                            let mut list = (*vars).clone();
                            for (prop, own) in &value.own {
                                match (list.binary_search_by_key(prop, |entry| entry.0), own) {
                                    (Ok(index), Some(own)) => list[index].1 = own.clone(),
                                    (Err(index), Some(own)) => {
                                        list.insert(index, (*prop, own.clone()))
                                    }
                                    (Ok(index), None) => {
                                        list.remove(index);
                                    }
                                    (Err(_), None) => {}
                                }
                            }
                            Rc::new(list)
                        };
                        (merges.entry(key).or_insert_with(merge).clone(), true)
                    })
                }
                _ => None,
            };
        let cached = scoped.nodes[id]
            .as_ref()
            .filter(|node| {
                animated.is_none() && matches.valid_since(&node.dependencies, &node.checked)
            })
            .and_then(|node| node.cached.as_ref())
            .filter(|cached| {
                let same = match (&cached.parent, parent_style) {
                    (None, None) => true,
                    (Some(old), Some(new)) => {
                        Rc::ptr_eq(&old.0, new)
                            || !cached.value.keyword
                                && *passed_on
                                    .entry((Rc::as_ptr(&old.0), Rc::as_ptr(new)))
                                    .or_insert_with(|| {
                                        let own = |style: &'_ Rc<Style>| {
                                            style
                                                .own()
                                                .filter(|(prop, _)| prop.inherited())
                                                .map(|(prop, value)| (prop, value.clone()))
                                                .collect::<Vec<_>>()
                                        };
                                        // A `contents` box hands down the display of its ancestors.
                                        old.0.get(Prop::DISPLAY) == new.get(Prop::DISPLAY)
                                            && new.get(Prop::DISPLAY) != Some("contents")
                                            && own(&old.0) == own(new)
                                    })
                    }
                    _ => false,
                };
                same && cached.units == inputs.hash()
                    && cached
                        .containers
                        .as_ref()
                        .is_none_or(|containers| *containers == seen)
                    && cached.same_rare(shadow.as_ref(), selection)
            })
            .and_then(|cached| {
                let (vars, changed) =
                    reuse(&cached.value, cached.inherited.as_ref(), &cached.vars)?;
                moved = changed;
                Some((cached.value.clone(), vars))
            })
            .or_else(|| {
                let group = (!unshared && !node.tag.starts_with('#')).then(|| {
                    let [own, up] = [Some(id), node.parent]
                        .map(|id| id.map(|id| matches.signature(id)).unwrap_or_default());
                    (scope, [own[0], own[3], own[5], own[7], up[0], up[5], up[8]])
                });
                let (matched, matched_pseudos) = if let Some(cached) = scoped.nodes[id].as_ref()
                    && matches.valid_since(&cached.dependencies, &cached.checked)
                {
                    (cached.rules.clone(), cached.pseudos.clone())
                } else if node.tag.starts_with('#') {
                    let none = intern(&mut scoped.lists, &[]);
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: Box::default(),
                        rules: none.clone(),
                        pseudos: no_pseudos.clone(),
                    });
                    (none, no_pseudos.clone())
                } else if let Some(share) = group.and_then(|group| shares.get(&group))
                    && share
                        .style
                        .is_none_or(|style| style == matches.signature(id)[6])
                    && matches.share(doc, share.from, id, &share.dependencies)
                {
                    matches.register(id);
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: matches.dependencies.as_slice().into(),
                        rules: share.rules.clone(),
                        pseudos: share.pseudos.clone(),
                    });
                    (share.rules.clone(), share.pseudos.clone())
                } else {
                    matches.dependencies.clear();
                    // Candidates come from the node's classes, so its matches always read them.
                    matches.depend(id, 0);
                    matches.depend(id, 7);
                    candidates.clear();
                    candidates.extend(&scoped.universal);
                    let classes = node.attrs.get("class").into_iter();
                    for (kind, name) in std::iter::once((2, node.tag.as_str()))
                        .chain(node.attrs.get("id").map(|id| (0, id.as_str())))
                        .chain(
                            classes
                                .flat_map(|c| c.split_ascii_whitespace().map(|class| (1, class))),
                        )
                        .chain(node.attrs.keys().map(|name| (3, name.as_str())))
                    {
                        if let Some(list) = scoped.buckets[kind].get(name) {
                            candidates.extend(list);
                        }
                    }
                    let fresh = scoped.nodes[id].is_none();
                    let ancestors = matches.ancestors(doc, id);
                    let mut read = 0u16;
                    let mut test = |selector: &Selector, matches: &mut SelectorCache| {
                        if let Some(ancestry) = selector.ancestry()
                            && ancestry
                                .0
                                .iter()
                                .zip(&ancestors)
                                .any(|(need, have)| need & !have != 0)
                        {
                            matches.reject(doc, id, ancestry, ancestry.2 & !read);
                            read |= ancestry.2;
                            return false;
                        }
                        selector.always()
                            || !selector.host_only()
                                && if fresh {
                                    selector.matches_fresh(doc, id, matches)
                                } else {
                                    selector.matches_cached(doc, id, None, matches)
                                }
                    };
                    let (mut matched, mut matched_pseudos) = (Vec::new(), Vec::new());
                    for &entry in candidates.iter() {
                        if entry >= PSEUDO {
                            let index = entry & !PSEUDO;
                            if test(&scoped.pseudos[index].1, matches) {
                                matched_pseudos.push(index);
                            }
                            continue;
                        }
                        let (order, index) = (entry >> SELECTOR, entry & ((1 << SELECTOR) - 1));
                        let selector = &rules[order].0[index];
                        if test(selector, matches) {
                            matched.push((order, selector.specificity));
                        }
                    }
                    matched.sort_unstable();
                    matched.dedup_by(|later, earlier| {
                        earlier.0 == later.0 && {
                            earlier.1 = earlier.1.max(later.1);
                            true
                        }
                    });
                    matched_pseudos.sort_unstable();
                    matched_pseudos.dedup();
                    // Cells take `cellpadding` from their table.
                    if matches!(node.tag.as_str(), "td" | "th") {
                        let mut at = node.parent;
                        for _ in 0..3 {
                            let Some(parent) = at else { break };
                            matches.depend(parent, 0);
                            matches.depend(parent, 1);
                            at = doc.nodes[parent].parent;
                        }
                    }
                    let style = matches.reads(id, 6);
                    matches.depend(id, 6);
                    matches.dependencies.sort_unstable();
                    matches.dependencies.dedup();
                    matches.register(id);
                    let shared = intern(&mut scoped.lists, &matched);
                    let matched_pseudos = if matched_pseudos.is_empty() {
                        no_pseudos.clone()
                    } else {
                        Rc::from(matched_pseudos)
                    };
                    if let Some(group) = group
                        && matches.shareable(doc, id)
                    {
                        shares.insert(
                            group,
                            Share {
                                from: id,
                                rules: shared.clone(),
                                pseudos: matched_pseudos.clone(),
                                dependencies: matches.dependencies.as_slice().into(),
                                style: style.then(|| matches.signature(id)[6]),
                            },
                        );
                    }
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: matches.dependencies.as_slice().into(),
                        rules: shared.clone(),
                        pseudos: matched_pseudos.clone(),
                    });
                    (shared, matched_pseudos)
                };
                let mut applies = |order: usize, passed: bool| {
                    if scoped.medias[order]
                        .as_ref()
                        .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                    {
                        return false;
                    }
                    let Some(conditions) = scoped.containers[order].as_ref().filter(|_| !passed)
                    else {
                        return true;
                    };
                    contained = true;
                    query_containers(
                        conditions,
                        seen.as_deref(),
                        (scope, order),
                        &style_cache.containers,
                        |container| lengths[container],
                        &mut style_cache.queries,
                    )
                };
                let conditional = |order: usize| {
                    scoped.medias[order].is_some() || scoped.containers[order].is_some()
                };
                let matched = match matched.iter().any(|&(order, _)| conditional(order)) {
                    true => {
                        let kept: Vec<_> = matched
                            .iter()
                            .copied()
                            .filter(|&(order, _)| applies(order, false))
                            .collect();
                        intern(&mut scoped.lists, &kept)
                    }
                    false => matched,
                };
                let matched_pseudos = match matched_pseudos
                    .iter()
                    .any(|&index| conditional(scoped.pseudos[index].2))
                {
                    true => matched_pseudos
                        .iter()
                        .copied()
                        .filter(|&index| {
                            let (kind, _, order, _) = &scoped.pseudos[index];
                            applies(*order, matches!(*kind, SLOTTED | PART))
                        })
                        .collect(),
                    false => matched_pseudos,
                };
                let graphic = matches!(
                    node.tag.as_str(),
                    "g" | "path"
                        | "rect"
                        | "circle"
                        | "ellipse"
                        | "line"
                        | "polyline"
                        | "polygon"
                        | "text"
                        | "use"
                        | "image"
                        | "foreignObject"
                        | "switch"
                );
                let mut hints = Vec::new();
                for (name, value) in node.attrs.iter() {
                    if graphic
                        && name == "transform"
                        && let Some(value) = svg_transform(value)
                    {
                        hints.push((Prop::TRANSFORM, value.into()));
                    } else if node.tag == "svg" && matches!(name.as_ref(), "width" | "height") {
                        // As in Chrome, these are CSS lengths like `1em` or `50%`, below author styles.
                        let value = value.trim();
                        let digits = value
                            .find(|c: char| !c.is_ascii_digit() && c != '.')
                            .unwrap_or(value.len());
                        let unit = value[digits..].to_ascii_lowercase();
                        if value[..digits].parse::<f32>().is_ok()
                            && matches!(
                                unit.as_str(),
                                "" | "%"
                                    | "px"
                                    | "em"
                                    | "rem"
                                    | "ex"
                                    | "ch"
                                    | "vw"
                                    | "vh"
                                    | "vmin"
                                    | "vmax"
                                    | "cm"
                                    | "mm"
                                    | "q"
                                    | "in"
                                    | "pt"
                                    | "pc"
                            )
                        {
                            let prop = if name == "width" {
                                Prop::WIDTH
                            } else {
                                Prop::HEIGHT
                            };
                            hints.push((
                                prop,
                                if unit.is_empty() {
                                    format!("{value}px").into()
                                } else {
                                    value.into()
                                },
                            ));
                        }
                    } else if let Some(prop) = presentation(name) {
                        let value = value.trim();
                        let pixels = matches!(
                            prop,
                            Prop::STROKE_WIDTH
                                | Prop::STROKE_DASHOFFSET
                                | Prop::FONT_SIZE
                                | Prop::LETTER_SPACING
                                | Prop::WORD_SPACING
                        ) && value.parse::<f32>().is_ok();
                        hints.push((
                            prop,
                            if pixels {
                                format!("{value}px").into()
                            } else {
                                value.into()
                            },
                        ));
                    }
                }
                if (graphic || !hints.is_empty()) && node.tag != "svg" {
                    let mut up = node.parent;
                    while let Some(parent) = up.filter(|&parent| {
                        !matches!(doc.nodes[parent].tag.as_str(), "svg" | "foreignObject")
                    }) {
                        up = doc.nodes[parent].parent;
                    }
                    if up.is_none_or(|parent| doc.nodes[parent].tag != "svg") {
                        hints.clear();
                    } else if graphic {
                        hints.push((Prop::TRANSFORM_ORIGIN, "0 0".into()));
                    }
                }
                // HTML table attributes, as presentation hints.
                let tag = node.tag.as_str();
                if matches!(
                    tag,
                    "table" | "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "col" | "colgroup"
                ) {
                    let length = |value: &str| -> Option<Rc<str>> {
                        let value = value.trim();
                        let (number, percent) = match value.strip_suffix('%') {
                            Some(number) => (number, true),
                            None => (value.strip_suffix("px").unwrap_or(value), false),
                        };
                        let number = number.trim().parse::<f32>().ok().filter(|v| *v >= 0.)?;
                        Some(format!("{number}{}", if percent { "%" } else { "px" }).into())
                    };
                    let cell = matches!(tag, "td" | "th");
                    let table = if cell {
                        std::iter::successors(node.parent, |&at| doc.nodes[at].parent)
                            .take(3)
                            .find(|&at| doc.nodes[at].tag == "table")
                    } else {
                        None
                    };
                    let padding = table
                        .and_then(|table| length(doc.nodes[table].attrs.get("cellpadding")?))
                        .filter(|value| !value.ends_with('%'));
                    let attrs = node
                        .attrs
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.trim()));
                    for (name, value) in
                        attrs.chain(padding.as_deref().map(|value| ("cellpadding", value)))
                    {
                        let sides = |props: [Prop; 4], value: Rc<str>| {
                            props.map(|prop| (prop, value.clone()))
                        };
                        let found: Vec<(Prop, Rc<str>)> = match (name, tag) {
                            ("width", "table" | "td" | "th" | "col" | "colgroup") => length(value)
                                .map(|value| (Prop::WIDTH, value))
                                .into_iter()
                                .collect(),
                            ("height", "table" | "td" | "th" | "tr") => length(value)
                                .map(|value| (Prop::HEIGHT, value))
                                .into_iter()
                                .collect(),
                            ("bgcolor", _) => vec![(Prop::BACKGROUND_COLOR, value.into())],
                            ("nowrap", "td" | "th") => vec![(Prop::WHITE_SPACE, "nowrap".into())],
                            ("align", "table") if value.eq_ignore_ascii_case("center") => vec![
                                (Prop::MARGIN_LEFT, "auto".into()),
                                (Prop::MARGIN_RIGHT, "auto".into()),
                            ],
                            ("align", "table") => Vec::new(),
                            ("align", _) => ["left", "right", "center", "justify"]
                                .into_iter()
                                .find(|side| value.eq_ignore_ascii_case(side))
                                .map(|side| (Prop::TEXT_ALIGN, side.into()))
                                .into_iter()
                                .collect(),
                            ("valign", _) => ["top", "middle", "bottom", "baseline"]
                                .into_iter()
                                .find(|side| value.eq_ignore_ascii_case(side))
                                .map(|side| (Prop::VERTICAL_ALIGN, side.into()))
                                .into_iter()
                                .collect(),
                            ("cellspacing", "table") => length(value)
                                .filter(|value| !value.ends_with('%'))
                                .map(|value| (Prop::BORDER_SPACING, value))
                                .into_iter()
                                .collect(),
                            ("border", "table") => sides(
                                [
                                    Prop::BORDER_TOP_WIDTH,
                                    Prop::BORDER_RIGHT_WIDTH,
                                    Prop::BORDER_BOTTOM_WIDTH,
                                    Prop::BORDER_LEFT_WIDTH,
                                ],
                                length(value)
                                    .filter(|value| !value.ends_with('%'))
                                    .unwrap_or("1px".into()),
                            )
                            .into(),
                            ("cellpadding", _) if cell => sides(
                                [
                                    Prop::PADDING_TOP,
                                    Prop::PADDING_RIGHT,
                                    Prop::PADDING_BOTTOM,
                                    Prop::PADDING_LEFT,
                                ],
                                value.into(),
                            )
                            .into(),
                            _ => Vec::new(),
                        };
                        hints.extend(found);
                    }
                }
                if node.state & (crate::dom::TOP_LAYER | crate::dom::LEAVING) != 0
                    && node.tag != "html"
                {
                    let auto = node.state & crate::dom::TOP_LAYER != 0;
                    hints.push((Prop::OVERLAY, if auto { "auto" } else { "none" }.into()));
                }
                passed = parent_style.map(|parent| {
                    if unshared {
                        return StyleRef(parent.clone());
                    }
                    let (_, passed) = style_cache
                        .heritage
                        .entry(Rc::as_ptr(parent))
                        .or_insert_with(|| {
                            let mut passed: Style = parent
                                .own()
                                .filter(|(prop, _)| prop.inherited())
                                .map(|(prop, value)| (prop, value.clone()))
                                .collect();
                            // Children of table parts take their roles from the parent's role.
                            if parent_role != 0 {
                                passed.insert(Prop::DISPLAY, TABLE_DISPLAYS[parent_role as usize]);
                            } else if item {
                                passed.insert(Prop::DISPLAY, "flex");
                            }
                            let passed = match style_cache.inherited.get(&passed) {
                                Some(old) => old.clone(),
                                None => {
                                    let passed = Rc::new(passed);
                                    style_cache.inherited.insert(passed.clone());
                                    passed
                                }
                            };
                            (parent.clone(), passed)
                        });
                    StyleRef(passed.clone())
                });
                let source = node
                    .attrs
                    .get("style")
                    .map(|source| Cow::Borrowed(source.as_str()))
                    .or_else(|| {
                        node.inline
                            .as_ref()
                            .map(|inline| Cow::Owned(inline.source()))
                    });
                let mut resolved = StyleKey {
                    scope,
                    tag: match style_cache.tags.get(node.tag.as_str()) {
                        Some(&tag) => tag,
                        None => {
                            let tag = style_cache.tags.len() as u32;
                            style_cache.tags.insert(node.tag.clone(), tag);
                            tag
                        }
                    },
                    inline: source.as_deref().map(|source| {
                        std::hash::BuildHasher::hash_one(&rustc_hash::FxBuildHasher, source)
                    }),
                    hints,
                    inputs: Inputs {
                        parent: passed.clone().map(|passed| (passed, false)),
                        ..inputs
                    },
                    rules: StyleRef(matched),
                    pseudos: matched_pseudos,
                    shadow: shadow.clone(),
                    selection: selection.cloned().map(StyleRef),
                };
                let mut values = style_cache.values.get(&resolved);
                if values.is_none()
                    && let Some(parent) = parent_style
                {
                    resolved.inputs.parent = Some((StyleRef(parent.clone()), true));
                    values = style_cache.values.get(&resolved);
                }
                let value = values.filter(|_| animated.is_none()).and_then(|values| {
                    values.iter().rev().find_map(|value| {
                        if value.inline.as_deref() != source.as_deref() {
                            return None;
                        }
                        let (vars, changed) = {
                            let held = value.vars.borrow();
                            reuse(value, held.0.as_ref(), &held.1)?
                        };
                        if changed {
                            *value.vars.borrow_mut() = (parent_vars.cloned(), vars.clone());
                        }
                        Some((value.clone(), vars))
                    })
                });
                key = Some(resolved);
                written = source;
                value
            });
        if let Some((cached, vars)) = cached {
            cached.used.set(style_cache.pass);
            if let Some(node) = scoped.nodes[id].as_mut() {
                if let Some(key) = &key {
                    node.cached = Some(CachedNode::new(
                        key,
                        parent_style,
                        cached.clone(),
                        parent_vars,
                        &vars,
                        contained.then(|| seen.clone()),
                    ));
                } else if let Some(node) = node.cached.as_mut() {
                    if let Some(parent) = parent_style {
                        node.parent = Some(StyleRef(parent.clone()));
                    }
                    if moved {
                        node.inherited = parent_vars.cloned();
                        node.vars = vars.clone();
                        *cached.vars.borrow_mut() = (parent_vars.cloned(), vars.clone());
                    }
                }
            }
            styles.set(id, cached.style.clone(), Rc::ptr_eq);
            custom.set(id, vars, Rc::ptr_eq);
            style_flags.set(id, cached.flags, u16::eq);
            lengths.set(
                id,
                Lengths {
                    viewport: [width, height],
                    ..cached.lengths
                },
            );
            if id == 0 || node.tag == "html" {
                root = cached.lengths.font;
            }
            if let Some(values) = &cached.pseudos {
                pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.text_pseudos {
                text_pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.backdrop {
                backdrops.insert(id, values.clone());
            }
            if let Some(values) = &cached.backdrop_start {
                backdrop_starts.insert(id, values.clone());
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
            if let Some(values) = &cached.important {
                important.insert(id, values.clone());
            }
            if let Some(values) = &cached.starting {
                starts.insert(id, values.clone());
            }
            for (values, list) in cached.shadowed.iter().zip([&mut *slotted, &mut *parts]) {
                if let Some(values) = values {
                    list.insert(id, values.clone());
                }
            }
            if let Some(text) = text {
                let outputs = (
                    styles[id].clone(),
                    custom[id].clone(),
                    style_flags[id],
                    lengths[id],
                    selections.get(&id).cloned(),
                );
                texts.insert(text, outputs);
            }
            settle!(id, up, seen);
            continue;
        }
        let mut key = key.unwrap();
        let mut style = match &theme {
            Some(theme) if id == 0 => (**theme).clone(),
            _ => Style::new(),
        };
        if let Some(parent) = parent_style {
            style.extend(
                parent
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone())),
            );
        }
        if let Some(vars) = parent_vars {
            style.set_vars(inherit(vars));
        }
        let mut cascade: BBTreeMap<Prop, (CascadeRank, Rc<str>)> = BBTreeMap::new();
        for &(order, specificity) in key.rules.0.iter() {
            let (_, declarations, layer) = &rules[order];
            for &(prop, ref value, important, position) in declarations.iter() {
                let rank = CascadeRank::rule(*layer, important, specificity, keys[order], position);
                put(&mut cascade, prop, rank, value.clone());
            }
        }
        for (prop, value) in &key.hints {
            let rank = CascadeRank::new(HINT, false, 0, [0; 3], 0);
            put(&mut cascade, *prop, rank, value.clone());
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (prop, value, important, position) in inline.resolved(true) {
                let rank = CascadeRank {
                    inline: true,
                    position,
                    ..CascadeRank::new(AUTHOR, important, UNLAYERED, [0; 3], u128::MAX)
                };
                put(&mut cascade, prop, rank, value);
            }
        }
        let ranked = |rule: &ShadowRule, important: bool, position: u16| CascadeRank {
            context: if important {
                -rule.context
            } else {
                rule.context
            },
            ..CascadeRank::rule(
                rule.layer,
                important,
                rule.specificity,
                rule.order,
                position,
            )
        };
        let foreign = shadow
            .iter()
            .flat_map(|rules| rules.iter())
            .filter(|rule| rule.kind == ELEMENT)
            .flat_map(|rule| {
                rule.declarations
                    .iter()
                    .map(move |(prop, value, important, position)| {
                        (*prop, value.clone(), ranked(rule, *important, *position))
                    })
            })
            .chain(
                animated
                    .into_iter()
                    .flat_map(Style::iter)
                    .map(|(prop, value)| {
                        (
                            prop,
                            value.clone(),
                            CascadeRank::new(ANIMATION, false, 0, [0; 3], 0),
                        )
                    }),
            );
        for (prop, value, rank) in foreign {
            put(&mut cascade, prop, rank, value);
        }
        if node.tag == "html"
            && let Some(scheme) = &style_cache.environment.scheme
        {
            cascade
                .entry(Prop::COLOR_SCHEME)
                .or_insert((CascadeRank::default(), scheme.clone()));
        }
        // Remove longhands reset by a stronger shorthand: longhands otherwise win during later expansion.
        for (shorthand, longhands) in [
            (
                Prop::TRANSITION,
                &[
                    Prop::TRANSITION_PROPERTY,
                    Prop::TRANSITION_DURATION,
                    Prop::TRANSITION_TIMING_FUNCTION,
                    Prop::TRANSITION_DELAY,
                    Prop::TRANSITION_BEHAVIOR,
                ][..],
            ),
            (
                Prop::ANIMATION,
                &[
                    Prop::ANIMATION_NAME,
                    Prop::ANIMATION_DURATION,
                    Prop::ANIMATION_TIMING_FUNCTION,
                    Prop::ANIMATION_DELAY,
                    Prop::ANIMATION_ITERATION_COUNT,
                    Prop::ANIMATION_DIRECTION,
                    Prop::ANIMATION_FILL_MODE,
                    Prop::ANIMATION_PLAY_STATE,
                ][..],
            ),
        ] {
            if let Some(&(rank, _)) = cascade.get(&shorthand) {
                for longhand in longhands {
                    if cascade.get(longhand).is_some_and(|(old, _)| *old < rank) {
                        cascade.remove(longhand);
                    }
                }
            }
        }
        if let Some((rank, value)) = cascade.remove(&Prop::ALL) {
            for (prop, _) in style.own() {
                cascade.entry(prop).or_insert((rank, value.clone()));
            }
            for (prop, entry) in &mut cascade {
                if entry.0 < rank
                    && !prop.is_custom()
                    && !matches!(*prop, Prop::DIRECTION | Prop::UNICODE_BIDI)
                {
                    *entry = (rank, value.clone());
                }
            }
        }
        for (&prop, (rank, value)) in cascade
            .iter_mut()
            .filter(|(_, (_, value))| matches!(&**value, "revert" | "revert-layer"))
        {
            let (revert, (origin, layer)) = (&**value == "revert", rank.source());
            let mut best: Option<(CascadeRank, &Rc<str>)> = None;
            for &(order, specificity) in key.rules.0.iter() {
                let (_, declarations, rule_layer) = &rules[order];
                let from = rule_layer.origin.min(HINT);
                if from > origin
                    || from == origin && (revert || !rank.inline && rule_layer.rank == layer)
                {
                    continue;
                }
                for (_, candidate, important, position) in declarations
                    .iter()
                    .filter(|(name, ..)| *name == prop || *name == Prop::ALL && !prop.is_custom())
                {
                    let candidate_rank = CascadeRank::rule(
                        *rule_layer,
                        *important,
                        specificity,
                        keys[order],
                        *position,
                    );
                    if !matches!(&**candidate, "revert" | "revert-layer")
                        && best.as_ref().is_none_or(|(old, _)| candidate_rank >= *old)
                    {
                        best = Some((candidate_rank, candidate));
                    }
                }
            }
            *value = best.map_or_else(|| "unset".into(), |(_, value)| value.clone());
        }
        if cascade
            .get(&Prop::OVERLAY)
            .is_some_and(|(rank, _)| !matches!(rank.level, HINT | ANIMATION))
        {
            cascade.remove(&Prop::OVERLAY);
            if let Some((_, value)) = key.hints.iter().find(|(prop, _)| *prop == Prop::OVERLAY) {
                cascade.insert(Prop::OVERLAY, (CascadeRank::default(), value.clone()));
            }
        }
        const DECORATION: [Prop; 4] = [
            Prop::TEXT_DECORATION_LINE,
            Prop::TEXT_DECORATION_STYLE,
            Prop::TEXT_DECORATION_COLOR,
            Prop::TEXT_DECORATION_THICKNESS,
        ];
        let mut decorating = false;
        if !node.tag.starts_with('#') {
            let declared = |prop: Prop| cascade.get(&prop).map(|(_, value)| &**value);
            if declared(Prop::DISPLAY).is_some_and(|value| value.starts_with("inline-"))
                || declared(Prop::FLOAT).is_some_and(|value| value != "none")
                || declared(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            {
                for prop in DECORATION {
                    style.remove(prop);
                }
            }
            match cascade
                .get(&Prop::TEXT_DECORATION_LINE)
                .map(|(_, value)| value.clone())
                .as_deref()
            {
                Some("none") if style.contains(Prop::TEXT_DECORATION_LINE) => {
                    for prop in DECORATION {
                        cascade.remove(&prop);
                    }
                }
                Some(own) if !matches!(own, "none" | "inherit" | "unset" | "initial") => {
                    decorating = true;
                    if let Some(inherited) = style
                        .get(Prop::TEXT_DECORATION_LINE)
                        .filter(|value| *value != "none")
                    {
                        let mut lines: Vec<&str> = inherited.split_ascii_whitespace().collect();
                        for line in own.split_ascii_whitespace() {
                            if !lines.contains(&line) {
                                lines.push(line);
                            }
                        }
                        let merged: Rc<str> = lines.join(" ").into();
                        if let Some((_, value)) = cascade.get_mut(&Prop::TEXT_DECORATION_LINE) {
                            *value = merged;
                        }
                    }
                }
                _ => {}
            }
        }
        physical(&mut cascade, &style);
        let explicit_size = cascade.contains_key(&Prop::FONT_SIZE);
        let mut reads = Some(Vec::new());
        let mut missing = Vec::new();
        let mut owned = Vec::new();
        // An invalid value of a registered inheriting property takes the parent's value.
        let mut fallback = Vec::new();
        for (prop, (_, value)) in &cascade {
            if prop.is_custom() {
                if matches!(
                    &**value,
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer"
                ) {
                    reads = None;
                }
                owned.push(*prop);
                if registry.get(prop).is_some_and(|entry| entry.1) {
                    fallback.push(*prop);
                }
            }
            references(value, &mut reads, &mut missing);
        }
        let agent_display = cascade
            .get(&Prop::DISPLAY)
            .is_some_and(|(rank, _)| rank.level == AGENT);
        let mut declared = Vec::new();
        let mut strong = Vec::new();
        style.extend(cascade.into_iter().filter_map(|(prop, (rank, value))| {
            if rank.important() {
                strong.push(prop);
            }
            if prop.is_custom() {
                declared.push((prop, value));
                return None;
            }
            Some((prop, value))
        }));
        resolve_style(
            &mut style,
            parent_style.map(Rc::as_ref),
            parent_vars,
            &declared,
            &registry,
        );
        let parent_units = up.map(|up| lengths[up]);
        let mut keyword = resolve_keywords(
            &mut style,
            parent_style.map(Rc::as_ref),
            parent_units.as_ref(),
        );
        if decorating
            && style
                .get(Prop::TEXT_DECORATION_COLOR)
                .is_none_or(|value| value.eq_ignore_ascii_case("currentcolor"))
            && let Some(ink) = style.value(Prop::COLOR).cloned()
        {
            style.insert(Prop::TEXT_DECORATION_COLOR, ink);
        }
        if style.get(Prop::TEXT_ALIGN) == Some("-internal-center") {
            let inherited = parent_style
                .and_then(|parent| parent.get(Prop::TEXT_ALIGN))
                .filter(|value| !matches!(*value, "start" | "-internal-center"))
                .unwrap_or("center")
                .to_owned();
            style.insert(Prop::TEXT_ALIGN, inherited);
        }
        if style
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "-webkit-box" | "-webkit-inline-box"))
            && style
                .get(Prop::WEBKIT_BOX_ORIENT)
                .is_some_and(|value| value == "vertical")
            && style
                .get(Prop::WEBKIT_LINE_CLAMP)
                .is_some_and(|value| value != "none")
        {
            let display = if style.get(Prop::DISPLAY) == Some("-webkit-box") {
                "flow-root"
            } else {
                "inline-block"
            };
            style.insert(Prop::DISPLAY, display);
        }
        let mut role = 0;
        if !node.tag.starts_with('#') {
            let root = node.parent == Some(0);
            if root && style.get(Prop::DISPLAY) == Some("contents") {
                style.insert(Prop::DISPLAY, "block");
            }
            blockify(&mut style, item || root, table_item);
            let grouped = table_box(parent_role) || matches!(parent_role, 5..=7);
            let display;
            (role, display) = match style.get(Prop::DISPLAY).unwrap_or("inline") {
                "table" => (1, "grid"),
                "inline-table" => (2, "inline-grid"),
                // A `<table>` box holds the anonymous table of its rows.
                "block" if node.tag == "table" => (3, "grid"),
                "table-row" if grouped => (ROW, "contents"),
                "table-row-group" if table_box(parent_role) => (5, "contents"),
                "table-header-group" if table_box(parent_role) => (HEADER, "contents"),
                "table-footer-group" if table_box(parent_role) => (FOOTER, "contents"),
                "table-row" => (10, "grid"),
                "table-row-group" | "table-footer-group" => (11, "grid"),
                "table-header-group" => (12, "grid"),
                "table-cell" => (CELL, "block"),
                "table-caption" => (CAPTION, "block"),
                _ => (0, ""),
            };
            if role != 0 {
                style.insert(Prop::DISPLAY, display);
            }
            // Like a table, the grid is as wide as its content when its width is `auto`.
            if table_box(role) && style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                style.insert(Prop::WIDTH, "fit-content");
            }
        }

        if table_box(role) {
            border_spacing(&mut style);
        }
        let parent_weight = parent_style
            .and_then(|parent| parent.get(Prop::FONT_WEIGHT))
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(400.);
        if let Some(weight) = style.get(Prop::FONT_WEIGHT) {
            let value = match weight {
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
            style.insert(Prop::FONT_WEIGHT, value.to_string());
        }
        if let Some(family) = context.and_then(|context| {
            context
                .aliases
                .get(style.get(Prop::FONT_FAMILY)?.trim_matches(['\'', '"']))
        }) {
            style.insert(Prop::FONT_FAMILY, family.as_str());
        }
        let mut units = Lengths {
            font: parent,
            root,
            viewport: [width, height],
            container: None,
        };
        let queried = [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains("cq")));
        if queried && let Some(container) = &seen {
            style_cache.sized.insert(container.id);
            units.container = style_cache.containers.get(&container.id).copied();
        }
        let viewport = queried && units.container.is_none()
            || [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
                .into_iter()
                .any(|prop| {
                    style.get(prop).is_some_and(|value| {
                        ["vw", "vh", "vi", "vb", "vmin", "vmax"]
                            .iter()
                            .any(|unit| value.contains(unit))
                    })
                });
        let size = match style.get(Prop::FONT_SIZE).unwrap_or("medium") {
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
        // Glyph rasterization allocates by font size, so huge sizes abort the process.
        .map(|size| size.min(10_000.))
        .unwrap_or(parent.em);
        let monospace = |style: Option<&Style>| {
            style
                .and_then(|style| style.get(Prop::FONT_FAMILY))
                .is_some_and(|family| family.trim().eq_ignore_ascii_case("monospace"))
        };
        let absolute = match style.get(Prop::FONT_SIZE).filter(|_| explicit_size) {
            None => parent.absolute,
            Some(value)
                if value.starts_with("calc(")
                    || matches!(value, "larger" | "smaller")
                    || ["%", "em", "ex", "ch", "cap", "ic", "lh"]
                        .iter()
                        .any(|unit| value.ends_with(unit)) =>
            {
                parent.absolute
            }
            Some(value) => value.starts_with(|c: char| c.is_ascii_digit() || c == '.'),
        };
        let size = if !explicit_size
            && !parent.absolute
            && monospace(Some(&style))
            && !monospace(parent_style.map(Rc::as_ref))
        {
            size * 13. / 16.
        } else {
            size
        };
        units.font = FontUnits::new(size);
        units.font.absolute = absolute;
        if let Some(text) = text_system {
            let font = font(&style);
            let (id, metrics) = font_metrics.entry(font.clone()).or_insert_with(|| {
                let id = text.face(&font);
                let size = renderer::px(1.);
                (
                    id,
                    [
                        text.advance(id, size, '0').map(f32::from).unwrap_or(0.5),
                        text.metrics(id).x_height * size.0,
                        text.metrics(id).cap_height * size.0,
                        text.advance(id, size, '水').map(f32::from).unwrap_or(1.),
                    ],
                )
            });
            let [ch, ex, cap, ic] = metrics.map(|value| value * size);
            units.font = FontUnits {
                em: size,
                absolute,
                ch,
                ex,
                cap,
                ic,
                lh: text
                    .line_metrics(*id, renderer::px(size * scale))
                    .into_iter()
                    .map(f32::from)
                    .sum::<f32>()
                    / scale,
            };
        }
        if let Some(value) = style.get(Prop::LINE_HEIGHT) {
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
                    Prop::LINE_HEIGHT,
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
        style.insert(Prop::FONT_SIZE, format!("{size}px"));
        // Children inherit these lengths, not their units.
        for prop in [
            Prop::LETTER_SPACING,
            Prop::WORD_SPACING,
            Prop::TEXT_INDENT,
            Prop::TEXT_UNDERLINE_OFFSET,
            Prop::WEBKIT_TEXT_STROKE_WIDTH,
            Prop::TEXT_SHADOW,
        ] {
            if let Some(value) = style.get(prop).and_then(|value| pixels(value, &units)) {
                style.insert(prop, value);
            }
        }
        let mut pseudo_cascade: [BBTreeMap<Prop, (CascadeRank, &Rc<str>)>; 17] = Default::default();
        let mut groups: [Vec<Passed>; 2] = Default::default();
        let own = Container::of(id, &style, seen.clone());
        let mut deferred = false;
        let mut query = |conditions: &ContainerQuery, key: (usize, usize)| {
            deferred = true;
            query_containers(
                conditions,
                own.as_ref(),
                key,
                &style_cache.containers,
                |container| {
                    if container == id {
                        units
                    } else {
                        lengths[container]
                    }
                },
                &mut style_cache.queries,
            )
        };
        let again: Option<Vec<usize>> = own.is_some().then(|| {
            scoped.nodes[id]
                .iter()
                .flat_map(|matched| matched.pseudos.iter().copied())
                .filter(|&index| {
                    let (kind, _, order, _) = &scoped.pseudos[index];
                    match &scoped.containers[*order] {
                        Some(conditions) if !matches!(kind, SLOTTED..=STARTING) => {
                            !scoped.medias[*order]
                                .as_ref()
                                .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                                && query(conditions, (scope, *order))
                        }
                        _ => key.pseudos.binary_search(&index).is_ok(),
                    }
                })
                .collect()
        });
        for &index in again.as_deref().unwrap_or(&key.pseudos) {
            let (pseudo, selector, order, _) = &scoped.pseudos[index];
            let (_, declarations, layer) = &rules[*order];
            if matches!(*pseudo, SLOTTED | PART) {
                if let Some((name, compound, target)) = scoped.arguments.get(&index) {
                    groups[usize::from(*pseudo == PART)].push(Passed {
                        name: name.clone(),
                        compound: compound.clone(),
                        conditions: scoped.containers[*order]
                            .clone()
                            .map(|conditions| (scope, *order, conditions)),
                        rule: ShadowRule {
                            kind: *target,
                            context: if *pseudo == PART { 1 } else { -1 },
                            layer: *layer,
                            specificity: selector.specificity,
                            order: keys[*order],
                            declarations: declarations.clone(),
                            query: None,
                        },
                    });
                }
                continue;
            }
            for &(prop, ref value, important, position) in declarations.iter() {
                let rank = CascadeRank::rule(
                    *layer,
                    important,
                    selector.specificity,
                    keys[*order],
                    position,
                );
                put(&mut pseudo_cascade[*pseudo], prop, rank, value);
            }
        }
        for rule in shadow
            .iter()
            .flat_map(|rules| rules.iter())
            .filter(|rule| rule.kind != ELEMENT)
        {
            if let Some((scope, order, applies)) = rule.query
                && !match own {
                    Some(_) => waiting
                        .iter()
                        .find(|waits| (waits.0, waits.1) == (scope, order))
                        .is_some_and(|waits| query(&waits.2, (scope, order))),
                    None => applies,
                }
            {
                continue;
            }
            for (prop, value, important, position) in rule.declarations.iter() {
                let rank = ranked(rule, *important, *position);
                put(&mut pseudo_cascade[rule.kind], *prop, rank, value);
            }
        }
        for (prop, (_, value)) in pseudo_cascade.iter().flatten() {
            if prop.is_custom() {
                reads = None;
            }
            references(value, &mut reads, &mut missing);
        }
        let keep = [
            Prop::ANIMATION,
            Prop::ANIMATION_NAME,
            Prop::TRANSITION,
            Prop::TRANSITION_PROPERTY,
        ]
        .into_iter()
        .any(|prop| {
            style.contains(prop)
                || pseudo_cascade[..2]
                    .iter()
                    .any(|cascade| cascade.contains_key(&prop))
        });
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
            _,
            _,
            starting,
            backdrop,
            backdrop_starting,
        ] = pseudo_cascade;
        let starting = (!starting.is_empty()).then(|| {
            let mut values = style.clone();
            let mut declared = Vec::new();
            values.extend(starting.into_iter().filter_map(|(prop, (_, value))| {
                if prop.is_custom() {
                    declared.push((prop, value.clone()));
                    return None;
                }
                Some((prop, value.clone()))
            }));
            resolve_style(
                &mut values,
                parent_style.map(Rc::as_ref),
                parent_vars,
                &declared,
                &registry,
            );
            keyword |= resolve_keywords(
                &mut values,
                parent_style.map(Rc::as_ref),
                parent_units.as_ref(),
            );
            if !keep && declared.is_empty() {
                values.set_vars(empty.clone());
            }
            let values = Rc::new(values);
            starts.insert(id, values.clone());
            values
        });
        // A pseudo-element inherits custom properties from its element, like a child.
        let handed = inherit(style.vars());
        let derive = |mut cascade: BBTreeMap<Prop, (CascadeRank, &Rc<str>)>| {
            (!cascade.is_empty()).then(|| {
                physical(&mut cascade, &style);
                let mut values: Style = style
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone()))
                    .collect();
                values.set_vars(handed.clone());
                let mut declared = Vec::new();
                values.extend(cascade.into_iter().filter_map(|(prop, (_, value))| {
                    if prop.is_custom() {
                        declared.push((prop, value.clone()));
                        return None;
                    }
                    Some((prop, value.clone()))
                }));
                resolve_style(&mut values, Some(&style), None, &declared, &registry);
                resolve_keywords(&mut values, Some(&style), Some(&units));
                if !keep && declared.is_empty() {
                    values.set_vars(empty.clone());
                }
                values
            })
        };
        let layered =
            node.state & (crate::dom::TOP_LAYER | crate::dom::LEAVING) != 0 && node.tag != "html";
        if layered && !backdrop_starting.is_empty() {
            let mut first = backdrop.clone();
            for (prop, (rank, value)) in backdrop_starting {
                put(&mut first, prop, rank, value);
            }
            if let Some(values) = derive(first) {
                backdrop_starts.insert(id, Rc::new(values));
            }
        }
        if layered && let Some(values) = derive(backdrop) {
            backdrops.insert(id, Rc::new(values));
        }
        if [&marker, &letter, &line, &placeholder]
            .iter()
            .any(|cascade| !cascade.is_empty())
        {
            text_pseudos.insert(
                id,
                Rc::new([marker, letter, line, placeholder].map(&derive)),
            );
        }
        if [&bar, &track, &thumb, &corner, &hovered]
            .iter()
            .any(|cascade| !cascade.is_empty())
            && (matches!(node.tag.as_str(), "html" | "body" | "progress")
                || node.tag == "input"
                    && ["range", "file"]
                        .iter()
                        .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
                || [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| {
                        style
                            .get(prop)
                            .is_some_and(|value| matches!(value, "auto" | "scroll"))
                    }))
        {
            if !hovered.is_empty() {
                for (prop, value) in &thumb {
                    hovered.entry(*prop).or_insert(*value);
                }
            }
            scrollbars.insert(
                id,
                Rc::new([bar, track, thumb, corner, hovered].map(&derive)),
            );
        }
        if !selected.is_empty() || up.is_some_and(|up| selections.contains_key(&up)) {
            let parent = up.and_then(|up| selections.get(&up));
            let mut values = parent.map(|parent| (**parent).clone()).unwrap_or_default();
            values.set_vars(handed.clone());
            if let Some(scheme) = style.value(Prop::COLOR_SCHEME) {
                values.insert(Prop::COLOR_SCHEME, scheme.clone());
            }
            let fallback = |prop: Prop| -> Rc<str> {
                parent
                    .and_then(|parent| parent.value(prop))
                    .cloned()
                    .unwrap_or_else(|| {
                        if prop == Prop::BACKGROUND_COLOR {
                            "transparent"
                        } else {
                            "currentColor"
                        }
                        .into()
                    })
            };
            let painted = |prop: Prop| {
                matches!(
                    prop,
                    Prop::COLOR | Prop::BACKGROUND_COLOR | Prop::WEBKIT_TEXT_FILL_COLOR
                )
            };
            for (prop, (_, value)) in selected {
                if painted(prop) {
                    values.insert(
                        prop,
                        if matches!(&**value, "inherit" | "unset") {
                            fallback(prop)
                        } else {
                            value.clone()
                        },
                    );
                }
            }
            let colored = values.contains(Prop::COLOR);
            resolve_style(
                &mut values,
                parent.map(|parent| &**parent),
                None,
                &[],
                &registry,
            );
            if !keep {
                values.set_vars(empty.clone());
            }
            if !colored {
                values.remove(Prop::COLOR);
            }
            values.retain(|prop, value| {
                if !painted(prop) {
                    return false;
                }
                if matches!(&**value, "inherit" | "unset") {
                    *value = fallback(prop);
                } else if &**value == "initial" {
                    *value = if prop == Prop::BACKGROUND_COLOR {
                        "transparent"
                    } else {
                        "currentColor"
                    }
                    .into();
                }
                if value.eq_ignore_ascii_case("currentcolor") || color(value).is_some() {
                    true
                } else if let Some(inherited) = parent.and_then(|parent| parent.value(prop)) {
                    *value = inherited.clone();
                    true
                } else {
                    false
                }
            });
            if !values.is_empty() {
                let values = match parent {
                    Some(parent) if **parent == values => parent.clone(),
                    _ => Rc::new(values),
                };
                selections.insert(id, values);
            }
        }
        if [&before, &after]
            .iter()
            .any(|values| values.contains_key(&Prop::CONTENT))
        {
            pseudos.insert(
                id,
                Rc::new([before, after].map(|cascade| {
                    let mut values = derive(cascade).unwrap_or_default();
                    if values.contains(Prop::CONTENT) {
                        let display = style.get(Prop::DISPLAY);
                        let flex = matches!(
                            display,
                            Some("flex" | "inline-flex" | "grid" | "inline-grid")
                        );
                        blockify(
                            &mut values,
                            flex || item && display == Some("contents"),
                            false,
                        );
                    }
                    values
                })),
            );
        }
        if node.tag != "slot" {
            groups[0].clear();
        }
        let [inner, outer] = groups;
        let mut store = |list: Vec<Passed>, part: bool| {
            (!list.is_empty()).then(|| {
                let list = last
                    .and_then(|last| if part { &last.parts } else { &last.slotted }.get(&id))
                    .filter(|old| ***old == list)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(list));
                if part { &mut *parts } else { &mut *slotted }.insert(id, list.clone());
                list
            })
        };
        let shadowed = [store(inner, false), store(outer, true)];
        let kept = previous.filter(|_| animated.is_some());
        if let Some(previous) = kept {
            if let Some(effects) = previous.animations.get(&id) {
                animations.insert(id, effects.clone());
            }
        } else {
            let mut effects =
                if style.contains(Prop::ANIMATION) || style.contains(Prop::ANIMATION_NAME) {
                    Animation::parse(&style, &scoped.keyframes, &registry)
                } else {
                    Vec::new()
                };
            for (slot, pseudo) in pseudos
                .get(&id)
                .into_iter()
                .flat_map(|pair| pair.iter().enumerate())
            {
                if pseudo.contains(Prop::ANIMATION) || pseudo.contains(Prop::ANIMATION_NAME) {
                    effects.extend(
                        Animation::parse(pseudo, &scoped.keyframes, &registry)
                            .into_iter()
                            .map(|mut effect| {
                                effect.pseudo = slot as u8 + 1;
                                effect.name =
                                    format!("{}\0{}", ["::before", "::after"][slot], effect.name);
                                effect
                            }),
                    );
                }
            }
            if !effects.is_empty() {
                animations.insert(id, effects.into());
            }
        }
        // Reuse equal styles from the previous frame. Pointer changes invalidate layout caches.
        let units_bits = |units: &Lengths| [units.font.bits(), units.root.bits()];
        let vars = match context
            .and_then(|context| context.previous.as_deref())
            .and_then(|previous| previous.vars.get(id))
        {
            Some(old) if !Rc::ptr_eq(old, style.vars()) && **old == **style.vars() => old.clone(),
            _ => style.vars().clone(),
        };
        let reads = reads.filter(|_| !keep).map(|mut names| {
            names.sort_unstable();
            names.dedup();
            names
                .into_iter()
                .filter(|prop| !owned.contains(prop))
                .chain(fallback)
                .map(|prop| (prop, parent_vars.and_then(|vars| var(vars, prop)).cloned()))
                .collect()
        });
        let own = owned
            .into_iter()
            .map(|prop| (prop, var(&vars, prop).cloned()))
            .collect();
        style.set_vars(if keep { vars.clone() } else { empty.clone() });
        let get = |prop| style.get(prop).unwrap_or("");
        let flag = |set, value| if set { value } else { 0 };
        let counted = [
            Prop::COUNTER_RESET,
            Prop::COUNTER_INCREMENT,
            Prop::COUNTER_SET,
        ]
        .into_iter()
        .any(|prop| style.contains(prop))
            || pseudos.get(&id).is_some_and(|pseudos| {
                pseudos.iter().any(|pseudo| {
                    pseudo
                        .get(Prop::CONTENT)
                        .is_some_and(|content| content.contains("counter"))
                        || pseudo.contains(Prop::COUNTER_INCREMENT)
                        || pseudo.contains(Prop::COUNTER_RESET)
                })
            });
        let bits = flag(get(Prop::DISPLAY) == "none", DISPLAY_NONE)
            | flag(get(Prop::DISPLAY) == "contents", DISPLAY_CONTENTS)
            | flag(
                matches!(get(Prop::VISIBILITY), "hidden" | "collapse"),
                INVISIBLE,
            )
            | match get(Prop::USER_SELECT) {
                "none" => SELECT_NONE,
                "all" => SELECT_ALL,
                "text" => SELECT_TEXT,
                _ => 0,
            }
            | flag(counted, COUNTED)
            | flag(agent_display, AGENT_DISPLAY)
            | role << 8;
        style_flags.set(id, bits, u16::eq);
        let style = match context
            .and_then(|context| context.previous.as_deref())
            .and_then(|previous| {
                Some((
                    previous,
                    previous.authored.get(&id).or(previous.values.get(id))?,
                ))
            }) {
            Some((previous, old))
                if **old == style
                    && previous.anonymous.get(&id).or(previous.flags.get(id))
                        == Some(&style_flags[id])
                    && previous.lengths.get(id).is_some_and(|old| {
                        old.container.is_none()
                            && units.container.is_none()
                            && units_bits(old) == units_bits(&units)
                    })
                    && previous.pseudos.get(&id) == pseudos.get(&id)
                    && previous.text_pseudos.get(&id) == text_pseudos.get(&id)
                    && previous.backdrops.get(&id) == backdrops.get(&id)
                    && previous.backdrop_starts.get(&id) == backdrop_starts.get(&id)
                    && previous.scrollbars.get(&id) == scrollbars.get(&id)
                    && previous.selections.get(&id) == selections.get(&id)
                    && previous.starting.get(&id) == starting.as_ref()
                    && !previous.animations.contains_key(&id)
                    && !animations.contains_key(&id)
                    // A `contents` box needs a new style when its ancestors change the items below it.
                    && style_flags[id] & DISPLAY_CONTENTS == 0 =>
            {
                old.clone()
            }
            _ => Rc::new(style),
        };
        let cached = Rc::new(CachedStyle {
            used: std::cell::Cell::new(style_cache.pass),
            inline: written.map(Into::into),
            style: style.clone(),
            vars: std::cell::RefCell::new((parent_vars.cloned(), vars.clone())),
            reads,
            own,
            missing: missing.into_boxed_slice(),
            flags: style_flags[id],
            lengths: units,
            pseudos: pseudos.get(&id).cloned(),
            text_pseudos: text_pseudos.get(&id).cloned(),
            backdrop: backdrops.get(&id).cloned(),
            backdrop_start: backdrop_starts.get(&id).cloned(),
            scrollbars: scrollbars.get(&id).cloned(),
            selection: selections.get(&id).cloned(),
            animations: animations.get(&id).cloned(),
            important: (!strong.is_empty()).then(|| {
                let values: Rc<[Prop]> = strong.into();
                important.insert(id, values.clone());
                values
            }),
            shadowed,
            starting,
            viewport,
            keyword,
        });
        if animated.is_none() {
            if let Some(node) = scoped.nodes[id].as_mut() {
                node.cached = Some(CachedNode::new(
                    &key,
                    parent_style,
                    cached.clone(),
                    parent_vars,
                    &vars,
                    (contained || queried || deferred).then(|| seen.clone()),
                ));
            }
            // The key doesn't hold the container, so styles with container units are not shared.
            if !queried && !deferred {
                if !keyword && passed.is_some() {
                    key.inputs.parent = passed.map(|passed| (passed, false));
                }
                let values = style_cache.values.entry(key).or_default();
                if values.len() == 8 {
                    values.remove(0);
                }
                values.push(cached);
            }
        }
        styles.set(id, style, Rc::ptr_eq);
        custom.set(id, vars, Rc::ptr_eq);
        lengths.set(id, units);
        settle!(id, up, seen);
    }
    style_cache.inheritable = inheritable;
    style_cache.changed = changed;
    if stored {
        if !fast {
            matches.dirty.fill(false);
        }
        style_cache.full = false;
        style_cache.styled = Some(Styled {
            nodes: walk.walk.nodes.clone(),
            serial: walk.walk.serial,
            tree: walk.tree.clone(),
            trees: walk.trees.clone(),
            environment,
            scopes: used,
        });
    }
    if counters || previous.is_some_and(|previous| fast && previous.counted) {
        *counted = true;
        let list = |value: Option<&str>, default: i64| -> Vec<(String, i64)> {
            let parts = value.map_or_else(Vec::new, |value| components(value, ' '));
            let mut list: Vec<(String, i64)> = Vec::new();
            for part in parts {
                match (part.parse::<i64>(), list.last_mut()) {
                    (Ok(number), Some(last)) => last.1 = number,
                    (Ok(_), None) => {}
                    (Err(_), _) if part == "none" => {}
                    (Err(_), _) => list.push((part.to_owned(), default)),
                }
            }
            list
        };
        let mut counters: Vec<(String, i64, usize)> = Vec::new();
        let apply = |counters: &mut Vec<(String, i64, usize)>,
                     style: &Style,
                     parent: usize,
                     implicit: Option<i64>| {
            for (name, value) in list(style.get(Prop::COUNTER_RESET), 0) {
                // A reset replaces the counter of a preceding sibling.
                if let Some(at) = counters
                    .iter()
                    .rposition(|counter| counter.0 == name && counter.2 == parent)
                {
                    counters.remove(at);
                }
                counters.push((name, value, parent));
            }
            for (name, value) in list(style.get(Prop::COUNTER_SET), 0) {
                match counters.iter_mut().rfind(|counter| counter.0 == name) {
                    Some(counter) => counter.1 = value,
                    None => counters.push((name, value, parent)),
                }
            }
            let mut increments = list(style.get(Prop::COUNTER_INCREMENT), 1);
            if let Some(step) =
                implicit.filter(|_| !increments.iter().any(|(name, _)| name == "list-item"))
            {
                increments.push(("list-item".into(), step));
            }
            for (name, step) in increments {
                match counters.iter_mut().rfind(|counter| counter.0 == name) {
                    Some(counter) => counter.1 += step,
                    None => counters.push((name, step, parent)),
                }
            }
        };
        let content = |counters: &[(String, i64, usize)], content: &str| -> Option<String> {
            if !content.contains("counter") {
                return None;
            }
            let parts: Vec<String> = components(content, ' ')
                .into_iter()
                .map(|part| {
                    let Some((function, arguments)) = part
                        .strip_suffix(')')
                        .and_then(|part| part.split_once('('))
                        .filter(|(function, _)| matches!(*function, "counter" | "counters"))
                    else {
                        return part.to_owned();
                    };
                    let arguments: Vec<&str> = components(arguments, ',')
                        .into_iter()
                        .map(str::trim)
                        .collect();
                    let name = arguments.first().copied().unwrap_or_default();
                    let (separator, kind) = if function == "counters" {
                        (
                            arguments
                                .get(1)
                                .map(|value| value.trim_matches(['"', '\'']))
                                .unwrap_or(""),
                            arguments.get(2),
                        )
                    } else {
                        ("", arguments.get(1))
                    };
                    let kind = kind.copied().unwrap_or("decimal");
                    let values: Vec<i64> = counters
                        .iter()
                        .filter(|counter| counter.0 == name)
                        .map(|counter| counter.1)
                        .collect();
                    let values = match (function, values.last()) {
                        ("counter", Some(&last)) => vec![last],
                        ("counter", None) => vec![0],
                        _ if values.is_empty() => vec![0],
                        _ => values,
                    };
                    serde_json::to_string(
                        &values
                            .into_iter()
                            .map(|value| counter_text(kind, value))
                            .collect::<Vec<_>>()
                            .join(separator),
                    )
                    .unwrap_or_default()
                })
                .collect();
            Some(parts.join(" "))
        };
        // A node that got no new style in a fast pass still holds the text, so its declared value is in
        // `counting`. A new style drops that entry.
        let number = |pseudos: &mut FxHashMap<usize, Rc<[Style; 2]>>,
                      counting: &mut FxHashMap<usize, [Option<Rc<str>>; 2]>,
                      counters: &[(String, i64, usize)],
                      id: usize,
                      side: usize| {
            let declared = counting.get(&id).and_then(|kept| kept[side].clone());
            let own = &pseudos[&id][side];
            let declared = declared.or_else(|| own.value(Prop::CONTENT).cloned());
            if let Some(text) = declared
                .as_deref()
                .and_then(|value| content(counters, value))
            {
                counting.entry(id).or_default()[side] = declared;
                if own.get(Prop::CONTENT) != Some(&text) {
                    Rc::make_mut(pseudos.get_mut(&id).unwrap())[side].insert(Prop::CONTENT, text);
                }
            }
        };
        for &(root, _) in walk.trees.iter() {
            counters.clear();
            let mut pending = vec![(root, false)];
            while let Some((id, exit)) = pending.pop() {
                let node = &doc.nodes[id];
                let parent = node.parent.unwrap_or(usize::MAX);
                if exit {
                    if let Some(own) = pseudos.get(&id).cloned() {
                        apply(&mut counters, &own[1], id, None);
                        number(pseudos, counting, &counters, id, 1);
                    }
                    counters.retain(|counter| counter.2 != id);
                    continue;
                }
                if style_flags[id] & DISPLAY_NONE != 0 || node.tag.starts_with('#') && id != root {
                    continue;
                }
                let style = styles[id].clone();
                if matches!(node.tag.as_str(), "ol" | "ul" | "menu")
                    && !style.contains(Prop::COUNTER_RESET)
                {
                    let items = node
                        .children
                        .iter()
                        .filter(|&&child| styles[child].get(Prop::DISPLAY) == Some("list-item"))
                        .count() as i64;
                    let reversed = node.attrs.contains_key("reversed");
                    let start = node
                        .attrs
                        .get("start")
                        .and_then(|value| value.parse::<i64>().ok());
                    let first = if reversed {
                        start.unwrap_or(items) + 1
                    } else {
                        start.unwrap_or(1) - 1
                    };
                    counters.push(("list-item".into(), first, parent));
                }
                let item = style.get(Prop::DISPLAY) == Some("list-item");
                let reversed = item
                    && node
                        .parent
                        .is_some_and(|parent| doc.nodes[parent].attrs.contains_key("reversed"));
                apply(
                    &mut counters,
                    &style,
                    parent,
                    item.then_some(if reversed { -1 } else { 1 }),
                );
                if item {
                    if let Some(value) = node
                        .attrs
                        .get("value")
                        .and_then(|value| value.parse::<i64>().ok())
                    {
                        match counters
                            .iter_mut()
                            .rfind(|counter| counter.0 == "list-item")
                        {
                            Some(counter) => counter.1 = value,
                            None => counters.push(("list-item".into(), value, parent)),
                        }
                    }
                    if let Some(counter) = counters.iter().rfind(|counter| counter.0 == "list-item")
                    {
                        list_items.insert(id, counter.1);
                    }
                }
                if let Some(own) = pseudos.get(&id).cloned() {
                    apply(&mut counters, &own[0], id, None);
                    number(pseudos, counting, &counters, id, 0);
                }
                pending.push((id, true));
                pending.extend(node.children.iter().rev().map(|&child| (child, false)));
            }
        }
    }
    // This adjusts styles after caching, so cached styles stay unadjusted.
    const SIDES: [[Prop; 4]; 4] = [
        [
            Prop::BORDER_TOP_STYLE,
            Prop::BORDER_TOP_WIDTH,
            Prop::MARGIN_TOP,
            Prop::PADDING_TOP,
        ],
        [
            Prop::BORDER_RIGHT_STYLE,
            Prop::BORDER_RIGHT_WIDTH,
            Prop::MARGIN_RIGHT,
            Prop::PADDING_RIGHT,
        ],
        [
            Prop::BORDER_BOTTOM_STYLE,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::MARGIN_BOTTOM,
            Prop::PADDING_BOTTOM,
        ],
        [
            Prop::BORDER_LEFT_STYLE,
            Prop::BORDER_LEFT_WIDTH,
            Prop::MARGIN_LEFT,
            Prop::PADDING_LEFT,
        ],
    ];
    let mut edges = FxHashMap::<*const Style, (Rc<Style>, [(f32, u8); 4])>::default();
    let own_style = |styles: &Chunked<Rc<Style>>, id: usize| {
        let style = &styles[id];
        previous
            .filter(|_| fast)
            .filter(|previous| {
                previous
                    .values
                    .get(id)
                    .is_some_and(|old| Rc::ptr_eq(old, style))
            })
            .and_then(|previous| previous.authored.get(&id))
            .unwrap_or(style)
            .clone()
    };
    for &table in &walk.tables {
        let table_style = own_style(styles, table);
        if table_style
            .get(Prop::BORDER_COLLAPSE)
            .is_none_or(|value| value != "collapse")
            || table_style
                .get(Prop::DISPLAY)
                .is_none_or(|value| value != "grid")
        {
            continue;
        }
        let lines: Vec<Line> = doc.nodes[table]
            .children
            .iter()
            .flat_map(|child| {
                let group = matches!(doc.nodes[*child].tag.as_str(), "thead" | "tbody" | "tfoot");
                let rows = if group {
                    &doc.nodes[*child].children[..]
                } else {
                    std::slice::from_ref(child)
                };
                rows.iter().map(move |&row| (row, group.then_some(*child)))
            })
            .filter(|&(row, _)| doc.nodes[row].tag == "tr")
            .map(|(row, group)| Line {
                row: (row, StyleRef(own_style(styles, row))),
                group: group.map(|group| (group, StyleRef(own_style(styles, group)))),
                cells: doc.nodes[row]
                    .children
                    .iter()
                    .filter(|&&cell| matches!(doc.nodes[cell].tag.as_str(), "td" | "th"))
                    .map(|&cell| {
                        let span = doc.nodes[cell]
                            .attrs
                            .get("colspan")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(1)
                            .clamp(1, 1000);
                        (cell, StyleRef(own_style(styles, cell)), span)
                    })
                    .collect(),
            })
            .filter(|line| !line.cells.is_empty())
            .collect();
        type Edge = (f32, u8, usize, usize);
        let wins = |a: Edge, b: Edge| if (b.0, b.1) > (a.0, a.1) { b } else { a };
        let mut edge = |style: &Rc<Style>, id: usize, side: usize| -> Edge {
            let sides = match edges.get(&Rc::as_ptr(style)) {
                Some((_, sides)) => *sides,
                None => {
                    let sides =
                        SIDES.map(
                            |[kind, width, ..]| match style.get(kind).unwrap_or("none") {
                                "none" | "hidden" => (0., 0),
                                kind => (
                                    style
                                        .get(width)
                                        .map_or(Some(3.), |value| lengths[id].resolve(value, 0.))
                                        .unwrap_or(0.)
                                        .max(0.),
                                    match kind {
                                        "double" => 8,
                                        "solid" => 7,
                                        "dashed" => 6,
                                        "dotted" => 5,
                                        "ridge" => 4,
                                        "outset" => 3,
                                        "groove" => 2,
                                        "inset" => 1,
                                        _ => 0,
                                    },
                                ),
                            },
                        );
                    edges.insert(Rc::as_ptr(style), (style.clone(), sides));
                    sides
                }
            };
            (sides[side].0, sides[side].1, id, side)
        };
        let frame = [0, 1, 2, 3].map(|side| edge(&table_style, table, side));
        let mut own = |index: usize| -> Vec<[Edge; 4]> {
            let line = &lines[index];
            let group = line.group.as_ref().map(|group| group.0);
            let first = index == 0 || lines[index - 1].group.as_ref().map(|group| group.0) != group;
            let last = lines
                .get(index + 1)
                .is_none_or(|next| next.group.as_ref().map(|group| group.0) != group);
            line.cells
                .iter()
                .enumerate()
                .map(|(column, (cell, style, _))| {
                    let mut sides = [0, 1, 2, 3].map(|side| edge(&style.0, *cell, side));
                    for (position, (owner, style)) in [Some(&line.row), line.group.as_ref()]
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        let whole = position == 0;
                        for (side, applies) in [
                            (0, whole || first),
                            (1, column + 1 == line.cells.len()),
                            (2, whole || last),
                            (3, column == 0),
                        ] {
                            if applies {
                                sides[side] = wins(sides[side], edge(&style.0, *owner, side));
                            }
                        }
                    }
                    sides
                })
                .collect()
        };
        let share = |above: &mut [[Edge; 4]], a: &Line, below: &mut [[Edge; 4]], b: &Line| {
            let ends = |line: &Line| {
                line.cells
                    .iter()
                    .scan(0, |end, cell| {
                        *end += cell.2;
                        Some(*end)
                    })
                    .collect::<Vec<_>>()
            };
            let (top, bottom) = (ends(a), ends(b));
            let (mut i, mut j) = (0, 0);
            while i < above.len() && j < below.len() {
                if top[i] - a.cells[i].2 < bottom[j] && bottom[j] - b.cells[j].2 < top[i] {
                    let shared = wins(above[i][2], below[j][0]);
                    (above[i][2], below[j][0]) = (shared, shared);
                }
                if top[i] <= bottom[j] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        };
        let mut old = style_cache.tables.remove(&table).unwrap_or_default();
        let whole = old.size != [width, height]
            || old
                .table
                .as_ref()
                .is_none_or(|(style, ..)| !Rc::ptr_eq(&style.0, &table_style));
        let mut moved: Option<FxHashMap<usize, usize>> = None;
        let mut shared = std::mem::take(&mut old.styles);
        let mut adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])> = Vec::with_capacity(lines.len());
        let mut widest = [0f32; 4];
        for index in 0..lines.len() {
            let row = lines[index].row.0;
            let before = match old.lines.get(index) {
                Some(line) if line.row.0 == row => Some(index),
                _ => moved
                    .get_or_insert_with(|| {
                        old.lines
                            .iter()
                            .enumerate()
                            .map(|(index, line)| (line.row.0, index))
                            .collect()
                    })
                    .get(&row)
                    .copied(),
            };
            let same = before.filter(|&before| {
                !whole
                    && (0..3).all(|step| {
                        lines.get((index + step).wrapping_sub(1))
                            == old.lines.get((before + step).wrapping_sub(1))
                    })
            });
            let line = match same {
                Some(before) => std::mem::take(&mut old.adjusted[before]),
                None => {
                    let mut cells = own(index);
                    if index > 0 {
                        share(
                            &mut own(index - 1),
                            &lines[index - 1],
                            &mut cells,
                            &lines[index],
                        );
                    }
                    if index + 1 < lines.len() {
                        share(
                            &mut cells,
                            &lines[index],
                            &mut own(index + 1),
                            &lines[index + 1],
                        );
                    }
                    for column in 1..cells.len() {
                        let shared = wins(cells[column - 1][1], cells[column][3]);
                        (cells[column - 1][1], cells[column][3]) = (shared, shared);
                    }
                    let mut outer = [0f32; 4];
                    let count = cells.len();
                    for (column, sides) in cells.iter_mut().enumerate() {
                        for (side, outer) in outer.iter_mut().enumerate() {
                            let at = match side {
                                0 => index == 0,
                                1 => column + 1 == count,
                                2 => index + 1 == lines.len(),
                                _ => column == 0,
                            };
                            if at {
                                sides[side] = wins(sides[side], frame[side]);
                                *outer = outer.max(sides[side].0);
                            }
                        }
                    }
                    let cells = lines[index]
                        .cells
                        .iter()
                        .zip(cells)
                        .map(|((cell, own, _), sides)| {
                            let side = |side: usize| {
                                let (width, _, owner, from) = sides[side];
                                let [_, kind, color] = BORDERS[from];
                                (
                                    width,
                                    (owner != *cell || from != side).then(|| {
                                        let owner = own_style(styles, owner);
                                        [
                                            if width > 0. {
                                                owner.value(kind).cloned()
                                            } else {
                                                Some("none".into())
                                            },
                                            owner.value(color).cloned(),
                                        ]
                                    }),
                                )
                            };
                            let sides = [side(0), side(1), side(2), side(3)];
                            let key = (
                                own.clone(),
                                sides.clone().map(|(width, taken)| (width.to_bits(), taken)),
                            );
                            shared
                                .entry(key)
                                .or_insert_with(|| {
                                    let mut style = (*own.0).clone();
                                    for (side, ([_, _, margin, _], (width, taken))) in
                                        SIDES.into_iter().zip(sides).enumerate()
                                    {
                                        if let Some([kind, color]) = taken {
                                            for (prop, value) in BORDERS[side].into_iter().zip([
                                                Some(format!("{width}px").into()),
                                                kind,
                                                color,
                                            ]) {
                                                if let Some(value) = value {
                                                    style.insert(prop, value);
                                                }
                                            }
                                        }
                                        style.insert(margin, format!("{}px", -width / 2.));
                                    }
                                    Rc::new(style)
                                })
                                .clone()
                        })
                        .collect();
                    (cells, outer)
                }
            };
            for (widest, outer) in widest.iter_mut().zip(line.1) {
                *widest = widest.max(outer);
            }
            adjusted.push(line);
        }
        let count: usize = lines.iter().map(|line| line.cells.len()).sum();
        if shared.len() > 2 * count + 64 {
            shared.retain(|_, style| Rc::strong_count(style) > 1);
        }
        let key = (StyleRef(table_style.clone()), widest.map(f32::to_bits));
        let style = match old.table.take() {
            Some((style, bits, adjusted)) if (style.clone(), bits) == key => adjusted,
            _ => {
                let mut style = (*table_style).clone();
                for ([_, width, _, padding], widest) in SIDES.into_iter().zip(widest) {
                    style.insert(width, "0");
                    style.insert(padding, format!("{}px", widest / 2.));
                }
                Rc::new(style)
            }
        };
        // Writes only where the style changed, as each write copies a shared chunk of the styles.
        authored.reserve(adjusted.iter().map(|line| line.0.len()).sum::<usize>() + 1);
        for (id, own, style) in lines
            .iter()
            .zip(&adjusted)
            .flat_map(|(line, (cells, _))| {
                line.cells
                    .iter()
                    .zip(cells)
                    .map(|((cell, own, _), style)| (*cell, &own.0, style))
            })
            .chain(std::iter::once((table, &table_style, &style)))
        {
            if !Rc::ptr_eq(&styles[id], style) {
                styles[id] = style.clone();
            }
            authored.insert(id, own.clone());
        }
        style_cache.tables.insert(
            table,
            CollapsedTable {
                pass: style_cache.pass,
                size: [width, height],
                table: Some((key.0, key.1, style)),
                lines,
                adjusted,
                styles: shared,
            },
        );
    }
    for (&table, old) in style_cache
        .tables
        .iter()
        .filter(|(_, old)| old.pass != style_cache.pass)
    {
        for id in old
            .lines
            .iter()
            .flat_map(|line| line.cells.iter().map(|cell| cell.0))
            .chain([table])
        {
            let own = own_style(styles, id);
            if !Rc::ptr_eq(&styles[id], &own) {
                styles[id] = own;
            }
        }
    }
    // A block whose children are all table parts outside a table lays out as their anonymous table, and its rows
    // and row groups pass their cells on to it. Blocks with other content keep their parts as they are.
    orphans.sort_unstable();
    orphans.dedup();
    orphans.retain(|&host| {
        let part = |child: usize| matches!(style_flags[child] >> 8, CELL | CAPTION | 10..=12);
        if host >= count
            || walk.walk.place(host).is_none()
            || !doc.nodes[host].children.iter().any(|&child| part(child))
        {
            return false;
        }
        let role = match styles[host].get(Prop::DISPLAY) {
            Some("block") => 13,
            Some("inline-block") => 14,
            _ => return true,
        };
        if style_flags[host] >> 8 != 0
            || shadows.roots.contains_key(&host)
            || !doc.nodes[host].children.iter().all(|&child| {
                let (node, style) = (&doc.nodes[child], &styles[child]);
                match node.tag.as_str() {
                    "#text" => node.text.trim_ascii().is_empty(),
                    "#comment" => true,
                    _ => {
                        part(child)
                            || style_flags[child] & DISPLAY_NONE != 0
                            || style
                                .get(Prop::DISPLAY)
                                .is_some_and(|value| value.starts_with("table-column"))
                            || style
                                .get(Prop::POSITION)
                                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
                    }
                }
            })
        {
            return true;
        }
        let rows = doc.nodes[host]
            .children
            .iter()
            .copied()
            .filter(|&child| matches!(style_flags[child] >> 8, 10..=12))
            .collect::<Vec<_>>();
        for id in std::iter::once(host).chain(rows) {
            let (own, flags) = (styles[id].clone(), style_flags[id]);
            let (role, display) = match flags >> 8 {
                10 => (ROW, "contents"),
                11 => (5, "contents"),
                12 => (HEADER, "contents"),
                _ => (role, ["grid", "inline-grid"][usize::from(role == 14)]),
            };
            let kept = previous
                .filter(|previous| fast && previous.anonymous.contains_key(&id))
                .filter(|previous| {
                    previous
                        .authored
                        .get(&id)
                        .is_some_and(|old| Rc::ptr_eq(old, &own))
                })
                .and_then(|previous| previous.values.get(id).cloned());
            styles[id] = kept.unwrap_or_else(|| {
                let mut style = (*own).clone();
                style.insert(Prop::DISPLAY, display);
                if id == host {
                    style.insert(Prop::JUSTIFY_CONTENT, "start");
                    style.insert(Prop::ALIGN_CONTENT, "start");
                    border_spacing(&mut style);
                }
                Rc::new(style)
            });
            style_flags[id] =
                (flags & 0xff) | role << 8 | (u16::from(display == "contents") * DISPLAY_CONTENTS);
            authored.insert(id, own);
            anonymous.insert(id, flags);
        }
        true
    });
    style_cache.orphans = orphans;
    #[cfg(debug_assertions)]
    if fast {
        style_cache.incremental += 1;
        let mut fresh = StyleCache {
            cssom: style_cache.cssom.clone(),
            constructed: style_cache.constructed.clone(),
            adopted: style_cache.adopted.clone(),
            containers: style_cache.containers.clone(),
            scripted: style_cache.scripted.clone(),
            tailwind: std::mem::take(&mut style_cache.tailwind),
            unshared: true,
            ..Default::default()
        };
        let full = compute_styles(doc, width, height, text_system, cache, &mut fresh, context);
        style_cache.tailwind = fresh.tailwind;
        let groups = |a: Option<&Shadowed>, b: Option<&Shadowed>| match (a, b) {
            (Some(a), Some(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b.iter()).all(|(a, b)| {
                        a.name == b.name
                            && a.compound.len() == b.compound.len()
                            && a.conditions.is_some() == b.conditions.is_some()
                            && (a.rule.kind, a.rule.context, a.rule.specificity)
                                == (b.rule.kind, b.rule.context, b.rule.specificity)
                            && a.rule.declarations == b.rule.declarations
                    })
            }
            (a, b) => a.is_none() && b.is_none(),
        };
        for id in 0..doc.nodes.len() {
            let same = [
                groups(full.slotted.get(&id), out.slotted.get(&id)),
                groups(full.parts.get(&id), out.parts.get(&id)),
                groups(full.exported.get(&id), out.exported.get(&id)),
                *full.values[id] == *out.values[id],
                full.vars[id] == out.vars[id],
                full.flags[id] == out.flags[id],
                full.lengths[id] == out.lengths[id],
                full.pseudos.get(&id) == out.pseudos.get(&id),
                full.text_pseudos.get(&id) == out.text_pseudos.get(&id),
                full.backdrops.get(&id) == out.backdrops.get(&id),
                full.backdrop_starts.get(&id) == out.backdrop_starts.get(&id),
                full.scrollbars.get(&id) == out.scrollbars.get(&id),
                full.selections.get(&id) == out.selections.get(&id),
                format!("{:?}", full.animations.get(&id))
                    == format!("{:?}", out.animations.get(&id)),
                full.important.get(&id) == out.important.get(&id),
                full.starting.get(&id) == out.starting.get(&id),
                full.containers[id] == out.containers[id],
            ];
            assert!(
                same.iter().all(|same| *same),
                "incremental style of node {id} <{}> differs from a full pass: {same:?}",
                doc.nodes[id].tag
            );
        }
    }
    out
}

pub(crate) type Registry = FxHashMap<Prop, (Rc<str>, bool, Option<Rc<str>>)>;

// On equal ranks the later declaration wins.
fn put<V>(
    cascade: &mut BBTreeMap<Prop, (CascadeRank, V)>,
    prop: Prop,
    rank: CascadeRank,
    value: V,
) {
    match cascade.entry(prop) {
        bbtree::unsync::bbtree_map::Entry::Occupied(mut old) => {
            if rank >= old.get().0 {
                old.insert((rank, value));
            }
        }
        bbtree::unsync::bbtree_map::Entry::Vacant(slot) => {
            slot.insert((rank, value));
        }
    }
}

fn physical<V: AsRef<str>>(cascade: &mut BBTreeMap<Prop, (CascadeRank, V)>, parent: &Style) {
    if !LOGICAL
        .iter()
        .any(|(logical, ..)| cascade.contains_key(logical))
    {
        return;
    }
    let rtl = match cascade
        .get(&Prop::DIRECTION)
        .map(|(_, value)| value.as_ref())
    {
        Some("rtl") => true,
        Some("ltr") => false,
        _ => parent.get(Prop::DIRECTION) == Some("rtl"),
    };
    for (logical, ltr, rtl_side) in LOGICAL {
        if let Some((rank, value)) = cascade.remove(&logical) {
            let physical = if rtl { rtl_side } else { ltr };
            put(cascade, physical, rank, value);
        }
    }
}

const LOGICAL: [(Prop, Prop, Prop); 16] = [
    (
        Prop::MARGIN_INLINE_START,
        Prop::MARGIN_LEFT,
        Prop::MARGIN_RIGHT,
    ),
    (
        Prop::MARGIN_INLINE_END,
        Prop::MARGIN_RIGHT,
        Prop::MARGIN_LEFT,
    ),
    (
        Prop::PADDING_INLINE_START,
        Prop::PADDING_LEFT,
        Prop::PADDING_RIGHT,
    ),
    (
        Prop::PADDING_INLINE_END,
        Prop::PADDING_RIGHT,
        Prop::PADDING_LEFT,
    ),
    (Prop::INSET_INLINE_START, Prop::LEFT, Prop::RIGHT),
    (Prop::INSET_INLINE_END, Prop::RIGHT, Prop::LEFT),
    (
        Prop::BORDER_INLINE_START_WIDTH,
        Prop::BORDER_LEFT_WIDTH,
        Prop::BORDER_RIGHT_WIDTH,
    ),
    (
        Prop::BORDER_INLINE_START_STYLE,
        Prop::BORDER_LEFT_STYLE,
        Prop::BORDER_RIGHT_STYLE,
    ),
    (
        Prop::BORDER_INLINE_START_COLOR,
        Prop::BORDER_LEFT_COLOR,
        Prop::BORDER_RIGHT_COLOR,
    ),
    (
        Prop::BORDER_INLINE_END_WIDTH,
        Prop::BORDER_RIGHT_WIDTH,
        Prop::BORDER_LEFT_WIDTH,
    ),
    (
        Prop::BORDER_INLINE_END_STYLE,
        Prop::BORDER_RIGHT_STYLE,
        Prop::BORDER_LEFT_STYLE,
    ),
    (
        Prop::BORDER_INLINE_END_COLOR,
        Prop::BORDER_RIGHT_COLOR,
        Prop::BORDER_LEFT_COLOR,
    ),
    (
        Prop::BORDER_START_START_RADIUS,
        Prop::BORDER_TOP_LEFT_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
    ),
    (
        Prop::BORDER_START_END_RADIUS,
        Prop::BORDER_TOP_RIGHT_RADIUS,
        Prop::BORDER_TOP_LEFT_RADIUS,
    ),
    (
        Prop::BORDER_END_START_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
    ),
    (
        Prop::BORDER_END_END_RADIUS,
        Prop::BORDER_BOTTOM_RIGHT_RADIUS,
        Prop::BORDER_BOTTOM_LEFT_RADIUS,
    ),
];

// `var()` results stop at this many bytes, like in browsers, so references that fan out cannot grow exponentially.
const SUBSTITUTION_LIMIT: usize = 1 << 20;

fn substitute(
    value: &str,
    lookup: &mut dyn FnMut(Prop) -> Option<Rc<str>>,
    out: &mut String,
    nesting: u8,
) -> bool {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '%' | '#');
    let mut rest = value;
    while let Some(start) = next_var(rest) {
        if rest[..start].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '-' | '_')) {
            out.push_str(&rest[..start + 4]);
            rest = &rest[start + 4..];
            continue;
        }
        out.push_str(&rest[..start]);
        let body = &rest[start + 4..];
        let (mut depth, mut comma, mut end, mut quote, mut escaped) = (1, None, None, None, false);
        for (index, c) in body.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' => escaped = true,
                _ if quote.is_some() => {
                    if quote == Some(c) {
                        quote = None;
                    }
                }
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                ',' if depth == 1 && comma.is_none() => comma = Some(index),
                _ => {}
            }
        }
        let Some(end) = end else {
            return false;
        };
        let mark = out.len();
        let name = body[..comma.unwrap_or(end)].trim();
        match name
            .starts_with("--")
            .then(|| Prop::find(name))
            .flatten()
            .and_then(&mut *lookup)
        {
            Some(resolved) => out.push_str(&resolved),
            None => match comma {
                // Nested fallbacks recurse, and so do the references in them, so their depth is capped.
                Some(comma)
                    if nesting < 32
                        && substitute(body[comma + 1..end].trim(), lookup, out, nesting + 1) => {}
                _ => return false,
            },
        }
        rest = &body[end + 1..];
        // `var()` substitutes tokens, so `var(--a)var(--b)` stays two values.
        if out[mark..].starts_with(word) && out[..mark].ends_with(|c: char| word(c) || c == ')') {
            out.insert(mark, ' ');
        }
        if out.ends_with(|c: char| word(c) || c == ')') && rest.starts_with(word) {
            out.push(' ');
        }
        if out.len() > SUBSTITUTION_LIMIT {
            return false;
        }
    }
    out.push_str(rest);
    true
}

fn syntax_matches(syntax: &str, value: &str) -> bool {
    let units = Lengths::new(16., 100., 100.);
    let syntax = syntax.trim();
    let mut rest = syntax;
    let mut alternatives = Vec::new();
    while !rest.is_empty() {
        let end = if rest.starts_with('[') {
            rest.find(']').map_or(rest.len(), |end| end + 1)
        } else {
            rest.find('|').unwrap_or(rest.len())
        };
        let end = end
            + match rest.as_bytes()[end..].first() {
                Some(b'+' | b'#') => 1,
                Some(b'{') => rest[end..].find('}').map_or(0, |close| close + 1),
                _ => 0,
            };
        alternatives.push(rest[..end].trim());
        rest = rest[end..]
            .trim_start()
            .trim_start_matches('|')
            .trim_start();
    }
    alternatives.into_iter().any(|alternative| {
        if !alternative.starts_with(['<', '[', '*']) && !alternative.ends_with(['(', '+', '#', '}'])
        {
            return value.trim().eq_ignore_ascii_case(alternative);
        }
        let mut range = 1..=usize::MAX;
        let (kinds, separator) = match alternative.strip_suffix(['+', '#']) {
            Some(kind) => (
                kind,
                Some(if alternative.ends_with('#') { ',' } else { ' ' }),
            ),
            None => match alternative
                .strip_suffix('}')
                .and_then(|rest| rest.rsplit_once('{'))
                .and_then(|(kind, count)| {
                    let (min, max) = count.split_once(',')?;
                    Some((kind, min.trim().parse().ok()?..=max.trim().parse().ok()?))
                }) {
                Some((kind, counts)) => {
                    range = counts;
                    (kind, Some(' '))
                }
                None => (alternative, None),
            },
        };
        let kinds = kinds.trim_start_matches('[').trim_end_matches(']');
        let one = |value: &str| {
            kinds.split('|').any(|kind| {
                let (kind, value) = (kind.trim(), value.trim());
                if !kind.starts_with('<') && !kind.ends_with('(') && kind != "*" {
                    return value.eq_ignore_ascii_case(kind);
                }
                let (kind, sign) = match kind {
                    "<non-negative-length>" => ("<length>", true),
                    "<non-negative-length-percentage>" => ("<length-percentage>", true),
                    "<non-negative-number>" => ("<number>", true),
                    kind => (kind, false),
                };
                if sign && value.starts_with('-') {
                    return false;
                }
                let function = || {
                    ["calc(", "min(", "max(", "clamp("]
                        .iter()
                        .any(|function| value.starts_with(function))
                };
                let math = || units.math(value, f32::NAN, &[]);
                // Math in an `<integer>` rounds.
                let number = |integer: bool| {
                    math().is_some_and(|(n, kind)| {
                        kind == parser::Kind::Number && (!integer || n.fract() == 0. || function())
                    })
                };
                match kind {
                    "<length>" => units.resolve(value, f32::NAN).is_some(),
                    "<percentage>" => {
                        value.contains('%')
                            && units
                                .math(value, 100., &[])
                                .is_some_and(|(_, kind)| kind == parser::Kind::Length)
                            && units.math(value, 0., &[]) == Some((0., parser::Kind::Length))
                    }
                    "<length-percentage>" => units.resolve(value, 100.).is_some(),
                    "<number>" => number(false),
                    "<integer>" => number(true),
                    "<angle>" => math().is_some_and(|(_, kind)| kind == parser::Kind::Angle),
                    "<time>" => math().is_some_and(|(_, kind)| kind == parser::Kind::Time),
                    "<color>" => {
                        value.eq_ignore_ascii_case("currentcolor")
                            || value.eq_ignore_ascii_case("-webkit-focus-ring-color")
                            || color(value).is_some()
                            || value.contains("(from ")
                                && color(
                                    &value.to_ascii_lowercase().replace("currentcolor", "black"),
                                )
                                .is_some()
                            || ["light-dark(", "color-mix("]
                                .iter()
                                .any(|function| value.starts_with(function) && value.ends_with(')'))
                    }
                    "<custom-ident>" => {
                        !value.is_empty()
                            && value
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                    }
                    "<image>" | "<url>" => {
                        [
                            "url(",
                            "image-set(",
                            "-webkit-image-set(",
                            "image(",
                            "cross-fade(",
                        ]
                        .iter()
                        .any(|function| value.starts_with(function))
                            || value.contains("gradient(")
                                && value.split_once('(').is_some_and(|(_, inner)| {
                                    inner.trim_end_matches(')').trim() != ""
                                })
                    }
                    "<transform-function>" | "<transform-list>" => value.contains('('),
                    "<string>" => value.starts_with(['"', '\'']),
                    "<easing-function>" => Easing::parse(value).is_some(),
                    "*" => !value.is_empty(),
                    _ if kind.ends_with('(') => {
                        value.len() > kind.len()
                            && value.as_bytes()[..kind.len()].eq_ignore_ascii_case(kind.as_bytes())
                            && value.ends_with(')')
                    }
                    _ => value.eq_ignore_ascii_case(kind),
                }
            })
        };
        match separator {
            Some(separator) => {
                let parts = components(value, separator);
                range.contains(&parts.len()) && parts.into_iter().all(one)
            }
            None => one(value),
        }
    })
}

// The computed value of a registered custom property. Relative lengths like `2em` and colors stay as written, and
// false means that the value depends on the element.
fn computed(syntax: &str, value: &str) -> (String, bool) {
    let mut units = Lengths::new(f32::NAN, f32::NAN, f32::NAN);
    units.root = units.font;
    let has = |kind: &str| syntax.contains(kind);
    let number = value::number;
    let mut independent = true;
    let (separator, joint) = if has("#") { (',', ", ") } else { (' ', " ") };
    let parts: Vec<String> = components(value, separator)
        .into_iter()
        .map(|part| {
            let part = part.trim();
            match units.math(part, f32::NAN, &[]) {
                Some((n, parser::Kind::Length)) if has("<length") => format!("{}px", number(n)),
                Some((n, parser::Kind::Number)) if has("<number>") => number(n),
                Some((n, parser::Kind::Number)) if has("<integer>") => number(n.round()),
                Some((n, parser::Kind::Number)) if n == 0. && has("<length") => "0px".into(),
                Some((n, parser::Kind::Angle)) if has("<angle>") => format!("{}deg", number(n)),
                Some((n, parser::Kind::Time)) if has("<time>") => format!("{}s", number(n)),
                Some(_) => part.into(),
                None => match (units.math(part, 0., &[]), units.math(part, 100., &[])) {
                    (Some((0., parser::Kind::Length)), Some((n, parser::Kind::Length))) => {
                        format!("{}%", number(n))
                    }
                    (Some((a, parser::Kind::Length)), Some((b, parser::Kind::Length))) => {
                        format!("calc({}% + {}px)", number(b - a), number(a))
                    }
                    _ => {
                        independent &= Lengths::new(16., 100., 100.)
                            .math(part, 100., &[])
                            .is_none();
                        part.into()
                    }
                },
            }
        })
        .collect();
    (parts.join(joint), independent)
}

struct Substitution<'a> {
    declared: &'a [(Prop, Rc<str>)],
    state: Vec<(u8, usize, Option<Rc<str>>)>,
    inherited: &'a Vars,
    parent: Option<&'a Style>,
    vars: Option<&'a Vars>,
    registry: &'a Registry,
    depth: usize,
    cycle: usize,
}

impl Substitution<'_> {
    fn get(&mut self, prop: Prop) -> Option<Rc<str>> {
        let Ok(index) = self.declared.binary_search_by_key(&prop, |entry| entry.0) else {
            return var(self.inherited, prop)
                .cloned()
                .or_else(|| self.registry.get(&prop)?.2.clone());
        };
        match self.state[index].0 {
            2 => return self.state[index].2.clone(),
            1 => {
                self.cycle = self.cycle.min(self.state[index].1);
                return None;
            }
            _ => {}
        }
        let raw = self.declared[index].1.clone();
        let registered = self.registry.get(&prop);
        let (parent, vars) = (self.parent, self.vars);
        let initial = || registered.and_then(|(_, _, initial)| initial.clone());
        let inherit = || {
            match vars {
                Some(vars) => var(vars, prop).cloned(),
                None => parent.and_then(|parent| parent.value(prop).cloned()),
            }
            .or_else(initial)
        };
        let unset = || {
            if registered.is_some_and(|(_, inherits, _)| !inherits) {
                initial()
            } else {
                inherit()
            }
        };
        let value = match &*raw {
            "inherit" => inherit(),
            "initial" => initial(),
            "unset" | "revert" | "revert-layer" => unset(),
            _ => {
                let mut value = Some(raw.clone());
                if raw.contains("var(") {
                    // Each nested reference recurses, so chains are invalid past this depth.
                    if self.depth > 128 {
                        value = None;
                    } else {
                        let (depth, outer) = (self.depth, self.cycle);
                        self.state[index] = (1, depth, None);
                        (self.depth, self.cycle) = (depth + 1, usize::MAX);
                        let mut out = String::new();
                        let ok = substitute(&raw, &mut |prop| self.get(prop), &mut out, 0);
                        let reached = self.cycle;
                        self.depth = depth;
                        self.cycle = outer.min(if reached < depth { reached } else { usize::MAX });
                        if reached <= depth {
                            self.state[index] = (2, depth, None);
                            return None;
                        }
                        value = ok.then(|| out.into());
                    }
                }
                match (value, registered) {
                    (Some(value), Some((syntax, ..))) if syntax.trim() != "*" => {
                        match syntax_matches(syntax, &value) {
                            true => Some(computed(syntax, &value).0.into()),
                            false => unset(),
                        }
                    }
                    (Some(value), Some(_)) if value.trim().is_empty() => unset(),
                    (None, Some(_)) => unset(),
                    (value, _) => value,
                }
            }
        };
        self.state[index] = (2, 0, value.clone());
        value
    }
}

fn resolve_style(
    style: &mut Style,
    parent: Option<&Style>,
    parent_vars: Option<&Vars>,
    declared: &[(Prop, Rc<str>)],
    registry: &Registry,
) {
    if !declared.is_empty() {
        // Only custom properties are declared or referenced here.
        let inherited = style.vars().clone();
        let mut resolver = Substitution {
            declared,
            state: vec![(0, 0, None); declared.len()],
            inherited: &inherited,
            parent,
            vars: parent_vars,
            registry,
            depth: 0,
            cycle: usize::MAX,
        };
        let values: Vec<_> = declared
            .iter()
            .map(|&(prop, _)| (prop, resolver.get(prop)))
            .collect();
        if values
            .iter()
            .any(|(prop, value)| var(&inherited, *prop) != value.as_ref())
        {
            let mut vars = (**style.vars()).clone();
            for (prop, value) in values {
                match (vars.binary_search_by_key(&prop, |entry| entry.0), value) {
                    (Ok(index), Some(value)) => vars[index].1 = value,
                    (Err(index), Some(value)) => vars.insert(index, (prop, value)),
                    (Ok(index), None) => {
                        vars.remove(index);
                    }
                    (Err(_), None) => {}
                }
            }
            style.set_vars(Rc::new(vars));
        }
    }
    // The longhands of a `var()` shorthand share one pending value, so it is substituted and expanded once.
    let mut values: Vec<(Rc<str>, Option<String>, Option<Vec<_>>)> = Vec::new();
    let resolved: Vec<_> = style
        .own()
        .filter(|(_, value)| next_var(value).is_some())
        .map(|(prop, value)| {
            let shared = value
                .starts_with('\u{1}')
                .then(|| values.iter().position(|(raw, ..)| raw == value))
                .flatten();
            let index = shared.unwrap_or_else(|| {
                let mut out = String::new();
                let ok = substitute(
                    value,
                    &mut |name| {
                        style
                            .value(name)
                            .cloned()
                            .or_else(|| registry.get(&name)?.2.clone())
                    },
                    &mut out,
                    0,
                );
                values.push((value.clone(), ok.then_some(out), None));
                values.len() - 1
            });
            (prop, index)
        })
        .collect();
    for (prop, index) in resolved {
        let (raw, substituted, expanded) = &mut values[index];
        let value = match substituted
            .as_deref()
            .and_then(|value| value.strip_prefix('\u{1}')?.split_once('\u{1}'))
        {
            Some((shorthand, value)) => {
                let rtl = style.get(Prop::DIRECTION) == Some("rtl");
                expanded
                    .get_or_insert_with(|| {
                        let mut expanded = Vec::new();
                        if !parser::invalid(shorthand, value) {
                            expand(shorthand, value.trim(), &mut expanded);
                        }
                        expanded
                    })
                    .iter()
                    .find(|(name, _)| {
                        name == prop.name()
                            || LOGICAL.iter().any(|(logical, ltr, rtl_side)| {
                                logical.name() == name && *if rtl { rtl_side } else { ltr } == prop
                            })
                    })
                    .map(|(_, value)| value.clone())
            }
            // Values that start with U+0001 can be shared by other properties.
            None if raw.starts_with('\u{1}') => substituted.clone(),
            None => substituted.take(),
        }
        .filter(|value| !parser::invalid(prop.name(), value))
        .map(|value| fold(prop.name(), &value).unwrap_or(value));
        match value.map(Rc::from).or_else(|| {
            parent
                .filter(|_| prop.inherited())
                .and_then(|parent| parent.value(prop).cloned())
        }) {
            Some(value) => style.insert(prop, value),
            None => {
                style.remove(prop);
            }
        }
    }
    let dark = dark(style);
    style.retain(|prop, slot| {
        if prop.is_custom() || !slot.contains("light-dark(") {
            return true;
        }
        // Mark removed ranges, then copy once. Repeated in-place replacements are quadratic.
        let bytes = slot.as_bytes();
        let mut dropped = vec![0i32; bytes.len() + 1];
        let mut open: Vec<(usize, Option<usize>, u32)> = Vec::new();
        let (mut quote, mut index) = (0u8, 0);
        while index < bytes.len() {
            match bytes[index] {
                b'\\' => index += 1,
                byte if quote != 0 => quote = if byte == quote { 0 } else { quote },
                byte @ (b'"' | b'\'') => quote = byte,
                b'(' => open.push((index, None, 0)),
                b',' => {
                    if let Some((_, comma, commas)) = open.last_mut() {
                        comma.get_or_insert(index);
                        *commas += 1;
                    }
                }
                b')' => {
                    if let Some((start, Some(comma), 1)) = open.pop()
                        && slot[..start].ends_with("light-dark")
                    {
                        let parts = [start + 1..comma, comma + 1..index];
                        if parts
                            .iter()
                            .all(|part| !slot[part.clone()].trim().is_empty())
                        {
                            let kept = parts[usize::from(dark)].clone();
                            let text = &slot[kept.clone()];
                            dropped[start - 10] += 1;
                            dropped[kept.start + text.len() - text.trim_start().len()] -= 1;
                            dropped[kept.start + text.trim_end().len()] += 1;
                            dropped[index + 1] -= 1;
                        }
                    }
                }
                _ => {}
            }
            index += 1;
        }
        let mut value = String::with_capacity(slot.len());
        let (mut depth, mut from) = (0, 0);
        for (index, &change) in dropped
            .iter()
            .enumerate()
            .filter(|(_, change)| **change != 0)
        {
            if depth == 0 {
                value.push_str(&slot[from..index]);
            }
            depth += change;
            if depth == 0 {
                from = index;
            }
        }
        value.push_str(&slot[from..]);
        *slot = value.into();
        true
    });
    style.retain(|prop, value| {
        if (prop.name().ends_with("color")
            || prop.name().starts_with("-terminal-")
            || matches!(prop, Prop::FILL | Prop::STROKE))
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && let Some(system) = color::system(value, dark)
        {
            *value = system.into();
        }
        true
    });
    if !style.contains(Prop::COLOR) && parent.is_some_and(|parent| !parent.contains(Prop::COLOR)) {
        style.insert(
            Prop::COLOR,
            color::system("canvastext", dark).unwrap_or_default(),
        );
    }
    let convert = |value: &str, current: &str| {
        if !value.starts_with("color-mix(") && !value.contains("(from ") {
            return None;
        }
        color::serialize(&value.to_ascii_lowercase().replace("currentcolor", current))
    };
    let inherited = parent
        .and_then(|parent| parent.get(Prop::COLOR))
        .unwrap_or("canvastext");
    if let Some(value) = style
        .get(Prop::COLOR)
        .and_then(|value| convert(value, inherited))
    {
        style.insert(Prop::COLOR, value);
    }
    let current = style
        .get(Prop::COLOR)
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .unwrap_or(inherited)
        .to_owned();
    style.retain(|prop, value| {
        if prop != Prop::COLOR
            && !prop.is_custom()
            && (prop.name().ends_with("color")
                || prop.name().starts_with("-terminal-")
                || matches!(prop, Prop::FILL | Prop::STROKE))
            && let Some(converted) = convert(value, &current)
        {
            *value = converted.into();
        }
        true
    });
}

pub(crate) fn dark(style: &Style) -> bool {
    style.get(Prop::COLOR_SCHEME).is_some_and(|v| {
        v.contains("dark")
            && (!v.contains("light") || crate::desktop::native::theme_state() & 1 != 0)
    })
}

// A table's border spacing is the gap of its grid and padding at its edges.
fn border_spacing(style: &mut Style) {
    let spacing = if style
        .get(Prop::BORDER_COLLAPSE)
        .is_some_and(|value| value == "collapse")
    {
        "0".to_owned()
    } else {
        style.get(Prop::BORDER_SPACING).unwrap_or("0").to_owned()
    };
    let parts = components(&spacing, ' ');
    if let Some(&column) = parts.first() {
        let row = parts.get(1).copied().unwrap_or(column);
        style.insert(Prop::COLUMN_GAP, column);
        style.insert(Prop::ROW_GAP, row);
        if spacing != "0" {
            for (prop, gap) in [
                (Prop::PADDING_TOP, row),
                (Prop::PADDING_RIGHT, column),
                (Prop::PADDING_BOTTOM, row),
                (Prop::PADDING_LEFT, column),
            ] {
                let value = format!("calc({} + {gap})", style.get(prop).unwrap_or("0px"));
                style.insert(prop, value);
            }
        }
    }
}

// A canvas that lays out its children as blocks, for the page to draw them with `drawElementImage()`.
pub(crate) fn drawable_canvas(node: &crate::dom::Node, style: &Style) -> bool {
    node.tag == "canvas" && style.get(Prop::CANVAS_CONTENT) == Some("drawable")
}

fn blockify(style: &mut Style, item: bool, table_item: bool) {
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| matches!(value, "absolute" | "fixed"));
    if positioned {
        style.remove(Prop::FLOAT);
    }
    if !(item || positioned || style.get(Prop::FLOAT).is_some_and(|value| value != "none")) {
        return;
    }
    let display = match style.get(Prop::DISPLAY).unwrap_or("inline") {
        "inline" | "inline-block" => "block",
        "inline-flex" => "flex",
        "inline-grid" => "grid",
        "inline-table" => "table",
        "table-row" | "table-row-group" | "table-header-group" | "table-footer-group"
        | "table-cell" | "table-caption"
            if !table_item =>
        {
            "block"
        }
        _ => return,
    };
    style.insert(Prop::DISPLAY, display);
}

// Font-relative lengths in px, or None when the value has none.
fn pixels(value: &str, units: &Lengths) -> Option<String> {
    let relative = |text: &str| {
        let bytes = text.as_bytes();
        (1..bytes.len()).any(|at| {
            bytes[at - 1].is_ascii_digit() && {
                let unit = &text[at..];
                let unit = &unit[..unit
                    .find(|c: char| !c.is_ascii_alphabetic())
                    .unwrap_or(unit.len())];
                let unit = unit.strip_prefix(['r', 'R']).unwrap_or(unit);
                ["em", "ex", "ch", "cap", "ic", "lh"]
                    .iter()
                    .any(|name| unit.eq_ignore_ascii_case(name))
            }
        })
    };
    if !relative(value) {
        return None;
    }
    let lists: Vec<String> = components(value, ',')
        .into_iter()
        .map(|list| {
            components(list, ' ')
                .into_iter()
                .map(|token| match units.parse(token, 0.) {
                    Some((pixels, true)) if relative(token) => format!("{pixels}px"),
                    _ => token.to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    Some(lists.join(", "))
}

fn resolve_keywords(style: &mut Style, parent: Option<&Style>, units: Option<&Lengths>) -> bool {
    let mut keyword = false;
    style.retain(|prop, value| {
        let inherit = match &**value {
            _ if prop.is_custom() => return true,
            "inherit" => true,
            "unset" | "revert" | "revert-layer" => prop.inherited(),
            "initial" => false,
            _ => return true,
        };
        keyword |= inherit && !prop.inherited();
        match parent
            .and_then(|parent| parent.value(prop))
            .filter(|_| inherit)
        {
            Some(inherited) if !prop.inherited() => {
                let line = prop
                    .name()
                    .strip_suffix("-width")
                    .filter(|side| {
                        side.starts_with("border-") || matches!(*side, "outline" | "column-rule")
                    })
                    .and_then(|side| Prop::find(&format!("{side}-style")));
                *value = match line {
                    Some(line)
                        if parent
                            .and_then(|parent| parent.get(line))
                            .is_none_or(|style| matches!(style, "none" | "hidden")) =>
                    {
                        "0px".into()
                    }
                    _ => units
                        .and_then(|units| pixels(inherited, units))
                        .map_or_else(|| inherited.clone(), Rc::from),
                };
                true
            }
            Some(inherited) => {
                *value = inherited.clone();
                true
            }
            None => false,
        }
    });
    if style
        .get(Prop::COLOR)
        .is_some_and(|value| value.eq_ignore_ascii_case("currentcolor"))
    {
        match parent.and_then(|parent| parent.value(Prop::COLOR)) {
            Some(inherited) => style.insert(Prop::COLOR, inherited.clone()),
            None => {
                style.remove(Prop::COLOR);
            }
        }
    }
    let Some(current) = style.value(Prop::COLOR).cloned() else {
        return keyword;
    };
    style.retain(|prop, slot| {
        if prop.is_custom() || prop == Prop::COLOR || prop.inherited() {
            return true;
        }
        let mut start = 0;
        let mut value = None::<String>;
        while let Some(index) = value.as_deref().unwrap_or(slot).as_bytes()[start..]
            .windows(12)
            .position(|window| window.eq_ignore_ascii_case(b"currentcolor"))
        {
            value
                .get_or_insert_with(|| slot.to_string())
                .replace_range(start + index..start + index + 12, &current);
            start += index + current.len();
        }
        if let Some(value) = value {
            *slot = value.into();
        }
        true
    });
    keyword
}

pub(crate) fn declarations(source: &str) -> Style {
    Declarations::parse(source).resolve()
}

impl Declarations {
    fn resolve(&self) -> Style {
        self.resolved(false)
            .into_iter()
            .map(|(prop, value, important, _)| match important {
                true => (prop, format!("{value} !important").into()),
                false => (prop, value),
            })
            .collect()
    }

    // Drop unknown inline properties before interning: scripts can create unlimited names, and interned names
    // never free.
    fn resolved(&self, inline: bool) -> Vec<Declared> {
        let mut result = Vec::new();
        for (position, declaration) in self.0.iter().enumerate() {
            let Declaration::Property(key, value) = declaration else {
                continue;
            };
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
                let value = fold(&key, &value).unwrap_or(value);
                let prop = match inline && !key.starts_with("--") {
                    true => match Prop::find(&key) {
                        Some(prop) => prop,
                        None => continue,
                    },
                    false => Prop::new(&key),
                };
                result.push((prop, value.into(), important, position.min(65535) as u16));
            }
        }
        // Keep one declaration per property: the last important one, or else the last one.
        result.sort_unstable_by_key(|entry| (entry.0, !entry.2, std::cmp::Reverse(entry.3)));
        result.dedup_by_key(|entry| entry.0);
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

impl Rule {
    fn cssom(&self) -> serde_json::Value {
        use serde_json::json;
        let declarations = |values: &Declarations| {
            values
                .0
                .iter()
                .filter_map(|declaration| {
                    let Declaration::Property(name, value) = declaration else {
                        return None;
                    };
                    let (value, important) = match value.strip_suffix("!important") {
                        Some(value) => (value.trim_end(), "important"),
                        None => (value.as_str(), ""),
                    };
                    Some(json!([name, value, important]))
                })
                .collect::<Vec<_>>()
        };
        let list = |children: &[Rule]| {
            children
                .iter()
                .filter(|rule| !matches!(rule, Self::Declarations(_)))
                .map(Self::cssom)
                .collect::<Vec<_>>()
        };
        match self {
            Self::Style(selectors, children) => json!([
                "style",
                selectors
                    .iter()
                    .map(Selector::source)
                    .collect::<Vec<_>>()
                    .join(", "),
                children
                    .iter()
                    .flat_map(|rule| match rule {
                        Self::Declarations(values) => declarations(values),
                        _ => Vec::new(),
                    })
                    .collect::<Vec<_>>(),
                list(children),
            ]),
            Self::Media(queries, children) => json!([
                "media",
                queries.iter().map(MediaQuery::source).collect::<Vec<_>>(),
                list(children)
            ]),
            Self::Supports(condition, _, children) => {
                json!(["supports", condition.trim(), list(children)])
            }
            Self::Starting(children) => json!(["starting-style", "", list(children)]),
            Self::Layer(names, children) if children.is_empty() => json!(["layers", names]),
            Self::Layer(names, children) => json!(["layer", names.join(","), list(children)]),
            Self::Container(name, _, query, children) => json!([
                "container",
                name.as_ref()
                    .map_or_else(String::new, |name| format!("{name} "))
                    + &query.source(),
                list(children)
            ]),
            Self::Keyframes(name, frames) => json!([
                "keyframes",
                name,
                frames
                    .iter()
                    .map(|(at, values)| json!([format!("{}%", at * 100.), declarations(values)]))
                    .collect::<Vec<_>>()
            ]),
            Self::FontFace(values) => json!(["font-face", declarations(values)]),
            Self::Property(name, syntax, inherits, initial) => {
                json!(["property", name, syntax, inherits, initial])
            }
            Self::Tailwind(Some(true)) => json!(["import", "tailwindcss"]),
            Self::Tailwind(Some(false)) => json!(["import", "tailwindcss/utilities"]),
            Self::Tailwind(None) => json!(["other", "@reference 'tailwindcss';"]),
            Self::Declarations(_) | Self::Theme(..) | Self::Variant(..) | Self::Utility(..) => {
                json!(["other", self.source()])
            }
        }
    }
}

pub(crate) enum Edit<'a> {
    Insert(&'a str),
    Delete,
    Declarations(&'a str),
    Selector(&'a str),
    Keyframe(&'a str),
    Replace(&'a str),
}

impl Stylesheet {
    pub(crate) fn cssom(&self) -> serde_json::Value {
        serde_json::Value::Array(self.0.iter().map(|rule| rule.cssom()).collect())
    }

    pub(crate) fn property(&self) -> Option<(Rc<str>, bool, Option<Rc<str>>)> {
        let [rule] = self.0.as_slice() else {
            return None;
        };
        let Rule::Property(_, syntax, inherits, initial) = &**rule else {
            return None;
        };
        Some((
            syntax.as_str().into(),
            *inherits,
            initial.as_deref().map(Rc::from),
        ))
    }

    pub(crate) fn edit(
        &mut self,
        path: &[usize],
        edit: Edit,
    ) -> Result<serde_json::Value, &'static str> {
        let (&last, path) = path.split_last().ok_or("IndexSizeError")?;
        let Some((&first, path)) = path.split_first() else {
            return edit_rules(&mut self.0, last, edit, None, Rc::make_mut);
        };
        let mut parent = None;
        let index = position(&self.0, first).ok_or("IndexSizeError")?;
        let mut rules = children(Rc::make_mut(&mut self.0[index]), &mut parent)?;
        for &index in path {
            let index = position(rules, index).ok_or("IndexSizeError")?;
            rules = children(&mut rules[index], &mut parent)?;
        }
        edit_rules(rules, last, edit, parent, |rule| rule)
    }

    pub(crate) fn font_faces(&self) -> Vec<(String, String, Option<String>)> {
        let mut faces = Vec::new();
        let mut pending: Vec<&Rule> = self.0.iter().rev().map(|rule| &**rule).collect();
        while let Some(rule) = pending.pop() {
            let values = match rule {
                Rule::FontFace(values) => values.resolve(),
                Rule::Layer(_, children)
                | Rule::Media(_, children)
                | Rule::Supports(_, true, children) => {
                    pending.extend(children.iter().rev());
                    continue;
                }
                _ => continue,
            };
            let (Some(family), Some(src)) =
                (values.get(Prop::FONT_FAMILY), values.get(Prop::new("src")))
            else {
                continue;
            };
            faces.push((
                family.trim_matches(['\'', '"']).to_owned(),
                src.to_owned(),
                values.get(Prop::new("unicode-range")).map(str::to_owned),
            ));
        }
        faces
    }

    pub(crate) fn matches_media(&self, width: f32, height: f32, theme: u32) -> bool {
        let [rule] = self.0.as_slice() else {
            return false;
        };
        matches!(&**rule, Rule::Media(queries, _)
            if MediaQuery::list(queries, [width, height], reduced_motion(), theme))
    }

    pub(crate) fn bind_parents(&mut self) -> anyhow::Result<()> {
        for rule in &mut self.0 {
            Rc::make_mut(rule).bind_parent(None)?;
        }
        Ok(())
    }

    pub(crate) fn source(&self) -> String {
        self.0.iter().map(|rule| rule.source()).collect()
    }
}

fn position<R: std::borrow::Borrow<Rule>>(rules: &[R], n: usize) -> Option<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| !matches!((*rule).borrow(), Rule::Declarations(_)))
        .nth(n)
        .map(|(index, _)| index)
}

fn children<'a>(
    rule: &'a mut Rule,
    parent: &mut Option<SelectorList>,
) -> Result<&'a mut Vec<Rule>, &'static str> {
    match rule {
        Rule::Style(selectors, children) => {
            *parent = Some(selectors.clone());
            Ok(children)
        }
        Rule::Media(_, children)
        | Rule::Supports(_, _, children)
        | Rule::Layer(_, children)
        | Rule::Container(.., children) => Ok(children),
        _ => Err("HierarchyRequestError"),
    }
}

fn edit_rules<R: std::borrow::Borrow<Rule> + From<Rule>>(
    rules: &mut Vec<R>,
    last: usize,
    edit: Edit,
    parent: Option<SelectorList>,
    rule_mut: fn(&mut R) -> &mut Rule,
) -> Result<serde_json::Value, &'static str> {
    let at = position(rules, last);
    let parse = |text: &str| {
        let mut parsed = match &parent {
            Some(parent) => Rule::parse_nested(text, parent),
            None => Stylesheet::parse(text)
                .0
                .into_iter()
                .map(Rc::unwrap_or_clone)
                .collect(),
        };
        if parsed.len() != 1 || matches!(parsed[0], Rule::Declarations(_)) {
            return Err("SyntaxError");
        }
        Ok(parsed.remove(0))
    };
    if let Edit::Insert(text) = edit {
        let at = match at {
            Some(at) => at,
            None if last == 0 || position(rules, last - 1).is_some() => rules.len(),
            None => return Err("IndexSizeError"),
        };
        let rule = parse(text)?;
        let tree = rule.cssom();
        rules.insert(at, R::from(rule));
        return Ok(tree);
    }
    let index = at.ok_or("IndexSizeError")?;
    if matches!(edit, Edit::Delete) {
        rules.remove(index);
        return Ok(serde_json::Value::Null);
    }
    match (edit, rule_mut(&mut rules[index])) {
        (Edit::Declarations(text), Rule::Style(_, children)) => {
            children.retain(|rule| !matches!(rule, Rule::Declarations(_)));
            children.insert(0, Rule::Declarations(Declarations::parse(text)));
        }
        (Edit::Declarations(text), Rule::FontFace(values)) => {
            *values = Declarations::descriptors(text)
        }
        (Edit::Selector(text), Rule::Style(selectors, children)) => {
            let parsed = Selector::parse(text, parent.as_ref());
            if parsed.is_empty() {
                return Err("SyntaxError");
            }
            *selectors = Rc::new(parsed);
            for child in children {
                let _ = child.bind_parent(Some(selectors));
            }
            return Ok(selectors
                .iter()
                .map(Selector::source)
                .collect::<Vec<_>>()
                .join(", ")
                .into());
        }
        (Edit::Replace(text), rule) => {
            let mut new = parse(text)?;
            if std::mem::discriminant(&new) != std::mem::discriminant(rule) {
                return Err("SyntaxError");
            }
            if let (Ok(old), Ok(added)) = (children(rule, &mut None), children(&mut new, &mut None))
            {
                std::mem::swap(old, added);
            }
            *rule = new;
        }
        (Edit::Keyframe(text), Rule::Keyframes(name, frames)) => {
            let parsed = Stylesheet::parse(&format!("@keyframes {name}{{{text}}}"));
            match text.split_once('{') {
                Some(_) => match parsed.0.into_iter().next().map(Rc::unwrap_or_clone) {
                    Some(Rule::Keyframes(_, mut added)) if !added.is_empty() => {
                        let tree = Rule::Keyframes(String::new(), added.clone()).cssom()[2].clone();
                        frames.append(&mut added);
                        return Ok(tree);
                    }
                    _ => return Err("SyntaxError"),
                },
                None => {
                    let index: usize = text.parse().map_err(|_| "IndexSizeError")?;
                    if index >= frames.len() {
                        return Err("IndexSizeError");
                    }
                    frames.remove(index);
                }
            }
        }
        _ => return Err("InvalidModificationError"),
    }
    Ok(serde_json::Value::Null)
}
