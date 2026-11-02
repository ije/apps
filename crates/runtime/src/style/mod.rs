use crate::dom::{Document, Selector, SelectorCache, SelectorList};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::{borrow::Cow, collections::HashMap, rc::Rc};

mod animation;
mod color;
pub(crate) mod parser;
mod props;
mod shorthand;
mod tailwind;
#[cfg(test)]
mod tests;
mod text;
mod value;

pub(crate) use animation::{Animation, Motion};
pub use color::color;
pub(crate) use parser::{DROPPED, FontUnits, Lengths};
use shorthand::expand;
pub(crate) use text::{
    InlineText, LineHeight, cursor, font, inline_text, line_height, phantom, restyle,
    selection_colors, spacing, splits, text_run, text_transform, word_break,
};
pub use value::length;
pub(crate) use value::{BORDERS, Filter, MARGIN, PADDING, clip_path, components};

pub(crate) use props::Prop;
pub use props::Style;

pub(crate) const DISPLAY_NONE: u8 = 1;
pub(crate) const DISPLAY_CONTENTS: u8 = 2;
pub(crate) const INVISIBLE: u8 = 4;
pub(crate) const SELECT_NONE: u8 = 8;
pub(crate) const SELECT_ALL: u8 = 16;
pub(crate) const SELECT_TEXT: u8 = 32;

fn flags(style: &Style) -> u8 {
    let get = |prop| style.get(prop).unwrap_or("");
    let flag = |set, value| if set { value } else { 0 };
    flag(get(Prop::DISPLAY) == "none", DISPLAY_NONE)
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
}

