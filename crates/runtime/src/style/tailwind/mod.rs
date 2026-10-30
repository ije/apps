//! Tailwind CSS v4 without a build step. Like the Tailwind compiler, every class found in the
//! document becomes a CSS rule in `@layer utilities`, and `@apply` expands into the author rules.

use super::{Declaration, Declarations, Rule, Style, Stylesheet};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write,
    rc::Rc,
};

const THEME: &str = include_str!("theme.css");
const PREFLIGHT: &str = include_str!("preflight.css");

thread_local! {
    static DEFAULTS: HashMap<&'static str, &'static str> = THEME
        .lines()
        .filter_map(|line| line.strip_suffix(';')?.split_once(':'))
        .collect();
    /// Parsed once and added after the generated sheet; its layer order puts it in `base`.
    static BASE: Rc<Stylesheet> = Rc::new(Stylesheet::parse(&format!("@layer base{{{PREFLIGHT}}}")));
}

const KEYFRAMES: &[(&str, &str)] = &[
    ("spin", "to{transform:rotate(360deg)}"),
    ("ping", "75%,100%{transform:scale(2);opacity:0}"),
    ("pulse", "50%{opacity:.5}"),
    (
        "bounce",
        "0%,100%{transform:translateY(-25%);animation-timing-function:cubic-bezier(.8,0,1,1)}50%{transform:none;animation-timing-function:cubic-bezier(0,0,.2,1)}",
    ),
];

/// Initial values of the `--tw-*` properties. Tailwind registers them with `@property`; like its
/// fallback for older browsers, they are set on every element so they don't inherit.
const PROPERTIES: &[(&str, &str)] = &[
    ("--tw-translate-x", "0"),
    ("--tw-translate-y", "0"),
    ("--tw-translate-z", "0"),
    ("--tw-scale-x", "1"),
    ("--tw-scale-y", "1"),
    ("--tw-scale-z", "1"),
    ("--tw-space-x-reverse", "0"),
    ("--tw-space-y-reverse", "0"),
    ("--tw-border-style", "solid"),
    ("--tw-leading", "initial"),
    ("--tw-font-weight", "initial"),
    ("--tw-tracking", "initial"),
    ("--tw-ordinal", "initial"),
    ("--tw-slashed-zero", "initial"),
    ("--tw-numeric-figure", "initial"),
    ("--tw-numeric-spacing", "initial"),
    ("--tw-numeric-fraction", "initial"),
    ("--tw-shadow", "0 0 #0000"),
    ("--tw-shadow-color", "initial"),
    ("--tw-inset-shadow", "0 0 #0000"),
    ("--tw-ring-color", "initial"),
    ("--tw-ring-shadow", "0 0 #0000"),
    ("--tw-inset-ring-shadow", "0 0 #0000"),
    ("--tw-ring-inset", "initial"),
    ("--tw-ring-offset-width", "0px"),
    ("--tw-ring-offset-color", "#fff"),
    ("--tw-ring-offset-shadow", "0 0 #0000"),
    ("--tw-outline-style", "solid"),
    ("--tw-blur", "initial"),
    ("--tw-brightness", "initial"),
    ("--tw-contrast", "initial"),
    ("--tw-grayscale", "initial"),
    ("--tw-hue-rotate", "initial"),
    ("--tw-invert", "initial"),
    ("--tw-saturate", "initial"),
    ("--tw-sepia", "initial"),
    ("--tw-drop-shadow", "initial"),
    ("--tw-backdrop-blur", "initial"),
    ("--tw-backdrop-brightness", "initial"),
    ("--tw-backdrop-contrast", "initial"),
    ("--tw-backdrop-grayscale", "initial"),
    ("--tw-backdrop-hue-rotate", "initial"),
    ("--tw-backdrop-invert", "initial"),
    ("--tw-backdrop-opacity", "initial"),
    ("--tw-backdrop-saturate", "initial"),
    ("--tw-backdrop-sepia", "initial"),
    ("--tw-duration", "initial"),
    ("--tw-ease", "initial"),
    ("--tw-contain-size", "initial"),
    ("--tw-contain-layout", "initial"),
    ("--tw-contain-paint", "initial"),
    ("--tw-contain-style", "initial"),
    ("--tw-content", "\"\""),
];

/// Tailwind's property order, used to sort utilities that set the same property.
const ORDER: &[&str] = &[
    "container-type",
    "pointer-events",
    "visibility",
    "position",
    "inset",
    "inset-inline",
    "inset-block",
    "inset-inline-start",
    "inset-inline-end",
    "top",
    "right",
    "bottom",
    "left",
    "isolation",
    "z-index",
    "order",
    "grid-column",
    "grid-column-start",
    "grid-column-end",
    "grid-row",
    "grid-row-start",
    "grid-row-end",
    "float",
    "clear",
    "margin",
    "margin-inline",
    "margin-block",
    "margin-inline-start",
    "margin-inline-end",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "box-sizing",
    "display",
    "field-sizing",
    "aspect-ratio",
    "height",
    "max-height",
    "min-height",
    "width",
    "max-width",
    "min-width",
    "flex",
    "flex-shrink",
    "flex-grow",
    "flex-basis",
    "table-layout",
    "caption-side",
    "border-collapse",
    "border-spacing",
    "transform-origin",
    "translate",
    "--tw-translate-x",
    "--tw-translate-y",
    "--tw-translate-z",
    "scale",
    "--tw-scale-x",
    "--tw-scale-y",
    "--tw-scale-z",
    "rotate",
    "transform",
    "animation",
    "cursor",
    "touch-action",
    "resize",
    "scroll-margin",
    "scroll-margin-inline",
    "scroll-margin-block",
    "scroll-margin-top",
    "scroll-margin-right",
    "scroll-margin-bottom",
    "scroll-margin-left",
    "scroll-padding",
    "scroll-padding-inline",
    "scroll-padding-block",
    "scroll-padding-top",
    "scroll-padding-right",
    "scroll-padding-bottom",
    "scroll-padding-left",
    "list-style-position",
    "list-style-type",
    "appearance",
    "columns",
    "grid-auto-columns",
    "grid-auto-flow",
    "grid-auto-rows",
    "grid-template-columns",
    "grid-template-rows",
    "flex-direction",
    "flex-wrap",
    "place-content",
    "place-items",
    "align-content",
    "align-items",
    "justify-content",
    "justify-items",
    "gap",
    "column-gap",
    "row-gap",
    "--tw-space-x-reverse",
    "--tw-space-y-reverse",
    "place-self",
    "align-self",
    "justify-self",
    "overflow",
    "overflow-x",
    "overflow-y",
    "overscroll-behavior",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "scroll-behavior",
    "border-radius",
    "border-start-start-radius",
    "border-start-end-radius",
    "border-end-end-radius",
    "border-end-start-radius",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-bottom-right-radius",
    "border-bottom-left-radius",
    "border-width",
    "border-inline-width",
    "border-block-width",
    "border-inline-start-width",
    "border-inline-end-width",
    "border-top-width",
    "border-right-width",
    "border-bottom-width",
    "border-left-width",
    "border-style",
    "border-inline-style",
    "border-block-style",
    "border-inline-start-style",
    "border-inline-end-style",
    "border-top-style",
    "border-right-style",
    "border-bottom-style",
    "border-left-style",
    "border-color",
    "border-inline-color",
    "border-block-color",
    "border-inline-start-color",
    "border-inline-end-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "background-color",
    "background-image",
    "background-size",
    "background-attachment",
    "background-clip",
    "background-position",
    "background-repeat",
    "background-origin",
    "fill",
    "stroke",
    "stroke-width",
    "object-fit",
    "object-position",
    "padding",
    "padding-inline",
    "padding-block",
    "padding-inline-start",
    "padding-inline-end",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "text-align",
    "text-indent",
    "vertical-align",
    "font-family",
    "font-size",
    "line-height",
    "font-weight",
    "letter-spacing",
    "text-wrap",
    "overflow-wrap",
    "word-break",
    "text-overflow",
    "hyphens",
    "white-space",
    "color",
    "text-transform",
    "font-style",
    "font-stretch",
    "font-variant-numeric",
    "text-decoration-line",
    "text-decoration-color",
    "text-decoration-style",
    "text-decoration-thickness",
    "text-underline-offset",
    "-webkit-font-smoothing",
    "caret-color",
    "accent-color",
    "color-scheme",
    "opacity",
    "background-blend-mode",
    "mix-blend-mode",
    "box-shadow",
    "--tw-shadow",
    "--tw-shadow-color",
    "--tw-ring-shadow",
    "--tw-ring-color",
    "--tw-inset-shadow",
    "--tw-inset-ring-shadow",
    "--tw-ring-offset-width",
    "--tw-ring-offset-color",
    "outline",
    "outline-width",
    "outline-offset",
    "outline-color",
    "--tw-blur",
    "--tw-brightness",
    "--tw-contrast",
    "--tw-drop-shadow",
    "--tw-grayscale",
    "--tw-hue-rotate",
    "--tw-invert",
    "--tw-saturate",
    "--tw-sepia",
    "filter",
    "--tw-backdrop-blur",
    "--tw-backdrop-brightness",
    "--tw-backdrop-contrast",
    "--tw-backdrop-grayscale",
    "--tw-backdrop-hue-rotate",
    "--tw-backdrop-invert",
    "--tw-backdrop-opacity",
    "--tw-backdrop-saturate",
    "--tw-backdrop-sepia",
    "backdrop-filter",
    "transition-property",
    "transition-behavior",
    "transition-delay",
    "transition-duration",
    "transition-timing-function",
    "will-change",
    "contain",
    "content",
    "forced-color-adjust",
];

