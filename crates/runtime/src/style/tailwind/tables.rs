pub(super) use super::text::{BACKDROP, FILTER, MASK, SHADOW, TRANSFORM};
use super::*;

// The tables of `text.rs` and the Tailwind CSS files.
pub(super) struct Text {
    pub(super) properties: &'static str,
    order: &'static str,
    pub(super) statics: &'static str,
    pub(super) specs: &'static str,
    theme: &'static str,
    pub(super) preflight: &'static str,
    pub(super) static_lines: Vec<u16>,
    pub(super) spec_lines: Vec<u16>,
}

pub(super) static TEXT: LazyLock<Text> = LazyLock::new(|| {
    let mut parts = packed!("tailwind").split('\0');
    let mut part = || parts.next().unwrap();
    let (properties, order, statics, specs) = (part(), part(), part(), part());
    Text {
        properties,
        order,
        statics,
        specs,
        theme: part(),
        preflight: part(),
        static_lines: starts(statics),
        spec_lines: starts(specs),
    }
});

pub(super) static DEFAULTS: LazyLock<FxHashMap<&str, &str>> = LazyLock::new(|| {
    TEXT.theme
        .lines()
        .filter_map(|line| line.strip_suffix(';')?.split_once(':'))
        .collect()
});

pub(super) static ORDER_INDEX: LazyLock<FxHashMap<&str, u16>> =
    LazyLock::new(|| TEXT.order.split_ascii_whitespace().zip(0..).collect());

thread_local! {
    pub(super) static BASE: Rc<Stylesheet> = Rc::new(Stylesheet::parse(&format!("@layer base{{{}}}", TEXT.preflight)));
}

pub(super) const KEYFRAMES: &[(&str, &str)] = &[
    ("spin", "to{transform:rotate(360deg)}"),
    ("ping", "75%,100%{transform:scale(2);opacity:0}"),
    ("pulse", "50%{opacity:.5}"),
    (
        "bounce",
        "0%,100%{transform:translateY(-25%);animation-timing-function:cubic-bezier(.8,0,1,1)}50%{transform:none;animation-timing-function:cubic-bezier(0,0,.2,1)}",
    ),
];

pub(super) const INLINE: &[&str] = &[
    "--blur",
    "--shadow",
    "--shadow-inner",
    "--drop-shadow",
    "--radius",
    "--max-width-prose",
];

pub(super) const KEPT: &[(&str, &[&str])] = &[
    ("--font", &["--font-weight", "--font-size"]),
    ("--inset", &["--inset-shadow", "--inset-ring"]),
    (
        "--text",
        &[
            "--text-color",
            "--text-decoration-color",
            "--text-decoration-thickness",
            "--text-indent",
            "--text-shadow",
            "--text-underline-offset",
        ],
    ),
    (
        "--grid-column",
        &["--grid-column-start", "--grid-column-end"],
    ),
    ("--grid-row", &["--grid-row-start", "--grid-row-end"]),
];

pub(super) const VARIANTS: &[(&str, &str)] = &[
    ("*", ":is(& > *)"),
    ("**", ":is(& *)"),
    ("not", ""),
    ("group", ""),
    ("peer", ""),
    ("first-letter", "&::first-letter"),
    ("first-line", "&::first-line"),
    (
        "marker",
        "& *::marker, &::marker, & *::-webkit-details-marker, &::-webkit-details-marker",
    ),
    ("selection", "& *::selection, &::selection"),
    ("file", "&::file-selector-button"),
    ("placeholder", "&::placeholder"),
    ("backdrop", "&::backdrop"),
    ("details-content", "&::details-content"),
    ("before", "&::before"),
    ("after", "&::after"),
    ("first", "&:first-child"),
    ("last", "&:last-child"),
    ("only", "&:only-child"),
    ("odd", "&:nth-child(odd)"),
    ("even", "&:nth-child(even)"),
    ("first-of-type", "&:first-of-type"),
    ("last-of-type", "&:last-of-type"),
    ("only-of-type", "&:only-of-type"),
    ("visited", "&:visited"),
    ("target", "&:target"),
    ("open", "&:is([open], :popover-open, :open)"),
    ("default", "&:default"),
    ("checked", "&:checked"),
    ("indeterminate", "&:indeterminate"),
    ("placeholder-shown", "&:placeholder-shown"),
    ("autofill", "&:autofill"),
    ("optional", "&:optional"),
    ("required", "&:required"),
    ("valid", "&:valid"),
    ("invalid", "&:invalid"),
    ("user-valid", "&:user-valid"),
    ("user-invalid", "&:user-invalid"),
    ("in-range", "&:in-range"),
    ("out-of-range", "&:out-of-range"),
    ("read-only", "&:read-only"),
    ("empty", "&:empty"),
    ("focus-within", "&:focus-within"),
    ("hover", "&:hover"),
    ("focus", "&:focus"),
    ("focus-visible", "&:focus-visible"),
    ("active", "&:active"),
    ("enabled", "&:enabled"),
    ("disabled", "&:disabled"),
    ("inert", "&:is([inert], [inert] *)"),
    ("in", ""),
    ("has", ""),
    ("aria", ""),
    ("data", ""),
    ("nth", ""),
    ("nth-last", ""),
    ("nth-of-type", ""),
    ("nth-last-of-type", ""),
    ("supports", ""),
    (
        "motion-safe",
        "@media (prefers-reduced-motion: no-preference)",
    ),
    ("motion-reduce", "@media (prefers-reduced-motion: reduce)"),
    ("contrast-more", "@media (prefers-contrast: more)"),
    ("contrast-less", "@media (prefers-contrast: less)"),
    ("max", ""),
    ("min", ""),
    ("@max", ""),
    ("@min", ""),
    ("portrait", "@media (orientation: portrait)"),
    ("landscape", "@media (orientation: landscape)"),
    ("ltr", "&:where(:dir(ltr), [dir=\"ltr\"], [dir=\"ltr\"] *)"),
    ("rtl", "&:where(:dir(rtl), [dir=\"rtl\"], [dir=\"rtl\"] *)"),
    ("dark", "@media (prefers-color-scheme: dark)"),
    ("starting", "@starting-style"),
    ("print", "@media print"),
    ("forced-colors", "@media (forced-colors: active)"),
    ("inverted-colors", "@media (inverted-colors: inverted)"),
    ("pointer-none", "@media (pointer: none)"),
    ("pointer-coarse", "@media (pointer: coarse)"),
    ("pointer-fine", "@media (pointer: fine)"),
    ("any-pointer-none", "@media (any-pointer: none)"),
    ("any-pointer-coarse", "@media (any-pointer: coarse)"),
    ("any-pointer-fine", "@media (any-pointer: fine)"),
    ("noscript", "@media (scripting: none)"),
];

