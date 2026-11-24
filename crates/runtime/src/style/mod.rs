use crate::css::BORDERS;
use crate::css::LOGICAL;
use crate::css::counter_text;
use crate::css::number;
use crate::css::percent_sum;
use crate::css::percent_text;
use crate::css::{
    Declaration, Declarations, Declared, FontUnits, Lengths, MediaQuery, Prop, Registry, Rule,
    Style, Stylesheet, Vars, color, components, computed, expand, fold, syntax_matches,
};
use crate::css::{Selector, SelectorList};
use crate::dom::{Document, SelectorCache};
use bbtree::unsync::BBTreeMap;
use rustc_hash::FxHashMap;
use std::{borrow::Cow, collections::HashMap, rc::Rc};

mod animation;
mod chunked;
mod tailwind;
#[cfg(test)]
mod tests;
mod text;
pub(crate) use chunked::Chunked;

pub(crate) use animation::{Animation, Motion, Scripted};
pub(crate) use text::{
    Atom, CUT_AFTER, CUT_BEFORE, CUT_FIRST, CUT_LAST, InlineBox, InlineText, LineHeight, atom_key,
    cursor, font, holds_block_box, inline_text, line_height, makes_no_line, own_box,
    placed_out_of_flow, pseudo_blocks, pseudo_content, restyle, selection_colors, spacing, spliced,
    static_after, static_inline, text_run, text_transform, word_break,
};

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

/// A table cell or caption that a grid doesn't blockify: it takes percentages from its table, not from a grid area.
pub(crate) fn table_part(doc: &crate::dom::Document, styles: &ComputedStyles, id: usize) -> bool {
    matches!(role(styles, id), CELL | CAPTION)
        && doc.nodes[id].parent.is_none_or(|parent| {
            styles.values[parent]
                .get(Prop::DISPLAY)
                .is_none_or(|display| !display.ends_with("grid"))
        })
}