/// Variants in Tailwind's order. Templates use `&` for the utility selector; `@` starts an at-rule.
const VARIANTS: &[(&str, &str)] = &[
    ("*", ":is(& > *)"),
    ("**", ":is(& *)"),
    ("not", ""),
    ("group", ""),
    ("peer", ""),
    ("first-letter", "&::first-letter"),
    ("first-line", "&::first-line"),
    ("marker", "& *::marker, &::marker"),
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
    ("open", "&:is([open], :open)"),
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

const STATIC: &[(&str, &str)] = &[
    ("@container", "container-type:inline-size"),
    (
        "sr-only",
        "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap;border-width:0",
    ),
    (
        "not-sr-only",
        "position:static;width:auto;height:auto;padding:0;margin:0;overflow:visible;clip-path:none;white-space:normal",
    ),
    ("pointer-events-none", "pointer-events:none"),
    ("pointer-events-auto", "pointer-events:auto"),
    ("visible", "visibility:visible"),
    ("invisible", "visibility:hidden"),
    ("collapse", "visibility:collapse"),
    ("static", "position:static"),
    ("fixed", "position:fixed"),
    ("absolute", "position:absolute"),
    ("relative", "position:relative"),
    ("sticky", "position:sticky"),
    ("isolate", "isolation:isolate"),
    ("isolation-auto", "isolation:auto"),
    ("block", "display:block"),
    ("inline-block", "display:inline-block"),
    ("inline", "display:inline"),
    ("flex", "display:flex"),
    ("inline-flex", "display:inline-flex"),
    ("table", "display:table"),
    ("table-row", "display:table-row"),
    ("table-cell", "display:table-cell"),
    ("flow-root", "display:flow-root"),
    ("grid", "display:grid"),
    ("inline-grid", "display:inline-grid"),
    ("contents", "display:contents"),
    ("list-item", "display:list-item"),
    ("hidden", "display:none"),
    ("flex-row", "flex-direction:row"),
    ("flex-row-reverse", "flex-direction:row-reverse"),
    ("flex-col", "flex-direction:column"),
    ("flex-col-reverse", "flex-direction:column-reverse"),
    ("flex-wrap", "flex-wrap:wrap"),
    ("flex-nowrap", "flex-wrap:nowrap"),
    ("flex-wrap-reverse", "flex-wrap:wrap-reverse"),
    ("flex-auto", "flex:auto"),
    ("flex-initial", "flex:0 auto"),
    ("flex-none", "flex:none"),
    ("items-start", "align-items:flex-start"),
    ("items-end", "align-items:flex-end"),
    ("items-center", "align-items:center"),
    ("items-baseline", "align-items:baseline"),
    ("items-stretch", "align-items:stretch"),
    ("justify-normal", "justify-content:normal"),
    ("justify-start", "justify-content:flex-start"),
    ("justify-end", "justify-content:flex-end"),
    ("justify-center", "justify-content:center"),
    ("justify-between", "justify-content:space-between"),
    ("justify-around", "justify-content:space-around"),
    ("justify-evenly", "justify-content:space-evenly"),
    ("justify-stretch", "justify-content:stretch"),
    ("content-normal", "align-content:normal"),
    ("content-start", "align-content:flex-start"),
    ("content-end", "align-content:flex-end"),
    ("content-center", "align-content:center"),
    ("content-between", "align-content:space-between"),
    ("content-around", "align-content:space-around"),
    ("content-evenly", "align-content:space-evenly"),
    ("content-stretch", "align-content:stretch"),
    ("self-auto", "align-self:auto"),
    ("self-start", "align-self:flex-start"),
    ("self-end", "align-self:flex-end"),
    ("self-center", "align-self:center"),
    ("self-stretch", "align-self:stretch"),
    ("self-baseline", "align-self:baseline"),
    ("justify-items-start", "justify-items:start"),
    ("justify-items-end", "justify-items:end"),
    ("justify-items-center", "justify-items:center"),
    ("justify-items-stretch", "justify-items:stretch"),
    ("justify-self-auto", "justify-self:auto"),
    ("justify-self-start", "justify-self:start"),
    ("justify-self-end", "justify-self:end"),
    ("justify-self-center", "justify-self:center"),
    ("justify-self-stretch", "justify-self:stretch"),
    ("place-content-center", "place-content:center"),
    ("place-content-start", "place-content:start"),
    ("place-content-end", "place-content:end"),
    ("place-content-between", "place-content:space-between"),
    ("place-content-stretch", "place-content:stretch"),
    ("place-items-start", "place-items:start"),
    ("place-items-end", "place-items:end"),
    ("place-items-center", "place-items:center"),
    ("place-items-stretch", "place-items:stretch"),
    ("place-self-auto", "place-self:auto"),
    ("place-self-start", "place-self:start"),
    ("place-self-end", "place-self:end"),
    ("place-self-center", "place-self:center"),
    ("place-self-stretch", "place-self:stretch"),
    ("grid-flow-row", "grid-auto-flow:row"),
    ("grid-flow-col", "grid-auto-flow:column"),
    ("grid-flow-dense", "grid-auto-flow:dense"),
    ("grid-flow-row-dense", "grid-auto-flow:row dense"),
    ("grid-flow-col-dense", "grid-auto-flow:column dense"),
    (
        "truncate",
        "overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
    ),
    ("text-ellipsis", "text-overflow:ellipsis"),
    ("text-clip", "text-overflow:clip"),
    ("overflow-auto", "overflow:auto"),
    ("overflow-hidden", "overflow:hidden"),
    ("overflow-clip", "overflow:clip"),
    ("overflow-visible", "overflow:visible"),
    ("overflow-scroll", "overflow:scroll"),
    ("overflow-x-auto", "overflow-x:auto"),
    ("overflow-x-hidden", "overflow-x:hidden"),
    ("overflow-x-clip", "overflow-x:clip"),
    ("overflow-x-visible", "overflow-x:visible"),
    ("overflow-x-scroll", "overflow-x:scroll"),
    ("overflow-y-auto", "overflow-y:auto"),
    ("overflow-y-hidden", "overflow-y:hidden"),
    ("overflow-y-clip", "overflow-y:clip"),
    ("overflow-y-visible", "overflow-y:visible"),
    ("overflow-y-scroll", "overflow-y:scroll"),
    ("overscroll-auto", "overscroll-behavior:auto"),
    ("overscroll-contain", "overscroll-behavior:contain"),
    ("overscroll-none", "overscroll-behavior:none"),
    ("overscroll-x-contain", "overscroll-behavior-x:contain"),
    ("overscroll-y-contain", "overscroll-behavior-y:contain"),
    ("scroll-auto", "scroll-behavior:auto"),
    ("scroll-smooth", "scroll-behavior:smooth"),
    ("whitespace-normal", "white-space:normal"),
    ("whitespace-nowrap", "white-space:nowrap"),
    ("whitespace-pre", "white-space:pre"),
    ("whitespace-pre-line", "white-space:pre-line"),
    ("whitespace-pre-wrap", "white-space:pre-wrap"),
    ("whitespace-break-spaces", "white-space:break-spaces"),
    ("break-normal", "overflow-wrap:normal;word-break:normal"),
    ("break-words", "overflow-wrap:break-word"),
    ("break-all", "word-break:break-all"),
    ("break-keep", "word-break:keep-all"),
    ("wrap-anywhere", "overflow-wrap:anywhere"),
    ("wrap-break-word", "overflow-wrap:break-word"),
    ("wrap-normal", "overflow-wrap:normal"),
    ("text-wrap", "text-wrap:wrap"),
    ("text-nowrap", "text-wrap:nowrap"),
    ("text-balance", "text-wrap:balance"),
    ("text-pretty", "text-wrap:pretty"),
    ("text-left", "text-align:left"),
    ("text-center", "text-align:center"),
    ("text-right", "text-align:right"),
    ("text-justify", "text-align:justify"),
    ("text-start", "text-align:start"),
    ("text-end", "text-align:end"),
    ("align-baseline", "vertical-align:baseline"),
    ("align-top", "vertical-align:top"),
    ("align-middle", "vertical-align:middle"),
    ("align-bottom", "vertical-align:bottom"),
    ("align-text-top", "vertical-align:text-top"),
    ("align-text-bottom", "vertical-align:text-bottom"),
    ("align-sub", "vertical-align:sub"),
    ("align-super", "vertical-align:super"),
    ("uppercase", "text-transform:uppercase"),
    ("lowercase", "text-transform:lowercase"),
    ("capitalize", "text-transform:capitalize"),
    ("normal-case", "text-transform:none"),
    ("italic", "font-style:italic"),
    ("not-italic", "font-style:normal"),
    ("underline", "text-decoration-line:underline"),
    ("overline", "text-decoration-line:overline"),
    ("line-through", "text-decoration-line:line-through"),
    ("no-underline", "text-decoration-line:none"),
    ("decoration-solid", "text-decoration-style:solid"),
    ("decoration-double", "text-decoration-style:double"),
    ("decoration-dotted", "text-decoration-style:dotted"),
    ("decoration-dashed", "text-decoration-style:dashed"),
    ("decoration-wavy", "text-decoration-style:wavy"),
    ("decoration-auto", "text-decoration-thickness:auto"),
    (
        "decoration-from-font",
        "text-decoration-thickness:from-font",
    ),
    ("underline-offset-auto", "text-underline-offset:auto"),
    (
        "antialiased",
        "-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale",
    ),
    (
        "subpixel-antialiased",
        "-webkit-font-smoothing:auto;-moz-osx-font-smoothing:auto",
    ),
    ("normal-nums", "font-variant-numeric:normal"),
    (
        "ordinal",
        "--tw-ordinal:ordinal;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "slashed-zero",
        "--tw-slashed-zero:slashed-zero;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "lining-nums",
        "--tw-numeric-figure:lining-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "oldstyle-nums",
        "--tw-numeric-figure:oldstyle-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "proportional-nums",
        "--tw-numeric-spacing:proportional-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "tabular-nums",
        "--tw-numeric-spacing:tabular-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "diagonal-fractions",
        "--tw-numeric-fraction:diagonal-fractions;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "stacked-fractions",
        "--tw-numeric-fraction:stacked-fractions;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("select-none", "user-select:none"),
    ("select-text", "user-select:text"),
    ("select-all", "user-select:all"),
    ("select-auto", "user-select:auto"),
    ("resize", "resize:both"),
    ("resize-none", "resize:none"),
    ("resize-x", "resize:horizontal"),
    ("resize-y", "resize:vertical"),
    ("bg-clip-border", "background-clip:border-box"),
    ("bg-clip-padding", "background-clip:padding-box"),
    ("bg-clip-content", "background-clip:content-box"),
    ("bg-clip-text", "background-clip:text"),
    ("bg-fixed", "background-attachment:fixed"),
    ("bg-local", "background-attachment:local"),
    ("bg-scroll", "background-attachment:scroll"),
    ("bg-repeat", "background-repeat:repeat"),
    ("bg-no-repeat", "background-repeat:no-repeat"),
    ("bg-auto", "background-size:auto"),
    ("bg-cover", "background-size:cover"),
    ("bg-contain", "background-size:contain"),
    ("bg-center", "background-position:center"),
    ("bg-none", "background-image:none"),
    ("object-contain", "object-fit:contain"),
    ("object-cover", "object-fit:cover"),
    ("object-fill", "object-fit:fill"),
    ("object-none", "object-fit:none"),
    ("object-scale-down", "object-fit:scale-down"),
    ("object-center", "object-position:center"),
    ("object-top", "object-position:top"),
    ("object-bottom", "object-position:bottom"),
    ("object-left", "object-position:left"),
    ("object-right", "object-position:right"),
    ("outline-none", "--tw-outline-style:none;outline-style:none"),
    (
        "outline-hidden",
        "outline:2px solid transparent;outline-offset:2px",
    ),
    (
        "outline-solid",
        "--tw-outline-style:solid;outline-style:solid",
    ),
    (
        "outline-dashed",
        "--tw-outline-style:dashed;outline-style:dashed",
    ),
    (
        "outline-dotted",
        "--tw-outline-style:dotted;outline-style:dotted",
    ),
    (
        "outline-double",
        "--tw-outline-style:double;outline-style:double",
    ),
    ("border-solid", "--tw-border-style:solid;border-style:solid"),
    (
        "border-dashed",
        "--tw-border-style:dashed;border-style:dashed",
    ),
    (
        "border-dotted",
        "--tw-border-style:dotted;border-style:dotted",
    ),
    (
        "border-double",
        "--tw-border-style:double;border-style:double",
    ),
    (
        "border-hidden",
        "--tw-border-style:hidden;border-style:hidden",
    ),
    ("border-none", "--tw-border-style:none;border-style:none"),
    ("border-collapse", "border-collapse:collapse"),
    ("border-separate", "border-collapse:separate"),
    ("table-auto", "table-layout:auto"),
    ("table-fixed", "table-layout:fixed"),
    ("will-change-auto", "will-change:auto"),
    ("will-change-scroll", "will-change:scroll-position"),
    ("will-change-contents", "will-change:contents"),
    ("will-change-transform", "will-change:transform"),
    ("contain-none", "contain:none"),
    ("contain-content", "contain:content"),
    ("contain-strict", "contain:strict"),
    (
        "contain-size",
        "--tw-contain-size:size;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-inline-size",
        "--tw-contain-size:inline-size;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-layout",
        "--tw-contain-layout:layout;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-paint",
        "--tw-contain-paint:paint;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-style",
        "--tw-contain-style:style;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    ("appearance-none", "appearance:none"),
    ("appearance-auto", "appearance:auto"),
    ("box-border", "box-sizing:border-box"),
    ("box-content", "box-sizing:content-box"),
    (
        "filter",
        "filter:var(--tw-blur,) var(--tw-brightness,) var(--tw-contrast,) var(--tw-grayscale,) var(--tw-hue-rotate,) var(--tw-invert,) var(--tw-saturate,) var(--tw-sepia,) var(--tw-drop-shadow,)",
    ),
    ("filter-none", "filter:none"),
    (
        "backdrop-filter-none",
        "-webkit-backdrop-filter:none;backdrop-filter:none",
    ),
    ("ring-inset", "--tw-ring-inset:inset"),
    ("field-sizing-content", "field-sizing:content"),
    ("field-sizing-fixed", "field-sizing:fixed"),
    ("list-none", "list-style-type:none"),
    ("list-disc", "list-style-type:disc"),
    ("list-decimal", "list-style-type:decimal"),
    ("list-inside", "list-style-position:inside"),
    ("list-outside", "list-style-position:outside"),
    ("float-left", "float:left"),
    ("float-right", "float:right"),
    ("float-none", "float:none"),
    ("clear-both", "clear:both"),
    ("clear-none", "clear:none"),
    ("backface-hidden", "backface-visibility:hidden"),
    ("backface-visible", "backface-visibility:visible"),
    ("forced-color-adjust-auto", "forced-color-adjust:auto"),
    ("forced-color-adjust-none", "forced-color-adjust:none"),
];

const SHADOW: &str = "var(--tw-inset-shadow), var(--tw-inset-ring-shadow), var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow)";
const FILTER: &str = "var(--tw-blur,) var(--tw-brightness,) var(--tw-contrast,) var(--tw-grayscale,) var(--tw-hue-rotate,) var(--tw-invert,) var(--tw-saturate,) var(--tw-sepia,) var(--tw-drop-shadow,)";
const BACKDROP: &str = "var(--tw-backdrop-blur,) var(--tw-backdrop-brightness,) var(--tw-backdrop-contrast,) var(--tw-backdrop-grayscale,) var(--tw-backdrop-hue-rotate,) var(--tw-backdrop-invert,) var(--tw-backdrop-opacity,) var(--tw-backdrop-saturate,) var(--tw-backdrop-sepia,)";
const TRANSITION: &str = "transition-timing-function:var(--tw-ease, var(--default-transition-timing-function));transition-duration:var(--tw-duration, var(--default-transition-duration))";
const COLORS: &str = "color, background-color, border-color, outline-color, text-decoration-color, fill, stroke, --tw-gradient-from, --tw-gradient-via, --tw-gradient-to";

type Pairs = Vec<(String, String)>;

#[derive(Default)]
struct Body {
    declarations: Pairs,
    nested: Vec<(String, Pairs)>,
    sort: Option<&'static str>,
}

impl Body {
    fn new(declarations: &[(&str, String)]) -> Option<Self> {
        Some(Self {
            declarations: declarations
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            ..Self::default()
        })
    }

    fn parse(source: &str) -> Self {
        Self {
            declarations: source
                .split(';')
                .filter_map(|item| item.split_once(':'))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Variant {
    selectors: Vec<String>,
    rules: Vec<String>,
    content: bool,
}

type Rank = (u16, String);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Sort {
    variants: Vec<Rank>,
    properties: Vec<u16>,
    count: Reverse<usize>,
    name: String,
}

struct Compiled {
    sort: Sort,
    inline: Option<Pairs>,
    css: String,
}

/// The utilities compiled for one document, kept across style passes.
#[derive(Default)]
pub(crate) struct Compiler {
    sheets: Vec<*const Stylesheet>,
    theme: Style,
    /// Variables of `@theme inline`, which utilities use as values instead of `var()`.
    inline: BTreeSet<String>,
    variants: Vec<(String, String)>,
    utilities: Vec<(String, String)>,
    preflight: bool,
    classes: HashMap<String, Option<(Sort, String)>>,
    applied: Vec<(Rc<Stylesheet>, Rc<Stylesheet>, String)>,
    sheet: Option<Rc<Stylesheet>>,
    /// The theme variables, also set on the document node for pages without `<html>`.
    pub(crate) root: Rc<Style>,
}

impl Compiler {
    /// Adds the generated sheet before the first sheet that imports Tailwind and expands `@apply`.
    /// Returns whether Tailwind is used and the replaced sheet, so the caller can drop its scopes.
    pub(crate) fn update<'a>(
        &mut self,
        sheets: &mut Vec<Rc<Stylesheet>>,
        classes: impl Iterator<Item = &'a str>,
    ) -> (bool, Option<Rc<Stylesheet>>) {
        let Some(index) = sheets
            .iter()
            .position(|sheet| sheet.0.iter().any(|rule| matches!(rule, Rule::Tailwind(_))))
        else {
            return (false, None);
        };
        let pointers: Vec<_> = sheets.iter().map(Rc::as_ptr).collect();
        let mut dirty = self.sheet.is_none();
        let mut previous = None;
        if pointers != self.sheets {
            let mut theme = Style::new();
            let mut inline = BTreeSet::new();
            let mut variants = Vec::new();
            let mut utilities = Vec::new();
            let mut preflight = false;
            for sheet in sheets.iter() {
                for rule in &sheet.0 {
                    match rule {
                        Rule::Tailwind(included) => preflight |= included,
                        Rule::Theme(inlined, children) => {
                            for child in children {
                                if let Rule::Declarations(values) = child {
                                    let values = values.resolve();
                                    if *inlined {
                                        inline.extend(values.keys().cloned());
                                    }
                                    theme.extend(values);
                                }
                            }
                        }
                        Rule::Variant(name, template) => {
                            variants.retain(|(other, _)| other != name);
                            variants.push((name.clone(), template.clone()));
                        }
                        Rule::Utility(name, body) => utilities.push((name.clone(), body.clone())),
                        _ => {}
                    }
                }
            }
            if theme != self.theme
                || inline != self.inline
                || variants != self.variants
                || utilities != self.utilities
                || preflight != self.preflight
            {
                previous = self.sheet.take();
                *self = Self {
                    theme,
                    inline,
                    variants,
                    utilities,
                    preflight,
                    ..Self::default()
                };
                dirty = true;
            }
            self.sheets = pointers;
            self.applied
                .retain(|(sheet, ..)| sheets.iter().any(|other| Rc::ptr_eq(sheet, other)));
        }
        for class in classes {
            if !self.classes.contains_key(class) {
                let compiled = self.compile(class, &escape(class)).map(|compiled| {
                    let mut css = compiled.css;
                    if let Some(inline) = compiled.inline {
                        let _ = write!(css, "{}{{{}}}", escape(class), join(&inline, false));
                    }
                    (compiled.sort, css)
                });
                dirty |= compiled.is_some();
                self.classes.insert(class.to_owned(), compiled);
            }
        }
        for sheet in sheets.iter_mut() {
            if let Some((_, applied, _)) = self
                .applied
                .iter()
                .find(|(source, ..)| Rc::ptr_eq(source, sheet))
            {
                *sheet = applied.clone();
                continue;
            }
            let mut css = String::new();
            let rules = self.apply(&sheet.0, None, &mut css);
            let source: String = sheet.0.iter().map(Rule::source).collect();
            for name in names(&source, "var(--") {
                let _ = write!(css, "var(--{name})");
            }
            let applied = rules.map_or_else(|| sheet.clone(), |rules| Rc::new(Stylesheet(rules)));
            dirty |= !css.is_empty();
            self.applied.push((sheet.clone(), applied.clone(), css));
            *sheet = applied;
        }
        let replaced = if dirty {
            let sheet = Rc::new(Stylesheet::parse(&self.build()));
            self.sheet.replace(sheet).or(previous)
        } else {
            None
        };
        if let Some(sheet) = &self.sheet {
            sheets.insert(index, sheet.clone());
            if self.preflight {
                sheets.insert(index + 1, BASE.with(Clone::clone));
            }
        }
        (true, replaced)
    }

    fn build(&mut self) -> String {
        let mut utilities: Vec<_> = self.classes.values().flatten().collect();
        utilities.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut used = String::new();
        for (_, css) in &utilities {
            used.push_str(css);
        }
        for (.., css) in &self.applied {
            used.push_str(css);
        }
        for value in self.theme.values() {
            used.push_str(value);
        }
        if self.preflight {
            used.push_str(PREFLIGHT);
        }
        let mut vars = BTreeMap::new();
        DEFAULTS.with(|defaults| {
            let mut pending: Vec<String> = names(&used, "var(--").map(str::to_owned).collect();
            while let Some(name) = pending.pop() {
                let key = format!("--{name}");
                if self.theme.contains_key(&key) || vars.contains_key(&key) {
                    continue;
                }
                let Some(&value) = defaults.get(key.as_str()) else {
                    continue;
                };
                // `--theme(--name, initial)` points at another theme variable and is dropped without it.
                let value = match value
                    .strip_prefix("--theme(")
                    .and_then(|value| value.strip_suffix(", initial)"))
                {
                    Some(name) if self.inline.contains(name) => self.theme[name].clone(),
                    Some(name) if self.theme.contains_key(name) || defaults.contains_key(name) => {
                        format!("var({name})")
                    }
                    Some(_) => continue,
                    None => value.to_owned(),
                };
                pending.extend(names(&value, "var(--").map(str::to_owned));
                vars.insert(key, value);
            }
        });
        let mut css = String::from(
            "@layer properties;@layer theme,base,components,utilities;@layer properties{*,::before,::after{",
        );
        let tw: BTreeSet<_> = names(&used, "--tw-").collect();
        for (name, value) in PROPERTIES {
            if tw.contains(&name[5..]) {
                let _ = write!(css, "{name}:{value};");
            }
        }
        css.push_str("}}@layer theme{:root{");
        let keyframes: Vec<_> = KEYFRAMES
            .iter()
            .filter(|(name, _)| vars.contains_key(&format!("--animate-{name}")))
            .collect();
        let mut root = vars;
        root.extend(
            self.theme
                .iter()
                .filter(|(name, _)| !self.inline.contains(*name))
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        for (name, value) in &root {
            let _ = write!(css, "{name}:{value};");
        }
        self.root = Rc::new(root);
        css.push_str("}}@layer utilities{");
        for (_, rule) in utilities {
            css.push_str(rule);
        }
        css.push('}');
        for (name, frames) in keyframes {
            let _ = write!(css, "@keyframes {name}{{{frames}}}");
        }
        css
    }

    /// Expands `@apply` in `rules`; `None` when nothing changed. Generated CSS is added to `used`.
    fn apply(&self, rules: &[Rule], parent: Option<&str>, used: &mut String) -> Option<Vec<Rule>> {
        let mut changed = false;
        let mut result = Vec::with_capacity(rules.len());
        for rule in rules {
            match rule {
                Rule::Style(selectors, children) => {
                    let base = selectors
                        .iter()
                        .map(|selector| match parent {
                            Some(parent) => {
                                selector.source().replace('&', &format!(":is({parent})"))
                            }
                            None => selector.source(),
                        })
                        .collect::<Vec<_>>();
                    let base = if base.len() == 1 {
                        base[0].clone()
                    } else {
                        format!(":is({})", base.join(","))
                    };
                    let mut extra = String::new();
                    let mut body = Vec::with_capacity(children.len());
                    for child in children {
                        let Rule::Declarations(values) = child else {
                            body.push(child.clone());
                            continue;
                        };
                        if !values
                            .0
                            .iter()
                            .any(|value| matches!(value, Declaration::Apply(_)))
                        {
                            body.push(child.clone());
                            continue;
                        }
                        changed = true;
                        let mut declarations = Vec::new();
                        for value in &values.0 {
                            let Declaration::Apply(classes) = value else {
                                declarations.push(value.clone());
                                continue;
                            };
                            let mut compiled: Vec<_> = classes
                                .iter()
                                .filter_map(|class| self.compile(class, &base))
                                .collect();
                            compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                            for compiled in compiled {
                                extra.push_str(&compiled.css);
                                for (key, value) in compiled.inline.into_iter().flatten() {
                                    used.push_str(&value);
                                    declarations.push(Declaration::Property(key, value));
                                }
                            }
                        }
                        body.push(Rule::Declarations(Declarations(declarations)));
                    }
                    let nested = self.apply(&body, Some(&base), used);
                    changed |= nested.is_some();
                    result.push(Rule::Style(selectors.clone(), nested.unwrap_or(body)));
                    if !extra.is_empty() {
                        used.push_str(&extra);
                        result.extend(Stylesheet::parse(&extra).0);
                    }
                }
                Rule::Media(queries, children) => match self.apply(children, parent, used) {
                    Some(children) => {
                        changed = true;
                        result.push(Rule::Media(queries.clone(), children));
                    }
                    None => result.push(rule.clone()),
                },
                Rule::Layer(names, children) => match self.apply(children, parent, used) {
                    Some(children) => {
                        changed = true;
                        result.push(Rule::Layer(names.clone(), children));
                    }
                    None => result.push(rule.clone()),
                },
                Rule::Supports(condition, supported, children) => {
                    match self.apply(children, parent, used) {
                        Some(children) => {
                            changed = true;
                            result.push(Rule::Supports(condition.clone(), *supported, children));
                        }
                        None => result.push(rule.clone()),
                    }
                }
                _ => result.push(rule.clone()),
            }
        }
        changed.then_some(result)
    }

    fn compile(&self, class: &str, base: &str) -> Option<Compiled> {
        let mut parts = split(class, ':');
        let utility = parts.pop()?;
        let (utility, important) = match utility
            .strip_prefix('!')
            .or_else(|| utility.strip_suffix('!'))
        {
            Some(utility) => (utility, true),
            None => (utility, false),
        };
        let body = self.utility(utility)?;
        let mut selectors = vec![base.to_owned()];
        let mut rules = Vec::new();
        let mut ranks = Vec::new();
        let mut content = false;
        for name in parts {
            let (rank, variant) = self.variant(name)?;
            ranks.push(rank);
            if !variant.selectors.is_empty() {
                selectors = selectors
                    .iter()
                    .flat_map(|selector| {
                        variant
                            .selectors
                            .iter()
                            .map(move |template| template.replace('&', selector))
                    })
                    .collect();
            }
            rules.extend(variant.rules);
            content |= variant.content;
        }
        ranks.sort_unstable_by(|a, b| b.cmp(a));
        ranks.dedup();
        let mut properties = BTreeSet::new();
        let mut count = 0;
        for (key, _) in body
            .declarations
            .iter()
            .chain(body.nested.iter().flat_map(|(_, values)| values))
        {
            count += 1;
            if let Some(index) = ORDER
                .iter()
                .position(|name| *name == body.sort.unwrap_or(key))
            {
                properties.insert(index as u16);
            }
        }
        let sort = Sort {
            variants: ranks,
            properties: properties.into_iter().collect(),
            count: Reverse(count),
            name: class.to_owned(),
        };
        if selectors.len() == 1
            && selectors[0] == base
            && rules.is_empty()
            && body.nested.is_empty()
        {
            let mut inline = body.declarations;
            if important {
                for (_, value) in &mut inline {
                    value.push_str(" !important");
                }
            }
            return Some(Compiled {
                sort,
                inline: Some(inline),
                css: String::new(),
            });
        }
        let mut css = String::new();
        let mut rule = String::new();
        let list = selectors.join(",");
        if !body.declarations.is_empty() {
            let content = if content {
                "content:var(--tw-content);"
            } else {
                ""
            };
            let _ = write!(
                rule,
                "{list}{{{content}{}}}",
                join(&body.declarations, important)
            );
        }
        for (template, values) in &body.nested {
            let list = selectors
                .iter()
                .map(|selector| template.replace('&', selector))
                .collect::<Vec<_>>()
                .join(",");
            let _ = write!(rule, "{list}{{{}}}", join(values, important));
        }
        for at in &rules {
            let _ = write!(css, "{at}{{");
        }
        css.push_str(&rule);
        css.push_str(&"}".repeat(rules.len()));
        Some(Compiled {
            sort,
            inline: None,
            css,
        })
    }

    fn variant(&self, name: &str) -> Option<(Rank, Variant)> {
        let order = |name: &str| VARIANTS.iter().position(|(other, _)| *other == name);
        if let Some(raw) = arbitrary(name) {
            let value = decode(raw);
            let source = if value.starts_with('@') || value.contains('&') {
                value.clone()
            } else {
                format!("&:is({value})")
            };
            return Some(((u16::MAX, value), template(&source)));
        }
        if let Some(index) = self.variants.iter().position(|(other, _)| other == name) {
            let rank = order(name).unwrap_or(VARIANTS.len() + index) as u16;
            return Some(((rank, String::new()), template(&self.variants[index].1)));
        }
        if let Some(index) = order(name)
            && !VARIANTS[index].1.is_empty()
        {
            let mut variant = template(VARIANTS[index].1);
            if name == "hover" {
                variant.rules.push("@media (hover: hover)".into());
            }
            variant.content = matches!(name, "before" | "after");
            return Some(((index as u16, String::new()), variant));
        }
        if let Some(width) = self.value("--breakpoint", name) {
            return self.media(">=", &width, "min");
        }
        let (root, value) = [
            "group",
            "peer",
            "not",
            "has",
            "in",
            "aria",
            "data",
            "nth-last-of-type",
            "nth-of-type",
            "nth-last",
            "nth",
            "supports",
            "min",
            "max",
        ]
        .iter()
        .find_map(|root| Some((*root, name.strip_prefix(root)?.strip_prefix('-')?)))?;
        let rank = order(root)? as u16;
        match root {
            "group" | "peer" | "not" | "has" | "in" => {
                let (value, modifier) = modifier(value);
                let (inner_rank, inner) = self.variant(value)?;
                let mut variant = Variant {
                    rules: inner.rules,
                    ..Variant::default()
                };
                if inner.selectors.is_empty() {
                    if root != "not" {
                        return None;
                    }
                    for rule in &mut variant.rules {
                        *rule = rule.replacen("@media ", "@media not ", 1);
                    }
                } else {
                    let marker = match modifier {
                        Some(name) => format!(":where(.{root}\\/{})", &escape(name)[1..]),
                        None => format!(":where(.{root})"),
                    };
                    let target = if matches!(root, "group" | "peer") {
                        marker.as_str()
                    } else {
                        "*"
                    };
                    let selectors: Vec<_> = inner
                        .selectors
                        .iter()
                        .map(|selector| selector.replace('&', target))
                        .collect();
                    let list = if selectors.len() > 1 && matches!(root, "group" | "peer") {
                        format!(":is({})", selectors.join(", "))
                    } else {
                        selectors.join(", ")
                    };
                    variant.selectors = vec![match root {
                        "group" => format!("&:is({list} *)"),
                        "peer" => format!("&:is({list} ~ *)"),
                        "not" => format!("&:not({list})"),
                        "has" => format!("&:has({list})"),
                        _ => format!(":where({list}) &"),
                    }];
                }
                Some((
                    (rank, format!("{:05}{}", inner_rank.0, inner_rank.1)),
                    variant,
                ))
            }
            "aria" | "data" => {
                let selector = match arbitrary(value) {
                    Some(raw) => format!("&[{root}-{}]", quote(&decode(raw))),
                    None if root == "aria" => format!("&[aria-{value}=\"true\"]"),
                    None => format!("&[data-{value}]"),
                };
                Some(((rank, value.to_owned()), template(&selector)))
            }
            "nth" | "nth-last" | "nth-of-type" | "nth-last-of-type" => {
                let argument = arbitrary(value)
                    .map(decode)
                    .or_else(|| value.parse::<u32>().ok().map(|number| number.to_string()))?;
                let pseudo = match root {
                    "nth" => "nth-child",
                    "nth-last" => "nth-last-child",
                    _ => root,
                };
                Some((
                    (rank, argument.clone()),
                    template(&format!("&:{pseudo}({argument})")),
                ))
            }
            "supports" => {
                let raw = decode(arbitrary(value)?);
                let condition = if raw.starts_with('(') {
                    raw.clone()
                } else {
                    format!("({raw})")
                };
                Some((
                    (rank, raw),
                    Variant {
                        rules: vec![format!("@supports {condition}")],
                        ..Variant::default()
                    },
                ))
            }
            _ => {
                let width = match arbitrary(value) {
                    Some(raw) => decode(raw),
                    None => self.value("--breakpoint", value)?,
                };
                self.media(if root == "min" { ">=" } else { "<" }, &width, root)
            }
        }
    }

    fn media(&self, operator: &str, width: &str, root: &str) -> Option<(Rank, Variant)> {
        let pixels = width
            .strip_suffix("rem")
            .or_else(|| width.strip_suffix("em"))
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| value * 16.)
            .or_else(|| width.strip_suffix("px")?.parse().ok())
            .unwrap_or(0.);
        let key = if root == "max" { 1e6 - pixels } else { pixels };
        Some((
            (
                VARIANTS.iter().position(|(name, _)| *name == root)? as u16,
                format!("{key:012.3}"),
            ),
            Variant {
                rules: vec![format!("@media (width {operator} {width})")],
                ..Variant::default()
            },
        ))
    }

    /// A theme value as `var(--name)`, from the author theme or Tailwind's defaults.
    fn theme(&self, namespace: &str, key: &str) -> Option<String> {
        let name = format!("{namespace}-{key}");
        if self.inline.contains(&name) {
            return self.theme.get(&name).cloned();
        }
        (self.theme.contains_key(&name)
            || DEFAULTS.with(|defaults| defaults.contains_key(name.as_str())))
        .then(|| format!("var({name})"))
    }

    /// An `@utility` from the author CSS, with `--value()` and `--modifier()` resolved.
    fn custom(&self, class: &str) -> Option<Body> {
        let (candidate, modifier) = modifier(class);
        self.utilities.iter().rev().find_map(|(name, body)| {
            let value = match name.strip_suffix('*') {
                Some(prefix) => Some(
                    candidate
                        .strip_prefix(prefix)
                        .filter(|value| !value.is_empty())?,
                ),
                None if name == class => None,
                None => return None,
            };
            let mut resolved = false;
            let body = self.expand(body, value, modifier, &mut resolved)?;
            (value.is_none() || resolved).then_some(body)
        })
    }

    fn expand(
        &self,
        source: &str,
        value: Option<&str>,
        modifier: Option<&str>,
        resolved: &mut bool,
    ) -> Option<Body> {
        let mut body = Body::default();
        let mut depth = 0;
        let mut start = 0;
        for (index, character) in source.char_indices() {
            match character {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let item = &source[start..index];
                        let (selector, inner) = item.split_once('{')?;
                        let inner = self.expand(inner, value, modifier, resolved)?;
                        body.nested
                            .push((selector.trim().to_owned(), inner.declarations));
                        start = index + 1;
                    }
                }
                ';' if depth == 0 => {
                    let item = &source[start..index];
                    start = index + 1;
                    let Some((property, raw)) = item.split_once(':') else {
                        continue;
                    };
                    let mut raw = raw.trim().to_owned();
                    let mut valid = true;
                    for (function, input) in [("--value(", value), ("--modifier(", modifier)] {
                        while let Some(position) = raw.find(function) {
                            let open = position + function.len();
                            let mut depth = 0;
                            let close = open
                                + raw[open..]
                                    .find(|c| {
                                        depth += i32::from(c == '(') - i32::from(c == ')');
                                        depth < 0
                                    })
                                    .unwrap_or(raw.len() - open - 1);
                            match input.and_then(|input| self.resolve(&raw[open..close], input)) {
                                Some(result) => raw.replace_range(position..=close, &result),
                                None => {
                                    valid = false;
                                    break;
                                }
                            }
                            *resolved |= function == "--value(";
                        }
                    }
                    if valid {
                        body.declarations.push((property.trim().to_owned(), raw));
                    }
                }
                _ => {}
            }
        }
        Some(body)
    }

    /// Resolves one `--value(...)`: theme namespaces, `"literals"`, bare types and `[types]`.
    fn resolve(&self, arguments: &str, value: &str) -> Option<String> {
        split(arguments, ',')
            .into_iter()
            .map(str::trim)
            .find_map(|argument| {
                if let Some(namespace) = argument
                    .strip_suffix("-*")
                    .filter(|name| name.starts_with("--"))
                {
                    return arbitrary(value)
                        .is_none()
                        .then(|| self.theme(namespace, value))
                        .flatten();
                }
                if let Some(literal) = argument.strip_prefix('"').and_then(|a| a.strip_suffix('"'))
                {
                    return (literal == value).then(|| value.to_owned());
                }
                match (arbitrary(argument), arbitrary(value)) {
                    (Some(kind), Some(raw)) => {
                        let (hint, raw) = hint(raw);
                        let raw = decode(raw);
                        (kind == "*" || hint == Some(kind) || hint.is_none() && is_type(kind, &raw))
                            .then_some(raw)
                    }
                    (None, None) => is_type(argument, value).then(|| value.to_owned()),
                    _ => None,
                }
            })
    }

    fn value(&self, namespace: &str, key: &str) -> Option<String> {
        let name = format!("{namespace}-{key}");
        self.theme.get(&name).cloned().or_else(|| {
            DEFAULTS.with(|defaults| defaults.get(name.as_str()).map(|value| (*value).to_owned()))
        })
    }

    fn utility(&self, class: &str) -> Option<Body> {
        if let Some(body) = self.custom(class) {
            return Some(body);
        }
        if let Some(raw) = arbitrary(class) {
            let (property, value) = raw.split_once(':')?;
            if property.is_empty()
                || !property
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || value.is_empty()
            {
                return None;
            }
            return Body::new(&[(property, decode(value))]);
        }
        if let Some((_, source)) = STATIC.iter().find(|(name, _)| *name == class) {
            return Some(Body::parse(source));
        }
        let (negative, class) = match class.strip_prefix('-') {
            Some(class) => (true, class),
            None => (false, class),
        };
        let (class, modifier) = modifier(class);
        let mut depth = 0;
        let mut dashes = Vec::new();
        for (index, byte) in class.bytes().enumerate() {
            match byte {
                b'[' | b'(' => depth += 1,
                b']' | b')' => depth -= 1,
                b'-' if depth == 0 && index > 0 => dashes.push(index),
                _ => {}
            }
        }
        dashes
            .iter()
            .rev()
            .find_map(|&index| {
                self.functional(
                    &class[..index],
                    Some(&class[index + 1..]),
                    modifier,
                    negative,
                )
            })
            .or_else(|| self.functional(class, None, modifier, negative))
    }

    fn spacing(&self, value: &str) -> Option<String> {
        if let Some(raw) = arbitrary(value) {
            return Some(decode(hint(raw).1));
        }
        if value == "px" {
            return Some("1px".into());
        }
        if let Some(value) = self.theme("--spacing", value) {
            return Some(value);
        }
        let number: f64 = value.parse().ok()?;
        (number >= 0.
            && (number * 4.).fract() == 0.
            && value.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        .then(|| format!("calc(var(--spacing) * {value})"))
    }

    fn color(&self, value: &str, modifier: Option<&str>) -> Option<String> {
        let color = if let Some(raw) = arbitrary(value) {
            let (kind, raw) = hint(raw);
            let raw = decode(raw);
            if kind.is_some_and(|kind| kind != "color")
                || kind.is_none() && !is_color(&raw) && !raw.starts_with("var(")
            {
                return None;
            }
            raw
        } else {
            match value {
                "inherit" => "inherit".into(),
                "current" => "currentcolor".into(),
                "transparent" => "transparent".into(),
                _ => self.theme("--color", value)?,
            }
        };
        let Some(modifier) = modifier else {
            return Some(color);
        };
        let alpha = match arbitrary(modifier) {
            Some(raw) => {
                let raw = decode(raw);
                match raw.parse::<f32>() {
                    Ok(number) if number <= 1. => format!("{}%", number * 100.),
                    _ => raw,
                }
            }
            None => {
                let number: f32 = modifier.parse().ok()?;
                if !(0. ..=100.).contains(&number) {
                    return None;
                }
                format!("{modifier}%")
            }
        };
        (color != "inherit").then(|| format!("color-mix(in oklab, {color} {alpha}, transparent)"))
    }

    fn functional(
        &self,
        root: &str,
        value: Option<&str>,
        modifier: Option<&str>,
        negative: bool,
    ) -> Option<Body> {
        let negate = |value: String| {
            if !negative {
                value
            } else if let Some(number) = value.strip_prefix("calc(var(--spacing) * ") {
                format!("calc(var(--spacing) * -{number}")
            } else {
                format!("calc({value} * -1)")
            }
        };
        let fraction = |value: &str| {
            let modifier = modifier?;
            (value.bytes().all(|b| b.is_ascii_digit())
                && modifier.bytes().all(|b| b.is_ascii_digit())
                && modifier != "0")
                .then(|| format!("calc({value}/{modifier} * 100%)"))
        };
        let length = |raw: &str| {
            let (kind, raw) = hint(raw);
            let raw = decode(raw);
            (matches!(
                kind,
                Some("length" | "line-width" | "number" | "percentage")
            ) || kind.is_none() && is_length(&raw))
            .then_some(raw)
        };
        match root {
            "w" | "min-w" | "max-w" | "h" | "min-h" | "max-h" | "size" => {
                let value = value?;
                let horizontal = root.ends_with('w') || root == "size";
                let resolved = match (value, modifier) {
                    (value, Some(_)) => fraction(value)?,
                    ("full", _) => "100%".into(),
                    ("auto", _) if !root.starts_with("max") => "auto".into(),
                    ("none", _) if root.starts_with("max") => "none".into(),
                    ("screen", _) if root != "size" => {
                        if horizontal { "100vw" } else { "100vh" }.into()
                    }
                    ("dvw" | "svw" | "lvw", _) if root != "size" && horizontal => {
                        format!("100{value}")
                    }
                    ("dvh" | "svh" | "lvh", _) if root != "size" && !horizontal => {
                        format!("100{value}")
                    }
                    ("min", _) => "min-content".into(),
                    ("max", _) => "max-content".into(),
                    ("fit", _) => "fit-content".into(),
                    ("lh", _) => "1lh".into(),
                    _ => horizontal
                        .then(|| self.theme("--container", value))
                        .flatten()
                        .or_else(|| self.spacing(value))?,
                };
                let properties: &[&str] = match root {
                    "w" => &["width"],
                    "min-w" => &["min-width"],
                    "max-w" => &["max-width"],
                    "h" => &["height"],
                    "min-h" => &["min-height"],
                    "max-h" => &["max-height"],
                    _ => &["width", "height"],
                };
                Body::new(
                    &properties
                        .iter()
                        .map(|property| (*property, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "inset" | "inset-x" | "inset-y" | "start" | "end" | "top" | "right" | "bottom"
            | "left" => {
                let value = value?;
                let resolved = match value {
                    "auto" if !negative => "auto".into(),
                    "full" => "100%".into(),
                    _ => fraction(value).or_else(|| self.spacing(value))?,
                };
                let property = match root {
                    "inset-x" => "inset-inline",
                    "inset-y" => "inset-block",
                    "start" => "inset-inline-start",
                    "end" => "inset-inline-end",
                    _ => root,
                };
                Body::new(&[(property, negate(resolved))])
            }
            "m" | "mx" | "my" | "ms" | "me" | "mt" | "mr" | "mb" | "ml" | "p" | "px" | "py"
            | "ps" | "pe" | "pt" | "pr" | "pb" | "pl" | "scroll-m" | "scroll-mx" | "scroll-my"
            | "scroll-mt" | "scroll-mr" | "scroll-mb" | "scroll-ml" | "scroll-p" | "scroll-px"
            | "scroll-py" | "scroll-pt" | "scroll-pr" | "scroll-pb" | "scroll-pl" => {
                let value = value?;
                let (scroll, short) = match root.strip_prefix("scroll-") {
                    Some(short) => ("scroll-", short),
                    None => ("", root),
                };
                let margin = short.starts_with('m');
                if negative && !margin {
                    return None;
                }
                let resolved = match value {
                    "auto" if margin && !negative => "auto".into(),
                    _ => self.spacing(value)?,
                };
                let name = if margin { "margin" } else { "padding" };
                let property = match &short[1..] {
                    "" => format!("{scroll}{name}"),
                    "x" => format!("{scroll}{name}-inline"),
                    "y" => format!("{scroll}{name}-block"),
                    "s" => format!("{scroll}{name}-inline-start"),
                    "e" => format!("{scroll}{name}-inline-end"),
                    "t" => format!("{scroll}{name}-top"),
                    "r" => format!("{scroll}{name}-right"),
                    "b" => format!("{scroll}{name}-bottom"),
                    _ => format!("{scroll}{name}-left"),
                };
                Body::new(&[(property.as_str(), negate(resolved))])
            }
            "gap" | "gap-x" | "gap-y" if !negative => {
                let property = match root {
                    "gap-x" => "column-gap",
                    "gap-y" => "row-gap",
                    _ => "gap",
                };
                Body::new(&[(property, self.spacing(value?)?)])
            }
            "space-x" | "space-y" => {
                let value = negate(self.spacing(value?)?);
                let axis = &root[6..];
                let (start, end) = if axis == "x" {
                    ("margin-inline-start", "margin-inline-end")
                } else {
                    ("margin-block-start", "margin-block-end")
                };
                let reverse = format!("--tw-space-{axis}-reverse");
                Some(Body {
                    nested: vec![(
                        ":where(& > :not(:last-child))".into(),
                        vec![
                            (reverse.clone(), "0".into()),
                            (start.into(), format!("calc({value} * var({reverse}))")),
                            (
                                end.into(),
                                format!("calc({value} * calc(1 - var({reverse})))"),
                            ),
                        ],
                    )],
                    sort: Some(if axis == "x" { "column-gap" } else { "row-gap" }),
                    ..Body::default()
                })
            }
            "translate" | "translate-x" | "translate-y" | "translate-z" => {
                let value = value?;
                if value == "none" && root == "translate" {
                    return Body::new(&[("translate", "none".into())]);
                }
                let resolved = negate(match value {
                    "full" => "100%".into(),
                    _ => fraction(value).or_else(|| self.spacing(value))?,
                });
                let axis = [&root[root.len().min(10)..]];
                let axes: &[&str] = if root == "translate" {
                    &["x", "y"]
                } else {
                    &axis
                };
                let mut body: Vec<_> = axes
                    .iter()
                    .map(|axis| (format!("--tw-translate-{axis}"), resolved.clone()))
                    .collect();
                body.push((
                    "translate".into(),
                    if root == "translate-z" {
                        "var(--tw-translate-x) var(--tw-translate-y) var(--tw-translate-z)"
                    } else {
                        "var(--tw-translate-x) var(--tw-translate-y)"
                    }
                    .into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "scale" | "scale-x" | "scale-y" | "scale-z" => {
                let value = value?;
                if value == "none" && root == "scale" {
                    return Body::new(&[("scale", "none".into())]);
                }
                let resolved = negate(match arbitrary(value) {
                    Some(raw) => decode(raw),
                    None => format!("{}%", value.parse::<u32>().ok()?),
                });
                let axis = [&root[root.len().min(6)..]];
                let axes: &[&str] = if root == "scale" {
                    &["x", "y", "z"]
                } else {
                    &axis
                };
                let mut body: Vec<_> = axes
                    .iter()
                    .map(|axis| (format!("--tw-scale-{axis}"), resolved.clone()))
                    .collect();
                body.push(("scale".into(), "var(--tw-scale-x) var(--tw-scale-y)".into()));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "rotate" => {
                let value = value?;
                let resolved = match arbitrary(value) {
                    Some(raw) => decode(raw),
                    None if value == "none" => return Body::new(&[("rotate", "none".into())]),
                    None => format!("{}deg", value.parse::<f32>().ok()?),
                };
                Body::new(&[("rotate", negate(resolved))])
            }
            "z" | "order" => {
                let value = value?;
                let resolved = match (root, value) {
                    ("z", "auto") => "auto".into(),
                    ("order", "first") => "-9999".into(),
                    ("order", "last") => "9999".into(),
                    ("order", "none") => "0".into(),
                    _ => arbitrary(value)
                        .map(decode)
                        .or_else(|| value.parse::<u32>().ok().map(|number| number.to_string()))?,
                };
                Body::new(&[(
                    if root == "z" { "z-index" } else { "order" },
                    negate(resolved),
                )])
            }
            "grid-cols" | "grid-rows" => {
                let value = value?;
                let resolved = match value {
                    "none" | "subgrid" => value.into(),
                    _ => arbitrary(value).map(decode).or_else(|| {
                        value
                            .parse::<u16>()
                            .ok()
                            .filter(|number| *number > 0)
                            .map(|number| format!("repeat({number}, minmax(0, 1fr))"))
                    })?,
                };
                let property = if root == "grid-cols" {
                    "grid-template-columns"
                } else {
                    "grid-template-rows"
                };
                Body::new(&[(property, resolved)])
            }
            "col" | "row" | "col-span" | "row-span" | "col-start" | "col-end" | "row-start"
            | "row-end" => {
                let value = value?;
                let resolved = match (root, value) {
                    ("col-span" | "row-span", "full") => "1 / -1".into(),
                    ("col-span" | "row-span", _) => {
                        let number = value.parse::<u16>().ok()?;
                        format!("span {number} / span {number}")
                    }
                    (_, "auto") => "auto".into(),
                    _ => arbitrary(value)
                        .map(decode)
                        .or_else(|| value.parse::<i32>().ok().map(|number| number.to_string()))?,
                };
                let property = match root {
                    "col" | "col-span" => "grid-column",
                    "row" | "row-span" => "grid-row",
                    "col-start" => "grid-column-start",
                    "col-end" => "grid-column-end",
                    "row-start" => "grid-row-start",
                    _ => "grid-row-end",
                };
                Body::new(&[(property, negate(resolved))])
            }
            "flex" => {
                let value = value?;
                let resolved = fraction(value)
                    .or_else(|| arbitrary(value).map(decode))
                    .or_else(|| value.parse::<u16>().ok().map(|number| number.to_string()))?;
                Body::new(&[("flex", resolved)])
            }
            "grow" | "shrink" | "flex-grow" | "flex-shrink" => {
                let resolved = match value {
                    None => "1".into(),
                    Some(value) => arbitrary(value)
                        .map(decode)
                        .or_else(|| value.parse::<u16>().ok().map(|number| number.to_string()))?,
                };
                Body::new(&[(
                    if root.ends_with("grow") {
                        "flex-grow"
                    } else {
                        "flex-shrink"
                    },
                    resolved,
                )])
            }
            "basis" => {
                let value = value?;
                let resolved = match value {
                    "full" => "100%".into(),
                    "auto" => "auto".into(),
                    _ => fraction(value)
                        .or_else(|| self.theme("--container", value))
                        .or_else(|| self.spacing(value))?,
                };
                Body::new(&[("flex-basis", resolved)])
            }
            "indent" => Body::new(&[("text-indent", negate(self.spacing(value?)?))]),
            _ if root == "rounded" || root.starts_with("rounded-") => {
                let corners: &[&str] = match root.strip_prefix("rounded-").unwrap_or("") {
                    "" => &["border-radius"],
                    "t" => &["border-top-left-radius", "border-top-right-radius"],
                    "r" => &["border-top-right-radius", "border-bottom-right-radius"],
                    "b" => &["border-bottom-right-radius", "border-bottom-left-radius"],
                    "l" => &["border-top-left-radius", "border-bottom-left-radius"],
                    "s" => &["border-start-start-radius", "border-end-start-radius"],
                    "e" => &["border-start-end-radius", "border-end-end-radius"],
                    "tl" => &["border-top-left-radius"],
                    "tr" => &["border-top-right-radius"],
                    "br" => &["border-bottom-right-radius"],
                    "bl" => &["border-bottom-left-radius"],
                    "ss" => &["border-start-start-radius"],
                    "se" => &["border-start-end-radius"],
                    "ee" => &["border-end-end-radius"],
                    "es" => &["border-end-start-radius"],
                    _ => return None,
                };
                let resolved = match value {
                    None => self
                        .theme("--radius", "DEFAULT")
                        .unwrap_or_else(|| "0.25rem".into()),
                    Some("none") => "0".into(),
                    Some("full") => "calc(infinity * 1px)".into(),
                    Some(value) => match arbitrary(value) {
                        Some(raw) => decode(hint(raw).1),
                        None => self.theme("--radius", value)?,
                    },
                };
                if negative || modifier.is_some() {
                    return None;
                }
                Body::new(
                    &corners
                        .iter()
                        .map(|corner| (*corner, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "border" | "border-x" | "border-y" | "border-t" | "border-r" | "border-b"
            | "border-l" | "border-s" | "border-e" => {
                let suffix = match &root[6..] {
                    "" => "",
                    "-x" => "-inline",
                    "-y" => "-block",
                    "-t" => "-top",
                    "-r" => "-right",
                    "-b" => "-bottom",
                    "-l" => "-left",
                    "-s" => "-inline-start",
                    _ => "-inline-end",
                };
                let width = match value {
                    None => Some("1px".into()),
                    Some(value) => match arbitrary(value) {
                        Some(raw) => length(raw),
                        None => value
                            .parse::<u16>()
                            .ok()
                            .map(|number| format!("{number}px")),
                    },
                };
                if let Some(width) = width.filter(|_| modifier.is_none() && !negative) {
                    return Body::new(&[
                        (
                            format!("border{suffix}-style").as_str(),
                            "var(--tw-border-style)".into(),
                        ),
                        (format!("border{suffix}-width").as_str(), width),
                    ]);
                }
                Body::new(&[(
                    format!("border{suffix}-color").as_str(),
                    self.color(value?, modifier)?,
                )])
            }
            "bg" => {
                let value = value?;
                if let Some(raw) = arbitrary(value) {
                    let (kind, raw) = hint(raw);
                    let raw = decode(raw);
                    if kind == Some("image")
                        || kind == Some("url")
                        || raw.starts_with("url(")
                        || raw.contains("gradient(")
                    {
                        return Body::new(&[("background-image", raw)]);
                    }
                    if matches!(kind, Some("length" | "size" | "bg-size")) {
                        return Body::new(&[("background-size", raw)]);
                    }
                    if kind == Some("position") || kind.is_none() && is_length(&raw) {
                        return Body::new(&[("background-position", raw)]);
                    }
                }
                Body::new(&[("background-color", self.color(value, modifier)?)])
            }
            "text" => {
                let value = value?;
                if let Some(raw) = arbitrary(value)
                    && let Some(size) = length(raw)
                {
                    return Body::new(&[("font-size", size)]);
                }
                if let Some(size) = self.theme("--text", value) {
                    let mut body = vec![("font-size", size)];
                    let leading = modifier.and_then(|modifier| {
                        arbitrary(modifier)
                            .map(decode)
                            .or_else(|| self.theme("--leading", modifier))
                            .or_else(|| self.spacing(modifier))
                    });
                    if let Some(leading) = leading {
                        body.push(("line-height", leading));
                    } else if modifier.is_some() {
                        return None;
                    } else if let Some(height) =
                        self.theme("--text", &format!("{value}--line-height"))
                    {
                        body.push(("line-height", format!("var(--tw-leading, {height})")));
                    }
                    return Body::new(&body);
                }
                Body::new(&[("color", self.color(value, modifier)?)])
            }
            "font" => {
                let value = value?;
                if let Some(raw) = arbitrary(value) {
                    let raw = decode(hint(raw).1);
                    if raw.parse::<u16>().is_ok() || raw.starts_with("var(") {
                        return Body::new(&[
                            ("--tw-font-weight", raw.clone()),
                            ("font-weight", raw),
                        ]);
                    }
                    return Body::new(&[("font-family", raw)]);
                }
                if let Some(family) = self.theme("--font", value) {
                    return Body::new(&[("font-family", family)]);
                }
                let weight = self.theme("--font-weight", value)?;
                Body::new(&[
                    ("--tw-font-weight", weight.clone()),
                    ("font-weight", weight),
                ])
            }
            "leading" => {
                let value = value?;
                let resolved = match value {
                    "none" => "1".into(),
                    _ => arbitrary(value)
                        .map(decode)
                        .or_else(|| self.theme("--leading", value))
                        .or_else(|| self.spacing(value))?,
                };
                Body::new(&[
                    ("--tw-leading", resolved.clone()),
                    ("line-height", resolved),
                ])
            }
            "tracking" => {
                let value = value?;
                let resolved = negate(
                    arbitrary(value)
                        .map(decode)
                        .or_else(|| self.theme("--tracking", value))?,
                );
                Body::new(&[
                    ("--tw-tracking", resolved.clone()),
                    ("letter-spacing", resolved),
                ])
            }
            "opacity" => {
                let value = value?;
                let resolved = arbitrary(value).map(decode).or_else(|| {
                    value
                        .parse::<f32>()
                        .ok()
                        .filter(|n| (0. ..=100.).contains(n))
                        .map(|_| format!("{value}%"))
                })?;
                Body::new(&[("opacity", resolved)])
            }
            "shadow" => {
                let shadow = match value {
                    None => {
                        Some("0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)".into())
                    }
                    Some("none") => Some("0 0 #0000".into()),
                    Some(value) => match arbitrary(value) {
                        Some(raw)
                            if !is_color(&decode(raw))
                                && hint(raw).0.is_none_or(|kind| kind != "color") =>
                        {
                            Some(decode(raw))
                        }
                        Some(_) => None,
                        None => self.value("--shadow", value),
                    },
                };
                if let Some(shadow) = shadow.filter(|_| modifier.is_none()) {
                    let shadow = if value == Some("none") {
                        shadow
                    } else {
                        shadow_color(&shadow, "--tw-shadow-color")
                    };
                    return Body::new(&[("--tw-shadow", shadow), ("box-shadow", SHADOW.into())]);
                }
                Body::new(&[("--tw-shadow-color", self.color(value?, modifier)?)])
            }
            "ring" => {
                let width = match value {
                    None => Some("1px".into()),
                    Some(value) => match arbitrary(value) {
                        Some(raw) => length(raw),
                        None => value
                            .parse::<u16>()
                            .ok()
                            .map(|number| format!("{number}px")),
                    },
                };
                if let Some(width) = width.filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        (
                            "--tw-ring-shadow",
                            format!(
                                "var(--tw-ring-inset,) 0 0 0 calc({width} + var(--tw-ring-offset-width)) var(--tw-ring-color, currentcolor)"
                            ),
                        ),
                        ("box-shadow", SHADOW.into()),
                    ]);
                }
                Body::new(&[("--tw-ring-color", self.color(value?, modifier)?)])
            }
            "ring-offset" => {
                let value = value?;
                let width = match arbitrary(value) {
                    Some(raw) => length(raw),
                    None => value
                        .parse::<u16>()
                        .ok()
                        .map(|number| format!("{number}px")),
                };
                if let Some(width) = width {
                    return Body::new(&[
                        ("--tw-ring-offset-width", width),
                        ("--tw-ring-offset-shadow", "var(--tw-ring-inset,) 0 0 0 var(--tw-ring-offset-width) var(--tw-ring-offset-color)".into()),
                    ]);
                }
                Body::new(&[("--tw-ring-offset-color", self.color(value, modifier)?)])
            }
            "outline" => {
                let width = match value {
                    None => Some("1px".into()),
                    Some(value) => match arbitrary(value) {
                        Some(raw) => length(raw),
                        None => value
                            .parse::<u16>()
                            .ok()
                            .map(|number| format!("{number}px")),
                    },
                };
                if let Some(width) = width.filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        ("outline-style", "var(--tw-outline-style)".into()),
                        ("outline-width", width),
                    ]);
                }
                Body::new(&[("outline-color", self.color(value?, modifier)?)])
            }
            "outline-offset" | "underline-offset" => {
                let value = value?;
                let resolved = arbitrary(value).map(decode).or_else(|| {
                    value
                        .parse::<u16>()
                        .ok()
                        .map(|number| format!("{number}px"))
                })?;
                Body::new(&[(
                    if root == "outline-offset" {
                        "outline-offset"
                    } else {
                        "text-underline-offset"
                    },
                    negate(resolved),
                )])
            }
            "decoration" => {
                let value = value?;
                let thickness = match arbitrary(value) {
                    Some(raw) => length(raw),
                    None => value
                        .parse::<u16>()
                        .ok()
                        .map(|number| format!("{number}px")),
                };
                if let Some(thickness) = thickness {
                    return Body::new(&[("text-decoration-thickness", thickness)]);
                }
                Body::new(&[("text-decoration-color", self.color(value, modifier)?)])
            }
            "stroke" => {
                let value = value?;
                let width = match arbitrary(value) {
                    Some(raw) => length(raw),
                    None => value.parse::<u16>().ok().map(|number| number.to_string()),
                };
                if let Some(width) = width {
                    return Body::new(&[("stroke-width", width)]);
                }
                Body::new(&[("stroke", self.color(value, modifier)?)])
            }
            "fill" | "accent" | "caret" => {
                let property = match root {
                    "fill" => "fill",
                    "accent" => "accent-color",
                    _ => "caret-color",
                };
                Body::new(&[(property, self.color(value?, modifier)?)])
            }
            "duration" | "delay" => {
                let value = value?;
                let resolved = arbitrary(value).map(decode).or_else(|| {
                    value
                        .parse::<u32>()
                        .ok()
                        .map(|number| format!("{number}ms"))
                })?;
                if root == "delay" {
                    return Body::new(&[("transition-delay", resolved)]);
                }
                Body::new(&[
                    ("--tw-duration", resolved.clone()),
                    ("transition-duration", resolved),
                ])
            }
            "ease" => {
                let value = value?;
                let resolved = match value {
                    "linear" | "initial" => value.into(),
                    _ => arbitrary(value)
                        .map(decode)
                        .or_else(|| self.theme("--ease", value))?,
                };
                Body::new(&[
                    ("--tw-ease", resolved.clone()),
                    ("transition-timing-function", resolved),
                ])
            }
            "transition" => {
                let properties = match value {
                    None => format!(
                        "{COLORS}, opacity, box-shadow, transform, translate, scale, rotate, filter, -webkit-backdrop-filter, backdrop-filter, display, content-visibility, overlay, pointer-events"
                    ),
                    Some("all") => "all".into(),
                    Some("colors") => COLORS.into(),
                    Some("opacity") => "opacity".into(),
                    Some("shadow") => "box-shadow".into(),
                    Some("transform") => "transform, translate, scale, rotate".into(),
                    Some("none") => return Body::new(&[("transition-property", "none".into())]),
                    Some(value) => decode(arbitrary(value)?),
                };
                let mut body = Body::parse(TRANSITION);
                body.declarations
                    .insert(0, ("transition-property".into(), properties));
                Some(body)
            }
            "blur" | "backdrop-blur" => {
                let resolved = match value {
                    None => "blur(8px)".into(),
                    Some("none") => " ".into(),
                    Some(value) => match arbitrary(value) {
                        Some(raw) => format!("blur({})", decode(raw)),
                        None => format!("blur({})", self.theme("--blur", value)?),
                    },
                };
                if root == "blur" {
                    return Body::new(&[("--tw-blur", resolved), ("filter", FILTER.into())]);
                }
                Body::new(&[
                    ("--tw-backdrop-blur", resolved),
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ])
            }
            "animate" => {
                let value = value?;
                let resolved = match value {
                    "none" => "none".into(),
                    _ => arbitrary(value)
                        .map(decode)
                        .or_else(|| self.theme("--animate", value))?,
                };
                Body::new(&[("animation", resolved)])
            }
            "content" => {
                let value = value?;
                let resolved = match value {
                    "none" => "none".into(),
                    _ => decode(arbitrary(value)?),
                };
                Body::new(&[
                    ("--tw-content", resolved),
                    ("content", "var(--tw-content)".into()),
                ])
            }
            "line-clamp" => {
                let value = value?;
                if value == "none" {
                    return Body::new(&[
                        ("overflow", "visible".into()),
                        ("display", "block".into()),
                        ("-webkit-box-orient", "horizontal".into()),
                        ("-webkit-line-clamp", "unset".into()),
                    ]);
                }
                let lines = arbitrary(value)
                    .map(decode)
                    .or_else(|| value.parse::<u16>().ok().map(|n| n.to_string()))?;
                Body::new(&[
                    ("overflow", "hidden".into()),
                    ("display", "-webkit-box".into()),
                    ("-webkit-box-orient", "vertical".into()),
                    ("-webkit-line-clamp", lines),
                ])
            }
            "aspect" => {
                let value = value?;
                let resolved = match value {
                    "auto" => "auto".into(),
                    "square" => "1 / 1".into(),
                    "video" => self.theme("--aspect", "video")?,
                    _ => match (arbitrary(value), modifier) {
                        (Some(raw), _) => decode(raw),
                        (None, Some(modifier)) => format!("{value} / {modifier}"),
                        _ => return None,
                    },
                };
                Body::new(&[("aspect-ratio", resolved)])
            }
            "cursor" => {
                let value = value?;
                let resolved = arbitrary(value).map(decode).or_else(|| {
                    value
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
                        .then(|| value.to_owned())
                })?;
                Body::new(&[("cursor", resolved)])
            }
            "origin" => {
                let value = value?;
                let resolved = arbitrary(value)
                    .map(decode)
                    .unwrap_or_else(|| value.replace('-', " "));
                Body::new(&[("transform-origin", resolved)])
            }
            "will-change" => Body::new(&[("will-change", decode(arbitrary(value?)?))]),
            "columns" => {
                let value = value?;
                let resolved = arbitrary(value)
                    .map(decode)
                    .or_else(|| self.theme("--container", value))
                    .or_else(|| value.parse::<u16>().ok().map(|n| n.to_string()))?;
                Body::new(&[("columns", resolved)])
            }
            _ => None,
        }
    }
}

fn template(source: &str) -> Variant {
    if source.starts_with('@') {
        return Variant {
            rules: vec![source.to_owned()],
            ..Variant::default()
        };
    }
    Variant {
        selectors: split(source, ',')
            .into_iter()
            .map(|part| part.trim().to_owned())
            .collect(),
        ..Variant::default()
    }
}

/// Splits on `separator` outside brackets and parentheses.
fn split(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            _ if character == separator && depth == 0 => {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn modifier(value: &str) -> (&str, Option<&str>) {
    let parts = split(value, '/');
    match parts.as_slice() {
        [value, modifier] if !value.is_empty() && !modifier.is_empty() => (value, Some(modifier)),
        _ => (value, None),
    }
}

fn arbitrary(value: &str) -> Option<&str> {
    value
        .strip_prefix('[')?
        .strip_suffix(']')
        .filter(|raw| !raw.is_empty())
}

fn hint(raw: &str) -> (Option<&str>, &str) {
    match raw.split_once(':') {
        Some((kind, value))
            if matches!(
                kind,
                "color"
                    | "length"
                    | "percentage"
                    | "number"
                    | "integer"
                    | "url"
                    | "image"
                    | "position"
                    | "size"
                    | "bg-size"
                    | "line-width"
                    | "family-name"
                    | "absolute-size"
                    | "relative-size"
                    | "angle"
                    | "ratio"
                    | "vector"
                    | "any"
            ) =>
        {
            (Some(kind), value)
        }
        _ => (None, raw),
    }
}

/// Decodes an arbitrary value: `_` is a space, and math operators get the spaces CSS needs.
fn decode(raw: &str) -> String {
    let mut value = String::with_capacity(raw.len());
    let mut escaped = false;
    for character in raw.chars() {
        if escaped {
            value.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            value.push(if character == '_' { ' ' } else { character });
        }
    }
    if !value.contains('(') {
        return value;
    }
    let mut result = String::with_capacity(value.len() + 8);
    let mut math = Vec::new();
    let mut start = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        match character {
            '(' => {
                let name = &value[start..index];
                math.push(
                    matches!(
                        name,
                        "calc" | "min" | "max" | "clamp" | "round" | "mod" | "rem" | "abs"
                    ) || name.is_empty() && math.last() == Some(&true),
                );
                result.push(character);
            }
            ')' => {
                math.pop();
                result.push(character);
            }
            '+' | '-' | '*' | '/' if math.last() == Some(&true) => {
                let previous = result.trim_end().chars().last();
                if previous
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, ')' | '%' | '.'))
                {
                    result.truncate(result.trim_end().len());
                    result.push(' ');
                    result.push(character);
                    result.push(' ');
                    start = next;
                    continue;
                }
                result.push(character);
            }
            ',' if math.last() == Some(&true) => result.push_str(", "),
            ' ' if result.ends_with(' ') => {}
            _ => result.push(character),
        }
        if !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_')) {
            start = next;
        }
    }
    result
}

fn is_color(value: &str) -> bool {
    value.starts_with('#')
        || [
            "rgb(",
            "rgba(",
            "hsl(",
            "hsla(",
            "hwb(",
            "lab(",
            "lch(",
            "oklab(",
            "oklch(",
            "color(",
            "color-mix(",
            "light-dark(",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
        || value.eq_ignore_ascii_case("currentcolor")
        || value == "transparent"
        || super::color(value).is_some()
}

fn is_type(kind: &str, value: &str) -> bool {
    match kind {
        "number" => value.parse::<f64>().is_ok_and(|number| number >= 0.),
        "integer" => value.parse::<u64>().is_ok(),
        "percentage" => value
            .strip_suffix('%')
            .is_some_and(|number| number.parse::<f64>().is_ok()),
        "length" => is_length(value) && !value.ends_with('%'),
        "duration" => {
            value.ends_with("ms")
                || value.ends_with('s') && value[..value.len() - 1].parse::<f64>().is_ok()
        }
        "color" => is_color(value),
        _ => false,
    }
}

fn is_length(value: &str) -> bool {
    value.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
        || ["calc(", "min(", "max(", "clamp("]
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

/// Wraps the color of each shadow layer in `var(name, color)`, so shadow color utilities apply.
fn shadow_color(shadow: &str, name: &str) -> String {
    split(shadow, ',')
        .into_iter()
        .map(|layer| {
            let parts = split(layer.trim(), ' ');
            if parts.len() < 3 {
                return layer.trim().to_owned();
            }
            parts
                .iter()
                .map(|part| {
                    if *part != "inset" && !is_length(part) && !part.is_empty() {
                        format!("var({name}, {part})")
                    } else {
                        (*part).to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tailwind's attribute selector quoting: `state=open` becomes `state="open"`.
fn quote(value: &str) -> String {
    let Some((name, rest)) = value.split_once('=') else {
        return value.to_owned();
    };
    let rest = rest.trim();
    if rest.starts_with(['\'', '"']) {
        return value.to_owned();
    }
    format!("{name}=\"{rest}\"")
}

fn escape(class: &str) -> String {
    let mut selector = String::with_capacity(class.len() + 8);
    selector.push('.');
    for (index, character) in class.chars().enumerate() {
        if index == 0 && character.is_ascii_digit() {
            let _ = write!(selector, "\\{:x} ", character as u32);
        } else if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_')
            || !character.is_ascii()
        {
            selector.push(character);
        } else {
            selector.push('\\');
            selector.push(character);
        }
    }
    selector
}

fn join(declarations: &[(String, String)], important: bool) -> String {
    let mut source = String::new();
    for (key, value) in declarations {
        let _ = write!(
            source,
            "{key}:{value}{};",
            if important { " !important" } else { "" }
        );
    }
    source
}

/// Identifiers that follow `prefix` in `text`.
fn names<'a>(text: &'a str, prefix: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(prefix).filter_map(move |(index, _)| {
        let rest = &text[index + prefix.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    })
}

#[cfg(test)]
mod tests;