// The start of each line.
fn starts(text: &str) -> Vec<u16> {
    assert!(text.len() < 1 << 16 && text.ends_with('\n'));
    std::iter::once(0)
        .chain(
            text[..text.len() - 1]
                .match_indices('\n')
                .map(|(index, _)| index as u16 + 1),
        )
        .collect()
}

// The text behind a name in a sorted table.
pub(super) fn find(table: &'static str, lines: &[u16], name: &str) -> Option<&'static str> {
    let index = lines
        .binary_search_by(|&start| {
            // The space behind a key sorts before the bytes of names.
            let line = &table.as_bytes()[usize::from(start)..];
            match line[..line.len().min(name.len())].cmp(name.as_bytes()) {
                Ordering::Equal if line.get(name.len()) != Some(&b' ') => Ordering::Greater,
                order => order,
            }
        })
        .ok()?;
    let text = &table[usize::from(lines[index]) + name.len() + 1..];
    Some(&text[..text.find('\n')?])
}

// The longest root in `text::SPECS`.
pub(super) const ROOT: usize = 19;

pub(super) enum Bare {
    None,
    Integer(&'static str),
    Spacing,
    Start,
    Space,
    Gap,
    Opacity,
    Repeat,
    Ratio,
    Stretch,
    Angle,
    Color,
}

pub(super) struct Spec {
    pub(super) keys: &'static str,
    pub(super) bare: Bare,
    pub(super) negative: bool,
    pub(super) fraction: bool,
    words: &'static str,
    pub(super) css: &'static str,
}

impl Spec {
    pub(super) fn parse(line: &'static str) -> Option<Self> {
        let mut fields = line.split('|');
        let keys = fields.next()?;
        let bare = match fields.next()? {
            "typed" => return None,
            "spacing" => Bare::Spacing,
            "start" => Bare::Start,
            "space" => Bare::Space,
            "gap" => Bare::Gap,
            "opacity" => Bare::Opacity,
            "repeat" => Bare::Repeat,
            "ratio" => Bare::Ratio,
            "stretch" => Bare::Stretch,
            "angle" => Bare::Angle,
            "color" => Bare::Color,
            integer => match integer.strip_prefix('#') {
                Some(unit) => Bare::Integer(unit),
                None => Bare::None,
            },
        };
        let flags = fields.next()?;
        Some(Self {
            keys,
            bare,
            negative: flags.contains('n'),
            fraction: flags.contains('f'),
            words: fields.next()?,
            css: fields.next()?,
        })
    }

    pub(super) fn word(&self, name: &str) -> Option<&'static str> {
        self.words
            .split('~')
            .filter_map(|word| word.split_once('='))
            .find(|(word, _)| *word == name)
            .map(|(_, value)| value)
    }
}

pub(super) struct Typed {
    pub(super) colors: &'static str,
    pub(super) types: &'static str,
    pub(super) widths: &'static str,
    pub(super) unit: &'static str,
    pub(super) default: &'static str,
    pub(super) color: &'static str,
    pub(super) width: &'static str,
}

impl Typed {
    pub(super) fn parse(line: &'static str) -> Option<Self> {
        let mut fields = line.split('|');
        let mut field = || fields.next();
        let colors = field()?;
        field()?;
        Some(Self {
            colors,
            types: field()?,
            widths: field()?,
            unit: field()?,
            default: field()?,
            color: field()?,
            width: field()?,
        })
    }
}

pub(super) const EDGES: &str =
    "var(--tw-mask-left), var(--tw-mask-right), var(--tw-mask-bottom), var(--tw-mask-top)";