const UA: &str = "
html,body,address,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hr,legend,main,p,pre,search,article,aside,h1,h2,h3,h4,h5,h6,hgroup,nav,section,dd,dl,dt,menu,ol,ul,details,summary,fieldset,optgroup,option,caption,td,th{display:block}
li{display:list-item}
img,svg,canvas,video,audio,iframe,embed,object,input,button,select,textarea,progress,meter{display:inline-block}
head,script,style,title,meta,link,template,noscript,datalist,base,[hidden],input[type=hidden],dialog:not([open]),audio:not([controls]){display:none}
video{object-fit:contain}
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
table[border]{border:1px outset}
table[border] td,table[border] th{border:1px inset}
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
input[type=checkbox i],input[type=radio i]{width:13px;height:13px;margin:3px 3px 3px 4px;padding:0;border:0;background-color:transparent;box-sizing:border-box}
input[type=radio i]{margin:3px 3px 0 5px}
input[type=range i]{width:129px;height:16px;margin:2px;padding:0;border:0;background-color:transparent;color:light-dark(#101010,#fff)}
input[type=color i]{width:50px;height:27px;border:1px solid light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box}
input[type=file i]{padding:0;border:0;background-color:transparent}
input[type=search i]{box-sizing:border-box}
summary{list-style-position:inside}
apps-shadow-root{display:block}
button:disabled,input[type=button i]:disabled,input[type=submit i]:disabled,input[type=reset i]:disabled{color:light-dark(rgba(16,16,16,.3),rgba(255,255,255,.3));background-color:light-dark(rgba(239,239,239,.3),rgba(19,1,1,.3));border-color:light-dark(rgba(118,118,118,.3),rgba(195,195,195,.3))}
input:disabled,textarea:disabled{color:light-dark(#545454,#aaa)}
input:disabled:not([type=checkbox i],[type=radio i],[type=range i],[type=color i],[type=file i],[type=button i],[type=submit i],[type=reset i]),textarea:disabled{background-color:light-dark(rgba(239,239,239,.3),rgba(59,59,59,.3));border-color:rgba(118,118,118,.3)}
select:disabled{opacity:.7;color:light-dark(#808080,#aaa);border-color:rgba(118,118,118,.3)}
input:focus-visible,textarea:focus-visible{outline:3px solid #8fb8f4}
progress,meter{width:10em;height:1em;vertical-align:-.2em;box-sizing:border-box}
meter{width:5em}
iframe{border:2px inset}
q::before{content:open-quote}
q::after{content:close-quote}
noembed,noframes{display:none}
select:not([multiple]):not([size]) option:not(:checked){display:none}
select[multiple] option,select[size]:not([size='0']):not([size='1']) option{padding:0 2px 1px}
select[multiple] option:checked,select[size]:not([size='0']):not([size='1']) option:checked{background-color:light-dark(#cecece,#545454)}
select[multiple],select[size]:not([size='0']):not([size='1']){overflow-y:auto;vertical-align:text-bottom}
dialog{position:absolute;left:0;right:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:1em;background-color:#fff;color:#000}
dialog:modal{position:fixed;top:0;bottom:0;max-width:calc(100% - 6px - 2em);max-height:calc(100% - 6px - 2em);overflow:auto;z-index:2147483647}
";

thread_local! {
    static UA_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(UA));
    pub(crate) static FONT_NAMES: std::cell::RefCell<(HashMap<String, String>, HashMap<String, renderer::SharedString>)> = Default::default();
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
    /// `::slotted()` and `::part()` declarations from the outer tree.
    shadowed: [Option<Vec<Declared>>; 2],
}

struct CachedNode {
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    host: Option<Style>,
    selection: Option<Style>,
    shadowed: [Option<Vec<Declared>>; 2],
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
            shadowed: key.shadowed.clone(),
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
    shadowed: Option<(bool, Shadowed)>,
    starting: Option<Rc<Style>>,
    /// The font size or line height uses viewport units, so a resize changes the values.
    viewport: bool,
}

/// A bucket key: id, class, tag or attribute name.
type Key = Option<(u8, String)>;
/// Marks pseudo-element selectors in the buckets, by index in `StyleScope.pseudos`.
const PSEUDO: usize = 1 << (usize::BITS - 1);
struct StyleScope {
    id: usize,
    used: u64,
    restricted: bool,
    sheets: Vec<Rc<Stylesheet>>,
    /// Selectors, declarations with their `!important` flag, and layer.
    rules: Vec<(SelectorList, Vec<Declared>, i32)>,
    layers: Vec<(String, i32)>,
    /// Pseudo-element kind, selector without it, rule and key.
    pseudos: Vec<(usize, Selector, usize, Key)>,
    arguments: FxHashMap<usize, Rc<str>>,
    /// Element selectors of each rule, by index in the rule's list.
    elements: Vec<Vec<(usize, Key)>>,
    /// Rules, and pseudo-element selectors with `PSEUDO`, by the key they require, per key kind.
    buckets: [FxHashMap<String, Vec<usize>>; 4],
    universal: Vec<usize>,
    containers: Vec<Option<ContainerQuery>>,
    /// Whether each `@media` rule of the sheets matched when the rules were collected.
    media: Vec<bool>,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    properties: Registry,
    /// Interaction states the selectors use.
    states: u8,
    matches: SelectorCache,
    nodes: Vec<Option<MatchedRules>>,
    /// The cascade order of each rule. Tailwind's generated rules share one block of keys in
    /// Tailwind's order, so new utilities join the scope without rebuilding it.
    keys: Vec<u64>,
    /// The key prefix of Tailwind's block, and of the latest other rule.
    block: Option<u64>,
    sequence: u64,
}

type Collected<'a> = (
    &'a SelectorList,
    &'a Declarations,
    i32,
    Option<ContainerQuery>,
);

impl StyleScope {
    /// Adds collected rules; `tailwind` has the keys of generated rules by selector list.
    fn push(&mut self, rules: Vec<Collected>, tailwind: Option<&FxHashMap<usize, u64>>) {
        let mut lists = Vec::new();
        for (selectors, body, layer, container) in rules {
            let order = self.rules.len();
            let key = tailwind.and_then(|keys| keys.get(&(Rc::as_ptr(selectors) as usize)));
            self.keys.push(match key {
                Some(key) => *self.block.get_or_insert(self.sequence) << 40 | key,
                None => {
                    self.sequence += 1;
                    self.sequence << 40
                }
            });
            let elements = selectors
                .iter()
                .enumerate()
                .filter_map(|(index, selector)| {
                    if let Some((kind, pseudo)) = selector.without_pseudo_element() {
                        if matches!(kind, 12 | 13) {
                            self.arguments.insert(
                                self.pseudos.len(),
                                selector.pseudo_argument().unwrap_or_default().trim().into(),
                            );
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
                    match &key {
                        Some((kind, name)) => self.buckets[*kind as usize]
                            .entry(name.clone())
                            .or_default()
                            .push(order),
                        None => self.universal.push(order),
                    }
                    Some((index, key))
                })
                .collect();
            self.elements.push(elements);
            self.containers.push(container);
            let body = body
                .resolve()
                .iter()
                .map(|(prop, value)| match value.strip_suffix("!important") {
                    Some(plain) => (prop, plain.trim().into(), true),
                    None => (prop, value.clone(), false),
                })
                .collect();
            self.rules.push((selectors.clone(), body, layer));
            lists.push(selectors.clone());
        }
        let mut seen = std::collections::HashSet::new();
        while let Some(list) = lists.pop() {
            if seen.insert(Rc::as_ptr(&list)) {
                for selector in list.iter() {
                    self.states |= selector.states(&mut lists);
                }
            }
        }
    }

    /// Adds the new rules of a Tailwind update and restyles only nodes they may match; false when
    /// the scope must be rebuilt.
    fn insert(
        &mut self,
        update: &tailwind::Update,
        rules: &[Collected],
        collected: &Collect,
        doc: &Document,
    ) -> bool {
        let generated = |key: &u64| Some(key >> 40) == self.block && key & ((1 << 40) - 1) != 0;
        let Some(layer) = self
            .keys
            .iter()
            .position(generated)
            .map(|order| self.rules[order].2)
        else {
            return false;
        };
        if update
            .replaced
            .iter()
            .any(|old| self.sheets.iter().any(|sheet| Rc::ptr_eq(sheet, old)))
        {
            return false;
        }
        for (old, new) in &update.added {
            let Some(index) = self.sheets.iter().position(|sheet| Rc::ptr_eq(sheet, old)) else {
                return false;
            };
            self.sheets.splice(index..=index, new.iter().cloned());
        }
        let start = self.rules.len();
        let rules = rules
            .iter()
            .map(|(selectors, body, _, container)| (*selectors, *body, layer, container.clone()))
            .collect();
        self.push(rules, Some(update.keys));
        self.media.clear();
        for sheet in &self.sheets {
            for rule in &sheet.0 {
                rule.media(collected, &mut self.media);
            }
        }
        let keys: Vec<&Key> = self.elements[start..]
            .iter()
            .flatten()
            .map(|(_, key)| key)
            .chain(
                self.pseudos
                    .iter()
                    .filter(|pseudo| pseudo.2 >= start)
                    .map(|pseudo| &pseudo.3),
            )
            .collect();
        if keys.iter().any(|key| key.is_none()) {
            self.nodes.clear();
            return true;
        }
        for (id, node) in doc.nodes.iter().enumerate() {
            if keys.iter().any(|key| has_key(node, key))
                && let Some(matched) = self.nodes.get_mut(id)
            {
                *matched = None;
            }
        }
        true
    }
}

/// Whether a node has the id, class, tag or attribute that a selector requires.
fn has_key(node: &crate::dom::Node, key: &Key) -> bool {
    match key {
        Some((0, value)) => node.attrs.get("id") == Some(value),
        Some((1, value)) => node
            .attrs
            .get("class")
            .is_some_and(|classes| classes.split_ascii_whitespace().any(|class| class == value)),
        Some((2, value)) => node.tag == *value,
        Some((3, name)) => node.attrs.contains_key(name),
        _ => true,
    }
}

struct MatchedRules {
    checked: std::cell::Cell<u64>,
    cached: Option<CachedNode>,
    dependencies: Vec<(usize, u64)>,
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    columns: usize,
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
    viewport: [f32; 2],
    /// The scale when font metrics are used.
    metrics: Option<f32>,
    reduced_motion: bool,
    native_theme: u32,
    scopes: Vec<StyleScope>,
    next: usize,
    pass: u64,
    /// Interaction states used by the scopes of the latest pass.
    pub(crate) states: u8,
    /// Parsed `<style>` text by node, with the pass that last used it.
    sheets: HashMap<usize, (u64, Rc<str>, Rc<Stylesheet>)>,
    pub(crate) cssom: HashMap<usize, Cssom>,
    pub(crate) constructed: HashMap<u32, Rc<Stylesheet>>,
    derived: Derived,
    pub(crate) adopted: HashMap<usize, Vec<u32>>,
    values: rustc_hash::FxHashMap<StyleKey, Rc<CachedStyle>>,
    containers: HashMap<usize, [f32; 2]>,
    sized: std::collections::HashSet<usize>,
    queries: QueryCache,
    queried: bool,
    tables: FxHashMap<usize, CollapsedTable>,
    tailwind: tailwind::Compiler,
    registry: Rc<Registry>,
    pub(crate) scripted: Registry,
    scheme: Option<Rc<str>>,
}

type ContainerQuery = Rc<[(Option<String>, bool, MediaQuery)]>;
type Cssom = (u64, Rc<str>, bool, bool, Rc<Stylesheet>);
type QueryCache = HashMap<(usize, usize, usize, usize), (bool, bool, FontUnits)>;
/// A collapsed table: the pass that used it, the styles and columns of its parts, the viewport,
/// and the styles its borders adjusted.
type CollapsedTable = (
    u64,
    Vec<(usize, StyleRef, usize)>,
    [f32; 2],
    Vec<(usize, Rc<Style>)>,
);

impl StyleCache {
    pub(crate) fn set_containers(&mut self, sizes: HashMap<usize, [f32; 2]>) -> bool {
        if sizes == self.containers {
            return false;
        }
        if self
            .sized
            .iter()
            .any(|id| sizes.get(id) != self.containers.get(id))
        {
            self.sized.clear();
            self.values.clear();
            self.queries.clear();
            for scope in &mut self.scopes {
                for node in scope.nodes.iter_mut().flatten() {
                    node.cached = None;
                }
            }
            self.containers = sizes;
            return true;
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
        if let Some((used, _, _, disabled, sheet)) =
            self.cssom.get_mut(&id).filter(|(_, text, was, ..)| {
                *was == bundled.is_some() && (bundled.is_some() || same(text))
            })
        {
            *used = pass;
            return (!*disabled).then(|| sheet.clone());
        }
        self.cssom.remove(&id);
        if let Some(sheet) = bundled {
            return Some(sheet.clone());
        }
        if let Some((used, _, sheet)) = self.sheets.get_mut(&id).filter(|(_, text, _)| same(text)) {
            *used = pass;
            return Some(sheet.clone());
        }
        let text = doc.text(id);
        let (text, sheet) = match cache.get_key_value(text.as_str()) {
            Some((text, sheet)) => (text.clone(), sheet.clone()),
            None => {
                let previous = self.sheets.get(&id).filter(|(_, old, _)| {
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
                    Some((_, old, base)) => {
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
        self.sheets.insert(id, (pass, text, sheet.clone()));
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
        self.registry.clone()
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
            |(.., disabled, sheet)| (sheet, disabled),
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
        self.cssom
            .insert(id, (self.pass, text.into(), bundled, disabled, sheet));
        result
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
        self.tables.retain(|_, (used, ..)| *used == pass);
        self.derived.clear();
        self.cssom.retain(|_, (used, ..)| *used >= pass);
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
type CascadeRank = (bool, i32, [usize; 4], u64);
type Declared = (Prop, Rc<str>, bool);

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Declarations(Vec<Declaration>);

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
enum Declaration {
    Property(String, String),
    Apply(Vec<String>),
}

/// Top-level rules are shared, so a sheet that changes or grows copies only the rules it edits.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Stylesheet(Vec<Rc<Rule>>);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum Rule {
    Style(SelectorList, Vec<Rule>),
    Media(Vec<MediaQuery>, Vec<Rule>),
    Layer(Vec<String>, Vec<Rule>),
    Supports(String, bool, Vec<Rule>),
    Declarations(Declarations),
    /// `@theme [inline]`: variables and keyframes.
    Theme(bool, Vec<Rule>),
    /// `@custom-variant name (template)` or `{ block with @slot }`; in a rule, `@variant name`
    /// with its `{body}`.
    Variant(String, String),
    Utility(String, String),
    FontFace(Declarations),
    /// `@import "tailwindcss"` or one of its parts, `Some(true)` when it includes preflight;
    /// `None` for `@reference`.
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
}

// WARNING: keep sorted; `@supports`, `CSS.supports()`, `el.style` and the devtools lint all read this, so list only what h2o applies.
const SUPPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("-moz-osx-font-smoothing", "auto|grayscale"),
    ("-webkit-app-region", "none|drag|no-drag"),
    (
        "-webkit-backdrop-filter",
        "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    ),
    (
        "-webkit-background-clip",
        "[border-box|padding-box|content-box|text]#",
    ),
    (
        "-webkit-box-orient",
        "horizontal|vertical|inline-axis|block-axis",
    ),
    (
        "-webkit-clip-path",
        "none|[inset(|circle(|ellipse(|polygon(|path(|url(|border-box|padding-box|content-box|margin-box|fill-box|stroke-box|view-box]+",
    ),
    (
        "-webkit-filter",
        "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    ),
    (
        "-webkit-font-smoothing",
        "auto|none|antialiased|subpixel-antialiased",
    ),
    ("-webkit-line-clamp", "none|<integer>"),
    ("-webkit-mask", "*"),
    ("-webkit-mask-image", "[none|<image>]#"),
    ("-webkit-mask-position", "*"),
    (
        "-webkit-mask-repeat",
        "[repeat|repeat-x|repeat-y|no-repeat|space|round]#",
    ),
    ("-webkit-mask-size", "*"),
    ("-webkit-tap-highlight-color", "<color>"),
    ("-webkit-text-decoration", "*"),
    ("-webkit-text-fill-color", "<color>"),
    ("-webkit-user-select", "auto|none|text|all|contain"),
    ("accent-color", "auto|<color>"),
    (
        "align-content",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "align-items",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "align-self",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    ("all", "*"),
    ("animation", "*"),
    ("animation-delay", "[<time>]#"),
    (
        "animation-direction",
        "[normal|reverse|alternate|alternate-reverse]#",
    ),
    ("animation-duration", "[auto|<time>]#"),
    ("animation-fill-mode", "[none|forwards|backwards|both]#"),
    ("animation-iteration-count", "[infinite|<number>]#"),
    ("animation-name", "[none|<custom-ident>|<string>]#"),
    ("animation-play-state", "[running|paused]#"),
    (
        "animation-timing-function",
        "[linear|ease|ease-in|ease-out|ease-in-out|step-start|step-end|steps(|cubic-bezier(|linear(]#",
    ),
    ("app-region", "none|drag|no-drag"),
    (
        "appearance",
        "none|auto|menulist-button|textfield|button|checkbox|radio|searchfield|textarea|menulist|listbox|meter|progress-bar|push-button|square-button|slider-horizontal|base-select",
    ),
    ("aspect-ratio", "*"),
    (
        "backdrop-filter",
        "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    ),
    ("backface-visibility", "visible|hidden"),
    ("background", "*"),
    (
        "background-blend-mode",
        "[normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity|plus-lighter]#",
    ),
    (
        "background-clip",
        "[border-box|padding-box|content-box|text]#",
    ),
    ("background-color", "<color>"),
    ("background-image", "[none|<image>]#"),
    ("background-position", "*"),
    ("background-repeat", "*"),
    ("background-size", "*"),
    (
        "block-size",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    ("border", "*"),
    ("border-block", "*"),
    ("border-block-color", "[<color>]+"),
    ("border-block-end", "*"),
    ("border-block-end-color", "<color>"),
    (
        "border-block-end-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-block-end-width", "thin|medium|thick|<length>"),
    ("border-block-start", "*"),
    ("border-block-start-color", "<color>"),
    (
        "border-block-start-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-block-start-width", "thin|medium|thick|<length>"),
    (
        "border-block-style",
        "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]+",
    ),
    ("border-block-width", "[thin|medium|thick|<length>]+"),
    ("border-bottom", "*"),
    ("border-bottom-color", "<color>"),
    ("border-bottom-left-radius", "[<length-percentage>]+"),
    ("border-bottom-right-radius", "[<length-percentage>]+"),
    (
        "border-bottom-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-bottom-width", "thin|medium|thick|<length>"),
    ("border-collapse", "separate|collapse"),
    ("border-color", "[<color>]+"),
    ("border-end-end-radius", "[<length-percentage>]+"),
    ("border-end-start-radius", "[<length-percentage>]+"),
    ("border-inline", "*"),
    ("border-inline-color", "[<color>]+"),
    ("border-inline-end", "*"),
    ("border-inline-end-color", "<color>"),
    (
        "border-inline-end-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-inline-end-width", "thin|medium|thick|<length>"),
    ("border-inline-start", "*"),
    ("border-inline-start-color", "<color>"),
    (
        "border-inline-start-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-inline-start-width", "thin|medium|thick|<length>"),
    (
        "border-inline-style",
        "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]+",
    ),
    ("border-inline-width", "[thin|medium|thick|<length>]+"),
    ("border-left", "*"),
    ("border-left-color", "<color>"),
    (
        "border-left-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-left-width", "thin|medium|thick|<length>"),
    ("border-radius", "*"),
    ("border-right", "*"),
    ("border-right-color", "<color>"),
    (
        "border-right-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-right-width", "thin|medium|thick|<length>"),
    ("border-spacing", "[<length>]+"),
    ("border-start-end-radius", "[<length-percentage>]+"),
    ("border-start-start-radius", "[<length-percentage>]+"),
    (
        "border-style",
        "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]+",
    ),
    ("border-top", "*"),
    ("border-top-color", "<color>"),
    ("border-top-left-radius", "[<length-percentage>]+"),
    ("border-top-right-radius", "[<length-percentage>]+"),
    (
        "border-top-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("border-top-width", "thin|medium|thick|<length>"),
    ("border-width", "[thin|medium|thick|<length>]+"),
    ("bottom", "auto|<length-percentage>"),
    ("box-decoration-break", "slice|clone"),
    ("box-shadow", "*"),
    ("box-sizing", "content-box|border-box"),
    ("caret-color", "auto|<color>"),
    ("clear", "none|left|right|both|inline-start|inline-end"),
    (
        "clip-path",
        "none|[inset(|circle(|ellipse(|polygon(|path(|url(|border-box|padding-box|content-box|margin-box|fill-box|stroke-box|view-box]+",
    ),
    ("color", "<color>"),
    ("color-scheme", "normal|[light|dark|only]+"),
    ("column-count", "auto|<integer>"),
    ("column-gap", "normal|<length-percentage>"),
    ("column-rule", "*"),
    ("column-rule-color", "<color>"),
    (
        "column-rule-style",
        "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("column-rule-width", "thin|medium|thick|<length>"),
    ("column-width", "auto|<length>"),
    ("columns", "*"),
    ("container", "*"),
    ("container-name", "none|[<custom-ident>]+"),
    ("container-type", "normal|size|inline-size"),
    ("content", "*"),
    ("counter-increment", "*"),
    ("counter-reset", "*"),
    ("counter-set", "*"),
    ("cursor", "*"),
    ("direction", "ltr|rtl"),
    (
        "display",
        "none|block|inline|inline-block|flex|inline-flex|grid|inline-grid|contents|list-item|flow-root",
    ),
    ("field-sizing", "fixed|content"),
    ("fill", "*"),
    ("fill-opacity", "<number>|<percentage>"),
    ("fill-rule", "nonzero|evenodd"),
    (
        "filter",
        "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    ),
    ("flex", "*"),
    (
        "flex-basis",
        "content|auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    ("flex-direction", "row|row-reverse|column|column-reverse"),
    (
        "flex-flow",
        "[row|row-reverse|column|column-reverse|nowrap|wrap|wrap-reverse]+",
    ),
    ("flex-grow", "<number>"),
    ("flex-shrink", "<number>"),
    ("flex-wrap", "nowrap|wrap|wrap-reverse"),
    ("float", "none|left|right|inline-start|inline-end"),
    ("font", "*"),
    ("font-family", "*"),
    ("font-feature-settings", "*"),
    (
        "font-size",
        "xx-small|x-small|small|medium|large|x-large|xx-large|xxx-large|larger|smaller|<length-percentage>",
    ),
    (
        "font-stretch",
        "normal|ultra-condensed|extra-condensed|condensed|semi-condensed|semi-expanded|expanded|extra-expanded|ultra-expanded|<percentage>",
    ),
    ("font-style", "normal|italic|oblique|*"),
    ("font-variant", "normal"),
    ("font-variation-settings", "*"),
    ("font-weight", "normal|bold|bolder|lighter|<number>"),
    ("gap", "[normal|<length-percentage>]+"),
    ("grid", "*"),
    ("grid-area", "*"),
    ("grid-auto-columns", "*"),
    ("grid-auto-flow", "[row|column|dense]+"),
    ("grid-auto-rows", "*"),
    ("grid-column", "*"),
    ("grid-column-end", "*"),
    ("grid-column-gap", "normal|<length-percentage>"),
    ("grid-column-start", "*"),
    ("grid-gap", "[normal|<length-percentage>]+"),
    ("grid-row", "*"),
    ("grid-row-end", "*"),
    ("grid-row-gap", "normal|<length-percentage>"),
    ("grid-row-start", "*"),
    ("grid-template", "*"),
    ("grid-template-areas", "*"),
    ("grid-template-columns", "*"),
    ("grid-template-rows", "*"),
    (
        "height",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    (
        "inline-size",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    ("inset", "[auto|<length-percentage>]+"),
    ("inset-block", "[auto|<length-percentage>]+"),
    ("inset-block-end", "auto|<length-percentage>"),
    ("inset-block-start", "auto|<length-percentage>"),
    ("inset-inline", "[auto|<length-percentage>]+"),
    ("inset-inline-end", "auto|<length-percentage>"),
    ("inset-inline-start", "auto|<length-percentage>"),
    ("isolation", "auto|isolate"),
    (
        "justify-content",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "justify-items",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "justify-self",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    ("left", "auto|<length-percentage>"),
    ("letter-spacing", "normal|<length-percentage>"),
    ("line-clamp", "none|<integer>|*"),
    ("line-height", "normal|<number>|<length-percentage>"),
    ("list-style", "*"),
    ("list-style-position", "inside|outside"),
    ("list-style-type", "*"),
    ("margin", "[auto|<length-percentage>]+"),
    ("margin-block", "[auto|<length-percentage>]+"),
    ("margin-block-end", "auto|<length-percentage>"),
    ("margin-block-start", "auto|<length-percentage>"),
    ("margin-bottom", "auto|<length-percentage>"),
    ("margin-inline", "[auto|<length-percentage>]+"),
    ("margin-inline-end", "auto|<length-percentage>"),
    ("margin-inline-start", "auto|<length-percentage>"),
    ("margin-left", "auto|<length-percentage>"),
    ("margin-right", "auto|<length-percentage>"),
    ("margin-top", "auto|<length-percentage>"),
    ("mask", "*"),
    ("mask-image", "[none|<image>]#"),
    ("mask-position", "*"),
    (
        "mask-repeat",
        "[repeat|repeat-x|repeat-y|no-repeat|space|round]#",
    ),
    ("mask-size", "*"),
    (
        "max-block-size",
        "none|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    ),
    (
        "max-height",
        "none|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    ),
    (
        "max-inline-size",
        "none|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    ),
    (
        "max-width",
        "none|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    ),
    (
        "min-block-size",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    (
        "min-height",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    (
        "min-inline-size",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    (
        "min-width",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    (
        "mix-blend-mode",
        "[normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity|plus-lighter]#",
    ),
    ("object-fit", "fill|contain|cover|none|scale-down"),
    ("opacity", "<number>|<percentage>"),
    ("order", "<integer>"),
    ("outline", "*"),
    ("outline-color", "auto|<color>"),
    ("outline-offset", "<length>"),
    (
        "outline-style",
        "auto|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    ),
    ("outline-width", "thin|medium|thick|<length>"),
    ("overflow", "[visible|hidden|clip|scroll|auto|overlay]+"),
    ("overflow-wrap", "normal|break-word|anywhere"),
    ("overflow-x", "visible|hidden|clip|scroll|auto|overlay"),
    ("overflow-y", "visible|hidden|clip|scroll|auto|overlay"),
    ("padding", "[<length-percentage>]+"),
    ("padding-block", "[<length-percentage>]+"),
    ("padding-block-end", "<length-percentage>"),
    ("padding-block-start", "<length-percentage>"),
    ("padding-bottom", "<length-percentage>"),
    ("padding-inline", "[<length-percentage>]+"),
    ("padding-inline-end", "<length-percentage>"),
    ("padding-inline-start", "<length-percentage>"),
    ("padding-left", "<length-percentage>"),
    ("padding-right", "<length-percentage>"),
    ("padding-top", "<length-percentage>"),
    ("perspective", "none|<length>"),
    ("perspective-origin", "*"),
    (
        "place-content",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "place-items",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "place-self",
        "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ),
    (
        "pointer-events",
        "auto|none|visiblepainted|visiblefill|visiblestroke|visible|painted|fill|stroke|all",
    ),
    ("position", "static|relative|absolute|fixed|sticky"),
    ("resize", "none|both|horizontal|vertical|block|inline"),
    ("right", "auto|<length-percentage>"),
    ("rotate", "*"),
    ("row-gap", "normal|<length-percentage>"),
    ("scale", "*"),
    ("scroll-behavior", "auto|smooth"),
    ("scroll-margin", "[<length>]+"),
    ("scroll-padding", "[auto|<length-percentage>]+"),
    ("scrollbar-color", "auto|[<color>]+"),
    ("scrollbar-gutter", "auto|stable|[stable|both-edges]+"),
    ("scrollbar-width", "auto|thin|none"),
    ("stroke", "*"),
    ("stroke-linecap", "butt|round|square"),
    ("stroke-linejoin", "miter|round|bevel|miter-clip|arcs"),
    ("stroke-opacity", "<number>|<percentage>"),
    ("stroke-width", "<length-percentage>|<number>"),
    (
        "text-align",
        "start|end|left|right|center|justify|match-parent|-webkit-left|-webkit-right|-webkit-center",
    ),
    (
        "text-align-last",
        "auto|start|end|left|right|center|justify|match-parent",
    ),
    ("text-decoration", "*"),
    ("text-decoration-color", "<color>"),
    (
        "text-decoration-line",
        "none|[underline|overline|line-through|blink]+",
    ),
    ("text-decoration-style", "solid|double|dotted|dashed|wavy"),
    (
        "text-decoration-thickness",
        "auto|from-font|<length-percentage>",
    ),
    ("text-indent", "*"),
    ("text-overflow", "clip|ellipsis"),
    ("text-shadow", "*"),
    (
        "text-transform",
        "none|capitalize|uppercase|lowercase|full-width|full-size-kana",
    ),
    ("text-underline-offset", "auto|<length-percentage>"),
    ("text-wrap", "[wrap|nowrap|balance|pretty|stable|auto]+"),
    ("top", "auto|<length-percentage>"),
    (
        "transform",
        "none|[matrix(|matrix3d(|translate(|translate3d(|translatex(|translatey(|translatez(|scale(|scale3d(|scalex(|scaley(|scalez(|rotate(|rotate3d(|rotatex(|rotatey(|rotatez(|skew(|skewx(|skewy(|perspective(]+",
    ),
    ("transform-origin", "*"),
    ("transform-style", "flat|preserve-3d"),
    ("transition", "*"),
    ("transition-behavior", "[normal|allow-discrete]#"),
    ("transition-delay", "[<time>]#"),
    ("transition-duration", "[<time>]#"),
    ("transition-property", "*"),
    (
        "transition-timing-function",
        "[linear|ease|ease-in|ease-out|ease-in-out|step-start|step-end|steps(|cubic-bezier(|linear(]#",
    ),
    ("translate", "*"),
    (
        "unicode-bidi",
        "normal|embed|isolate|bidi-override|isolate-override|plaintext",
    ),
    ("user-select", "auto|none|text|all|contain"),
    (
        "vertical-align",
        "baseline|sub|super|text-top|text-bottom|middle|top|bottom|<length-percentage>",
    ),
    ("visibility", "visible|hidden|collapse"),
    (
        "white-space",
        "normal|pre|nowrap|pre-wrap|pre-line|break-spaces",
    ),
    (
        "width",
        "auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    ),
    ("will-change", "*"),
    ("word-break", "normal|break-all|break-word"),
    ("word-spacing", "normal|<length>"),
    ("word-wrap", "normal|break-word|anywhere"),
    (
        "writing-mode",
        "horizontal-tb|vertical-rl|vertical-lr|sideways-rl|sideways-lr",
    ),
    ("z-index", "auto|<integer>"),
];

pub(crate) fn supports(name: &str, value: &str) -> Option<bool> {
    let value = value.trim();
    if name.starts_with("--") {
        return Some(true);
    }
    let grammar = SUPPORTED_PROPERTIES
        .binary_search_by(|(property, _)| (*property).cmp(name))
        .ok()
        .map(|index| SUPPORTED_PROPERTIES[index].1)?;
    let lower = value.to_ascii_lowercase();
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

// WARNING: keep sorted.
const INITIAL: &[(&str, &str)] = &[
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
    ("caret-color", "auto"),
    ("clear", "none"),
    ("clip-path", "none"),
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
    ("text-decoration-color", "currentcolor"),
    ("text-decoration-line", "none"),
    ("text-decoration-style", "solid"),
    ("text-decoration-thickness", "auto"),
    ("text-indent", "0px"),
    ("text-overflow", "clip"),
    ("text-shadow", "none"),
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
    ("visibility", "visible"),
    ("white-space", "normal"),
    ("width", "auto"),
    ("will-change", "auto"),
    ("word-break", "normal"),
    ("word-spacing", "0px"),
    ("writing-mode", "horizontal-tb"),
    ("z-index", "auto"),
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
        "border-radius",
        &[
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
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
    ("flex-flow", &["flex-direction", "flex-wrap"]),
    ("gap", &["row-gap", "column-gap"]),
    ("overflow", &["overflow-x", "overflow-y"]),
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
    ("columns", &["column-width", "column-count"]),
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
    container: [f32; 2],
    registry: &Registry,
) -> String {
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let name = LOGICAL
        .iter()
        .find(|(logical, ..)| logical.name() == name)
        .map_or(name, |(_, ltr, rtl_side)| {
            if rtl { rtl_side } else { ltr }.name()
        });
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
    let list = |shorthand: Prop, key: &str| {
        let count = style
            .get(shorthand)
            .map_or(1, |value| components(value, ',').len().max(1));
        let parts: Vec<String> = components(&get(&format!("{}-{key}", shorthand.name())), ',')
            .into_iter()
            .map(|part| part.trim().to_owned())
            .collect();
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
            let images = list(Prop::BACKGROUND, "image");
            let layers: Vec<String> = (0..images.len())
                .map(|index| {
                    let pick = |key: &str| {
                        let values = components(&get(&format!("background-{key}")), ',')
                            .into_iter()
                            .map(str::trim)
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        values
                            .get(index % values.len().max(1))
                            .cloned()
                            .unwrap_or_default()
                    };
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
            let shorthand = if name == "transition" {
                Prop::TRANSITION
            } else {
                Prop::ANIMATION
            };
            let keys: &[(&str, &str)] = if name == "transition" {
                &[
                    ("property", "all"),
                    ("duration", ""),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("behavior", "normal"),
                ]
            } else {
                &[
                    ("duration", ""),
                    ("timing-function", "ease"),
                    ("delay", "0s"),
                    ("iteration-count", "1"),
                    ("direction", "normal"),
                    ("fill-mode", "none"),
                    ("play-state", "running"),
                    ("name", "none"),
                ]
            };
            let columns: Vec<Vec<String>> =
                keys.iter().map(|(key, _)| list(shorthand, key)).collect();
            let count = columns.iter().map(Vec::len).max().unwrap_or(1);
            return (0..count)
                .map(|index| {
                    keys.iter()
                        .zip(&columns)
                        .filter_map(|((_, initial), values)| {
                            let value = values.get(index).cloned().unwrap_or_default();
                            (initial.is_empty() || value != *initial).then_some(value)
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
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
            let axis = |index: usize| -> String {
                let values: Vec<&str> = corners
                    .iter()
                    .map(|corner| {
                        corner
                            .get(index)
                            .or(corner.first())
                            .map_or("0px", String::as_str)
                    })
                    .collect();
                let [top, right, bottom, left] = [values[0], values[1], values[2], values[3]];
                if left != right {
                    values.join(" ")
                } else if top != bottom {
                    format!("{top} {right} {bottom}")
                } else if top != right {
                    format!("{top} {right}")
                } else {
                    top.to_owned()
                }
            };
            let (horizontal, vertical) = (axis(0), axis(1));
            return if horizontal == vertical {
                horizontal
            } else {
                format!("{horizontal} / {vertical}")
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
            [top, right, bottom, left] => {
                if left != right {
                    values.join(" ")
                } else if top != bottom {
                    format!("{top} {right} {bottom}")
                } else if top != right {
                    format!("{top} {right}")
                } else {
                    top.clone()
                }
            }
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
    let grid = |shorthand: Prop, index: usize| {
        let value = style.get(shorthand)?;
        let parts: Vec<&str> = value.split('/').map(str::trim).collect();
        let part = parts
            .get(index)
            .copied()
            .or_else(|| match (shorthand, index) {
                (Prop::GRID_AREA, 1) | (Prop::GRID_AREA, 2) => parts
                    .first()
                    .copied()
                    .filter(|part| !part.starts_with(|c: char| c.is_ascii_digit() || c == '-')),
                (Prop::GRID_AREA, 3) => parts
                    .get(1)
                    .or(parts.first())
                    .copied()
                    .filter(|part| !part.starts_with(|c: char| c.is_ascii_digit() || c == '-')),
                (_, 1) => parts.first().copied().filter(|part| {
                    !part.starts_with(|c: char| c.is_ascii_digit() || c == '-')
                        && !part.starts_with("span")
                }),
                _ => None,
            });
        Some(part.unwrap_or("auto").to_owned())
    };
    let value = match style.get(prop) {
        Some(value) => value.to_owned(),
        None => match name {
            "grid-row-start" => grid(Prop::GRID_ROW, 0).or_else(|| grid(Prop::GRID_AREA, 0)),
            "grid-row-end" => grid(Prop::GRID_ROW, 1).or_else(|| grid(Prop::GRID_AREA, 2)),
            "grid-column-start" => grid(Prop::GRID_COLUMN, 0).or_else(|| grid(Prop::GRID_AREA, 1)),
            "grid-column-end" => grid(Prop::GRID_COLUMN, 1).or_else(|| grid(Prop::GRID_AREA, 3)),
            "column-width" | "column-count" => style.get(Prop::COLUMNS).and_then(|columns| {
                components(columns, ' ')
                    .into_iter()
                    .find(|part| {
                        (name == "column-count") == part.parse::<f32>().is_ok() && *part != "auto"
                    })
                    .map(str::to_owned)
            }),
            _ => None,
        }
        .or_else(|| match name.split_once('-') {
            Some(("animation", key)) => shorthand(Prop::ANIMATION, key),
            Some(("transition", key)) => shorthand(Prop::TRANSITION, key),
            _ => None,
        })
        .unwrap_or_else(|| initial.unwrap_or_default().to_owned()),
    };
    let number = |value: f32| self::value::number(f64::from(value));
    let px = |value: f32| format!("{}px", number(value));
    let vertical = matches!(
        name,
        "top" | "bottom" | "height" | "min-height" | "max-height"
    );
    let base = if vertical { container[1] } else { container[0] };
    let length = |value: &str| {
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
        "outline-width" if none(Prop::OUTLINE_STYLE) => "0px".into(),
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
                    Prop::BORDER,
                    Prop::BORDER_TOP_WIDTH,
                    Prop::BORDER_RIGHT_WIDTH,
                    Prop::BORDER_BOTTOM_WIDTH,
                    Prop::BORDER_LEFT_WIDTH,
                ],
            ]
            .map(|props| units.edges(style, props, container[0]));
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
            .map(|row| format!("\"{row}\""))
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
            let bounds = renderer::Bounds::new(
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
                None => value,
            }
        }
        // Like Chrome: the color first, then every length in px, then `inset`.
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
        // A box that clips or scrolls in one axis can't overflow visibly in the other.
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
        "transform-origin" | "perspective-origin" => {
            let [width, height] = size.unwrap_or_default();
            let mut parts: Vec<String> = components(&value, ' ')
                .into_iter()
                .enumerate()
                .map(|(index, part)| {
                    let part = match part {
                        "left" | "top" => "0%",
                        "center" => "50%",
                        "right" | "bottom" => "100%",
                        part => part,
                    };
                    units
                        .resolve(part, if index == 0 { width } else { height })
                        .map_or_else(|| part.to_owned(), px)
                })
                .collect();
            if parts.len() == 1 {
                parts.push(px(height / 2.));
            }
            parts.join(" ")
        }
        _ if name.ends_with("color") || matches!(name, "fill" | "stroke") => {
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
            components(&value, ' ')
                .into_iter()
                .map(length)
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => value,
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
        let known = name.starts_with("--") || Prop::names().binary_search(&name.as_str()).is_ok();
        if !known || supports(&name, value) == Some(false) {
            continue;
        }
        let mut expanded = Vec::new();
        if value.contains("var(") || !expand(&name, value, &mut expanded) {
            expanded = vec![(name.clone(), value.to_owned())];
        }
        result.extend(
            expanded
                .into_iter()
                .map(|(name, value)| [name, value, important.to_owned()]),
        );
    }
    result
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

impl Collect<'_> {
    fn media(&self, queries: &[MediaQuery]) -> bool {
        queries.is_empty()
            || queries.iter().any(|query| {
                query.matches(
                    self.width,
                    self.height,
                    self.reduced_motion,
                    self.native_theme,
                    FontUnits::new(16.),
                ) == Some(true)
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
    /// Full layer names with their path rank: 10 bits per level, siblings in first-seen order.
    layers: Vec<(String, i32)>,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    properties: Registry,
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
                if collected.media(queries) {
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
        }
    }

    /// Appends whether each `@media` rule matches, in tree order.
    fn media(&self, collected: &Collect, out: &mut Vec<bool>) {
        let children = match self {
            Self::Media(queries, children) => {
                out.push(collected.media(queries));
                children
            }
            Self::Style(_, children)
            | Self::Layer(_, children)
            | Self::Supports(_, _, children)
            | Self::Starting(children)
            | Self::Theme(_, children)
            | Self::Container(.., children) => children,
            _ => return,
        };
        for child in children {
            child.media(collected, out);
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
    pub hosts: FxHashMap<usize, Style>,
    pub slotted: FxHashMap<usize, Vec<Declared>>,
    pub parts: FxHashMap<usize, Vec<Declared>>,
    pub inherited: FxHashMap<usize, (Rc<Style>, Lengths)>,
    pub roots: FxHashMap<usize, (Rc<Style>, Lengths)>,
    pub required: Option<std::collections::HashSet<usize>>,
    pub animated: HashMap<usize, Style>,
    pub reduced_motion: bool,
    pub theme: u32,
    /// The device pixel ratio. Font metrics round to device pixels, like Chrome.
    pub scale: f32,
    /// Loaded font names by the family names pages use.
    pub aliases: HashMap<String, String>,
}

#[derive(Clone, Default)]
pub(crate) struct ComputedStyles {
    pub values: Vec<Rc<Style>>,
    pub flags: Vec<u8>,
    pub lengths: Vec<Lengths>,
    pub pseudos: FxHashMap<usize, Rc<[Style; 2]>>,
    pub text_pseudos: FxHashMap<usize, Rc<[Option<Style>; 4]>>,
    /// `::-webkit-scrollbar`, `-track`, `-thumb`, `-corner` and `-thumb:hover`.
    pub scrollbars: FxHashMap<usize, Rc<[Option<Style>; 5]>>,
    pub selections: FxHashMap<usize, Style>,
    pub user_select: Vec<(UserSelect, usize)>,
    pub animations: FxHashMap<usize, Vec<Animation>>,
    pub children: FxHashMap<usize, Vec<usize>>,
    pub host_declarations: Style,
    pub slotted: FxHashMap<usize, Shadowed>,
    pub parts: FxHashMap<usize, Shadowed>,
    pub starting: FxHashMap<usize, Rc<Style>>,
    pub list_items: FxHashMap<usize, i64>,
    pub registry: Rc<Registry>,
    /// The styles of nodes that layout adjusted, like cells of collapsed tables, before the adjustment.
    pub authored: FxHashMap<usize, Rc<Style>>,
}

pub(crate) type Shadowed = Rc<Vec<(Rc<str>, Vec<Declared>)>>;

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}

/// The computed styles of all nodes by property name.
pub fn styles(doc: &Document, width: f32, height: f32) -> Vec<BTreeMap<String, String>> {
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
    .iter()
    .map(|style| {
        style
            .iter()
            .map(|(prop, value)| (prop.name().to_owned(), value.to_string()))
            .collect()
    })
    .collect()
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
    text_system: Option<&renderer::TextSystem>,
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
        properties: Registry::default(),
    };
    let mut order = Vec::new();
    let mut sheets = Vec::new();
    let mut pending = vec![0];
    let mut scheme = None;
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        order.push(id);
        pending.extend(node.children.iter().rev());
        if node.tag == "meta"
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
        if node.tag != "style" {
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
    let root = match doc.nodes[0].children.as_slice() {
        &[child] if doc.nodes[child].tag == "apps-shadow-root" => child,
        _ => 0,
    };
    if let Some(adopted) = style_cache.adopted.get(&root) {
        sheets.extend(
            adopted
                .iter()
                .filter_map(|id| style_cache.constructed.get(id).cloned()),
        );
    }
    let scale = context.map_or(1., |context| context.scale);
    let metrics = text_system.map(|_| scale);
    if style_cache.viewport != [width, height]
        || style_cache.metrics != metrics
        || style_cache.reduced_motion != reduced_motion
        || style_cache.native_theme != native_theme
    {
        // A resize keeps the values that resolve no viewport unit, and they get the new viewport
        // when they are used.
        let resized = style_cache.metrics == metrics
            && style_cache.reduced_motion == reduced_motion
            && style_cache.native_theme == native_theme;
        style_cache.viewport = [width, height];
        style_cache.metrics = metrics;
        style_cache.reduced_motion = reduced_motion;
        style_cache.native_theme = native_theme;
        style_cache
            .values
            .retain(|_, value| resized && !value.viewport);
        // WARNING: kept nodes don't evaluate their container queries again, so their results stay
        // for `set_containers`.
        if !resized {
            style_cache.queries.clear();
            style_cache.queried = false;
        }
        // Selector matches stay valid while every `@media` rule keeps its result.
        style_cache.scopes.retain_mut(|scope| {
            let mut media = Vec::new();
            for sheet in &scope.sheets {
                for rule in &sheet.0 {
                    rule.media(&collected, &mut media);
                }
            }
            for node in scope.nodes.iter_mut().flatten() {
                if !resized
                    || node
                        .cached
                        .as_ref()
                        .is_some_and(|cached| cached.value.viewport)
                {
                    node.cached = None;
                }
            }
            media == scope.media
        });
        let live: Vec<_> = style_cache.scopes.iter().map(|scope| scope.id).collect();
        style_cache.queries.retain(|key, _| live.contains(&key.0));
    }
    let tailwind = style_cache.tailwind.update(&mut sheets, doc, &order);
    if let Some(update) = &tailwind
        && !(update.replaced.is_empty() && update.added.is_empty())
    {
        let mut added = Collect {
            width,
            height,
            reduced_motion,
            native_theme,
            rules: Vec::new(),
            container: None,
            layers: Vec::new(),
            keyframes: HashMap::new(),
            properties: Registry::default(),
        };
        for rule in &update.rules {
            rule.collect(None, "", &mut added);
        }
        let old: Vec<_> = update
            .replaced
            .iter()
            .chain(update.added.iter().map(|(old, _)| old))
            .collect();
        style_cache.scopes.retain_mut(|scope| {
            !scope
                .sheets
                .iter()
                .any(|sheet| old.iter().any(|old| Rc::ptr_eq(sheet, old)))
                || scope.insert(update, &added.rules, &collected, doc)
        });
        if !update.replaced.is_empty() {
            style_cache.queries.clear();
        }
    }
    let theme = tailwind.as_ref().map(|update| update.root.clone());
    let restricted = context.is_some_and(|context| context.required.is_some());
    let exact = style_cache.scopes.iter().position(|cached| {
        cached.restricted == restricted
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
                    (cached.restricted == restricted)
                        .then(|| fresh(&cached.sheets))
                        .flatten()
                        .map(|fresh| (index, fresh))
                })
        })
        .flatten();
    let scope = match (exact, incremental) {
        (Some(scope), _) => scope,
        (None, Some((index, (fresh, gone)))) => {
            let mut media = Vec::new();
            let mut new = Vec::new();
            for (sheet, flags) in sheets.iter().zip(&fresh) {
                for (rule, &flag) in sheet.0.iter().zip(flags) {
                    rule.media(&collected, &mut media);
                    let start = collected.rules.len();
                    rule.collect(None, "", &mut collected);
                    new.extend(std::iter::repeat_n(flag, collected.rules.len() - start));
                }
            }
            let mut removed = Collect {
                width,
                height,
                reduced_motion,
                native_theme,
                rules: Vec::new(),
                container: None,
                layers: Vec::new(),
                keyframes: HashMap::new(),
                properties: Registry::default(),
            };
            for (sheet, index) in &gone {
                if let Some(rule) = sheet.0.get(*index) {
                    rule.collect(None, "", &mut removed);
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
            // WARNING: deleted rules stay in the scope with `i32::MIN` as their layer, no declarations and no keys, so rule orders stay valid.
            let mut author: Vec<usize> = (0..cached.rules.len())
                .filter(|&order| cached.rules[order].2 > i32::MIN + 1 && !deleted.contains(&order))
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
                let generated = |key: u64| {
                    cached
                        .block
                        .is_some_and(|block| key >> 40 == block && key & ((1 << 40) - 1) != 0)
                };
                let mut olds = author.iter().copied();
                let mut previous = cached
                    .keys
                    .iter()
                    .zip(&cached.rules)
                    .filter(|(_, rule)| rule.2 == i32::MIN + 1)
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
                        let step = ((next - previous) / (pending.len() as u64 + 1)).min(1 << 20);
                        room &= step > 0 && !generated(previous) && !generated(next);
                        for (index, rule) in pending.drain(..).enumerate() {
                            keys.push(previous + step * (index as u64 + 1));
                            inserted.push(rule);
                        }
                    }
                    previous = next;
                }
            }
            if room {
                let start = cached.rules.len();
                cached.push(inserted, None);
                for (order, key) in (start..).zip(keys) {
                    cached.keys[order] = key;
                }
                cached.push(pending, None);
                let keys: Vec<&Key> = cached.elements[start..]
                    .iter()
                    .flatten()
                    .map(|(_, key)| key)
                    .chain(
                        cached
                            .pseudos
                            .iter()
                            .filter(|pseudo| pseudo.2 >= start)
                            .map(|pseudo| &pseudo.3),
                    )
                    .collect();
                if keys.iter().any(|key| key.is_none()) {
                    cached.nodes.clear();
                } else {
                    for (id, node) in doc.nodes.iter().enumerate() {
                        if keys.iter().any(|key| has_key(node, key))
                            && let Some(matched) = cached.nodes.get_mut(id)
                        {
                            *matched = None;
                        }
                    }
                }
                let Collect {
                    keyframes,
                    properties,
                    ..
                } = std::mem::replace(
                    &mut collected,
                    Collect {
                        width,
                        height,
                        reduced_motion,
                        native_theme,
                        rules: Vec::new(),
                        container: None,
                        layers: Vec::new(),
                        keyframes: HashMap::new(),
                        properties: Registry::default(),
                    },
                );
                if !deleted.is_empty() {
                    for &order in &deleted {
                        cached.rules[order].1.clear();
                        cached.rules[order].2 = i32::MIN;
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
                        key.scope != cached.id || !stale(&key.rules, &key.pseudos)
                    });
                }
                if cached.keyframes != keyframes {
                    // WARNING: cached styles hold animations built from the old keyframes.
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
                    style_cache
                        .values
                        .retain(|key, value| key.scope != cached.id || !animated(value));
                }
                cached.sheets = sheets.clone();
                cached.media = media;
                cached.keyframes = keyframes;
                cached.properties = properties;
                index
            } else {
                collected.rules.clear();
                collected.layers.clear();
                collected.keyframes.clear();
                collected.properties.clear();
                usize::MAX
            }
        }
        (None, None) => usize::MAX,
    };
    let scope = if scope != usize::MAX {
        scope
    } else {
        let mut media = Vec::new();
        for sheet in &sheets {
            for rule in &sheet.0 {
                rule.media(&collected, &mut media);
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
            rules,
            keyframes,
            properties,
            layers,
            ..
        } = collected;
        style_cache.next += 1;
        let mut scope = StyleScope {
            id: style_cache.next,
            used: 0,
            restricted,
            sheets: sheets.clone(),
            rules: Vec::new(),
            layers,
            pseudos: Vec::new(),
            arguments: FxHashMap::default(),
            elements: Vec::new(),
            buckets: Default::default(),
            universal: Vec::new(),
            containers: Vec::new(),
            media,
            keyframes,
            properties,
            states: 0,
            matches: SelectorCache::default(),
            nodes: Vec::new(),
            keys: Vec::new(),
            block: None,
            sequence: 0,
        };
        scope.push(rules, tailwind.as_ref().map(|update| update.keys));
        style_cache.scopes.push(scope);
        style_cache.scopes.len() - 1
    };
    // WARNING: `@property` in shadow trees is ignored, like in browsers.
    if root == 0 {
        let mut registry = style_cache.scopes[scope].properties.clone();
        registry.extend(
            style_cache
                .scripted
                .iter()
                .map(|(&prop, value)| (prop, value.clone())),
        );
        if registry != *style_cache.registry {
            style_cache.registry = Rc::new(registry);
            style_cache.values.clear();
            for scope in &mut style_cache.scopes {
                for node in scope.nodes.iter_mut().flatten() {
                    node.cached = None;
                }
            }
        }
    }
    if root == 0 && scheme != style_cache.scheme {
        style_cache.scheme = scheme.clone();
        style_cache.values.clear();
        for scope in &mut style_cache.scopes {
            for node in scope.nodes.iter_mut().flatten() {
                node.cached = None;
            }
        }
    }
    let registry = style_cache.registry.clone();
    out.registry = registry.clone();
    let mut inheritable: FxHashMap<*const Vec<(Prop, Rc<str>)>, props::Vars> = FxHashMap::default();
    let scoped = &mut style_cache.scopes[scope];
    let scope = scoped.id;
    scoped.used = style_cache.pass;
    scoped.matches.update(doc, &order);
    if scoped.nodes.len() < doc.nodes.len() {
        scoped.nodes.resize_with(doc.nodes.len(), || None);
    }
    let rules = &scoped.rules;
    let keys = &scoped.keys;
    let matches = &mut scoped.matches;
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
        slotted,
        parts,
        starting: starts,
        list_items,
        authored,
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
        let shadowed = [
            context.and_then(|context| context.slotted.get(&id)),
            context.and_then(|context| context.parts.get(&id)),
        ];
        let animated = context.and_then(|context| context.animated.get(&id));
        let mut key = None;
        let cached = scoped.nodes[id]
            .as_ref()
            .filter(|node| {
                animated.is_none() && matches.valid_since(&node.dependencies, &node.checked)
            })
            .and_then(|node| node.cached.as_ref())
            .filter(|cached| {
                cached.parent.as_ref().map(|parent| Rc::as_ptr(&parent.0))
                    == parent_style.map(Rc::as_ptr)
                    && cached.font == font_key
                    && cached.root == root_font
                    && cached.host.as_ref() == host
                    && cached.selection.as_ref() == selection
                    && cached.shadowed.each_ref().map(Option::as_ref) == shadowed
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
                    let has = |key: &Key| has_key(node, key);
                    let split = candidates.partition_point(|&order| order < PSEUDO);
                    let matched: Vec<_> = candidates[..split]
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
                    let matched_pseudos: Vec<_> = candidates[split..]
                        .iter()
                        .map(|&index| index & !PSEUDO)
                        .filter(|&index| scoped.pseudos[index].1.matches_reusing(doc, id, matches))
                        .collect();
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
                                let kind = style.get(Prop::CONTAINER_TYPE).unwrap_or("");
                                if matches!(kind, "size" | "inline-size")
                                    && (!block || kind == "size")
                                    && name.as_ref().is_none_or(|name| {
                                        style.get(Prop::CONTAINER_NAME).is_some_and(|names| {
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
                    shadowed: shadowed.map(Option::<&Vec<Declared>>::cloned),
                };
                let value = style_cache
                    .values
                    .get(&resolved)
                    .filter(|_| animated.is_none())
                    .cloned();
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
            lengths[id] = Lengths {
                viewport: [width, height],
                ..cached.lengths
            };
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
            if let Some(values) = &cached.starting {
                starts.insert(id, values.clone());
            }
            if let Some((part, values)) = &cached.shadowed {
                if *part { &mut *parts } else { &mut *slotted }.insert(id, values.clone());
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
            style.extend(
                parent
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone())),
            );
            if !matches!(node.tag.as_str(), "#text" | "#comment") {
                let vars = parent.vars();
                style.set_vars(if registry.values().any(|(_, inherits, _)| !inherits) {
                    inheritable
                        .entry(Rc::as_ptr(vars))
                        .or_insert_with(|| {
                            let kept: Vec<_> = vars
                                .iter()
                                .filter(|(prop, _)| {
                                    registry.get(prop).is_none_or(|(_, inherits, _)| *inherits)
                                })
                                .cloned()
                                .collect();
                            if kept.len() == vars.len() {
                                vars.clone()
                            } else {
                                Rc::new(kept)
                            }
                        })
                        .clone()
                } else {
                    vars.clone()
                });
            }
        }
        let mut cascade: BTreeMap<Prop, (CascadeRank, Rc<str>)> = BTreeMap::new();
        for &(order, [ids, classes, tags]) in &key.rules {
            let (_, declarations, layer) = &rules[order];
            for &(prop, ref value, important) in declarations {
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    keys[order],
                );
                if cascade.get(&prop).is_none_or(|(old, _)| rank >= *old) {
                    cascade.insert(prop, (rank, value.clone()));
                }
            }
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (prop, value) in inline.resolve().iter() {
                let important = value.ends_with("!important");
                let rank = (important, i32::MAX, [1, 0, 0, 0], u64::MAX);
                if cascade.get(&prop).is_none_or(|(old, _)| rank >= *old) {
                    cascade.insert(
                        prop,
                        (
                            rank,
                            if important {
                                value.trim_end_matches("!important").trim().into()
                            } else {
                                value.clone()
                            },
                        ),
                    );
                }
            }
        }
        if let Some(declarations) = context.and_then(|context| context.hosts.get(&id)) {
            for (prop, value) in declarations.iter() {
                let important = value.ends_with("!important");
                if important
                    || cascade
                        .get(&prop)
                        .is_none_or(|((important, layer, ..), _)| {
                            !important && *layer == i32::MIN + 1
                        })
                {
                    cascade.insert(
                        prop,
                        (
                            (important, 0, [0; 4], 0),
                            if important {
                                value.trim_end_matches("!important").trim().into()
                            } else {
                                value.clone()
                            },
                        ),
                    );
                }
            }
        }
        for (prop, value, important) in context
            .and_then(|context| context.slotted.get(&id))
            .into_iter()
            .flatten()
        {
            if *important
                || cascade
                    .get(prop)
                    .is_none_or(|((important, layer, ..), _)| !important && *layer <= i32::MIN + 2)
            {
                cascade.insert(
                    *prop,
                    ((*important, i32::MIN + 2, [0; 4], 0), value.clone()),
                );
            }
        }
        for (prop, value, important) in context
            .and_then(|context| context.parts.get(&id))
            .into_iter()
            .flatten()
        {
            if cascade.get(prop).is_none_or(|((own, ..), _)| !own) {
                cascade.insert(
                    *prop,
                    ((*important, i32::MAX, [2, 0, 0, 0], 0), value.clone()),
                );
            }
        }
        for (prop, value) in animated.into_iter().flat_map(Style::iter) {
            if cascade
                .get(&prop)
                .is_none_or(|((important, ..), _)| !important)
            {
                cascade.insert(
                    prop,
                    ((false, i32::MAX, [3, 0, 0, 0], u64::MAX), value.clone()),
                );
            }
        }
        if node.tag == "html"
            && let Some(scheme) = &style_cache.scheme
        {
            cascade
                .entry(Prop::COLOR_SCHEME)
                .or_insert(((false, i32::MIN + 1, [0; 4], 0), scheme.clone()));
        }
        if node.tag == "apps-shadow-root" {
            *host_declarations = cascade
                .iter()
                .map(|(&prop, (rank, value))| {
                    (
                        prop,
                        if rank.0 {
                            format!("{value}!important").into()
                        } else {
                            value.clone()
                        },
                    )
                })
                .collect();
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
            let layer = |(important, layer, ..): &CascadeRank| {
                if *important { -*layer } else { *layer }
            };
            let (revert, inline, current) = (&**value == "revert", rank.2[0] == 1, layer(rank));
            let mut best: Option<(CascadeRank, &Rc<str>)> = None;
            for &(order, [ids, classes, tags]) in &key.rules {
                let (_, declarations, rule_layer) = &rules[order];
                if !(if revert {
                    *rule_layer == i32::MIN + 1
                } else {
                    inline || *rule_layer < current
                }) {
                    continue;
                }
                for (_, candidate, important) in declarations
                    .iter()
                    .filter(|(name, ..)| *name == prop || *name == Prop::ALL && !prop.is_custom())
                {
                    let candidate_rank = (
                        *important,
                        if *important {
                            -*rule_layer
                        } else {
                            *rule_layer
                        },
                        [0, ids, classes, tags],
                        keys[order],
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
        // WARNING: decorations propagate through inheritance: lines add up, keep the decorating box's color, and stop at atomic inlines, floats and out-of-flow boxes.
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
            match declared(Prop::TEXT_DECORATION_LINE)
                .map(str::to_owned)
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
        let mut declared = Vec::new();
        style.extend(cascade.into_iter().filter_map(|(prop, (_, value))| {
            if prop.is_custom() {
                declared.push((prop, value));
                return None;
            }
            Some((prop, value))
        }));
        resolve_style(
            &mut style,
            parent_style.map(Rc::as_ref),
            &declared,
            &registry,
        );
        resolve_keywords(&mut style, parent_style.map(Rc::as_ref));
        if decorating
            && style
                .get(Prop::TEXT_DECORATION_COLOR)
                .is_none_or(|value| value.eq_ignore_ascii_case("currentcolor"))
            && let Some(ink) = style.value(Prop::COLOR).cloned()
        {
            style.insert(Prop::TEXT_DECORATION_COLOR, ink);
        }
        // Like Chrome, `th` is centered unless it inherits another alignment.
        if style.get(Prop::TEXT_ALIGN) == Some("-internal-center") {
            let inherited = parent_style
                .and_then(|parent| parent.get(Prop::TEXT_ALIGN))
                .filter(|value| !matches!(*value, "start" | "-internal-center"))
                .unwrap_or("center")
                .to_owned();
            style.insert(Prop::TEXT_ALIGN, inherited);
        }
        // Like Chrome, a vertical `-webkit-box` that clamps lines is a block container.
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
        // Flex and grid items, floats and absolutely positioned boxes are blockified.
        if !node.tag.starts_with('#')
            && let Some(display) = match style.get(Prop::DISPLAY).unwrap_or("inline") {
                "inline" | "inline-block" => Some("block"),
                "inline-flex" => Some("flex"),
                "inline-grid" => Some("grid"),
                _ => None,
            }
            && (parent_style
                .and_then(|parent| parent.get(Prop::DISPLAY))
                .is_some_and(|value| {
                    matches!(value, "flex" | "inline-flex" | "grid" | "inline-grid")
                })
                || style
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
                || style.get(Prop::FLOAT).is_some_and(|value| value != "none"))
        {
            style.insert(Prop::DISPLAY, display);
        }
        if style
            .get(Prop::BACKGROUND_CLIP)
            .is_some_and(|value| value.contains("text"))
            && let Some(ink) = style.remove(Prop::BACKGROUND_IMAGE).and_then(|image| {
                let body = image.split_once('(')?.1.to_owned();
                components(&body, ',').into_iter().find_map(|part| {
                    components(part, ' ')
                        .into_iter()
                        .find(|token| color(token).is_some())
                        .map(str::to_owned)
                })
            })
        {
            style.insert(Prop::COLOR, ink.clone());
            style.insert(Prop::WEBKIT_TEXT_FILL_COLOR, ink);
        }

        if node.tag == "table"
            && style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value == "grid")
        {
            let spacing = if style
                .get(Prop::BORDER_COLLAPSE)
                .is_some_and(|value| value == "collapse")
            {
                "0".to_owned()
            } else {
                style
                    .get(Prop::BORDER_SPACING)
                    .map(str::to_owned)
                    .unwrap_or_else(|| "2px".into())
            };
            let parts = components(&spacing, ' ');
            if let Some(&column) = parts.first() {
                let row = parts.get(1).copied().unwrap_or(column);
                style.insert(Prop::COLUMN_GAP, column);
                style.insert(Prop::ROW_GAP, row);
                // Separated borders also keep the spacing between the table's edges and its cells.
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
            if key.columns > 1 && !style.contains(Prop::GRID_TEMPLATE_COLUMNS) {
                style.insert(
                    Prop::GRID_TEMPLATE_COLUMNS,
                    format!("repeat({},auto)", key.columns),
                );
            }
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
        if queried {
            let mut ancestor = node.parent;
            while let Some(id) = ancestor {
                if styles[id]
                    .get(Prop::CONTAINER_TYPE)
                    .is_some_and(|kind| matches!(kind, "size" | "inline-size"))
                {
                    break;
                }
                ancestor = doc.nodes[id].parent;
            }
            if let Some(container) = ancestor {
                style_cache.sized.insert(container);
                units.container = style_cache.containers.get(&container).copied();
            }
        }
        // WARNING: container units without a container use the viewport, so a resize changes them.
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
        // WARNING: glyph rasterization allocates by font size, so huge sizes abort the process.
        .map(|size| size.min(10_000.))
        .unwrap_or(parent.em);
        let monospace = |style: Option<&Style>| {
            style
                .and_then(|style| style.get(Prop::FONT_FAMILY))
                .is_some_and(|family| family.trim().eq_ignore_ascii_case("monospace"))
        };
        // Font-relative sizes keep the parent's kind, keywords are not absolute, and other lengths are.
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
                let id = text.resolve_font(&font);
                let size = renderer::px(1.);
                (
                    id,
                    [
                        text.ch_advance(id, size).map(f32::from).unwrap_or(0.5),
                        f32::from(text.x_height(id, size)),
                        f32::from(text.cap_height(id, size)),
                        text.advance(id, size, '水')
                            .map(|s| f32::from(s.width))
                            .unwrap_or(1.),
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
        let mut pseudo_cascade: [BTreeMap<Prop, (CascadeRank, &Rc<str>)>; 15] = Default::default();
        let mut shadowed: Vec<(CascadeRank, bool, &Rc<str>, &Vec<Declared>)> = Vec::new();
        for &index in &key.pseudos {
            let (pseudo, selector, order, _) = &scoped.pseudos[index];
            let (_, declarations, layer) = &rules[*order];
            let [ids, classes, tags] = selector.specificity;
            if matches!(*pseudo, 12 | 13) {
                if let Some(argument) = scoped.arguments.get(&index) {
                    shadowed.push((
                        (false, *layer, [0, ids, classes, tags], keys[*order]),
                        *pseudo == 13,
                        argument,
                        declarations,
                    ));
                }
                continue;
            }
            for &(prop, ref value, important) in declarations {
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    keys[*order],
                );
                if pseudo_cascade[*pseudo]
                    .get(&prop)
                    .is_none_or(|(old, _)| rank >= *old)
                {
                    pseudo_cascade[*pseudo].insert(prop, (rank, value));
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
            _,
            _,
            starting,
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
                &declared,
                &registry,
            );
            resolve_keywords(&mut values, parent_style.map(Rc::as_ref));
            let values = Rc::new(values);
            starts.insert(id, values.clone());
            values
        });
        let derive = |mut cascade: BTreeMap<Prop, (CascadeRank, &Rc<str>)>| {
            (!cascade.is_empty()).then(|| {
                physical(&mut cascade, &style);
                let mut values: Style = style
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone()))
                    .collect();
                values.set_vars(style.vars().clone());
                let mut declared = Vec::new();
                values.extend(cascade.into_iter().filter_map(|(prop, (_, value))| {
                    if prop.is_custom() {
                        declared.push((prop, value.clone()));
                        return None;
                    }
                    Some((prop, value.clone()))
                }));
                resolve_style(&mut values, Some(&style), &declared, &registry);
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
        if !selected.is_empty()
            || node
                .parent
                .is_some_and(|parent| selections.contains_key(&parent))
        {
            let parent = node.parent.and_then(|parent| selections.get(&parent));
            let mut values = parent.cloned().unwrap_or_default();
            values.set_vars(style.vars().clone());
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
            resolve_style(&mut values, parent, &[], &registry);
            // WARNING: resolve_style gives an element without a color the root `canvastext`, but a
            // `::selection` without one keeps the text color.
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
                selections.insert(id, values);
            }
        }
        if [&before, &after]
            .iter()
            .any(|values| values.contains_key(&Prop::CONTENT))
        {
            pseudos.insert(
                id,
                Rc::new([before, after].map(|cascade| derive(cascade).unwrap_or_default())),
            );
        }
        let shadowed = (!shadowed.is_empty()).then(|| {
            shadowed.sort_by_key(|entry| entry.0);
            let part = shadowed[0].1;
            let mut groups: Vec<(Rc<str>, Vec<Declared>)> = Vec::new();
            for (_, _, argument, declarations) in &shadowed {
                let index = match groups.iter().position(|(name, _)| name == *argument) {
                    Some(index) => index,
                    None => {
                        groups.push(((*argument).clone(), Vec::new()));
                        groups.len() - 1
                    }
                };
                groups[index].1.extend(declarations.iter().cloned());
            }
            for (_, declarations) in &mut groups {
                declarations.sort_by_key(|(_, _, important)| *important);
            }
            let groups = Rc::new(groups);
            if part { &mut *parts } else { &mut *slotted }.insert(id, groups.clone());
            (part, groups)
        });
        if style.contains(Prop::ANIMATION) || style.contains(Prop::ANIMATION_NAME) {
            let effects = Animation::parse(&style, &scoped.keyframes, &registry);
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
            shadowed,
            starting,
            viewport,
        });
        if animated.is_none() {
            if let Some(node) = scoped.nodes[id].as_mut() {
                node.cached = Some(CachedNode::new(&key, cached.clone()));
            }
            // WARNING: the key doesn't hold the container, so styles with container units are not shared.
            if !queried {
                style_cache.values.insert(key, cached);
            }
        }
        styles[id] = style;
        lengths[id] = units;
    }
    if context.is_some_and(|context| context.required.is_some()) {
        return;
    }
    if order.iter().any(|&id| {
        [
            Prop::COUNTER_RESET,
            Prop::COUNTER_INCREMENT,
            Prop::COUNTER_SET,
        ]
        .into_iter()
        .any(|prop| styles[id].contains(prop))
            || pseudos.get(&id).is_some_and(|pseudos| {
                pseudos.iter().any(|pseudo| {
                    pseudo
                        .get(Prop::CONTENT)
                        .is_some_and(|content| content.contains("counter"))
                        || pseudo.contains(Prop::COUNTER_INCREMENT)
                        || pseudo.contains(Prop::COUNTER_RESET)
                })
            })
    }) {
        let list = |value: Option<&str>, default: i64| -> Vec<(String, i64)> {
            let parts = components(value.unwrap_or("none"), ' ');
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
                    None => counters.push((name, step, usize::MAX)),
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
        let mut pending = vec![(0, false)];
        while let Some((id, exit)) = pending.pop() {
            let node = &doc.nodes[id];
            let parent = node.parent.unwrap_or(usize::MAX);
            if exit {
                if let Some(own) = pseudos.get(&id).cloned() {
                    apply(&mut counters, &own[1], id, None);
                    if let Some(text) = own[1]
                        .get(Prop::CONTENT)
                        .and_then(|value| content(&counters, value))
                    {
                        Rc::make_mut(pseudos.get_mut(&id).unwrap())[1].insert(Prop::CONTENT, text);
                    }
                }
                counters.retain(|counter| counter.2 != id);
                continue;
            }
            if style_flags[id] & DISPLAY_NONE != 0 || node.tag.starts_with('#') && id != 0 {
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
                if let Some(counter) = counters.iter().rfind(|counter| counter.0 == "list-item") {
                    list_items.insert(id, counter.1);
                }
            }
            if let Some(own) = pseudos.get(&id).cloned() {
                apply(&mut counters, &own[0], id, None);
                if let Some(text) = own[0]
                    .get(Prop::CONTENT)
                    .and_then(|value| content(&counters, value))
                {
                    Rc::make_mut(pseudos.get_mut(&id).unwrap())[0].insert(Prop::CONTENT, text);
                }
            }
            pending.push((id, true));
            pending.extend(node.children.iter().rev().map(|&child| (child, false)));
        }
    }
    // Collapsed table borders (CSS 2.1 17.6.2): cells overlap by half their borders, so shared borders straddle
    // the grid lines, and the table keeps half of the outer borders.
    // WARNING: this changes the styles after caching, so cached styles stay unadjusted.
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
    for &table in &order {
        if doc.nodes[table].tag != "table"
            || styles[table]
                .get(Prop::BORDER_COLLAPSE)
                .is_none_or(|value| value != "collapse")
            || styles[table]
                .get(Prop::DISPLAY)
                .is_none_or(|value| value != "grid")
        {
            continue;
        }
        let border = |id: usize, [style, width, ..]: [Prop; 4]| {
            if styles[id]
                .get(style)
                .is_none_or(|kind| matches!(kind, "none" | "hidden"))
            {
                return 0.;
            }
            styles[id]
                .get(width)
                .map_or(Some(3.), |value| lengths[id].resolve(value, 0.))
                .unwrap_or(0.)
                .max(0.)
        };
        let lines: Vec<(usize, Option<usize>, Vec<usize>)> = doc.nodes[table]
            .children
            .iter()
            .flat_map(|&child| match doc.nodes[child].tag.as_str() {
                "thead" | "tbody" | "tfoot" => doc.nodes[child]
                    .children
                    .iter()
                    .map(|&row| (row, Some(child)))
                    .collect(),
                _ => vec![(child, None)],
            })
            .filter(|&(row, _)| doc.nodes[row].tag == "tr")
            .map(|(row, group)| {
                let cells = doc.nodes[row]
                    .children
                    .iter()
                    .copied()
                    .filter(|&cell| matches!(doc.nodes[cell].tag.as_str(), "td" | "th"))
                    .collect();
                (row, group, cells)
            })
            .filter(|(_, _, cells): &(_, _, Vec<usize>)| !cells.is_empty())
            .collect();
        let spans: Vec<Vec<std::ops::Range<usize>>> = lines
            .iter()
            .map(|(_, _, cells)| {
                let mut start = 0;
                cells
                    .iter()
                    .map(|&cell| {
                        let span = doc.nodes[cell]
                            .attrs
                            .get("colspan")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(1)
                            .max(1);
                        start += span;
                        start - span..start
                    })
                    .collect()
            })
            .collect();
        // WARNING: the key must hold every input of the adjustment: the styles of the table, rows,
        // groups and cells, the cell columns and the viewport.
        let inputs: Vec<(usize, StyleRef, usize)> = std::iter::once((table, 0))
            .chain(
                lines
                    .iter()
                    .zip(&spans)
                    .flat_map(|((row, group, cells), spans)| {
                        std::iter::once((*row, 0))
                            .chain(group.map(|group| (group, 0)))
                            .chain(
                                cells
                                    .iter()
                                    .zip(spans)
                                    .map(|(&cell, span)| (cell, span.end)),
                            )
                    }),
            )
            .map(|(id, column)| (id, StyleRef(styles[id].clone()), column))
            .collect();
        if let Some((pass, old, size, adjusted)) = style_cache.tables.get_mut(&table)
            && *old == inputs
            && *size == [width, height]
        {
            *pass = style_cache.pass;
            for (id, style) in adjusted.iter() {
                authored.insert(*id, std::mem::replace(&mut styles[*id], style.clone()));
            }
            continue;
        }
        // Each shared edge takes the widest border of its cells, rows and row groups, then the strongest style.
        let edge = |id: usize, side: usize| {
            let rank = match styles[id].get(SIDES[side][0]).unwrap_or("none") {
                "double" => 8,
                "solid" => 7,
                "dashed" => 6,
                "dotted" => 5,
                "ridge" => 4,
                "outset" => 3,
                "groove" => 2,
                "inset" => 1,
                _ => 0,
            };
            (border(id, SIDES[side]), rank, id, side)
        };
        type Edge = (f32, u8, usize, usize);
        let wins = |a: Edge, b: Edge| if (b.0, b.1) > (a.0, a.1) { b } else { a };
        let mut resolved: Vec<Vec<[Edge; 4]>> = Vec::new();
        for (index, (row, group, cells)) in lines.iter().enumerate() {
            let first = index == 0 || lines[index - 1].1 != *group;
            let last = lines.get(index + 1).is_none_or(|next| next.1 != *group);
            let outer: Vec<usize> = [Some(*row), *group].into_iter().flatten().collect();
            resolved.push(
                cells
                    .iter()
                    .enumerate()
                    .map(|(column, &cell)| {
                        let mut sides = [0, 1, 2, 3].map(|side| edge(cell, side));
                        for (position, &owner) in outer.iter().enumerate() {
                            let whole = position == 0;
                            for (side, applies) in [
                                (0, whole || first),
                                (1, column + 1 == cells.len()),
                                (2, whole || last),
                                (3, column == 0),
                            ] {
                                if applies {
                                    sides[side] = wins(sides[side], edge(owner, side));
                                }
                            }
                        }
                        sides
                    })
                    .collect(),
            );
        }
        for row in 0..resolved.len() {
            for column in 1..resolved[row].len() {
                let shared = wins(resolved[row][column - 1][1], resolved[row][column][3]);
                (resolved[row][column - 1][1], resolved[row][column][3]) = (shared, shared);
            }
            if row + 1 < resolved.len() {
                for above in 0..resolved[row].len() {
                    for below in 0..resolved[row + 1].len() {
                        let [a, b] = [&spans[row][above], &spans[row + 1][below]];
                        if a.start < b.end && b.start < a.end {
                            let shared = wins(resolved[row][above][2], resolved[row + 1][below][0]);
                            (resolved[row][above][2], resolved[row + 1][below][0]) =
                                (shared, shared);
                        }
                    }
                }
            }
        }
        // The table's border is one more contender for each outer cell edge, which the cell then draws.
        let mut edges = [0f32; 4];
        let mut cells: Vec<(usize, [Edge; 4])> = Vec::with_capacity(inputs.len());
        for (index, ((_, _, row), sides)) in lines.iter().zip(resolved).enumerate() {
            for (column, (&cell, mut sides)) in row.iter().zip(sides).enumerate() {
                for (side, widest) in edges.iter_mut().enumerate() {
                    let outer = match side {
                        0 => index == 0,
                        1 => column + 1 == row.len(),
                        2 => index + 1 == lines.len(),
                        _ => column == 0,
                    };
                    if outer {
                        sides[side] = wins(sides[side], edge(table, side));
                        *widest = widest.max(sides[side].0);
                    }
                }
                cells.push((cell, sides));
            }
        }
        // The table and its cells were styled again in this pass, so their old authored styles are stale.
        for (id, ..) in &inputs {
            authored.remove(id);
        }
        // A cell takes the winning border of each side, from wherever it came.
        let taken: Vec<_> = cells
            .iter()
            .flat_map(|&(cell, sides)| {
                (0..4)
                    .filter(move |&side| sides[side].2 != cell || sides[side].3 != side)
                    .map(move |side| (cell, side, sides[side]))
            })
            .map(|(cell, side, (width, _, owner, from))| {
                let [_, kind, color] = BORDERS[from];
                (
                    cell,
                    side,
                    [
                        Some(format!("{width}px").into()),
                        if width > 0. {
                            styles[owner].value(kind).cloned()
                        } else {
                            Some("none".into())
                        },
                        styles[owner].value(color).cloned(),
                    ],
                )
            })
            .collect();
        for (cell, side, values) in taken {
            authored.entry(cell).or_insert_with(|| styles[cell].clone());
            let style = Rc::make_mut(&mut styles[cell]);
            for (prop, value) in BORDERS[side].into_iter().zip(values) {
                if let Some(value) = value {
                    style.insert(prop, value);
                }
            }
        }
        authored
            .entry(table)
            .or_insert_with(|| styles[table].clone());
        let style = Rc::make_mut(&mut styles[table]);
        // The table keeps half of its outer edges as space, like Chrome.
        for ([_, width, _, padding], widest) in SIDES.into_iter().zip(edges) {
            style.insert(width, "0");
            style.insert(padding, format!("{}px", widest / 2.));
        }
        let mut adjusted = vec![(table, styles[table].clone())];
        for (cell, sides) in cells {
            authored.entry(cell).or_insert_with(|| styles[cell].clone());
            let style = Rc::make_mut(&mut styles[cell]);
            for ([_, _, margin, _], (width, ..)) in SIDES.into_iter().zip(sides) {
                style.insert(margin, format!("{}px", -width / 2.));
            }
            adjusted.push((cell, styles[cell].clone()));
        }
        style_cache
            .tables
            .insert(table, (style_cache.pass, inputs, [width, height], adjusted));
    }
}

pub(crate) type Registry = FxHashMap<Prop, (Rc<str>, bool, Option<Rc<str>>)>;

/// Moves logical sides in a cascade to the physical sides of its direction, or of the parent's.
fn physical<V: AsRef<str>>(cascade: &mut BTreeMap<Prop, (CascadeRank, V)>, parent: &Style) {
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
            if cascade.get(&physical).is_none_or(|(old, _)| rank >= *old) {
                cascade.insert(physical, (rank, value));
            }
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

// WARNING: `var()` results stop at this many bytes, like in browsers, so references that fan out cannot grow exponentially.
const SUBSTITUTION_LIMIT: usize = 1 << 20;

fn substitute(
    value: &str,
    lookup: &mut dyn FnMut(Prop) -> Option<Rc<str>>,
    out: &mut String,
    nesting: u8,
) -> bool {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '%' | '#');
    let mut rest = value;
    while let Some(start) = rest.find("var(") {
        if rest[..start].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '-' | '_')) {
            out.push_str(&rest[..start + 4]);
            rest = &rest[start + 4..];
            continue;
        }
        out.push_str(&rest[..start]);
        let body = &rest[start + 4..];
        let (mut depth, mut comma, mut end) = (1, None, None);
        for (index, c) in body.char_indices() {
            match c {
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
                // WARNING: nested fallbacks recurse, and so do the references in them, so their depth is capped.
                Some(comma)
                    if nesting < 32
                        && substitute(body[comma + 1..end].trim(), lookup, out, nesting + 1) => {}
                _ => return false,
            },
        }
        rest = &body[end + 1..];
        // WARNING: `var()` substitutes tokens, so `var(--a)var(--b)` stays two values.
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
    let syntax = syntax.trim();
    let mut rest = syntax;
    let mut alternatives = Vec::new();
    while !rest.is_empty() {
        let end = if rest.starts_with('[') {
            rest.find(']').map_or(rest.len(), |end| end + 1)
        } else {
            rest.find('|').unwrap_or(rest.len())
        };
        let end = end + usize::from(rest[end..].starts_with(['+', '#']));
        alternatives.push(rest[..end].trim());
        rest = rest[end..]
            .trim_start()
            .trim_start_matches('|')
            .trim_start();
    }
    alternatives.into_iter().any(|alternative| {
        let (kinds, separator) = match alternative.strip_suffix(['+', '#']) {
            Some(kind) => (
                kind.trim_start_matches('[').trim_end_matches(']'),
                Some(if alternative.ends_with('#') { ',' } else { ' ' }),
            ),
            None => (alternative, None),
        };
        let one = |value: &str| {
            kinds.split('|').any(|kind| {
                let kind = kind.trim();
                let value = value.trim();
                let units = Lengths::new(16., 100., 100.);
                let math = units.math(value, f32::NAN, &[]);
                let number = |integer: bool| {
                    math.is_some_and(|(n, kind)| {
                        kind == parser::Kind::Number && (!integer || n.fract() == 0.)
                    })
                };
                match kind {
                    "<length>" => units.resolve(value, f32::NAN).is_some(),
                    "<percentage>" => value
                        .strip_suffix('%')
                        .is_some_and(|n| n.parse::<f32>().is_ok()),
                    "<length-percentage>" => units.resolve(value, 100.).is_some(),
                    "<number>" => number(false),
                    "<integer>" => number(true),
                    "<angle>" => math.is_some_and(|(_, kind)| kind == parser::Kind::Angle),
                    "<time>" => value
                        .strip_suffix("ms")
                        .or_else(|| value.strip_suffix('s'))
                        .is_some_and(|n| n.parse::<f32>().is_ok()),
                    "<color>" => {
                        value.eq_ignore_ascii_case("currentcolor")
                            || color(value).is_some()
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
                !value.trim().is_empty() && components(value, separator).into_iter().all(one)
            }
            None => one(value),
        }
    })
}

struct Substitution<'a> {
    declared: &'a [(Prop, Rc<str>)],
    state: Vec<(u8, usize, Option<Rc<str>>)>,
    inherited: &'a Style,
    parent: Option<&'a Style>,
    registry: &'a Registry,
    depth: usize,
    cycle: usize,
}

impl Substitution<'_> {
    fn get(&mut self, prop: Prop) -> Option<Rc<str>> {
        let Ok(index) = self.declared.binary_search_by_key(&prop, |entry| entry.0) else {
            return self
                .inherited
                .value(prop)
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
        let parent = self.parent;
        let initial = || registered.and_then(|(_, _, initial)| initial.clone());
        let inherit = || {
            parent
                .and_then(|parent| parent.value(prop).cloned())
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
                    // WARNING: each nested reference recurses, so chains are invalid past this depth.
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
                    (Some(value), Some((syntax, ..))) if !syntax_matches(syntax, &value) => unset(),
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
    declared: &[(Prop, Rc<str>)],
    registry: &Registry,
) {
    if !declared.is_empty() {
        let inherited = style.clone();
        let mut resolver = Substitution {
            declared,
            state: vec![(0, 0, None); declared.len()],
            inherited: &inherited,
            parent,
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
            .any(|(prop, value)| inherited.value(*prop) != value.as_ref())
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
    let resolved: Vec<_> = style
        .own()
        .filter(|(_, value)| value.contains("var("))
        .map(|(prop, value)| {
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
            (prop, ok.then_some(out))
        })
        .collect();
    for (prop, value) in resolved {
        let value = value
            .filter(|value| {
                supports(prop.name(), value) != Some(false) || value.starts_with('\u{1}')
            })
            .and_then(|value| {
                match value
                    .strip_prefix('\u{1}')
                    .and_then(|value| value.split_once('\u{1}'))
                {
                    Some((shorthand, value)) => {
                        let mut expanded = Vec::new();
                        expand(shorthand, value.trim(), &mut expanded);
                        let rtl = style.get(Prop::DIRECTION) == Some("rtl");
                        expanded
                            .into_iter()
                            .find(|(name, _)| {
                                name == prop.name()
                                    || LOGICAL.iter().any(|(logical, ltr, rtl_side)| {
                                        logical.name() == name
                                            && *if rtl { rtl_side } else { ltr } == prop
                                    })
                            })
                            .map(|(_, value)| value)
                    }
                    None => Some(value),
                }
            })
            .filter(|value| supports(prop.name(), value) != Some(false));
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
        // WARNING: replacing each call in place is quadratic, so one pass marks the dropped text as
        // ranges (+1 at the start, -1 at the end) and a second pass copies the rest.
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
        if (prop.name().ends_with("color") || matches!(prop, Prop::FILL | Prop::STROKE))
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
            && (prop.name().ends_with("color") || matches!(prop, Prop::FILL | Prop::STROKE))
            && let Some(converted) = convert(value, &current)
        {
            *value = converted.into();
        }
        true
    });
}

/// Whether `color-scheme` picks dark colors. `light dark` follows the system appearance, like browsers.
pub(crate) fn dark(style: &Style) -> bool {
    style.get(Prop::COLOR_SCHEME).is_some_and(|v| {
        v.contains("dark")
            && (!v.contains("light") || crate::desktop::native::theme_state() & 1 != 0)
    })
}

fn resolve_keywords(style: &mut Style, parent: Option<&Style>) {
    style.retain(|prop, value| {
        let inherit = match &**value {
            _ if prop.is_custom() => return true,
            "inherit" => true,
            "unset" | "revert" | "revert-layer" => prop.inherited(),
            "initial" => false,
            _ => return true,
        };
        match parent
            .and_then(|parent| parent.value(prop))
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
        return;
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
                        let prop = Prop::new(&key);
                        if important || result.get(prop).is_none_or(|v| !v.ends_with("!important"))
                        {
                            result.insert(
                                prop,
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

    /// The `font-family` and `src` of each `@font-face` rule.
    pub(crate) fn font_faces(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.0.iter().filter_map(|rule| {
            let Rule::FontFace(values) = &**rule else {
                return None;
            };
            let values = values.resolve();
            let family = values
                .get(Prop::FONT_FAMILY)?
                .trim_matches(['\'', '"'])
                .to_owned();
            Some((family, values.get(Prop::new("src"))?.to_owned()))
        })
    }

    pub(crate) fn matches_media(&self, width: f32, height: f32, theme: u32) -> bool {
        let reduced_motion = reduced_motion();
        let [rule] = self.0.as_slice() else {
            return false;
        };
        matches!(&**rule, Rule::Media(queries, _) if queries.is_empty() || queries.iter().any(|query| {
            query.matches(width, height, reduced_motion, theme, FontUnits::new(16.)) == Some(true)
        }))
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

/// The index of the `n`th rule that is not a block of declarations, as CSSOM counts them.
fn position<R: std::borrow::Borrow<Rule>>(rules: &[R], n: usize) -> Option<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| !matches!((*rule).borrow(), Rule::Declarations(_)))
        .nth(n)
        .map(|(index, _)| index)
}

/// The rules in a grouping rule. A style rule's selector becomes their parent.
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

/// Applies a CSSOM edit to the `last`th rule of a top-level or nested rule list.
fn edit_rules<R: std::borrow::Borrow<Rule> + From<Rule>>(
    rules: &mut Vec<R>,
    last: usize,
    edit: Edit,
    parent: Option<SelectorList>,
    rule_mut: fn(&mut R) -> &mut Rule,
) -> Result<serde_json::Value, &'static str> {
    let at = position(rules, last);
    if let Edit::Insert(text) = edit {
        let at = match at {
            Some(at) => at,
            None if last == 0 || position(rules, last - 1).is_some() => rules.len(),
            None => return Err("IndexSizeError"),
        };
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
        let rule = parsed.remove(0);
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
        (Edit::Declarations(text), Rule::FontFace(values)) => *values = Declarations::parse(text),
        (Edit::Selector(text), Rule::Style(selectors, children)) => {
            let parsed = Selector::parse(text, parent.as_ref());
            if !parsed.is_empty() {
                *selectors = Rc::new(parsed);
                for child in children {
                    let _ = child.bind_parent(Some(selectors));
                }
            }
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