const UA: &str = "
html,body,address,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hr,legend,main,p,pre,search,article,aside,h1,h2,h3,h4,h5,h6,hgroup,nav,section,dd,dl,dt,menu,ol,ul,details,summary,fieldset,optgroup,option{display:block}
li{display:list-item}
img,svg,canvas,video,audio,iframe,embed,object,input,button,select,textarea,progress,meter{display:inline-block}
input:not([type=hidden i],[type=image i],[type=file i]),button,select,textarea,meter,progress{appearance:auto}
canvas[content=drawable i]{-canvas-content:drawable}
canvas[content=drawable i] [drawable]{isolation:isolate;contain:layout}
head,script,style,title,meta,link,template,noscript,datalist,base,param,[hidden]:not([hidden=until-found i]),input[type=hidden],dialog:not([open]){display:none}
[hidden=until-found i]{content-visibility:hidden}
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
label,button,select,input[type=button i],input[type=submit i],input[type=reset i],input[type=checkbox i],input[type=radio i],input[type=range i],input[type=color i],input[type=file i]{cursor:default}
svg:not(:root){overflow:hidden}
input{padding:1px 2px;border:2px inset light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
textarea{font-family:monospace;padding:2px;overflow:auto;border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
select{border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b);box-sizing:border-box;white-space:pre}
button,input[type=button i],input[type=submit i],input[type=reset i]{padding:1px 6px;border:2px outset light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box;text-align:center}
input[type=checkbox i],input[type=radio i]{margin:3px 3px 3px 4px;padding:0;border:0;background-color:transparent;box-sizing:border-box}
input[type=radio i]{margin:3px 3px 0 5px}
input[type=range i]{margin:2px;padding:0;border:0;background-color:transparent;color:light-dark(#101010,#fff)}
input[type=color i]{width:50px;height:27px;border:1px solid light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box}
input[type=file i]{padding:0;border:0;background-color:transparent}
input[type=file i]::file-selector-button{margin:0 4px 0 0;padding:2px 7px;border:1px solid light-dark(#767676,#858585);border-radius:2px;background-color:light-dark(#efefef,#6b6b6b)}
input[type=search i]{box-sizing:border-box}
summary{list-style-position:inside}
button:disabled,input[type=button i]:disabled,input[type=submit i]:disabled,input[type=reset i]:disabled{color:light-dark(rgba(16,16,16,.3),rgba(255,255,255,.3));background-color:light-dark(rgba(239,239,239,.3),rgba(19,1,1,.3));border-color:light-dark(rgba(118,118,118,.3),rgba(195,195,195,.3))}
input:disabled,textarea:disabled{color:light-dark(#545454,#aaa)}
input:disabled:not([type=checkbox i],[type=radio i],[type=range i],[type=color i],[type=file i],[type=button i],[type=submit i],[type=reset i]),textarea:disabled{background-color:light-dark(rgba(239,239,239,.3),rgba(59,59,59,.3));border-color:rgba(118,118,118,.3)}
select:disabled{opacity:.7;color:light-dark(#808080,#aaa);border-color:rgba(118,118,118,.3)}
:focus-visible{outline:auto 1px -webkit-focus-ring-color}
input:focus-visible,textarea:focus-visible,select:focus-visible{outline-offset:0}
:-webkit-any-link:focus-visible{outline-offset:1px}
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
    // `container-type: normal` with a name, for named `style()` queries only.
    sized: bool,
    size: bool,
    names: Option<Rc<str>>,
    // Custom properties of a named container, only while a sheet has named `style()` queries.
    vars: Option<Vars>,
    // Queries in `em` depend on the font size.
    font: Option<Rc<str>>,
    up: Option<Rc<Container>>,
}

impl Container {
    fn of(
        id: usize,
        style: &Style,
        vars: Option<&Vars>,
        up: Option<Rc<Container>>,
    ) -> Option<Self> {
        let kind = style.get(Prop::CONTAINER_TYPE).unwrap_or("normal");
        let sized = matches!(kind, "size" | "inline-size");
        let names = style
            .value(Prop::CONTAINER_NAME)
            .filter(|names| &***names != "none")
            .cloned();
        (sized || names.is_some()).then(|| Self {
            id,
            sized,
            size: kind == "size",
            vars: vars.filter(|_| names.is_some()).cloned(),
            names,
            font: sized
                .then(|| style.value(Prop::FONT_SIZE).cloned())
                .flatten(),
            up,
        })
    }
}

// A `style()` query of custom properties against `vars`, the parent's, which adds the names it reads to
// `names`. `None` for other queries.
fn style_query(query: &MediaQuery, vars: Option<&Vars>, names: &mut Vec<Prop>) -> Option<bool> {
    match query {
        MediaQuery::Feature(kind, name, value) if kind == "style" => {
            // Interned, so a name that no rule declares is still read.
            let prop = Prop::new(name);
            names.push(prop);
            let found = vars.and_then(|vars| var(vars, prop));
            Some(found.is_some_and(|found| value.is_empty() || found.trim() == value))
        }
        MediaQuery::Not(query) => style_query(query, vars, names).map(|matched| !matched),
        MediaQuery::And(items) | MediaQuery::Or(items) => {
            let and = matches!(query, MediaQuery::And(_));
            let mut result = and;
            for item in items {
                if style_query(item, vars, names)? != and {
                    result = !and;
                }
            }
            Some(result)
        }
        _ => None,
    }
}

fn query_containers(
    conditions: &[(Option<String>, bool, MediaQuery)],
    chain: Option<&Container>,
    (scope, order): (usize, usize),
    sizes: &FxHashMap<usize, [f32; 2]>,
    lengths: impl Fn(usize) -> Lengths,
    queries: &mut QueryCache,
    (vars, names): (Option<&Vars>, &std::cell::RefCell<Vec<Prop>>),
) -> bool {
    conditions
        .iter()
        .enumerate()
        .all(|(index, (name, block, query))| {
            let named = |container: &Container| {
                name.as_ref().is_none_or(|name| {
                    container.names.as_ref().is_some_and(|names| {
                        names.split_ascii_whitespace().any(|item| item == name)
                    })
                })
            };
            let mut link = chain;
            // A named style query reads the nearest container of that name, of any type. The chain holds its
            // values, so a change gives a new chain and needs no reads.
            let mut names = names.borrow_mut();
            if name.is_some() {
                let mut found = chain;
                while let Some(container) = found.filter(|container| !named(container)) {
                    found = container.up.as_deref();
                }
                let Some(container) = found else {
                    return false;
                };
                let read = names.len();
                let matched = style_query(query, container.vars.as_ref(), &mut names);
                names.truncate(read);
                if let Some(matched) = matched {
                    return matched;
                }
            } else if let Some(matched) = style_query(query, vars, &mut names) {
                return matched;
            }
            drop(names);
            while let Some(container) = link {
                if container.sized && (!block || container.size) && named(container) {
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
    // Warning: the cascade resets native controls, so inputs that match the same rules can differ.
    control: Option<renderer::ControlKind>,
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

type Key = Box<[(u8, String)]>;

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
    buckets: [FxHashMap<String, Vec<usize>>; 5],
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
    named: bool,
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
                        let key: Key = pseudo
                            .keys()
                            .into_iter()
                            .map(|(kind, name)| (kind, name.into_owned()))
                            .collect();
                        let index = PSEUDO | self.pseudos.len();
                        for (kind, name) in &key {
                            self.buckets[*kind as usize]
                                .entry(name.clone())
                                .or_default()
                                .push(index);
                        }
                        if key.is_empty() {
                            self.universal.push(index);
                        }
                        self.pseudos.push((kind, pseudo, order, key));
                        return None;
                    }
                    let key: Key = selector
                        .keys()
                        .into_iter()
                        .map(|(kind, name)| (kind, name.into_owned()))
                        .collect();
                    let entry = order << SELECTOR | index;
                    for (kind, name) in &key {
                        self.buckets[*kind as usize]
                            .entry(name.clone())
                            .or_default()
                            .push(entry);
                    }
                    if key.is_empty() {
                        self.universal.push(entry);
                    }
                    Some((index, key))
                })
                .collect();
            self.elements.push(elements);
            self.named |= container.as_ref().is_some_and(|container| {
                container.iter().any(|(name, _, query)| {
                    name.is_some() && style_query(query, None, &mut Vec::new()).is_some()
                })
            });
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
        let mut names: [rustc_hash::FxHashSet<&str>; 5] = Default::default();
        for key in keys.chain(added.map(|pseudo| &pseudo.3)) {
            if key.is_empty() {
                self.nodes.clear();
                return false;
            }
            for (kind, name) in key {
                names[*kind as usize].insert(name);
            }
        }
        let [ids, classes, tags, attributes, values] = &names;
        let mut pair = String::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            let class = |name: &String| {
                name.split_ascii_whitespace()
                    .any(|class| classes.contains(class))
            };
            let lower = crate::css::lowercase;
            if (tags.contains(&*lower(&node.tag))
                || !attributes.is_empty()
                    && node
                        .attrs
                        .keys()
                        .any(|name| attributes.contains(&*lower(name)))
                || !values.is_empty()
                    && node.attrs.iter().any(|(name, value)| {
                        pair.clear();
                        pair.push_str(&lower(name));
                        pair.push('=');
                        pair.push_str(value);
                        values.contains(pair.as_str())
                    })
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
    // Resized containers with nodes that use container units.
    pub(crate) units: Vec<usize>,
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
    named: bool,
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
            self.sized.remove(&id);
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
        self.sized.retain(|&id| id < len);
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
        self.units.clear();
        if *sizes == self.containers {
            return false;
        }
        // Styles with container units are not shared, so only the nodes in a resized container need a new style.
        let mut flipped: rustc_hash::FxHashSet<usize> = self
            .sized
            .iter()
            .copied()
            .filter(|id| sizes.get(id) != self.containers.get(id))
            .collect();
        self.units.extend(&flipped);
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
                flipped.insert(container);
            }
        }
        self.containers = sizes.clone();
        if flipped.is_empty() {
            return false;
        }
        self.queries
            .retain(|&(_, _, _, container), _| !flipped.contains(&container));
        let mut pending: Vec<_> = flipped.iter().map(|&id| (id, true)).collect();
        while let Some((id, root)) = pending.pop() {
            // A flipped container inside another one is walked once, as its own root.
            let Some(node) = doc.nodes.get(id).filter(|_| root || !flipped.contains(&id)) else {
                continue;
            };
            pending.extend(
                self.shadows
                    .composed
                    .get(&id)
                    .unwrap_or(&node.children)
                    .iter()
                    .map(|&child| (child, false)),
            );
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

impl<'a> Collect<'a> {
    fn rule(&mut self, item: &'a Rule, parent: Option<&'a SelectorList>, layer: Option<usize>) {
        match item {
            Rule::Declarations(values) => {
                if let Some(parent) = parent {
                    let rank = layer.map_or(UNLAYERED, |index| index as u32);
                    self.rules.push((
                        parent,
                        values,
                        Layer {
                            origin: AUTHOR,
                            rank,
                        },
                        self.container.clone(),
                        self.media.clone(),
                    ));
                }
            }
            Rule::Theme(_, children) => {
                for rule in children {
                    if matches!(rule, Rule::Keyframes(..)) {
                        self.rule(rule, parent, layer);
                    }
                }
            }
            Rule::Variant(..) | Rule::Utility(..) | Rule::FontFace(_) | Rule::Tailwind(_) => {}
            Rule::Property(name, ..) | Rule::Keyframes(name, _) => {
                let path = |index: Option<usize>| {
                    index
                        .map_or(&[][..], |index| &self.layers[index].1[..])
                        .iter()
                        .copied()
                        .chain([u32::MAX])
                };
                let key = (matches!(item, Rule::Property(..)), name.clone());
                if self
                    .layered
                    .get(&key)
                    .is_some_and(|&old| path(old).gt(path(layer)))
                {
                    return;
                }
                self.layered.insert(key, layer);
                match item {
                    Rule::Property(name, syntax, inherits, initial) => {
                        self.properties.insert(
                            Prop::new(name),
                            (
                                syntax.as_str().into(),
                                *inherits,
                                initial.as_deref().map(Rc::from),
                            ),
                        );
                    }
                    Rule::Keyframes(name, frames) => {
                        self.keyframes.insert(name.clone(), frames.clone());
                    }
                    _ => {}
                }
            }
            Rule::Style(selectors, children) => {
                for rule in children {
                    self.rule(rule, Some(selectors), layer);
                }
            }
            Rule::Layer(names, children) => {
                let names = if names.is_empty() {
                    vec![format!("__anonymous_{}", self.layers.len())]
                } else {
                    names.clone()
                };
                for name in names {
                    let name = match layer {
                        Some(outer) => format!("{}.{name}", self.layers[outer].0),
                        None => name,
                    };
                    let (mut route, mut index) = (Vec::new(), 0);
                    for (end, _) in name.match_indices('.').chain([(name.len(), "")]) {
                        let path = &name[..end];
                        let owner = path.rsplit_once('.').map_or("", |(owner, _)| owner);
                        let found = self.layers.iter().position(|(name, _)| name == path);
                        index = found.unwrap_or(self.layers.len());
                        route = match found {
                            Some(found) => self.layers[found].1.clone(),
                            None => {
                                let siblings = self
                                    .layers
                                    .iter()
                                    .filter(|(name, _)| {
                                        name.rsplit_once('.').map_or("", |(owner, _)| owner)
                                            == owner
                                    })
                                    .count();
                                route.push(siblings as u32);
                                self.layers.push((path.to_owned(), route.clone()));
                                route
                            }
                        };
                    }
                    for rule in children {
                        self.rule(rule, parent, Some(index));
                    }
                }
            }
            Rule::Starting(children) => {
                for rule in children {
                    self.rule(rule, parent, layer);
                }
            }
            Rule::Supports(_, supported, children) => {
                if *supported {
                    for rule in children {
                        self.rule(rule, parent, layer);
                    }
                }
            }
            Rule::Container(name, block, query, children) => {
                let outer = self.container.clone();
                self.container = Some(
                    outer
                        .iter()
                        .flat_map(|conditions| conditions.iter().cloned())
                        .chain([(name.clone(), *block, query.clone())])
                        .collect(),
                );
                for rule in children {
                    self.rule(rule, parent, layer);
                }
                self.container = outer;
            }
            Rule::Media(queries, children) => {
                let matched = self.media(queries);
                let fixed = !children.iter().all(Rule::plain);
                let block = Rc::new(MediaBlock {
                    queries: queries.clone(),
                    matched: matched.into(),
                    fixed,
                });
                self.blocks.push(block.clone());
                if matched || !fixed {
                    let outer = self.media.clone();
                    if !fixed {
                        self.media = Some(
                            outer
                                .iter()
                                .flat_map(|stack| stack.iter().cloned())
                                .chain([block])
                                .collect(),
                        );
                    }
                    for rule in children {
                        self.rule(rule, parent, layer);
                    }
                    self.media = outer;
                }
            }
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
pub(crate) struct NodeLengths {
    ids: Chunked<u32>,
    list: Vec<Lengths>,
    last: u32,
}

impl NodeLengths {
    pub(crate) fn reset(&mut self, count: usize, lengths: Lengths) {
        self.ids.clear();
        self.ids.resize(count, 0);
        self.list.clear();
        self.list.push(lengths);
        self.last = 0;
    }

    pub(crate) fn set(&mut self, id: usize, lengths: Lengths) {
        if self.list[self.last as usize] != lengths {
            // Only the newest values are searched, or a page with a font size per node gets quadratic.
            let mut newest = self.list.iter().enumerate().rev().take(32);
            self.last = match newest.find(|(_, old)| **old == lengths) {
                Some((index, _)) => index as u32,
                None => {
                    self.list.push(lengths);
                    self.list.len() as u32 - 1
                }
            };
        }
        self.ids.set(id, self.last, u32::eq);
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Lengths> {
        self.ids.get(id).map(|&index| &self.list[index as usize])
    }

    pub(crate) fn grow(&mut self, count: usize) {
        self.ids.resize(count, 0);
    }

    pub(crate) fn unset(&mut self, id: usize) {
        self.ids[id] = 0;
    }

    pub(crate) fn list_len(&self) -> usize {
        self.list.len()
    }
}

impl std::ops::Index<usize> for NodeLengths {
    type Output = Lengths;

    fn index(&self, id: usize) -> &Lengths {
        &self.list[self.ids[id] as usize]
    }
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
    pub(crate) counting: FxHashMap<usize, [Option<Rc<str>>; 2]>,
    pub registry: Rc<Registry>,
    pub authored: FxHashMap<usize, Rc<Style>>,
    // The own flags of the nodes that an anonymous table changed. Their own styles are in `authored`.
    anonymous: FxHashMap<usize, u16>,
    containers: Chunked<Option<Rc<Container>>>,
    counted: bool,
    // The node revision of the document that this pass styled.
    revision: u64,
    // The native theme bits that this pass styled with.
    theme: u32,
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
    // The generation and revision of each dependency key in the list of the current node.
    let (mut unique, mut generation) = (Vec::<(u32, u32)>::new(), 0u32);
    let (reduced_motion, native_theme) = context.map_or_else(
        || (false, 0),
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
                added.rule(rule, None, None);
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
                    // A sheet in the place of one that left, like a `<style>` with new text, loses all old rules.
                    Some(previous)
                        if !sheets.iter().any(|sheet| Rc::ptr_eq(sheet, previous))
                            && !old.iter().any(|previous| Rc::ptr_eq(previous, sheet)) =>
                    {
                        next += 1;
                        flags.fill(true);
                        gone.extend((0..previous.0.len()).map(|index| (previous.clone(), index)));
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
                        collected.rule(rule, None, None);
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
                        removed.rule(rule, None, None);
                    }
                }
                let cached = &mut style_cache.scopes[index];
                let removed: rustc_hash::FxHashSet<_> = removed
                    .rules
                    .iter()
                    .map(|rule| Rc::as_ptr(rule.0))
                    .collect();
                let mut deleted: Vec<bool> = (cached.rules.iter())
                    .map(|rule| removed.contains(&Rc::as_ptr(&rule.0)))
                    .collect();
                // Preserve deleted rule positions with DELETED origin and no declarations or keys.
                let mut author: Vec<usize> = (0..cached.rules.len())
                    .filter(|&order| {
                        !matches!(cached.rules[order].2.origin, AGENT | DELETED) && !deleted[order]
                    })
                    .collect();
                let dead = (cached.rules.iter().zip(&deleted))
                    .filter(|(rule, deleted)| **deleted || rule.2.origin == DELETED)
                    .count();
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
                // A scope with more deleted rules than live ones is built again, so it does not grow.
                let mut room = same && dead * 2 <= cached.rules.len() + new.len() - old;
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
                    let plain =
                        local && cached.keyframes == keyframes && cached.properties == properties;
                    // The dirty marks stand for the new rules in the last stored pass.
                    match style_cache.styled.as_mut().filter(|_| plain) {
                        Some(styled) => {
                            for old in styled.scopes.iter_mut().filter(|old| old.1 == cached.id) {
                                old.2 = cached.rules.len();
                            }
                        }
                        None => fast = false,
                    }
                    if deleted.contains(&true) {
                        deleted.resize(cached.rules.len(), false);
                        for order in (0..deleted.len()).filter(|&order| deleted[order]) {
                            cached.rules[order].1 = Rc::new([]);
                            cached.rules[order].2.origin = DELETED;
                            cached.elements[order].clear();
                        }
                        let gone = |entry: &usize| {
                            if *entry >= PSEUDO {
                                deleted[cached.pseudos[entry & !PSEUDO].2]
                            } else {
                                deleted[entry >> SELECTOR]
                            }
                        };
                        for bucket in &mut cached.buckets {
                            bucket.retain(|_, list| {
                                list.retain(|entry| !gone(entry));
                                !list.is_empty()
                            });
                        }
                        cached.universal.retain(|entry| !gone(entry));
                        let stale = |rules: &[(usize, [usize; 3])], pseudos: &[usize]| {
                            rules.iter().any(|&(order, _)| deleted[order])
                                || pseudos
                                    .iter()
                                    .any(|&index| deleted[cached.pseudos[index].2])
                        };
                        // A fast pass styles these nodes again by their dirty marks.
                        for (id, matched) in cached.nodes.iter_mut().enumerate() {
                            if matched
                                .as_ref()
                                .is_some_and(|matched| stale(&matched.rules, &matched.pseudos))
                            {
                                *matched = None;
                                style_cache.matches.mark(id, false);
                            }
                        }
                        style_cache.values.retain(|key, _| {
                            key.scope != cached.id || !stale(&key.rules.0, &key.pseudos)
                        });
                        // The interaction states come from the rules that are left.
                        let mut lists: Vec<_> = (cached.rules.iter())
                            .filter(|rule| rule.2.origin != DELETED)
                            .map(|rule| rule.0.clone())
                            .collect();
                        let (mut seen, mut parents, mut subject) =
                            (std::collections::HashSet::new(), Vec::new(), true);
                        cached.states = 0;
                        while let Some(list) = lists.pop().or_else(|| {
                            subject = false;
                            parents.pop()
                        }) {
                            if seen.insert((Rc::as_ptr(&list), subject)) {
                                for selector in list.iter() {
                                    cached.states |= selector.states(&mut parents);
                                }
                            }
                        }
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
                    collected.rule(rule, None, None);
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
                collected.rule(rule, None, None);
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
            let old = &*style_cache.environment.registry;
            let changed: rustc_hash::FxHashSet<Prop> = registry
                .iter()
                .filter(|(prop, value)| old.get(prop) != Some(value))
                .map(|(prop, _)| *prop)
                .chain(
                    old.keys()
                        .filter(|prop| !registry.contains_key(prop))
                        .copied(),
                )
                .collect();
            // A registration changes only the styles that hold the name or read it, as `var()` reads initial values.
            let holds = |vars: &Vars| vars.iter().any(|(prop, _)| changed.contains(prop));
            let touched =
                |value: &CachedStyle| {
                    let vars = value.vars.borrow();
                    value.animations.is_some()
                        || value.reads.as_ref().is_none_or(|reads| {
                            reads.iter().any(|(prop, _)| changed.contains(prop))
                        })
                        || value.missing.iter().any(|name| {
                            Prop::find(name).is_some_and(|prop| changed.contains(&prop))
                        })
                        || vars.0.as_ref().is_some_and(holds)
                        || holds(&vars.1)
                };
            style_cache.values.retain(|_, values| {
                values.retain(|value| !touched(value));
                !values.is_empty()
            });
            for scope in &mut style_cache.scopes {
                for (id, node) in scope.nodes.iter_mut().enumerate() {
                    let Some(node) = node else { continue };
                    if node.cached.as_ref().is_none_or(|cached| {
                        touched(&cached.value)
                            || cached.inherited.as_ref().is_some_and(holds)
                            || holds(&cached.vars)
                    }) {
                        node.cached = None;
                        style_cache.matches.mark(id, false);
                    }
                }
            }
            style_cache.environment.registry = Rc::new(registry);
            style_cache.inheritable.clear();
            if let Some(styled) = &mut style_cache.styled {
                styled.environment.registry = style_cache.environment.registry.clone();
            }
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
    out.theme = native_theme;
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
        revision,
        ..
    } = &mut out;
    *revision = doc.nodes.latest();
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
    // A fast pass numbers the counters again only when one of their inputs changed.
    let mut recount = false;
    let item = |style: &Style| style.get(Prop::DISPLAY) == Some("list-item");
    let listed = |id: usize| matches!(doc.nodes[id].tag.as_str(), "ol" | "ul" | "menu" | "li");
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
            // Text and comments that move need no new count.
            let changed = walk.walk.changed_since(old.serial).map_or(0, <[u32]>::len);
            recount = walk.walk.since(old.serial).is_none_or(|touched| {
                touched[..touched.len() - changed].iter().any(|&id| {
                    doc.nodes
                        .get(id as usize)
                        .is_none_or(|node| !node.tag.starts_with('#'))
                })
            });
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
    // Named `style()` queries put the values of containers into the chain, so all chains change with them.
    let named_queries = style_cache.scopes.iter().any(|scope| scope.named);
    let mut all = std::mem::replace(&mut style_cache.named, named_queries) != named_queries;
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
                if !recount {
                    let old = previous.anonymous.get(&id).or(previous.flags.get(id));
                    let old = old.copied().unwrap_or(0);
                    let before = previous.authored.get(&id).or(previous.values.get(id));
                    recount = (old | style_flags[id]) & COUNTED != 0
                        || (old ^ style_flags[id]) & DISPLAY_NONE != 0
                        || listed(id)
                        || item(&styles[id])
                        || before.is_some_and(|before| item(before));
                }
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
            let link = match Container::of(id, &styles[id], named_queries.then(|| &custom[id]), $seen.clone()) {
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
        // Custom properties of the parent that `style()` queries read.
        let style_reads = std::cell::RefCell::new(Vec::new());
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
                    (parent_vars, &style_reads),
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
                        key.is_empty()
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
                    // Keys are lowercase. Only SVG names have uppercase letters, so HTML names need no copy.
                    let lower = crate::css::lowercase;
                    for (kind, name) in std::iter::once((2, lower(&node.tag)))
                        .chain(node.attrs.get("id").map(|id| (0, id.as_str().into())))
                        .chain(classes.flat_map(|c| {
                            c.split_ascii_whitespace().map(|class| (1, class.into()))
                        }))
                        .chain(node.attrs.keys().map(|name| (3, lower(name))))
                    {
                        if let Some(list) = scoped.buckets[kind].get(&*name) {
                            candidates.extend(list);
                        }
                    }
                    if !scoped.buckets[4].is_empty() {
                        let mut pair = String::new();
                        for (name, value) in node.attrs.iter() {
                            pair.clear();
                            pair.push_str(&lower(name));
                            pair.push('=');
                            pair.push_str(value);
                            if let Some(list) = scoped.buckets[4].get(pair.as_str()) {
                                candidates.extend(list);
                            }
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
                    // Each candidate selector reads the node again, so most entries repeat: drop them before sorting.
                    generation = generation.wrapping_add(1);
                    if generation == 0 {
                        unique.fill((0, 0));
                        generation = 1;
                    }
                    matches.dependencies.retain(|&(key, revision)| {
                        let key = key as usize;
                        if key >= unique.len() {
                            unique.resize(key + 1, (0, 0));
                        }
                        let seen = std::mem::replace(&mut unique[key], (generation, revision));
                        seen != (generation, revision)
                    });
                    matches.dependencies.sort_unstable();
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
                        (parent_vars, &style_reads),
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
                    } else if node.tag == "svg"
                        && matches!(name.as_ref(), "width" | "height")
                        // As in Chrome, only the outermost `<svg>` takes them as CSS lengths.
                        && !std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                            .any(|parent| doc.nodes[parent].tag == "svg")
                    {
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
                    } else if let Some(prop) = match (node.tag.as_str(), name.as_ref()) {
                        // As in Chrome, geometry attributes are presentation hints of the shapes that use them.
                        ("circle" | "ellipse", "cx" | "cy")
                        | ("circle", "r")
                        | ("ellipse" | "rect", "rx" | "ry")
                        | ("rect" | "image" | "foreignObject" | "svg" | "use", "x" | "y")
                        | ("rect" | "image" | "foreignObject", "width" | "height") => {
                            Prop::find(name)
                        }
                        _ => None,
                    } {
                        // As in Chrome, a negative size is no hint, and a corner radius that is no length is 0.
                        let value = value.trim();
                        match value.parse::<f32>() {
                            _ if value.starts_with('-')
                                && matches!(prop, Prop::WIDTH | Prop::HEIGHT) => {}
                            Ok(_) => hints.push((prop, format!("{value}px").into())),
                            _ if Lengths::new(16., 100., 100.).resolve(value, 100.).is_some() => {
                                hints.push((prop, value.into()))
                            }
                            _ if matches!(prop, Prop::RX | Prop::RY) => {
                                hints.push((prop, "0px".into()))
                            }
                            _ => {}
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
                // As in Chrome, `width` and `height` of a video give `aspect-ratio: auto <w> / <h>`.
                if node.tag == "video"
                    && let [Some(width), Some(height)] = ["width", "height"].map(|name| {
                        let value = node.attrs.get(name)?.trim();
                        value.parse::<f32>().ok().filter(|v| *v > 0.).map(|_| value)
                    })
                {
                    hints.push((
                        Prop::ASPECT_RATIO,
                        format!("auto {width} / {height}").into(),
                    ));
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
                    control: node.system_control(),
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
                        // The key doesn't hold what `style()` queries read, so the shared style must read it.
                        if value.inline.as_deref() != source.as_deref()
                            || style_reads.borrow().iter().any(|prop| {
                                value
                                    .reads
                                    .as_ref()
                                    .is_none_or(|reads| reads.iter().all(|(read, _)| read != prop))
                            })
                        {
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
        let mut reads = Some(style_reads.take());
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
            native_theme,
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
            style.insert_current(Prop::TEXT_DECORATION_COLOR, ink);
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
        // AppKit draws the control, so the box has no padding, border or background.
        if node.native_control(&style).is_some() {
            for prop in crate::css::PADDING
                .into_iter()
                .chain(
                    crate::css::BORDERS
                        .into_iter()
                        .flat_map(|side| [side[0], side[1]]),
                )
                .chain([Prop::BACKGROUND_COLOR, Prop::BACKGROUND_IMAGE])
            {
                style.remove(prop);
            }
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
        if queried
            && let Some(container) =
                std::iter::successors(seen.as_deref(), |link| link.up.as_deref())
                    .find(|link| link.sized)
        {
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
        let ratio = ((native_theme >> 16) as f32).max(100.) / 100.;
        if let Some(text) = style.get(Prop::LINE_HEIGHT) {
            let mut line_units = units;
            line_units.font.lh = parent.lh;
            if node.tag == "html" {
                line_units.root = units.font;
                line_units.root.lh = parent.lh;
            }
            if let Some((value, dimensional)) = line_units.parse(text, size)
                && value >= 0.
            {
                units.font.lh =
                    if dimensional { value } else { value * size }.min(33_554_431. / ratio);
                style.insert(
                    Prop::LINE_HEIGHT,
                    // Like Chrome, whose layout units count device pixels, a length is at most 2^25 device
                    // pixels, and a number gives at most the f32 range in device pixels.
                    if dimensional {
                        format!("{}px", value.min(33_554_432. / ratio))
                    } else if value >= 33_554_428. {
                        units
                            .math_unclamped(text, f32::NAN, &[])
                            .map_or(value, |(number, _)| {
                                (number as f32).min(f32::MAX / ratio / size)
                            })
                            .to_string()
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        // Like Chrome, border and outline widths snap to device pixels, and a width or offset under one device
        // pixel is one device pixel.
        for prop in [
            Prop::BORDER_TOP_WIDTH,
            Prop::BORDER_RIGHT_WIDTH,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::BORDER_LEFT_WIDTH,
            Prop::OUTLINE_WIDTH,
            Prop::COLUMN_RULE_WIDTH,
            Prop::OUTLINE_OFFSET,
        ] {
            let Some(value) = style.get(prop) else {
                continue;
            };
            // Keywords and whole device pixels, the common case, stay without parsing.
            let whole = |number: &str| {
                number
                    .parse::<f32>()
                    .is_ok_and(|number| (number * ratio).fract() == 0.)
            };
            if matches!(value, "0" | "thin" | "medium" | "thick")
                || value.strip_suffix("px").is_some_and(whole)
            {
                continue;
            }
            let Some(width) = units.resolve(value, f32::NAN) else {
                continue;
            };
            let device = width * ratio;
            let snapped = match device {
                _ if device != 0. && device.abs() < 1. => device.signum(),
                _ => device.trunc(),
            } / ratio;
            if snapped != width {
                style.insert(prop, format!("{}px", number(f64::from(snapped))));
            }
        }
        if id == 0 || node.tag == "html" {
            root = units.font;
        }
        units.root = root;
        style.insert(Prop::FONT_SIZE, format!("{size}px"));
        // Like Chrome, these keywords are 1, 3 and 5 24ths of the font size.
        if let Some(index) = (style.get(Prop::WEBKIT_TEXT_STROKE_WIDTH)).and_then(|value| {
            ["thin", "medium", "thick"]
                .iter()
                .position(|name| *name == value)
        }) {
            let width = units.font.em * (index * 2 + 1) as f32 / 24.;
            style.insert(Prop::WEBKIT_TEXT_STROKE_WIDTH, format!("{width}px"));
        }
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
        let own = Container::of(
            id,
            &style,
            named_queries.then(|| style.vars()),
            seen.clone(),
        );
        let mut deferred = false;
        let mut query = |conditions: &ContainerQuery, key: (usize, usize)| {
            deferred = true;
            query_containers(
                conditions,
                own.as_ref().or(seen.as_deref()),
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
                (Some(style.vars()), &style_reads),
            )
        };
        // Pseudo-elements query their own element, so their conditional rules are checked again here.
        let again: Option<Vec<usize>> = (own.is_some()
            || scoped.nodes[id].iter().any(|matched| {
                matched
                    .pseudos
                    .iter()
                    .any(|&index| scoped.containers[scoped.pseudos[index].2].is_some())
            }))
        .then(|| {
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
        if let Some(reads) = &mut reads {
            reads.extend(style_reads.take());
        }
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
                native_theme,
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
                resolve_style(
                    &mut values,
                    Some(&style),
                    None,
                    &declared,
                    &registry,
                    native_theme,
                );
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
                native_theme,
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
                    Animation::parse(&style, &scoped.keyframes, &registry, native_theme)
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
                        Animation::parse(pseudo, &scoped.keyframes, &registry, native_theme)
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
    let reused = previous
        .filter(|previous| fast && previous.counted)
        .filter(|previous| {
            // Attributes like `start`, `reversed` and `value` can change without a new style.
            !recount
                && !doc.nodes.changed_since(previous.revision).any(|id| {
                    listed(id)
                        || item(&styles[id])
                        || previous.values.get(id).is_some_and(|old| item(old))
                        || doc.nodes[id]
                            .children
                            .iter()
                            .any(|&child| item(&styles[child]))
                })
        });
    if let Some(previous) = reused {
        *counted = true;
        list_items.clone_from(&previous.list_items);
    } else if counters || previous.is_some_and(|previous| fast && previous.counted) {
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
                    Some(counter) => counter.1 = counter.1.saturating_add(step),
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
                // Like Chrome, lists reset `list-item` unless their `counter-reset` names it.
                if matches!(node.tag.as_str(), "ol" | "ul" | "menu")
                    && style
                        .get(Prop::COUNTER_RESET)
                        .is_none_or(|reset| !components(reset, ' ').contains(&"list-item"))
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
                        start.unwrap_or(items).saturating_add(1)
                    } else {
                        start.unwrap_or(1).saturating_sub(1)
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
                        // A cycle is invalid at computed-value time, so a registered property in it is unset.
                        value = (ok && reached > depth).then(|| out.into());
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
    theme: u32,
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
                        if !crate::css::invalid(shorthand, value) {
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
        .filter(|value| !crate::css::invalid(prop.name(), value))
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
    let dark = dark(style, theme);
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
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'-')
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

pub(crate) fn dark(style: &Style, theme: u32) -> bool {
    style
        .get(Prop::COLOR_SCHEME)
        .is_some_and(|v| v.contains("dark") && (!v.contains("light") || theme & 1 != 0))
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
    if positioned
        && style
            .get(Prop::DISPLAY)
            .is_none_or(|display| display.starts_with("inline"))
    {
        style.insert(Prop::INTERNAL_STATIC_INLINE, "1");
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
                    // A percentage resolves later, so only the lengths of its sum become pixels.
                    _ if token.contains('%') => {
                        percent_sum(token, units).map_or_else(|| token.to_owned(), percent_text)
                    }
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
            // A `currentcolor` of the parent stays a keyword for the child.
            Some(_) if parent.is_some_and(|parent| parent.current(prop)) => {
                *value = "currentcolor".into();
                true
            }
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
    style.retain_current(|prop, slot| {
        if prop.is_custom() || prop == Prop::COLOR || prop.inherited() {
            return (true, false);
        }
        if slot.eq_ignore_ascii_case("currentcolor") {
            *slot = current.clone();
            return (true, true);
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
        (true, false)
    });
    keyword
}
