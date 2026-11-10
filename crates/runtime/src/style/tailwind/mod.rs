use super::{Declaration, Declarations, Prop, Rule, Style, Stylesheet};
use crate::dom::Document;
use rustc_hash::FxHashMap;
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

const INLINE: &[&str] = &[
    "--blur",
    "--shadow",
    "--shadow-inner",
    "--drop-shadow",
    "--radius",
    "--max-width-prose",
];

const KEPT: &[(&str, &[&str])] = &[
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

const PROPERTIES: &[(&str, &str)] = &[
    ("--tw-border-spacing-x", "0"),
    ("--tw-border-spacing-y", "0"),
    ("--tw-translate-x", "0"),
    ("--tw-translate-y", "0"),
    ("--tw-translate-z", "0"),
    ("--tw-scale-x", "1"),
    ("--tw-scale-y", "1"),
    ("--tw-scale-z", "1"),
    ("--tw-rotate-x", "initial"),
    ("--tw-rotate-y", "initial"),
    ("--tw-rotate-z", "initial"),
    ("--tw-skew-x", "initial"),
    ("--tw-skew-y", "initial"),
    ("--tw-pan-x", "initial"),
    ("--tw-pan-y", "initial"),
    ("--tw-pinch-zoom", "initial"),
    ("--tw-scroll-snap-strictness", "proximity"),
    ("--tw-scrollbar-thumb", "#0000"),
    ("--tw-scrollbar-track", "#0000"),
    ("--tw-space-y-reverse", "0"),
    ("--tw-space-x-reverse", "0"),
    ("--tw-divide-x-reverse", "0"),
    ("--tw-border-style", "solid"),
    ("--tw-divide-y-reverse", "0"),
    ("--tw-gradient-position", "initial"),
    ("--tw-gradient-from", "#0000"),
    ("--tw-gradient-via", "#0000"),
    ("--tw-gradient-to", "#0000"),
    ("--tw-gradient-stops", "initial"),
    ("--tw-gradient-via-stops", "initial"),
    ("--tw-gradient-from-position", "0%"),
    ("--tw-gradient-via-position", "50%"),
    ("--tw-gradient-to-position", "100%"),
    ("--tw-mask-linear", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-radial", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-conic", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-left", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-right", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-bottom", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-top", "linear-gradient(#fff, #fff)"),
    ("--tw-mask-top-from-position", "0%"),
    ("--tw-mask-top-to-position", "100%"),
    ("--tw-mask-top-from-color", "black"),
    ("--tw-mask-top-to-color", "transparent"),
    ("--tw-mask-bottom-from-position", "0%"),
    ("--tw-mask-bottom-to-position", "100%"),
    ("--tw-mask-bottom-from-color", "black"),
    ("--tw-mask-bottom-to-color", "transparent"),
    ("--tw-mask-right-from-position", "0%"),
    ("--tw-mask-right-to-position", "100%"),
    ("--tw-mask-right-from-color", "black"),
    ("--tw-mask-right-to-color", "transparent"),
    ("--tw-mask-left-from-position", "0%"),
    ("--tw-mask-left-to-position", "100%"),
    ("--tw-mask-left-from-color", "black"),
    ("--tw-mask-left-to-color", "transparent"),
    ("--tw-mask-linear-position", "0deg"),
    ("--tw-mask-linear-from-position", "0%"),
    ("--tw-mask-linear-to-position", "100%"),
    ("--tw-mask-linear-from-color", "black"),
    ("--tw-mask-linear-to-color", "transparent"),
    ("--tw-mask-radial-from-position", "0%"),
    ("--tw-mask-radial-to-position", "100%"),
    ("--tw-mask-radial-from-color", "black"),
    ("--tw-mask-radial-to-color", "transparent"),
    ("--tw-mask-radial-shape", "ellipse"),
    ("--tw-mask-radial-size", "farthest-corner"),
    ("--tw-mask-radial-position", "center"),
    ("--tw-mask-conic-position", "0deg"),
    ("--tw-mask-conic-from-position", "0%"),
    ("--tw-mask-conic-to-position", "100%"),
    ("--tw-mask-conic-from-color", "black"),
    ("--tw-mask-conic-to-color", "transparent"),
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
    ("--tw-shadow-alpha", "100%"),
    ("--tw-inset-shadow", "0 0 #0000"),
    ("--tw-inset-shadow-color", "initial"),
    ("--tw-inset-shadow-alpha", "100%"),
    ("--tw-ring-color", "initial"),
    ("--tw-ring-shadow", "0 0 #0000"),
    ("--tw-inset-ring-color", "initial"),
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
    ("--tw-opacity", "initial"),
    ("--tw-saturate", "initial"),
    ("--tw-sepia", "initial"),
    ("--tw-drop-shadow", "initial"),
    ("--tw-drop-shadow-color", "initial"),
    ("--tw-drop-shadow-alpha", "100%"),
    ("--tw-drop-shadow-size", "initial"),
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
    ("--tw-text-shadow-color", "initial"),
    ("--tw-text-shadow-alpha", "100%"),
    ("--tw-content", "\"\""),
];

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
    "inset-block-start",
    "inset-block-end",
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
    "--tw-container-component",
    "margin",
    "margin-inline",
    "margin-block",
    "margin-inline-start",
    "margin-inline-end",
    "margin-block-start",
    "margin-block-end",
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
    "--tw-rotate-x",
    "--tw-rotate-y",
    "--tw-rotate-z",
    "--tw-skew-x",
    "--tw-skew-y",
    "transform",
    "zoom",
    "animation",
    "cursor",
    "touch-action",
    "--tw-pan-x",
    "--tw-pan-y",
    "--tw-pinch-zoom",
    "resize",
    "scroll-snap-type",
    "--tw-scroll-snap-strictness",
    "scroll-snap-align",
    "scroll-snap-stop",
    "scroll-margin",
    "scroll-margin-inline",
    "scroll-margin-block",
    "scroll-margin-inline-start",
    "scroll-margin-inline-end",
    "scroll-margin-block-start",
    "scroll-margin-block-end",
    "scroll-margin-top",
    "scroll-margin-right",
    "scroll-margin-bottom",
    "scroll-margin-left",
    "scroll-padding",
    "scroll-padding-inline",
    "scroll-padding-block",
    "scroll-padding-inline-start",
    "scroll-padding-inline-end",
    "scroll-padding-block-start",
    "scroll-padding-block-end",
    "scroll-padding-top",
    "scroll-padding-right",
    "scroll-padding-bottom",
    "scroll-padding-left",
    "scrollbar-width",
    "scrollbar-color",
    "scrollbar-gutter",
    "list-style-position",
    "list-style-type",
    "list-style-image",
    "appearance",
    "columns",
    "break-before",
    "break-inside",
    "break-after",
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
    "divide-x-width",
    "divide-y-width",
    "--tw-divide-y-reverse",
    "divide-style",
    "divide-color",
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
    "border-start-radius",
    "border-end-radius",
    "border-top-radius",
    "border-right-radius",
    "border-bottom-radius",
    "border-left-radius",
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
    "border-block-start-width",
    "border-block-end-width",
    "border-top-width",
    "border-right-width",
    "border-bottom-width",
    "border-left-width",
    "border-style",
    "border-inline-style",
    "border-block-style",
    "border-inline-start-style",
    "border-inline-end-style",
    "border-block-start-style",
    "border-block-end-style",
    "border-top-style",
    "border-right-style",
    "border-bottom-style",
    "border-left-style",
    "border-color",
    "border-inline-color",
    "border-block-color",
    "border-inline-start-color",
    "border-inline-end-color",
    "border-block-start-color",
    "border-block-end-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "background-color",
    "-macos-glass-tint-color",
    "background-image",
    "--tw-gradient-position",
    "--tw-gradient-stops",
    "--tw-gradient-via-stops",
    "--tw-gradient-from",
    "--tw-gradient-from-position",
    "--tw-gradient-via",
    "--tw-gradient-via-position",
    "--tw-gradient-to",
    "--tw-gradient-to-position",
    "mask-image",
    "--tw-mask-top",
    "--tw-mask-top-from-color",
    "--tw-mask-top-from-position",
    "--tw-mask-top-to-color",
    "--tw-mask-top-to-position",
    "--tw-mask-right",
    "--tw-mask-right-from-color",
    "--tw-mask-right-from-position",
    "--tw-mask-right-to-color",
    "--tw-mask-right-to-position",
    "--tw-mask-bottom",
    "--tw-mask-bottom-from-color",
    "--tw-mask-bottom-from-position",
    "--tw-mask-bottom-to-color",
    "--tw-mask-bottom-to-position",
    "--tw-mask-left",
    "--tw-mask-left-from-color",
    "--tw-mask-left-from-position",
    "--tw-mask-left-to-color",
    "--tw-mask-left-to-position",
    "--tw-mask-linear",
    "--tw-mask-linear-position",
    "--tw-mask-linear-from-color",
    "--tw-mask-linear-from-position",
    "--tw-mask-linear-to-color",
    "--tw-mask-linear-to-position",
    "--tw-mask-radial",
    "--tw-mask-radial-shape",
    "--tw-mask-radial-size",
    "--tw-mask-radial-position",
    "--tw-mask-radial-from-color",
    "--tw-mask-radial-from-position",
    "--tw-mask-radial-to-color",
    "--tw-mask-radial-to-position",
    "--tw-mask-conic",
    "--tw-mask-conic-position",
    "--tw-mask-conic-from-color",
    "--tw-mask-conic-from-position",
    "--tw-mask-conic-to-color",
    "--tw-mask-conic-to-position",
    "box-decoration-break",
    "background-size",
    "background-attachment",
    "background-clip",
    "background-position",
    "background-repeat",
    "background-origin",
    "mask-composite",
    "mask-mode",
    "mask-type",
    "mask-size",
    "mask-clip",
    "mask-position",
    "mask-repeat",
    "mask-origin",
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
    "padding-block-start",
    "padding-block-end",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "text-align",
    "text-indent",
    "vertical-align",
    "font-family",
    "font-feature-settings",
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
    "tab-size",
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
    "placeholder-color",
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
    "--tw-inset-shadow-color",
    "--tw-inset-ring-shadow",
    "--tw-inset-ring-color",
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

const VARIANTS: &[(&str, &str)] = &[
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

const STATIC: &[(&str, &str)] = &[
    ("absolute", "position:absolute"),
    ("accent-auto", "accent-color:auto"),
    ("align-baseline", "vertical-align:baseline"),
    ("align-bottom", "vertical-align:bottom"),
    ("align-middle", "vertical-align:middle"),
    ("align-sub", "vertical-align:sub"),
    ("align-super", "vertical-align:super"),
    ("align-text-bottom", "vertical-align:text-bottom"),
    ("align-text-top", "vertical-align:text-top"),
    ("align-top", "vertical-align:top"),
    (
        "antialiased",
        "-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale",
    ),
    ("appearance-auto", "appearance:auto"),
    ("appearance-none", "appearance:none"),
    (
        "backdrop-filter-none",
        "-webkit-backdrop-filter:none;backdrop-filter:none",
    ),
    ("backface-hidden", "backface-visibility:hidden"),
    ("backface-visible", "backface-visibility:visible"),
    ("bg-auto", "background-size:auto"),
    ("bg-blend-color", "background-blend-mode:color"),
    ("bg-blend-color-burn", "background-blend-mode:color-burn"),
    ("bg-blend-color-dodge", "background-blend-mode:color-dodge"),
    ("bg-blend-darken", "background-blend-mode:darken"),
    ("bg-blend-difference", "background-blend-mode:difference"),
    ("bg-blend-exclusion", "background-blend-mode:exclusion"),
    ("bg-blend-hard-light", "background-blend-mode:hard-light"),
    ("bg-blend-hue", "background-blend-mode:hue"),
    ("bg-blend-lighten", "background-blend-mode:lighten"),
    ("bg-blend-luminosity", "background-blend-mode:luminosity"),
    ("bg-blend-multiply", "background-blend-mode:multiply"),
    ("bg-blend-normal", "background-blend-mode:normal"),
    ("bg-blend-overlay", "background-blend-mode:overlay"),
    ("bg-blend-saturation", "background-blend-mode:saturation"),
    ("bg-blend-screen", "background-blend-mode:screen"),
    ("bg-blend-soft-light", "background-blend-mode:soft-light"),
    ("bg-bottom", "background-position:bottom"),
    ("bg-bottom-left", "background-position:left bottom"),
    ("bg-bottom-right", "background-position:right bottom"),
    ("bg-center", "background-position:center"),
    ("bg-clip-border", "background-clip:border-box"),
    ("bg-clip-content", "background-clip:content-box"),
    ("bg-clip-padding", "background-clip:padding-box"),
    ("bg-clip-text", "background-clip:text"),
    ("bg-contain", "background-size:contain"),
    ("bg-cover", "background-size:cover"),
    ("bg-fixed", "background-attachment:fixed"),
    ("bg-left", "background-position:left"),
    ("bg-left-bottom", "background-position:left bottom"),
    ("bg-left-top", "background-position:0 0"),
    ("bg-local", "background-attachment:local"),
    ("bg-no-repeat", "background-repeat:no-repeat"),
    ("bg-none", "background-image:none"),
    ("bg-origin-border", "background-origin:border-box"),
    ("bg-origin-content", "background-origin:content-box"),
    ("bg-origin-padding", "background-origin:padding-box"),
    ("bg-repeat", "background-repeat:repeat"),
    ("bg-repeat-round", "background-repeat:round"),
    ("bg-repeat-space", "background-repeat:space"),
    ("bg-repeat-x", "background-repeat:repeat-x"),
    ("bg-repeat-y", "background-repeat:repeat-y"),
    ("bg-right", "background-position:right"),
    ("bg-right-bottom", "background-position:right bottom"),
    ("bg-right-top", "background-position:right top"),
    ("bg-scroll", "background-attachment:scroll"),
    ("bg-top", "background-position:top"),
    ("bg-top-left", "background-position:left top"),
    ("bg-top-right", "background-position:right top"),
    ("block", "display:block"),
    ("border-collapse", "border-collapse:collapse"),
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
    ("border-separate", "border-collapse:separate"),
    ("border-solid", "--tw-border-style:solid;border-style:solid"),
    ("box-border", "box-sizing:border-box"),
    ("box-content", "box-sizing:content-box"),
    (
        "box-decoration-clone",
        "-webkit-box-decoration-break:clone;box-decoration-break:clone",
    ),
    (
        "box-decoration-slice",
        "-webkit-box-decoration-break:slice;box-decoration-break:slice",
    ),
    ("break-after-all", "break-after:all"),
    ("break-after-auto", "break-after:auto"),
    ("break-after-avoid", "break-after:avoid"),
    ("break-after-avoid-page", "break-after:avoid-page"),
    ("break-after-column", "break-after:column"),
    ("break-after-left", "break-after:left"),
    ("break-after-page", "break-after:page"),
    ("break-after-right", "break-after:right"),
    ("break-all", "word-break:break-all"),
    ("break-before-all", "break-before:all"),
    ("break-before-auto", "break-before:auto"),
    ("break-before-avoid", "break-before:avoid"),
    ("break-before-avoid-page", "break-before:avoid-page"),
    ("break-before-column", "break-before:column"),
    ("break-before-left", "break-before:left"),
    ("break-before-page", "break-before:page"),
    ("break-before-right", "break-before:right"),
    ("break-inside-auto", "break-inside:auto"),
    ("break-inside-avoid", "break-inside:avoid"),
    ("break-inside-avoid-column", "break-inside:avoid-column"),
    ("break-inside-avoid-page", "break-inside:avoid-page"),
    ("break-keep", "word-break:keep-all"),
    ("break-normal", "overflow-wrap:normal;word-break:normal"),
    ("break-words", "overflow-wrap:break-word"),
    ("capitalize", "text-transform:capitalize"),
    ("caption-bottom", "caption-side:bottom"),
    ("caption-top", "caption-side:top"),
    ("clear-both", "clear:both"),
    ("clear-end", "clear:inline-end"),
    ("clear-left", "clear:left"),
    ("clear-none", "clear:none"),
    ("clear-right", "clear:right"),
    ("clear-start", "clear:inline-start"),
    ("collapse", "visibility:collapse"),
    ("contain-content", "contain:content"),
    (
        "contain-inline-size",
        "--tw-contain-size:inline-size;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-layout",
        "--tw-contain-layout:layout;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    ("contain-none", "contain:none"),
    (
        "contain-paint",
        "--tw-contain-paint:paint;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    (
        "contain-size",
        "--tw-contain-size:size;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    ("contain-strict", "contain:strict"),
    (
        "contain-style",
        "--tw-contain-style:style;contain:var(--tw-contain-size,) var(--tw-contain-layout,) var(--tw-contain-paint,) var(--tw-contain-style,)",
    ),
    ("content-around", "align-content:space-around"),
    ("content-baseline", "align-content:baseline"),
    ("content-between", "align-content:space-between"),
    ("content-center", "align-content:center"),
    ("content-center-safe", "align-content:safe center"),
    ("content-end", "align-content:flex-end"),
    ("content-end-safe", "align-content:safe flex-end"),
    ("content-evenly", "align-content:space-evenly"),
    ("content-normal", "align-content:normal"),
    ("content-start", "align-content:flex-start"),
    ("content-stretch", "align-content:stretch"),
    ("contents", "display:contents"),
    ("cursor-alias", "cursor:alias"),
    ("cursor-all-scroll", "cursor:all-scroll"),
    ("cursor-auto", "cursor:auto"),
    ("cursor-cell", "cursor:cell"),
    ("cursor-col-resize", "cursor:col-resize"),
    ("cursor-context-menu", "cursor:context-menu"),
    ("cursor-copy", "cursor:copy"),
    ("cursor-crosshair", "cursor:crosshair"),
    ("cursor-default", "cursor:default"),
    ("cursor-e-resize", "cursor:e-resize"),
    ("cursor-ew-resize", "cursor:ew-resize"),
    ("cursor-grab", "cursor:grab"),
    ("cursor-grabbing", "cursor:grabbing"),
    ("cursor-help", "cursor:help"),
    ("cursor-move", "cursor:move"),
    ("cursor-n-resize", "cursor:n-resize"),
    ("cursor-ne-resize", "cursor:ne-resize"),
    ("cursor-nesw-resize", "cursor:nesw-resize"),
    ("cursor-no-drop", "cursor:no-drop"),
    ("cursor-none", "cursor:none"),
    ("cursor-not-allowed", "cursor:not-allowed"),
    ("cursor-ns-resize", "cursor:ns-resize"),
    ("cursor-nw-resize", "cursor:nw-resize"),
    ("cursor-nwse-resize", "cursor:nwse-resize"),
    ("cursor-pointer", "cursor:pointer"),
    ("cursor-progress", "cursor:progress"),
    ("cursor-row-resize", "cursor:row-resize"),
    ("cursor-s-resize", "cursor:s-resize"),
    ("cursor-se-resize", "cursor:se-resize"),
    ("cursor-sw-resize", "cursor:sw-resize"),
    ("cursor-text", "cursor:text"),
    ("cursor-vertical-text", "cursor:vertical-text"),
    ("cursor-w-resize", "cursor:w-resize"),
    ("cursor-wait", "cursor:wait"),
    ("cursor-zoom-in", "cursor:zoom-in"),
    ("cursor-zoom-out", "cursor:zoom-out"),
    ("decoration-auto", "text-decoration-thickness:auto"),
    ("decoration-dashed", "text-decoration-style:dashed"),
    ("decoration-dotted", "text-decoration-style:dotted"),
    ("decoration-double", "text-decoration-style:double"),
    (
        "decoration-from-font",
        "text-decoration-thickness:from-font",
    ),
    ("decoration-solid", "text-decoration-style:solid"),
    ("decoration-wavy", "text-decoration-style:wavy"),
    (
        "diagonal-fractions",
        "--tw-numeric-fraction:diagonal-fractions;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("field-sizing-content", "field-sizing:content"),
    ("field-sizing-fixed", "field-sizing:fixed"),
    ("fill-none", "fill:none"),
    ("filter-none", "filter:none"),
    ("fixed", "position:fixed"),
    ("flex", "display:flex"),
    ("flex-auto", "flex:auto"),
    ("flex-col", "flex-direction:column"),
    ("flex-col-reverse", "flex-direction:column-reverse"),
    ("flex-initial", "flex:0 auto"),
    ("flex-none", "flex:none"),
    ("flex-nowrap", "flex-wrap:nowrap"),
    ("flex-row", "flex-direction:row"),
    ("flex-row-reverse", "flex-direction:row-reverse"),
    ("flex-wrap", "flex-wrap:wrap"),
    ("flex-wrap-reverse", "flex-wrap:wrap-reverse"),
    ("float-end", "float:inline-end"),
    ("float-left", "float:left"),
    ("float-none", "float:none"),
    ("float-right", "float:right"),
    ("float-start", "float:inline-start"),
    ("flow-root", "display:flow-root"),
    ("font-stretch-condensed", "font-stretch:condensed"),
    ("font-stretch-expanded", "font-stretch:expanded"),
    (
        "font-stretch-extra-condensed",
        "font-stretch:extra-condensed",
    ),
    ("font-stretch-extra-expanded", "font-stretch:extra-expanded"),
    ("font-stretch-normal", "font-stretch:normal"),
    ("font-stretch-semi-condensed", "font-stretch:semi-condensed"),
    ("font-stretch-semi-expanded", "font-stretch:semi-expanded"),
    (
        "font-stretch-ultra-condensed",
        "font-stretch:ultra-condensed",
    ),
    ("font-stretch-ultra-expanded", "font-stretch:ultra-expanded"),
    ("forced-color-adjust-auto", "forced-color-adjust:auto"),
    ("forced-color-adjust-none", "forced-color-adjust:none"),
    ("grid", "display:grid"),
    ("grid-flow-col", "grid-auto-flow:column"),
    ("grid-flow-col-dense", "grid-auto-flow:column dense"),
    ("grid-flow-dense", "grid-auto-flow:dense"),
    ("grid-flow-row", "grid-auto-flow:row"),
    ("grid-flow-row-dense", "grid-auto-flow:row dense"),
    ("hidden", "display:none"),
    ("hyphens-auto", "-webkit-hyphens:auto;hyphens:auto"),
    ("hyphens-manual", "-webkit-hyphens:manual;hyphens:manual"),
    ("hyphens-none", "-webkit-hyphens:none;hyphens:none"),
    ("inline", "display:inline"),
    ("inline-block", "display:inline-block"),
    ("inline-flex", "display:inline-flex"),
    ("inline-grid", "display:inline-grid"),
    ("inline-table", "display:inline-table"),
    ("invisible", "visibility:hidden"),
    ("isolate", "isolation:isolate"),
    ("isolation-auto", "isolation:auto"),
    ("italic", "font-style:italic"),
    ("items-baseline", "align-items:baseline"),
    ("items-baseline-last", "align-items:last baseline"),
    ("items-center", "align-items:center"),
    ("items-center-safe", "align-items:safe center"),
    ("items-end", "align-items:flex-end"),
    ("items-end-safe", "align-items:safe flex-end"),
    ("items-start", "align-items:flex-start"),
    ("items-stretch", "align-items:stretch"),
    ("justify-around", "justify-content:space-around"),
    ("justify-baseline", "justify-content:baseline"),
    ("justify-between", "justify-content:space-between"),
    ("justify-center", "justify-content:center"),
    ("justify-center-safe", "justify-content:safe center"),
    ("justify-end", "justify-content:flex-end"),
    ("justify-end-safe", "justify-content:safe flex-end"),
    ("justify-evenly", "justify-content:space-evenly"),
    ("justify-items-center", "justify-items:center"),
    ("justify-items-center-safe", "justify-items:safe center"),
    ("justify-items-end", "justify-items:end"),
    ("justify-items-end-safe", "justify-items:safe end"),
    ("justify-items-normal", "justify-items:normal"),
    ("justify-items-start", "justify-items:start"),
    ("justify-items-stretch", "justify-items:stretch"),
    ("justify-normal", "justify-content:normal"),
    ("justify-self-auto", "justify-self:auto"),
    ("justify-self-center", "justify-self:center"),
    ("justify-self-center-safe", "justify-self:safe center"),
    ("justify-self-end", "justify-self:flex-end"),
    ("justify-self-end-safe", "justify-self:safe flex-end"),
    ("justify-self-start", "justify-self:flex-start"),
    ("justify-self-stretch", "justify-self:stretch"),
    ("justify-start", "justify-content:flex-start"),
    ("justify-stretch", "justify-content:stretch"),
    (
        "line-clamp-none",
        "overflow:visible;display:block;-webkit-box-orient:horizontal;-webkit-line-clamp:unset",
    ),
    ("line-through", "text-decoration-line:line-through"),
    (
        "lining-nums",
        "--tw-numeric-figure:lining-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("list-decimal", "list-style-type:decimal"),
    ("list-disc", "list-style-type:disc"),
    ("list-image-none", "list-style-image:none"),
    ("list-inside", "list-style-position:inside"),
    ("list-item", "display:list-item"),
    ("list-none", "list-style-type:none"),
    ("list-outside", "list-style-position:outside"),
    ("lowercase", "text-transform:lowercase"),
    ("mask-add", "mask-composite:add"),
    ("mask-alpha", "mask-mode:alpha"),
    ("mask-auto", "mask-size:auto"),
    ("mask-bottom", "mask-position:bottom"),
    ("mask-bottom-left", "mask-position:left bottom"),
    ("mask-bottom-right", "mask-position:right bottom"),
    ("mask-center", "mask-position:center"),
    ("mask-circle", "--tw-mask-radial-shape:circle"),
    ("mask-clip-border", "mask-clip:border-box"),
    ("mask-clip-content", "mask-clip:content-box"),
    ("mask-clip-fill", "mask-clip:fill-box"),
    ("mask-clip-padding", "mask-clip:padding-box"),
    ("mask-clip-stroke", "mask-clip:stroke-box"),
    ("mask-clip-view", "mask-clip:view-box"),
    ("mask-contain", "mask-size:contain"),
    ("mask-cover", "mask-size:cover"),
    ("mask-ellipse", "--tw-mask-radial-shape:ellipse"),
    ("mask-exclude", "mask-composite:exclude"),
    ("mask-intersect", "mask-composite:intersect"),
    ("mask-left", "mask-position:left"),
    ("mask-luminance", "mask-mode:luminance"),
    ("mask-match", "mask-mode:match-source"),
    ("mask-no-clip", "mask-clip:no-clip"),
    ("mask-no-repeat", "mask-repeat:no-repeat"),
    ("mask-none", "mask-image:none"),
    ("mask-origin-border", "mask-origin:border-box"),
    ("mask-origin-content", "mask-origin:content-box"),
    ("mask-origin-fill", "mask-origin:fill-box"),
    ("mask-origin-padding", "mask-origin:padding-box"),
    ("mask-origin-stroke", "mask-origin:stroke-box"),
    ("mask-origin-view", "mask-origin:view-box"),
    ("mask-radial-at-bottom", "--tw-mask-radial-position:bottom"),
    (
        "mask-radial-at-bottom-left",
        "--tw-mask-radial-position:bottom left",
    ),
    (
        "mask-radial-at-bottom-right",
        "--tw-mask-radial-position:bottom right",
    ),
    ("mask-radial-at-center", "--tw-mask-radial-position:center"),
    ("mask-radial-at-left", "--tw-mask-radial-position:left"),
    ("mask-radial-at-right", "--tw-mask-radial-position:right"),
    ("mask-radial-at-top", "--tw-mask-radial-position:top"),
    (
        "mask-radial-at-top-left",
        "--tw-mask-radial-position:top left",
    ),
    (
        "mask-radial-at-top-right",
        "--tw-mask-radial-position:top right",
    ),
    (
        "mask-radial-closest-corner",
        "--tw-mask-radial-size:closest-corner",
    ),
    (
        "mask-radial-closest-side",
        "--tw-mask-radial-size:closest-side",
    ),
    (
        "mask-radial-farthest-corner",
        "--tw-mask-radial-size:farthest-corner",
    ),
    (
        "mask-radial-farthest-side",
        "--tw-mask-radial-size:farthest-side",
    ),
    ("mask-repeat", "mask-repeat:repeat"),
    ("mask-repeat-round", "mask-repeat:round"),
    ("mask-repeat-space", "mask-repeat:space"),
    ("mask-repeat-x", "mask-repeat:repeat-x"),
    ("mask-repeat-y", "mask-repeat:repeat-y"),
    ("mask-right", "mask-position:right"),
    ("mask-subtract", "mask-composite:subtract"),
    ("mask-top", "mask-position:top"),
    ("mask-top-left", "mask-position:left top"),
    ("mask-top-right", "mask-position:right top"),
    ("mask-type-alpha", "mask-type:alpha"),
    ("mask-type-luminance", "mask-type:luminance"),
    ("mix-blend-color", "mix-blend-mode:color"),
    ("mix-blend-color-burn", "mix-blend-mode:color-burn"),
    ("mix-blend-color-dodge", "mix-blend-mode:color-dodge"),
    ("mix-blend-darken", "mix-blend-mode:darken"),
    ("mix-blend-difference", "mix-blend-mode:difference"),
    ("mix-blend-exclusion", "mix-blend-mode:exclusion"),
    ("mix-blend-hard-light", "mix-blend-mode:hard-light"),
    ("mix-blend-hue", "mix-blend-mode:hue"),
    ("mix-blend-lighten", "mix-blend-mode:lighten"),
    ("mix-blend-luminosity", "mix-blend-mode:luminosity"),
    ("mix-blend-multiply", "mix-blend-mode:multiply"),
    ("mix-blend-normal", "mix-blend-mode:normal"),
    ("mix-blend-overlay", "mix-blend-mode:overlay"),
    ("mix-blend-plus-darker", "mix-blend-mode:plus-darker"),
    ("mix-blend-plus-lighter", "mix-blend-mode:plus-lighter"),
    ("mix-blend-saturation", "mix-blend-mode:saturation"),
    ("mix-blend-screen", "mix-blend-mode:screen"),
    ("mix-blend-soft-light", "mix-blend-mode:soft-light"),
    ("no-underline", "text-decoration-line:none"),
    ("normal-case", "text-transform:none"),
    ("normal-nums", "font-variant-numeric:normal"),
    ("not-italic", "font-style:normal"),
    (
        "not-sr-only",
        "position:static;width:auto;height:auto;padding:0;margin:0;overflow:visible;clip-path:none;white-space:normal",
    ),
    ("object-bottom", "object-position:bottom"),
    ("object-bottom-left", "object-position:left bottom"),
    ("object-bottom-right", "object-position:right bottom"),
    ("object-center", "object-position:center"),
    ("object-contain", "object-fit:contain"),
    ("object-cover", "object-fit:cover"),
    ("object-fill", "object-fit:fill"),
    ("object-left", "object-position:left"),
    ("object-left-bottom", "object-position:left bottom"),
    ("object-left-top", "object-position:left top"),
    ("object-none", "object-fit:none"),
    ("object-right", "object-position:right"),
    ("object-right-bottom", "object-position:right bottom"),
    ("object-right-top", "object-position:right top"),
    ("object-scale-down", "object-fit:scale-down"),
    ("object-top", "object-position:top"),
    ("object-top-left", "object-position:left top"),
    ("object-top-right", "object-position:right top"),
    (
        "oldstyle-nums",
        "--tw-numeric-figure:oldstyle-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    (
        "ordinal",
        "--tw-ordinal:ordinal;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
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
    ("outline-none", "--tw-outline-style:none;outline-style:none"),
    (
        "outline-solid",
        "--tw-outline-style:solid;outline-style:solid",
    ),
    ("overflow-auto", "overflow:auto"),
    ("overflow-clip", "overflow:clip"),
    ("overflow-hidden", "overflow:hidden"),
    ("overflow-scroll", "overflow:scroll"),
    ("overflow-visible", "overflow:visible"),
    ("overflow-x-auto", "overflow-x:auto"),
    ("overflow-x-clip", "overflow-x:clip"),
    ("overflow-x-hidden", "overflow-x:hidden"),
    ("overflow-x-scroll", "overflow-x:scroll"),
    ("overflow-x-visible", "overflow-x:visible"),
    ("overflow-y-auto", "overflow-y:auto"),
    ("overflow-y-clip", "overflow-y:clip"),
    ("overflow-y-hidden", "overflow-y:hidden"),
    ("overflow-y-scroll", "overflow-y:scroll"),
    ("overflow-y-visible", "overflow-y:visible"),
    ("overline", "text-decoration-line:overline"),
    ("overscroll-auto", "overscroll-behavior:auto"),
    ("overscroll-contain", "overscroll-behavior:contain"),
    ("overscroll-none", "overscroll-behavior:none"),
    ("overscroll-x-auto", "overscroll-behavior-x:auto"),
    ("overscroll-x-contain", "overscroll-behavior-x:contain"),
    ("overscroll-x-none", "overscroll-behavior-x:none"),
    ("overscroll-y-auto", "overscroll-behavior-y:auto"),
    ("overscroll-y-contain", "overscroll-behavior-y:contain"),
    ("overscroll-y-none", "overscroll-behavior-y:none"),
    ("place-content-around", "place-content:space-around"),
    ("place-content-baseline", "place-content:baseline"),
    ("place-content-between", "place-content:space-between"),
    ("place-content-center", "place-content:center"),
    ("place-content-center-safe", "place-content:safe center"),
    ("place-content-end", "place-content:end"),
    ("place-content-end-safe", "place-content:safe end"),
    ("place-content-evenly", "place-content:space-evenly"),
    ("place-content-start", "place-content:start"),
    ("place-content-stretch", "place-content:stretch"),
    ("place-items-baseline", "place-items:baseline"),
    ("place-items-center", "place-items:center"),
    ("place-items-center-safe", "place-items:safe center"),
    ("place-items-end", "place-items:end"),
    ("place-items-end-safe", "place-items:safe end"),
    ("place-items-start", "place-items:start"),
    ("place-items-stretch", "place-items:stretch"),
    ("place-self-auto", "place-self:auto"),
    ("place-self-center", "place-self:center"),
    ("place-self-center-safe", "place-self:safe center"),
    ("place-self-end", "place-self:end"),
    ("place-self-end-safe", "place-self:safe end"),
    ("place-self-start", "place-self:start"),
    ("place-self-stretch", "place-self:stretch"),
    ("pointer-events-auto", "pointer-events:auto"),
    ("pointer-events-none", "pointer-events:none"),
    (
        "proportional-nums",
        "--tw-numeric-spacing:proportional-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("relative", "position:relative"),
    ("resize", "resize:both"),
    ("resize-none", "resize:none"),
    ("resize-x", "resize:horizontal"),
    ("resize-y", "resize:vertical"),
    ("ring-inset", "--tw-ring-inset:inset"),
    ("rotate-none", "rotate:none"),
    (
        "scale-3d",
        "scale:var(--tw-scale-x) var(--tw-scale-y) var(--tw-scale-z)",
    ),
    ("scale-none", "scale:none"),
    ("scheme-dark", "color-scheme:dark"),
    ("scheme-light", "color-scheme:light"),
    ("scheme-light-dark", "color-scheme:light dark"),
    ("scheme-normal", "color-scheme:normal"),
    ("scheme-only-dark", "color-scheme:only dark"),
    ("scheme-only-light", "color-scheme:only light"),
    ("scroll-auto", "scroll-behavior:auto"),
    ("scroll-smooth", "scroll-behavior:smooth"),
    ("scrollbar-auto", "scrollbar-width:auto"),
    ("scrollbar-gutter-auto", "scrollbar-gutter:auto"),
    (
        "scrollbar-gutter-both",
        "scrollbar-gutter:stable both-edges",
    ),
    ("scrollbar-gutter-stable", "scrollbar-gutter:stable"),
    ("scrollbar-none", "scrollbar-width:none"),
    ("scrollbar-thin", "scrollbar-width:thin"),
    ("select-all", "-webkit-user-select:all;user-select:all"),
    ("select-auto", "-webkit-user-select:auto;user-select:auto"),
    ("select-none", "-webkit-user-select:none;user-select:none"),
    ("select-text", "-webkit-user-select:text;user-select:text"),
    ("self-auto", "align-self:auto"),
    ("self-baseline", "align-self:baseline"),
    ("self-baseline-last", "align-self:last baseline"),
    ("self-center", "align-self:center"),
    ("self-center-safe", "align-self:safe center"),
    ("self-end", "align-self:flex-end"),
    ("self-end-safe", "align-self:safe flex-end"),
    ("self-start", "align-self:flex-start"),
    ("self-stretch", "align-self:stretch"),
    (
        "slashed-zero",
        "--tw-slashed-zero:slashed-zero;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("snap-align-none", "scroll-snap-align:none"),
    ("snap-always", "scroll-snap-stop:always"),
    (
        "snap-both",
        "scroll-snap-type:both var(--tw-scroll-snap-strictness)",
    ),
    ("snap-center", "scroll-snap-align:center"),
    ("snap-end", "scroll-snap-align:end"),
    ("snap-mandatory", "--tw-scroll-snap-strictness:mandatory"),
    ("snap-none", "scroll-snap-type:none"),
    ("snap-normal", "scroll-snap-stop:normal"),
    ("snap-proximity", "--tw-scroll-snap-strictness:proximity"),
    ("snap-start", "scroll-snap-align:start"),
    (
        "snap-x",
        "scroll-snap-type:x var(--tw-scroll-snap-strictness)",
    ),
    (
        "snap-y",
        "scroll-snap-type:y var(--tw-scroll-snap-strictness)",
    ),
    (
        "sr-only",
        "position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap;border-width:0",
    ),
    (
        "stacked-fractions",
        "--tw-numeric-fraction:stacked-fractions;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("static", "position:static"),
    ("sticky", "position:sticky"),
    ("stroke-none", "stroke:none"),
    (
        "subpixel-antialiased",
        "-webkit-font-smoothing:auto;-moz-osx-font-smoothing:auto",
    ),
    ("table", "display:table"),
    ("table-auto", "table-layout:auto"),
    ("table-caption", "display:table-caption"),
    ("table-cell", "display:table-cell"),
    ("table-column", "display:table-column"),
    ("table-column-group", "display:table-column-group"),
    ("table-fixed", "table-layout:fixed"),
    ("table-footer-group", "display:table-footer-group"),
    ("table-header-group", "display:table-header-group"),
    ("table-row", "display:table-row"),
    ("table-row-group", "display:table-row-group"),
    (
        "tabular-nums",
        "--tw-numeric-spacing:tabular-nums;font-variant-numeric:var(--tw-ordinal,) var(--tw-slashed-zero,) var(--tw-numeric-figure,) var(--tw-numeric-spacing,) var(--tw-numeric-fraction,)",
    ),
    ("text-balance", "text-wrap:balance"),
    ("text-center", "text-align:center"),
    ("text-clip", "text-overflow:clip"),
    ("text-ellipsis", "text-overflow:ellipsis"),
    ("text-end", "text-align:end"),
    ("text-justify", "text-align:justify"),
    ("text-left", "text-align:left"),
    ("text-nowrap", "text-wrap:nowrap"),
    ("text-pretty", "text-wrap:pretty"),
    ("text-right", "text-align:right"),
    ("text-start", "text-align:start"),
    ("text-wrap", "text-wrap:wrap"),
    ("touch-auto", "touch-action:auto"),
    ("touch-manipulation", "touch-action:manipulation"),
    ("touch-none", "touch-action:none"),
    (
        "touch-pan-down",
        "--tw-pan-y:pan-down;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pan-left",
        "--tw-pan-x:pan-left;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pan-right",
        "--tw-pan-x:pan-right;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pan-up",
        "--tw-pan-y:pan-up;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pan-x",
        "--tw-pan-x:pan-x;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pan-y",
        "--tw-pan-y:pan-y;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    (
        "touch-pinch-zoom",
        "--tw-pinch-zoom:pinch-zoom;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,)",
    ),
    ("transform-3d", "transform-style:preserve-3d"),
    ("transform-border", "transform-box:border-box"),
    ("transform-content", "transform-box:content-box"),
    (
        "transform-cpu",
        "transform:var(--tw-rotate-x,) var(--tw-rotate-y,) var(--tw-rotate-z,) var(--tw-skew-x,) var(--tw-skew-y,)",
    ),
    ("transform-fill", "transform-box:fill-box"),
    ("transform-flat", "transform-style:flat"),
    (
        "transform-gpu",
        "transform:translateZ(0) var(--tw-rotate-x,) var(--tw-rotate-y,) var(--tw-rotate-z,) var(--tw-skew-x,) var(--tw-skew-y,)",
    ),
    ("transform-none", "transform:none"),
    ("transform-stroke", "transform-box:stroke-box"),
    ("transform-view", "transform-box:view-box"),
    ("transition-discrete", "transition-behavior:allow-discrete"),
    ("transition-normal", "transition-behavior:normal"),
    (
        "translate-3d",
        "translate:var(--tw-translate-x) var(--tw-translate-y) var(--tw-translate-z)",
    ),
    (
        "truncate",
        "overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
    ),
    ("underline", "text-decoration-line:underline"),
    ("underline-offset-auto", "text-underline-offset:auto"),
    ("uppercase", "text-transform:uppercase"),
    ("via-none", "--tw-gradient-via-stops:initial"),
    ("visible", "visibility:visible"),
    ("whitespace-break-spaces", "white-space:break-spaces"),
    ("whitespace-normal", "white-space:normal"),
    ("whitespace-nowrap", "white-space:nowrap"),
    ("whitespace-pre", "white-space:pre"),
    ("whitespace-pre-line", "white-space:pre-line"),
    ("whitespace-pre-wrap", "white-space:pre-wrap"),
    ("will-change-auto", "will-change:auto"),
    ("will-change-contents", "will-change:contents"),
    ("will-change-scroll", "will-change:scroll-position"),
    ("will-change-transform", "will-change:transform"),
    ("wrap-anywhere", "overflow-wrap:anywhere"),
    ("wrap-break-word", "overflow-wrap:break-word"),
    ("wrap-normal", "overflow-wrap:normal"),
];

const NEGATIVE: &[&str] = &[
    "inset",
    "inset-x",
    "inset-y",
    "inset-s",
    "inset-e",
    "inset-bs",
    "inset-be",
    "start",
    "end",
    "top",
    "right",
    "bottom",
    "left",
    "z",
    "order",
    "col",
    "col-start",
    "col-end",
    "row",
    "row-start",
    "row-end",
    "m",
    "mx",
    "my",
    "ms",
    "me",
    "mbs",
    "mbe",
    "mt",
    "mr",
    "mb",
    "ml",
    "scroll-m",
    "scroll-mx",
    "scroll-my",
    "scroll-ms",
    "scroll-me",
    "scroll-mbs",
    "scroll-mbe",
    "scroll-mt",
    "scroll-mr",
    "scroll-mb",
    "scroll-ml",
    "space-x",
    "space-y",
    "translate",
    "translate-x",
    "translate-y",
    "translate-z",
    "scale",
    "scale-x",
    "scale-y",
    "scale-z",
    "rotate",
    "rotate-x",
    "rotate-y",
    "rotate-z",
    "skew",
    "skew-x",
    "skew-y",
    "tracking",
    "indent",
    "outline-offset",
    "underline-offset",
    "hue-rotate",
    "backdrop-hue-rotate",
    "bg-linear",
    "bg-conic",
    "mask-linear",
    "mask-conic",
];

const ORIGINS: &[(&str, &str)] = &[
    ("center", "center"),
    ("top", "top"),
    ("top-right", "100% 0"),
    ("right", "100%"),
    ("bottom-right", "100% 100%"),
    ("bottom", "bottom"),
    ("bottom-left", "0 100%"),
    ("left", "0"),
    ("top-left", "0 0"),
];

const SHADOW: &str = "var(--tw-inset-shadow), var(--tw-inset-ring-shadow), var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow)";
const FILTER: &str = "var(--tw-blur,) var(--tw-brightness,) var(--tw-contrast,) var(--tw-grayscale,) var(--tw-hue-rotate,) var(--tw-invert,) var(--tw-saturate,) var(--tw-sepia,) var(--tw-drop-shadow,)";
const BACKDROP: &str = "var(--tw-backdrop-blur,) var(--tw-backdrop-brightness,) var(--tw-backdrop-contrast,) var(--tw-backdrop-grayscale,) var(--tw-backdrop-hue-rotate,) var(--tw-backdrop-invert,) var(--tw-backdrop-opacity,) var(--tw-backdrop-saturate,) var(--tw-backdrop-sepia,)";
const TRANSFORM: &str = "var(--tw-rotate-x,) var(--tw-rotate-y,) var(--tw-rotate-z,) var(--tw-skew-x,) var(--tw-skew-y,)";
const TRANSITION: &str = "transition-timing-function:var(--tw-ease, var(--default-transition-timing-function));transition-duration:var(--tw-duration, var(--default-transition-duration))";
const COLORS: &str = "color, background-color, border-color, outline-color, text-decoration-color, fill, stroke, --tw-gradient-from, --tw-gradient-via, --tw-gradient-to";
const STOPS: &str = "var(--tw-gradient-via-stops, var(--tw-gradient-position), var(--tw-gradient-from) var(--tw-gradient-from-position), var(--tw-gradient-to) var(--tw-gradient-to-position))";
const MASK: &str = "mask-image:var(--tw-mask-linear), var(--tw-mask-radial), var(--tw-mask-conic);mask-composite:intersect";
const EDGES: &str =
    "var(--tw-mask-left), var(--tw-mask-right), var(--tw-mask-bottom), var(--tw-mask-top)";

type Pairs = Vec<(String, String)>;
type Nested = (Vec<String>, String, Pairs);

#[derive(Clone, Default)]
struct Body {
    declarations: Pairs,
    nested: Vec<Nested>,
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

    fn nest(mut self, at: &[&str], selector: &str, source: &str) -> Self {
        let pairs = Self::parse(source).declarations;
        self.nested.push((
            at.iter().map(|&rule| rule.to_owned()).collect(),
            selector.to_owned(),
            pairs,
        ));
        self
    }
}

#[derive(Default)]
struct Variant {
    branches: Vec<(Vec<String>, Option<String>)>,
    content: bool,
    never: bool,
    relative: bool,
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
    rules: Vec<Nested>,
}

pub(crate) struct Update<'a> {
    pub(crate) root: Rc<Style>,
    pub(crate) replaced: Vec<Rc<Stylesheet>>,
    pub(crate) added: Vec<(Rc<Stylesheet>, Vec<Rc<Stylesheet>>)>,
    pub(crate) rules: Vec<&'a Rule>,
    pub(crate) keys: &'a FxHashMap<usize, u64>,
}

#[derive(Default)]
struct Config {
    key: String,
    lists: Vec<Vec<Rc<Stylesheet>>>,
    theme: BTreeMap<String, String>,
    inline: BTreeSet<String>,
    cleared: Vec<String>,
    variants: Vec<(String, String)>,
    utilities: Vec<(String, String)>,
    expanding: std::cell::RefCell<Vec<usize>>,
    // Do not cache expansions cut short by cycles or depth limits: their result depends on the starting
    // point.
    #[allow(clippy::type_complexity)]
    expanded: std::cell::RefCell<FxHashMap<(usize, Option<String>, Option<String>), Option<Body>>>,
    cut: std::cell::Cell<bool>,
    import: Option<bool>,
    classes: FxHashMap<String, bool>,
    scanned: Vec<u64>,
    applied: Vec<(Rc<Stylesheet>, Rc<Stylesheet>)>,
    sorted: Vec<(Sort, u64, Vec<Rule>)>,
    chunks: Vec<(u64, Rc<Stylesheet>)>,
    keys: FxHashMap<usize, u64>,
    header: Option<Rc<Stylesheet>>,
    vars: BTreeMap<String, String>,
    properties: BTreeSet<&'static str>,
    keyframes: BTreeSet<&'static str>,
    root: Rc<Style>,
}

#[derive(Default)]
pub(crate) struct Compiler(Vec<Config>);

impl Compiler {
    pub(crate) fn update(
        &mut self,
        sheets: &mut Vec<Rc<Stylesheet>>,
        doc: &Document,
        order: &[(u32, u32)],
    ) -> Option<Update<'_>> {
        let index = sheets.iter().position(|sheet| {
            sheet
                .0
                .iter()
                .any(|rule| matches!(**rule, Rule::Tailwind(_)))
        })?;
        let same = |list: &Vec<Rc<Stylesheet>>| {
            list.len() == sheets.len()
                && list
                    .iter()
                    .zip(sheets.iter())
                    .all(|(a, b)| Rc::ptr_eq(a, b))
        };
        let at = match self
            .0
            .iter()
            .position(|config| config.lists.iter().any(same))
        {
            Some(at) => at,
            None => {
                let key: String = sheets
                    .iter()
                    .flat_map(|sheet| &sheet.0)
                    .filter(|rule| {
                        matches!(
                            ***rule,
                            Rule::Tailwind(_)
                                | Rule::Theme(..)
                                | Rule::Variant(..)
                                | Rule::Utility(..)
                        )
                    })
                    .map(|rule| rule.source())
                    .collect();
                let at = self
                    .0
                    .iter()
                    .position(|config| config.key == key)
                    .unwrap_or_else(|| {
                        if self.0.len() == 4 {
                            self.0.remove(0);
                        }
                        self.0.push(Config::new(key, sheets));
                        self.0.len() - 1
                    });
                let lists = &mut self.0[at].lists;
                if lists.len() == 4 {
                    lists.remove(0);
                }
                lists.push(sheets.clone());
                at
            }
        };
        let config = self.0.remove(at);
        self.0.push(config);
        let config = self.0.last_mut()?;
        Some(config.update(sheets, index, doc, order))
    }
}

impl Config {
    fn new(key: String, sheets: &[Rc<Stylesheet>]) -> Self {
        let mut config = Self {
            key,
            ..Self::default()
        };
        for rule in sheets.iter().flat_map(|sheet| &sheet.0) {
            match &**rule {
                Rule::Tailwind(Some(preflight)) => {
                    config.import = Some(config.import == Some(true) || *preflight);
                }
                Rule::Theme(inline, children) => {
                    for child in children {
                        let Rule::Declarations(values) = child else {
                            continue;
                        };
                        for value in &values.0 {
                            let Declaration::Property(name, value) = value else {
                                continue;
                            };
                            if name.ends_with("-*") || value == "initial" {
                                config.theme.retain(|other, _| !removes(name, other));
                                config.inline.retain(|other| !removes(name, other));
                                config.cleared.push(name.clone());
                                continue;
                            }
                            if *inline {
                                config.inline.insert(name.clone());
                            } else {
                                config.inline.remove(name);
                            }
                            config.theme.insert(name.clone(), value.clone());
                        }
                    }
                }
                Rule::Variant(name, template) => {
                    config.variants.retain(|(other, _)| other != name);
                    config.variants.push((name.clone(), template.clone()));
                }
                Rule::Utility(name, body) => config.utilities.push((name.clone(), body.clone())),
                _ => {}
            }
        }
        config
    }

    fn update(
        &mut self,
        sheets: &mut Vec<Rc<Stylesheet>>,
        index: usize,
        doc: &Document,
        order: &[(u32, u32)],
    ) -> Update<'_> {
        let mut used = String::new();
        let first = self.header.is_none();
        if first {
            let theme: Vec<_> = self
                .theme
                .keys()
                .filter(|name| !self.inline.contains(*name))
                .map(|name| format!("var({name})"))
                .collect();
            used = theme.concat();
            if self.import == Some(true) {
                used.push_str(PREFLIGHT);
            }
        }
        let mut fresh = Vec::new();
        if self.scanned.len() < doc.nodes.len() {
            self.scanned.resize(doc.nodes.len(), 0);
        }
        for &(id, _) in order {
            let id = id as usize;
            let revision = doc.nodes.revision(id);
            if std::mem::replace(&mut self.scanned[id], revision) == revision {
                continue;
            }
            let node = &doc.nodes[id];
            for (name, value) in node.attrs.iter() {
                if name != "class" {
                    if value.contains("var(--") {
                        used.push_str(value);
                    }
                    continue;
                }
                if self.import.is_none() {
                    continue;
                }
                for class in value.split_ascii_whitespace() {
                    if self.classes.contains_key(class) {
                        continue;
                    }
                    let compiled = self.compile(class, &escape(class));
                    self.classes.insert(class.to_owned(), compiled.is_some());
                    if let Some(compiled) = compiled {
                        let css = css(&compiled.rules);
                        used.push_str(&css);
                        let rules = Stylesheet::parse(&css).0.into_iter();
                        fresh.push((compiled.sort, rules.map(Rc::unwrap_or_clone).collect()));
                    }
                }
            }
        }
        for sheet in sheets.iter_mut() {
            if let Some((_, applied)) = self
                .applied
                .iter()
                .find(|(source, _)| Rc::ptr_eq(source, sheet))
            {
                *sheet = applied.clone();
                continue;
            }
            let rules = self.apply(&sheet.0, None, &mut used);
            let applied = rules.map_or_else(
                || sheet.clone(),
                |rules| Rc::new(Stylesheet(rules.into_iter().map(Rc::new).collect())),
            );
            for rule in &applied.0 {
                used.push_str(&rule.source());
            }
            self.applied.push((sheet.clone(), applied.clone()));
            *sheet = applied;
        }
        let lists = &self.lists;
        self.applied.retain(|(source, _)| {
            lists
                .iter()
                .any(|list| list.iter().any(|sheet| Rc::ptr_eq(sheet, source)))
        });
        let header = self.vars(&used) || first;
        let mut replaced = Vec::new();
        let mut added = Vec::new();
        let mut rules = Vec::new();
        if header {
            replaced.extend(self.header.take());
            let mut css = String::from(
                "@layer properties;@layer theme,base,components,utilities;@layer properties{*,::before,::after{",
            );
            for (name, value) in PROPERTIES {
                if self.properties.contains(name) {
                    let _ = write!(css, "{name}:{value};");
                }
            }
            css.push_str("}}@layer theme{:root,:host{");
            for (name, value) in &self.vars {
                let _ = write!(css, "{name}:{value};");
            }
            css.push_str("}}");
            for (name, frames) in KEYFRAMES {
                if self.keyframes.contains(name) {
                    let _ = write!(css, "@keyframes {name}{{{frames}}}");
                }
            }
            self.header = Some(Rc::new(Stylesheet::parse(&css)));
            self.root = Rc::new(
                self.vars
                    .iter()
                    .map(|(name, value)| (Prop::new(name), value.as_str().into()))
                    .collect(),
            );
        }
        let mut new = BTreeSet::new();
        if !fresh.is_empty() {
            fresh.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            // Keys must stay below 2^56, so a selector index fits below them in 64 bits.
            const KEYS: u64 = 1 << 56;
            let mut renumber = false;
            if fresh.len() <= 16 {
                for (sort, rules) in fresh {
                    let position = self.sorted.partition_point(|(other, ..)| *other < sort);
                    let low = position
                        .checked_sub(1)
                        .map_or(0, |previous| self.sorted[previous].1);
                    let high = self.sorted.get(position).map_or(KEYS, |(_, key, _)| *key);
                    let key = low + (high - low) / 2;
                    renumber |= key == low;
                    new.insert(key);
                    self.sorted.insert(position, (sort, key, rules));
                }
            } else {
                let mut merged = Vec::with_capacity(self.sorted.len() + fresh.len());
                let mut fresh = fresh.into_iter().peekable();
                let mut sorted = std::mem::take(&mut self.sorted).into_iter().peekable();
                while let Some(next) = fresh.peek() {
                    while let Some(old) = sorted.next_if(|old| old.0 < next.0) {
                        merged.push(old);
                    }
                    let low = merged.last().map_or(0, |(_, key, _)| *key);
                    let high = sorted.peek().map_or(KEYS, |(_, key, _)| *key);
                    let mut run = Vec::new();
                    while let Some(item) =
                        fresh.next_if(|item| sorted.peek().is_none_or(|old| item.0 < old.0))
                    {
                        run.push(item);
                    }
                    let step = (high - low) / (run.len() as u64 + 1);
                    renumber |= step == 0;
                    for (index, (sort, rules)) in run.into_iter().enumerate() {
                        let key = low + step * (index as u64 + 1);
                        new.insert(key);
                        merged.push((sort, key, rules));
                    }
                }
                merged.extend(sorted);
                self.sorted = merged;
            }
            if renumber {
                let step = KEYS / (self.sorted.len() as u64 + 1);
                for (index, (_, key, _)) in self.sorted.iter_mut().enumerate() {
                    *key = step * (index as u64 + 1);
                }
            }
            if renumber {
                self.keys.clear();
            }
            for (_, key, rules) in &self.sorted {
                if !renumber && !new.contains(key) {
                    continue;
                }
                let mut index = 0;
                for rule in rules {
                    selectors(rule, &mut |list| {
                        self.keys
                            .insert(Rc::as_ptr(list) as usize, *key << 8 | index);
                        index = (index + 1).min(255);
                    });
                }
            }
            if renumber || self.chunks.is_empty() {
                replaced.extend(self.chunks.drain(..).map(|(_, sheet)| sheet));
                for start in (0..self.sorted.len()).step_by(128) {
                    let end = (start + 128).min(self.sorted.len());
                    let next = self.sorted.get(end).map_or(u64::MAX, |(_, key, _)| *key);
                    self.chunks.push((next, chunk(&self.sorted[start..end])));
                }
            } else {
                let mut index = 0;
                while index < self.chunks.len() {
                    let low = index
                        .checked_sub(1)
                        .map_or(0, |previous| self.chunks[previous].0);
                    let high = self.chunks[index].0;
                    let start = self.sorted.partition_point(|(_, key, _)| *key < low);
                    let end = self.sorted.partition_point(|(_, key, _)| *key < high);
                    let range = &self.sorted[start..end];
                    if new.range(low..).next().is_none_or(|key| *key >= high) {
                        index += 1;
                        continue;
                    }
                    let parts = if range.len() > 256 {
                        let middle = range.len() / 2;
                        vec![
                            (range[middle].1, chunk(&range[..middle])),
                            (high, chunk(&range[middle..])),
                        ]
                    } else {
                        vec![(high, chunk(range))]
                    };
                    let count = parts.len();
                    let sheets = parts.iter().map(|(_, sheet)| sheet.clone()).collect();
                    added.push((self.chunks[index].1.clone(), sheets));
                    self.chunks.splice(index..=index, parts);
                    index += count;
                }
            }
        }
        if replaced.is_empty() {
            rules = new
                .iter()
                .filter_map(|key| {
                    self.sorted
                        .binary_search_by_key(key, |(_, key, _)| *key)
                        .ok()
                })
                .flat_map(|index| &self.sorted[index].2)
                .collect();
        } else {
            replaced.extend(added.drain(..).map(|(old, _)| old));
        }
        if self.import.is_some() {
            let generated = self
                .header
                .iter()
                .chain(self.chunks.iter().map(|(_, sheet)| sheet));
            let base = (self.import == Some(true)).then(|| BASE.with(Clone::clone));
            sheets.splice(index..index, generated.cloned().chain(base));
        }
        Update {
            root: self.root.clone(),
            replaced,
            added,
            rules,
            keys: &self.keys,
        }
    }

    fn vars(&mut self, css: &str) -> bool {
        let mut changed = false;
        let mut pending: Vec<String> = names(css, "var(--")
            .map(|name| format!("--{name}"))
            .collect();
        while let Some(name) = pending.pop() {
            if self.vars.contains_key(&name) || self.inline.contains(&name) {
                continue;
            }
            let value = match self.theme.get(&name) {
                Some(value) => value.clone(),
                None if self.removed(&name) || INLINE.contains(&name.as_str()) => continue,
                None => {
                    let Some(value) =
                        DEFAULTS.with(|defaults| defaults.get(name.as_str()).copied())
                    else {
                        continue;
                    };
                    match value
                        .strip_prefix("--theme(")
                        .and_then(|value| value.strip_suffix(", initial)"))
                    {
                        Some(other) if self.inline.contains(other) => self.theme[other].clone(),
                        Some(other) if self.raw(other).is_some() => format!("var({other})"),
                        Some(_) => continue,
                        None => value.to_owned(),
                    }
                }
            };
            pending.extend(names(&value, "var(--").map(|name| format!("--{name}")));
            if let Some(animation) = name.strip_prefix("--animate-")
                && let Some((frames, _)) = KEYFRAMES.iter().find(|(frames, _)| *frames == animation)
            {
                self.keyframes.insert(frames);
            }
            self.vars.insert(name, value);
            changed = true;
        }
        for name in names(css, "--tw-") {
            if let Some((property, _)) = PROPERTIES
                .iter()
                .find(|(property, _)| property[5..] == *name)
            {
                changed |= self.properties.insert(property);
            }
        }
        changed
    }

    fn apply<R: std::borrow::Borrow<Rule>>(
        &self,
        rules: &[R],
        parent: Option<&str>,
        used: &mut String,
    ) -> Option<Vec<Rule>> {
        let mut changed = false;
        let mut result = Vec::with_capacity(rules.len());
        for rule in rules {
            let rule = rule.borrow();
            let rewritten = match rule {
                Rule::Style(selectors, children) => {
                    let base: Vec<_> = selectors
                        .iter()
                        .map(|selector| match parent {
                            Some(parent) => {
                                selector.source().replace('&', &format!(":is({parent})"))
                            }
                            None => selector.source(),
                        })
                        .collect();
                    let base = match base.as_slice() {
                        [single] => single.clone(),
                        _ => format!(":is({})", base.join(",")),
                    };
                    self.apply(children, Some(&base), used)
                        .map(|children| vec![Rule::Style(selectors.clone(), children)])
                }
                Rule::Declarations(values)
                    if values.0.iter().any(|value| match value {
                        Declaration::Apply(_) => true,
                        Declaration::Property(_, value) => calls(value),
                    }) =>
                {
                    let mut declarations = Vec::new();
                    let mut extra = String::new();
                    for value in &values.0 {
                        match value {
                            Declaration::Property(name, value) => declarations
                                .push(Declaration::Property(name.clone(), self.functions(value))),
                            Declaration::Apply(classes) => {
                                let Some(base) = parent else {
                                    continue;
                                };
                                let mut compiled: Vec<_> = classes
                                    .iter()
                                    .filter_map(|class| self.compile(class, base))
                                    .collect();
                                compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                                for (at, selector, pairs) in
                                    compiled.into_iter().flat_map(|c| c.rules)
                                {
                                    if at.is_empty() && selector == base {
                                        for (name, value) in pairs {
                                            used.push_str(&value);
                                            declarations.push(Declaration::Property(name, value));
                                        }
                                    } else {
                                        extra.push_str(&css(&[(at, selector, pairs)]));
                                    }
                                }
                            }
                        }
                    }
                    used.push_str(&extra);
                    let mut rules = vec![Rule::Declarations(Declarations(declarations))];
                    rules.extend(
                        Stylesheet::parse(&extra)
                            .0
                            .into_iter()
                            .map(Rc::unwrap_or_clone),
                    );
                    Some(rules)
                }
                Rule::Variant(name, body) if parent.is_some() && body.starts_with('{') => {
                    let base = parent.unwrap_or_default();
                    let body = body
                        .strip_prefix('{')
                        .and_then(|body| body.strip_suffix('}'))
                        .unwrap_or_default();
                    let mut css = String::new();
                    let branches = self.variant(name, 0).map(|(_, variant)| variant.branches);
                    for (at, selector) in branches.into_iter().flatten() {
                        let selector =
                            selector.map_or_else(|| base.to_owned(), |t| t.replace('&', base));
                        for rule in &at {
                            let _ = write!(css, "{rule}{{");
                        }
                        let _ = write!(css, "{selector}{{{body}}}");
                        css.push_str(&"}".repeat(at.len()));
                    }
                    let parsed: Vec<_> = Stylesheet::parse(&css)
                        .0
                        .into_iter()
                        .map(Rc::unwrap_or_clone)
                        .collect();
                    Some(self.apply(&parsed, None, used).unwrap_or(parsed))
                }
                Rule::Media(queries, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Media(queries.clone(), children)]),
                Rule::Layer(names, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Layer(names.clone(), children)]),
                Rule::Supports(condition, supported, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Supports(condition.clone(), *supported, children)]),
                Rule::Container(name, block, query, children) => {
                    self.apply(children, parent, used).map(|children| {
                        vec![Rule::Container(
                            name.clone(),
                            *block,
                            query.clone(),
                            children,
                        )]
                    })
                }
                _ => None,
            };
            match rewritten {
                Some(rules) => {
                    changed = true;
                    result.extend(rules);
                }
                None => result.push(rule.clone()),
            }
        }
        changed.then_some(result)
    }

    fn compile(&self, class: &str, base: &str) -> Option<Compiled> {
        let mut parts = split(class, ':');
        if parts.len() > 16 {
            return None;
        }
        let utility = parts.pop()?;
        let (utility, important) = match utility
            .strip_suffix('!')
            .or_else(|| utility.strip_prefix('!'))
        {
            Some(utility) => (utility, true),
            None => (utility, false),
        };
        let body = self.utility(utility)?;
        let mut branches = vec![(Vec::<String>::new(), vec![base.to_owned()])];
        let mut ranks = Vec::new();
        let mut content = false;
        // Cap selector size: comma parts and & expansion multiply at each variant.
        let (mut size, limit) = (0, (base.len() + 64) * 256);
        for name in parts {
            let (rank, variant) = self.variant(name, 0)?;
            if variant.relative {
                return None;
            }
            ranks.push(rank);
            content |= variant.content;
            let mut next = Vec::new();
            for (at, selectors) in &branches {
                for (rules, template) in &variant.branches {
                    let mut at = at.clone();
                    at.extend(rules.iter().cloned());
                    let selectors = match template {
                        None => selectors.clone(),
                        Some(template) => {
                            let parts = split(template, ',');
                            let mut list = Vec::with_capacity(selectors.len() * parts.len());
                            for selector in selectors {
                                for part in &parts {
                                    let part = part.trim();
                                    size += part.len() + part.matches('&').count() * selector.len();
                                    if size > limit {
                                        return None;
                                    }
                                    list.push(part.replace('&', selector));
                                }
                            }
                            list
                        }
                    };
                    next.push((at, selectors));
                }
            }
            if next.len() > 16 {
                return None;
            }
            branches = next;
        }
        ranks.sort_unstable_by(|a, b| b.cmp(a));
        ranks.dedup();
        let mut properties = BTreeSet::new();
        let mut count = 0;
        for (key, _) in body
            .declarations
            .iter()
            .chain(body.nested.iter().flat_map(|(.., values)| values))
        {
            count += 1;
            if let Some(index) = ORDER
                .iter()
                .position(|name| *name == body.sort.unwrap_or(key))
            {
                properties.insert(index as u16);
            }
        }
        let mut properties: Vec<_> = properties.into_iter().collect();
        properties.push(u16::MAX);
        let sort = Sort {
            variants: ranks,
            properties,
            count: Reverse(count),
            name: class.to_owned(),
        };
        let suffix = if important { " !important" } else { "" };
        let pairs = |pairs: &Pairs| -> Pairs {
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), format!("{value}{suffix}")))
                .collect()
        };
        let mut rules = Vec::new();
        for (at, selectors) in &branches {
            if !body.declarations.is_empty() {
                let mut declarations = pairs(&body.declarations);
                if content && !declarations.iter().any(|(key, _)| key == "content") {
                    declarations.insert(0, ("content".into(), "var(--tw-content)".into()));
                }
                rules.push((at.clone(), selectors.join(","), declarations));
            }
            for (nested, template, values) in &body.nested {
                let mut at = at.clone();
                at.extend(nested.iter().cloned());
                let list: Vec<_> = selectors
                    .iter()
                    .map(|selector| template.replace('&', selector))
                    .collect();
                rules.push((at, list.join(","), pairs(values)));
            }
        }
        Some(Compiled { sort, rules })
    }

    fn variant(&self, name: &str, depth: u8) -> Option<(Rank, Variant)> {
        if depth > 8 {
            return None;
        }
        let order = |name: &str| VARIANTS.iter().position(|(other, _)| *other == name);
        let (name, modifier) = modifier(name);
        if let Some(raw) = name.strip_prefix('[') {
            if modifier.is_some() || raw.starts_with('@') && raw.contains('&') {
                return None;
            }
            let value = self.arbitrary(name)?;
            if value.contains(['{', '}']) {
                return None;
            }
            let relative = value.starts_with(['>', '+', '~']);
            let source = if relative {
                format!(" {} {}", &value[..1], value[1..].trim_start())
            } else if let Some(at) = value.strip_prefix('@') {
                let end = at
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .unwrap_or(at.len());
                format!("@{} {}", &at[..end], at[end..].trim_start())
            } else if value.contains('&') {
                value.clone()
            } else {
                format!("&:is({value})")
            };
            let mut variant = template(&source);
            variant.relative = relative;
            return Some(((u16::MAX, value), variant));
        }
        if let Some(index) = self.variants.iter().position(|(other, _)| other == name) {
            let rank = order(name).unwrap_or(VARIANTS.len() + index) as u16;
            return modifier
                .is_none()
                .then(|| ((rank, String::new()), template(&self.variants[index].1)));
        }
        if let Some(index) = order(name)
            && !VARIANTS[index].1.is_empty()
        {
            if modifier.is_some() {
                return None;
            }
            let mut variant = template(VARIANTS[index].1);
            if name == "hover" {
                variant.branches[0].0.push("@media (hover: hover)".into());
            }
            variant.content = matches!(name, "before" | "after");
            variant.never |= matches!(name, "*" | "**");
            return Some(((index as u16, String::new()), variant));
        }
        if modifier.is_none()
            && let Some(width) = self.raw(&format!("--breakpoint-{name}"))
        {
            return self.media(">=", &width, "min");
        }
        let (root, value) = if let Some(value) = name.strip_prefix("@min-") {
            ("@min", value)
        } else if let Some(value) = name.strip_prefix("@max-") {
            ("@max", value)
        } else if let Some(value) = name.strip_prefix('@') {
            ("@min", value)
        } else {
            [
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
            .find_map(|root| Some((*root, name.strip_prefix(root)?.strip_prefix('-')?)))?
        };
        if value.is_empty() {
            return None;
        }
        let rank = order(root)? as u16;
        match root {
            "group" | "peer" | "not" | "has" | "in" => {
                let named = matches!(root, "group" | "peer");
                let inner = match modifier {
                    Some(modifier) if !named => format!("{value}/{modifier}"),
                    _ => value.to_owned(),
                };
                let (inner_rank, inner) = self.variant(&inner, depth + 1)?;
                if inner.never || inner.relative && !matches!(root, "has" | "in") {
                    return None;
                }
                let branches = if root == "not" {
                    let [(at, selector)] = <[_; 1]>::try_from(inner.branches).ok()?;
                    if at.len() > 1 {
                        return None;
                    }
                    let mut branches = Vec::new();
                    if let Some(selector) = selector {
                        if selector.contains("::") {
                            return None;
                        }
                        let list: Vec<_> = split(&selector, ',')
                            .into_iter()
                            .map(|part| universal(part.trim(), true))
                            .collect();
                        branches.push((Vec::new(), Some(format!("&:not({})", list.join(", ")))));
                    }
                    if let Some(rule) = at.first() {
                        branches.push((vec![negate(rule)?], None));
                    }
                    branches
                } else {
                    let marker = match modifier {
                        Some(name) if named && named_value(name) => {
                            format!(":where(.{root}\\/{})", &escape(name)[1..])
                        }
                        Some(_) if named => return None,
                        _ => format!(":where(.{root})"),
                    };
                    let mut branches = Vec::new();
                    for (at, selector) in inner.branches {
                        let selector = selector?;
                        let selector = match root {
                            "has" => format!("&:has({})", universal(&selector, true)),
                            "in" => format!(":where({}) &", universal(&selector, false)),
                            _ => {
                                let list = selector.replace('&', &marker);
                                let list = if split(&list, ',').len() > 1 {
                                    format!(":is({list})")
                                } else {
                                    list
                                };
                                let target = if root == "group" { "*" } else { "~ *" };
                                format!("&:is({list} {target})")
                            }
                        };
                        branches.push((at, Some(selector)));
                    }
                    branches
                };
                if branches.is_empty() {
                    return None;
                }
                Some((
                    (rank, format!("{:05}{}", inner_rank.0, inner_rank.1)),
                    Variant {
                        branches,
                        ..Variant::default()
                    },
                ))
            }
            _ if modifier.is_some() && !root.starts_with('@') => None,
            "aria" | "data" => {
                let selector = if value.starts_with('[') {
                    format!("&[{root}-{}]", quote(&self.arbitrary(value)?))
                } else if !named_value(value) {
                    return None;
                } else if root == "aria" {
                    format!("&[aria-{value}=\"true\"]")
                } else {
                    format!("&[data-{value}]")
                };
                Some(((rank, value.to_owned()), template(&selector)))
            }
            "nth" | "nth-last" | "nth-of-type" | "nth-last-of-type" => {
                let argument = if value.starts_with('[') {
                    self.arbitrary(value)?
                } else {
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|_| integer(value))?
                        .to_string()
                };
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
                if value == "bg-glass" {
                    return Some(((rank, value.into()), template("@media (-macos-glass)")));
                }
                let raw = if value.starts_with(['[', '(']) {
                    self.arbitrary(value)?
                } else if named_value(value) {
                    value.to_owned()
                } else {
                    return None;
                };
                let name_end = raw
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(raw.len());
                let condition = if raw[name_end..].trim_start().starts_with('(') {
                    raw.clone()
                } else {
                    let raw = if raw.contains(':') {
                        raw.clone()
                    } else {
                        format!("{raw}: var(--tw)")
                    };
                    if raw.starts_with('(') && raw.ends_with(')') {
                        raw
                    } else {
                        format!("({raw})")
                    }
                };
                Some((
                    (rank, raw),
                    Variant {
                        branches: vec![(vec![format!("@supports {condition}")], None)],
                        ..Variant::default()
                    },
                ))
            }
            "min" | "max" => {
                let width = match self.arbitrary(value) {
                    Some(raw) => raw,
                    None => self.raw(&format!("--breakpoint-{value}"))?,
                };
                self.media(if root == "min" { ">=" } else { "<" }, &width, root)
            }
            _ => {
                let width = match self.arbitrary(value) {
                    Some(raw) => raw,
                    None => self.raw(&format!("--container-{value}"))?,
                };
                let name = match modifier {
                    Some(name) if named_value(name) => format!("{name} "),
                    Some(_) => return None,
                    None => String::new(),
                };
                let operator = if root == "@max" { "<" } else { ">=" };
                let ((_, key), _) = self.media(operator, &width, root)?;
                Some((
                    (rank, format!("{key}{name}")),
                    Variant {
                        branches: vec![(
                            vec![format!("@container {name}(width {operator} {width})")],
                            None,
                        )],
                        ..Variant::default()
                    },
                ))
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
        let key = if operator == "<" {
            1e6 - pixels
        } else {
            pixels
        };
        Some((
            (
                VARIANTS.iter().position(|(name, _)| *name == root)? as u16,
                format!("{key:012.3}"),
            ),
            Variant {
                branches: vec![(vec![format!("@media (width {operator} {width})")], None)],
                ..Variant::default()
            },
        ))
    }

    fn removed(&self, name: &str) -> bool {
        self.cleared.iter().any(|cleared| removes(cleared, name))
    }

    fn raw(&self, name: &str) -> Option<String> {
        if let Some(value) = self.theme.get(name) {
            return Some(value.clone());
        }
        if self.removed(name) {
            return None;
        }
        DEFAULTS.with(|defaults| defaults.get(name).map(|value| (*value).to_owned()))
    }

    fn theme(&self, namespace: &str, key: &str) -> Option<String> {
        let name = format!("{namespace}-{key}");
        let name = if self.raw(&name).is_none() && key.contains('.') {
            format!("{namespace}-{}", key.replace('.', "_"))
        } else {
            name
        };
        if kept(&name, namespace) {
            return None;
        }
        if self.inline.contains(&name) || INLINE.contains(&name.as_str()) {
            return self.raw(&name);
        }
        self.raw(&name).map(|_| format!("var({name})"))
    }

    fn custom(&self, class: &str) -> Option<Body> {
        let (candidate, modifier) = modifier(class);
        self.utilities
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, (name, body))| {
                let value = match name.strip_suffix('*') {
                    Some(prefix) => Some(
                        candidate
                            .strip_prefix(prefix)
                            .filter(|value| !value.is_empty())?,
                    ),
                    None if name == class => None,
                    None => return None,
                };
                let key = (index, value.map(str::to_owned), modifier.map(str::to_owned));
                if let Some(body) = self.expanded.borrow().get(&key) {
                    return body.clone();
                }
                // Bound recursive @apply expansion to stop cycles and deep chains.
                let expanding = self.expanding.borrow().len();
                if expanding > 16 || self.expanding.borrow().contains(&index) {
                    self.cut.set(true);
                    return None;
                }
                let outer = self.cut.replace(false);
                self.expanding.borrow_mut().push(index);
                let mut resolved = false;
                let body = self
                    .expand(body, value, modifier, &mut resolved, 0)
                    .filter(|_| value.is_none() || resolved);
                self.expanding.borrow_mut().truncate(expanding);
                if !self.cut.get() {
                    let mut expanded = self.expanded.borrow_mut();
                    if expanded.len() >= 4096 {
                        expanded.clear();
                    }
                    expanded.insert(key, body.clone());
                }
                self.cut.set(outer || self.cut.get());
                body
            })
    }

    fn expand(
        &self,
        source: &str,
        value: Option<&str>,
        modifier: Option<&str>,
        resolved: &mut bool,
        depth: u8,
    ) -> Option<Body> {
        if depth > 8 {
            return None;
        }
        let mut body = Body::default();
        // Cap @apply output: multiple utilities can multiply at every level.
        let mut size = 0;
        let weigh = |pairs: &Pairs| {
            pairs
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
        };
        for (prelude, block) in statements(source) {
            if size > 1 << 16 {
                return None;
            }
            let Some(inner) = block else {
                if let Some(classes) = prelude.strip_prefix("@apply") {
                    let mut compiled: Vec<_> = classes
                        .split_ascii_whitespace()
                        .filter_map(|class| self.compile(class, "&"))
                        .collect();
                    compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                    for (at, selector, pairs) in compiled.into_iter().flat_map(|c| c.rules) {
                        size += weigh(&pairs) + selector.len();
                        if at.is_empty() && selector == "&" {
                            body.declarations.extend(pairs);
                        } else {
                            body.nested.push((at, selector, pairs));
                        }
                    }
                    continue;
                }
                let Some((property, raw)) = prelude.split_once(':') else {
                    continue;
                };
                let mut raw = raw.trim().to_owned();
                let mut valid = true;
                // Never scan substituted text again: it comes from the class and could hold more calls.
                let mut from = 0;
                while let Some(((start, open, close), function, input)) =
                    [("--value", value), ("--modifier", modifier)]
                        .into_iter()
                        .filter_map(|(function, input)| {
                            Some((call(&raw[from..], function)?, function, input))
                        })
                        .min_by_key(|((start, ..), ..)| *start)
                {
                    let (start, open, close) = (from + start, from + open, from + close);
                    let result = (close < raw.len())
                        .then(|| input.and_then(|input| self.resolve(&raw[open..close], input)))
                        .flatten();
                    let Some(result) = result else {
                        valid = false;
                        break;
                    };
                    raw.replace_range(start..=close, &result);
                    from = start + result.len();
                    *resolved |= function == "--value";
                }
                if valid {
                    body.declarations
                        .push((property.trim().to_owned(), self.functions(&raw)));
                }
                continue;
            };
            let inner = self.expand(inner, value, modifier, resolved, depth + 1)?;
            let (at, selector) = if prelude.starts_with('@') {
                (vec![prelude.to_owned()], "&".to_owned())
            } else if prelude.contains('&') {
                (Vec::new(), prelude.to_owned())
            } else {
                (Vec::new(), format!("& {prelude}"))
            };
            if !inner.declarations.is_empty() {
                size += weigh(&inner.declarations) + selector.len();
                body.nested
                    .push((at.clone(), selector.clone(), inner.declarations));
            }
            for (nested, template, pairs) in inner.nested {
                size +=
                    weigh(&pairs) + template.len() + template.matches('&').count() * selector.len();
                if size > 1 << 16 {
                    return None;
                }
                let mut rules = at.clone();
                rules.extend(nested);
                body.nested
                    .push((rules, template.replace('&', &selector), pairs));
            }
        }
        (size <= 1 << 16).then_some(body)
    }

    fn resolve(&self, arguments: &str, value: &str) -> Option<String> {
        split(arguments, ',')
            .into_iter()
            .map(str::trim)
            .find_map(|argument| {
                if let Some(namespace) = argument
                    .strip_suffix("-*")
                    .filter(|name| name.starts_with("--"))
                {
                    return named_value(value)
                        .then(|| self.theme(namespace, value))
                        .flatten();
                }
                if let Some(literal) = argument.strip_prefix('"').and_then(|a| a.strip_suffix('"'))
                {
                    return (literal == value).then(|| value.to_owned());
                }
                match (
                    argument.strip_prefix('[').and_then(|a| a.strip_suffix(']')),
                    self.typed(value),
                ) {
                    (Some(kind), Some((hint, raw))) => (kind == "*"
                        || hint.as_deref() == Some(kind)
                        || hint.is_none() && is_type(kind, &raw))
                    .then_some(raw),
                    (None, None) => {
                        (named_value(value) && is_type(argument, value)).then(|| value.to_owned())
                    }
                    _ => None,
                }
            })
    }

    fn arbitrary(&self, value: &str) -> Option<String> {
        let raw = match value
            .strip_prefix('[')
            .and_then(|raw| raw.strip_suffix(']'))
        {
            Some(raw) => decode(raw),
            None => {
                let raw = value.strip_prefix('(')?.strip_suffix(')')?;
                let (kind, name) = match split(raw, ':').as_slice() {
                    [kind, name] => (Some(*kind), *name),
                    _ => (None, raw),
                };
                if !name.starts_with("--") || !valid(name) {
                    return None;
                }
                decode(&match kind {
                    Some(kind) => format!("{kind}:var({name})"),
                    None => format!("var({name})"),
                })
            }
        };
        let raw = self.functions(&raw);
        (valid(&raw) && !raw.trim().is_empty()).then_some(raw)
    }

    fn typed(&self, value: &str) -> Option<(Option<String>, String)> {
        let raw = self.arbitrary(value)?;
        let end = raw
            .find(|c: char| !(c == '-' || c.is_ascii_lowercase()))
            .unwrap_or(raw.len());
        if raw[end..].starts_with(':') {
            let (hint, rest) = (&raw[..end], &raw[end + 1..]);
            if hint.is_empty() || rest.trim().is_empty() {
                return None;
            }
            return Some((Some(hint.to_owned()), rest.to_owned()));
        }
        Some((None, raw))
    }

    fn functions(&self, value: &str) -> String {
        if !calls(value) {
            return value.to_owned();
        }
        // Resolve innermost calls in one pass. Repeated scans of failed calls can be exponential.
        let mut out = String::with_capacity(value.len());
        let mut open: Vec<(usize, Option<&str>)> = Vec::new();
        // Each call reads all of its arguments, so calls nested past this depth stay as written.
        let mut nested = 0;
        let (mut quote, mut chars) = (None, value.chars());
        while let Some(character) = chars.next() {
            out.push(character);
            match character {
                '\\' => out.extend(chars.next()),
                _ if quote.is_some() => {
                    if quote == Some(character) {
                        quote = None;
                    }
                }
                '"' | '\'' => quote = Some(character),
                '(' => {
                    let before = &out[..out.len() - 1];
                    let function = ["--spacing", "--alpha", "--theme", "theme"]
                        .into_iter()
                        .find(|name| {
                            before.strip_suffix(name).is_some_and(|rest| {
                                *name != "theme"
                                    || !rest.ends_with(|c: char| c == '-' || c.is_alphanumeric())
                            })
                        })
                        .filter(|_| nested < 16);
                    nested += usize::from(function.is_some());
                    open.push((before.len() - function.map_or(0, str::len), function));
                }
                ')' => {
                    if let Some((start, Some(function))) = open.pop() {
                        nested -= 1;
                        let arguments = &out[start + function.len() + 1..out.len() - 1];
                        if let Some(result) = self.function(function, arguments.trim()) {
                            out.truncate(start);
                            out.push_str(&result);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn function(&self, function: &str, arguments: &str) -> Option<String> {
        match function {
            "--spacing" => {
                let base = self.raw("--spacing")?;
                let base = match self.inline.contains("--spacing") {
                    true => base,
                    false => "var(--spacing)".into(),
                };
                Some(match arguments {
                    "0" => "0px".into(),
                    "1" => base,
                    _ => format!("calc({base} * {arguments})"),
                })
            }
            "--alpha" => {
                let [color, alpha] = split(arguments, '/')[..] else {
                    return None;
                };
                Some(with_alpha(color.trim(), alpha.trim()))
            }
            "--theme" => {
                let (name, fallback) = match split(arguments, ',').as_slice() {
                    [name] => (*name, None),
                    [name, rest @ ..] => (*name, Some(rest.join(",").trim().to_owned())),
                    _ => return None,
                };
                let (name, inline) = match name.trim().strip_suffix(" inline") {
                    Some(name) => (name.trim(), true),
                    None => (name.trim(), false),
                };
                match self.raw(name) {
                    Some(value) if inline || self.inline.contains(name) => Some(value),
                    Some(_) => Some(match fallback {
                        Some(fallback) => format!("var({name}, {fallback})"),
                        None => format!("var({name})"),
                    }),
                    None => fallback,
                }
            }
            _ => {
                // Author CSS separates `spacing` and `.4` with `/**/` when it is serialized.
                let arguments = arguments.replace("/**/", "");
                let (path, alpha) = match split(&arguments, '/').as_slice() {
                    [path] => (path.trim(), None),
                    [path, alpha] => (path.trim(), Some(alpha.trim())),
                    _ => return None,
                };
                let value = if path.starts_with("--") {
                    self.raw(path)?
                } else {
                    let (namespace, key) = path.split_once('.')?;
                    if namespace == "spacing" {
                        let base = self.raw("--spacing")?;
                        let number: f64 = key.parse().ok()?;
                        let unit =
                            base.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
                        let scale: f64 = base[..base.len() - unit.len()].parse().ok()?;
                        format!("{}{unit}", scale * number)
                    } else {
                        let namespace = match namespace {
                            "colors" => "--color",
                            "fontSize" => "--text",
                            "fontFamily" => "--font",
                            "fontWeight" => "--font-weight",
                            "letterSpacing" => "--tracking",
                            "lineHeight" => "--leading",
                            "borderRadius" => "--radius",
                            "boxShadow" => "--shadow",
                            "screens" => "--breakpoint",
                            "maxWidth" | "width" | "containers" => "--container",
                            "blur" => "--blur",
                            "animation" => "--animate",
                            "transitionTimingFunction" => "--ease",
                            _ => return None,
                        };
                        let key = key.replace('.', "-");
                        self.raw(&format!("{namespace}-{key}"))
                            .or_else(|| self.raw(&format!("--spacing-{key}")))?
                    }
                };
                Some(match alpha {
                    Some(alpha) => with_alpha(&value, alpha),
                    None => value,
                })
            }
        }
    }

    fn utility(&self, class: &str) -> Option<Body> {
        if let Some(body) = self.custom(class) {
            return Some(body);
        }
        if let Some(raw) = class
            .strip_prefix('[')
            .and_then(|raw| raw.strip_suffix(']'))
        {
            let (property, value) = raw.split_once(':')?;
            if !property.starts_with(|c: char| c == '-' || c.is_ascii_lowercase())
                || !property
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            let value = self.functions(&decode(value));
            return (valid(&value) && !value.trim().is_empty())
                .then(|| Body::new(&[(property, value)]))
                .flatten();
        }
        if let Ok(index) = STATIC.binary_search_by(|(name, _)| (*name).cmp(class)) {
            return Some(Body::parse(STATIC[index].1));
        }
        let (negative, class) = match class.strip_prefix('-') {
            Some(class) => (true, class),
            None => (false, class),
        };
        let (class, modifier) = modifier(class);
        if modifier
            .is_some_and(|modifier| !named_value(modifier) && !modifier.starts_with(['[', '(']))
        {
            return None;
        }
        let functional = |root: &str, value: Option<&str>| {
            (!negative || NEGATIVE.contains(&root))
                .then(|| self.functional(root, value, modifier, negative))
                .flatten()
        };
        if matches!(
            class,
            "bg-glass" | "bg-glass-clear" | "bg-glass-interactive"
        ) {
            return None;
        }
        if let Some(value) = class.strip_prefix("bg-glass-") {
            return functional("bg-glass", Some(value));
        }
        if class.ends_with(']') || class.ends_with(')') {
            let open = if class.ends_with(']') { "-[" } else { "-(" };
            let index = class.find(open)?;
            return functional(&class[..index], Some(&class[index + 1..]));
        }
        // Scan valid suffix boundaries once: checking each suffix again is quadratic.
        let named = class
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '%' | '-')))
            .map_or(0, |index| index + 1);
        functional(class, None).or_else(|| {
            class
                .match_indices('-')
                .rev()
                .take_while(|(index, _)| *index >= named)
                .filter(|(index, _)| *index > 0 && index + 1 < class.len())
                .find_map(|(index, _)| functional(&class[..index], Some(&class[index + 1..])))
        })
    }

    fn spacing(&self, value: &str) -> Option<String> {
        if value.starts_with(['[', '(']) {
            return self.typed(value).map(|(_, raw)| raw);
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
        .then(|| self.function("--spacing", value))
        .flatten()
    }

    fn color(&self, value: &str, modifier: Option<&str>) -> Option<String> {
        let color = if value.starts_with(['[', '(']) {
            let (kind, raw) = self.typed(value)?;
            if kind.as_deref().is_some_and(|kind| kind != "color")
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
        let alpha = self.opacity(modifier)?;
        (color != "inherit").then(|| with_alpha(&color, &alpha))
    }

    fn opacity(&self, modifier: &str) -> Option<String> {
        if modifier.starts_with(['[', '(']) {
            return self.arbitrary(modifier);
        }
        let number: f64 = modifier.parse().ok()?;
        ((0. ..=100.).contains(&number) && (number * 4.).fract() == 0.)
            .then(|| format!("{modifier}%"))
    }

    fn shadow(&self, value: &str, modifier: Option<&str>, alpha: &str) -> Option<String> {
        if matches!(value, "initial" | "inherit") && modifier.is_none() {
            return Some(value.into());
        }
        let color = self.color(value, modifier)?;
        Some(format!(
            "color-mix(in oklab, {color} var(--tw-{alpha}-alpha), transparent)"
        ))
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
                return value;
            }
            if let Some(number) = value.strip_prefix("calc(var(--spacing) * ") {
                return format!("calc(var(--spacing) * -{number}");
            }
            match value.as_str() {
                "var(--spacing)" => "calc(var(--spacing) * -1)".into(),
                _ => format!("calc({value} * -1)"),
            }
        };
        let typed = value.and_then(|value| {
            value
                .starts_with(['[', '('])
                .then(|| self.typed(value))
                .flatten()
        });
        let raw = typed.as_ref().map(|(_, raw)| raw.clone());
        let hint = typed.as_ref().and_then(|(hint, _)| hint.as_deref());
        let bare = value.filter(|value| !value.starts_with(['[', '(']));
        let spaced = |value: String| match bare {
            Some("0") => value,
            Some("px") if negative => "-1px".into(),
            Some("full") if negative => "-100%".into(),
            _ => negate(value),
        };
        if value.is_some_and(|value| value.starts_with(['[', '('])) && typed.is_none() {
            return None;
        }
        let fraction = |spaces: bool| {
            let (value, modifier) = (bare?, modifier?);
            (integer(value) && integer(modifier) && modifier != "0").then(|| {
                if spaces {
                    format!("calc({value} / {modifier} * 100%)")
                } else {
                    format!("calc({value}/{modifier} * 100%)")
                }
            })
        };
        let number = |unit: &str| {
            let value = bare?;
            (integer(value)
                || value
                    .parse::<f64>()
                    .is_ok_and(|n| n >= 0. && (n * 4.).fract() == 0.))
            .then(|| format!("{value}{unit}"))
        };
        let width = |default: &str| match value {
            None => Some(default.to_owned()),
            Some(_) => match hint {
                Some("length" | "line-width" | "number" | "percentage") => raw.clone(),
                Some(_) => None,
                None => match &raw {
                    Some(raw) => is_length(raw).then(|| raw.clone()),
                    None => bare
                        .filter(|value| integer(value))
                        .map(|value| format!("{value}px")),
                },
            },
        };
        let filter = |name: &str, unit: &str, default: Option<&str>| {
            let value = match (value, &raw) {
                (None, _) => default?.to_owned(),
                (_, Some(raw)) => raw.clone(),
                _ => number(unit)?,
            };
            let backdrop = root.starts_with("backdrop-");
            let property = format!("--tw-{root}");
            let value = format!("{name}({})", negate(value));
            if backdrop {
                return Body::new(&[
                    (&property, value),
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ]);
            }
            Body::new(&[(&property, value), ("filter", FILTER.into())])
        };
        if modifier.is_some()
            && !matches!(
                root,
                "w" | "min-w"
                    | "max-w"
                    | "h"
                    | "min-h"
                    | "max-h"
                    | "size"
                    | "inset"
                    | "inset-x"
                    | "inset-y"
                    | "inset-s"
                    | "inset-e"
                    | "inset-bs"
                    | "inset-be"
                    | "start"
                    | "end"
                    | "top"
                    | "right"
                    | "bottom"
                    | "left"
                    | "translate"
                    | "translate-x"
                    | "translate-y"
                    | "inline"
                    | "min-inline"
                    | "max-inline"
                    | "block"
                    | "min-block"
                    | "max-block"
                    | "basis"
                    | "flex"
                    | "aspect"
                    | "text"
                    | "bg"
                    | "bg-glass"
                    | "bg-linear"
                    | "bg-radial"
                    | "bg-conic"
                    | "from"
                    | "via"
                    | "to"
                    | "border"
                    | "border-x"
                    | "border-y"
                    | "border-s"
                    | "border-e"
                    | "border-bs"
                    | "border-be"
                    | "border-t"
                    | "border-r"
                    | "border-b"
                    | "border-l"
                    | "divide"
                    | "divide-x"
                    | "divide-y"
                    | "outline"
                    | "ring"
                    | "inset-ring"
                    | "ring-offset"
                    | "shadow"
                    | "inset-shadow"
                    | "drop-shadow"
                    | "text-shadow"
                    | "decoration"
                    | "accent"
                    | "caret"
                    | "fill"
                    | "stroke"
                    | "placeholder"
                    | "scrollbar-thumb"
                    | "scrollbar-track"
                    | "mask-linear-from"
                    | "mask-linear-to"
                    | "mask-radial-from"
                    | "mask-radial-to"
                    | "mask-conic-from"
                    | "mask-conic-to"
                    | "mask-x-from"
                    | "mask-x-to"
                    | "mask-y-from"
                    | "mask-y-to"
                    | "mask-t-from"
                    | "mask-t-to"
                    | "mask-r-from"
                    | "mask-r-to"
                    | "mask-b-from"
                    | "mask-b-to"
                    | "mask-l-from"
                    | "mask-l-to"
                    | "@container"
            )
        {
            return None;
        }
        match root {
            "w" | "min-w" | "max-w" | "h" | "min-h" | "max-h" | "size" | "inline"
            | "min-inline" | "max-inline" | "block" | "min-block" | "max-block" => {
                let value = value?;
                let max = root.starts_with("max");
                let vertical = root.ends_with("block") || root.ends_with('h');
                let inline = root.ends_with("inline");
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "full" => "100%".into(),
                        "auto" if !max => "auto".into(),
                        "none" if max => "none".into(),
                        "px" => "1px".into(),
                        "screen" if root != "size" && root != "max-w" => {
                            if vertical { "100vh" } else { "100vw" }.into()
                        }
                        "dvw" | "svw" | "lvw" if !root.ends_with("block") => format!("100{value}"),
                        "dvh" | "svh" | "lvh" if !inline => format!("100{value}"),
                        "min" => "min-content".into(),
                        "max" => "max-content".into(),
                        "fit" => "fit-content".into(),
                        "lh" if vertical => "1lh".into(),
                        "prose" if root == "max-w" => "65ch".into(),
                        _ => (!vertical && root != "size")
                            .then(|| self.theme("--container", value))
                            .flatten()
                            .or_else(|| {
                                (root == "max-w")
                                    .then(|| {
                                        self.theme("--breakpoint", value.strip_prefix("screen-")?)
                                    })
                                    .flatten()
                            })
                            .or_else(|| self.spacing(value))?,
                    },
                };
                let properties: &[&str] = match root {
                    "w" => &["width"],
                    "min-w" => &["min-width"],
                    "max-w" => &["max-width"],
                    "h" => &["height"],
                    "min-h" => &["min-height"],
                    "max-h" => &["max-height"],
                    "size" => &["width", "height"],
                    "inline" => &["inline-size"],
                    "min-inline" => &["min-inline-size"],
                    "max-inline" => &["max-inline-size"],
                    "block" => &["block-size"],
                    "min-block" => &["min-block-size"],
                    _ => &["max-block-size"],
                };
                Body::new(
                    &properties
                        .iter()
                        .map(|property| (*property, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "inset" | "inset-x" | "inset-y" | "inset-s" | "inset-e" | "inset-bs" | "inset-be"
            | "start" | "end" | "top" | "right" | "bottom" | "left" => {
                let value = value?;
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "auto" if !negative => "auto".into(),
                        "full" => "100%".into(),
                        // Tailwind keeps the old spacing for the deprecated `start-*` and `end-*`.
                        _ if matches!(root, "start" | "end") && value != "px" => {
                            let base = self.function("--spacing", "1")?;
                            self.spacing(value)?;
                            let sign = if negative { "-" } else { "" };
                            return Body::new(&[(
                                if root == "start" {
                                    "inset-inline-start"
                                } else {
                                    "inset-inline-end"
                                },
                                format!("calc({base} * {sign}{value})"),
                            )]);
                        }
                        _ => self.spacing(value)?,
                    },
                };
                let resolved = if raw.is_some() || modifier.is_some() {
                    negate(resolved)
                } else {
                    spaced(resolved)
                };
                let property = match root {
                    "inset-x" => "inset-inline",
                    "inset-y" => "inset-block",
                    "start" | "inset-s" => "inset-inline-start",
                    "end" | "inset-e" => "inset-inline-end",
                    "inset-bs" => "inset-block-start",
                    "inset-be" => "inset-block-end",
                    _ => root,
                };
                Body::new(&[(property, resolved)])
            }
            "m" | "mx" | "my" | "ms" | "me" | "mbs" | "mbe" | "mt" | "mr" | "mb" | "ml" | "p"
            | "px" | "py" | "ps" | "pe" | "pbs" | "pbe" | "pt" | "pr" | "pb" | "pl"
            | "scroll-m" | "scroll-mx" | "scroll-my" | "scroll-ms" | "scroll-me" | "scroll-mbs"
            | "scroll-mbe" | "scroll-mt" | "scroll-mr" | "scroll-mb" | "scroll-ml" | "scroll-p"
            | "scroll-px" | "scroll-py" | "scroll-ps" | "scroll-pe" | "scroll-pbs"
            | "scroll-pbe" | "scroll-pt" | "scroll-pr" | "scroll-pb" | "scroll-pl" => {
                let value = value?;
                let (scroll, short) = match root.strip_prefix("scroll-") {
                    Some(short) => ("scroll-", short),
                    None => ("", root),
                };
                let margin = short.starts_with('m');
                let resolved = match value {
                    "auto" if margin && !negative && scroll.is_empty() => "auto".into(),
                    _ => self.spacing(value)?,
                };
                let name = if margin { "margin" } else { "padding" };
                let side = match &short[1..] {
                    "" => "",
                    "x" => "-inline",
                    "y" => "-block",
                    "s" => "-inline-start",
                    "e" => "-inline-end",
                    "bs" => "-block-start",
                    "be" => "-block-end",
                    "t" => "-top",
                    "r" => "-right",
                    "b" => "-bottom",
                    _ => "-left",
                };
                Body::new(&[(&format!("{scroll}{name}{side}"), spaced(resolved))])
            }
            "gap" | "gap-x" | "gap-y" => {
                let property = match root {
                    "gap-x" => "column-gap",
                    "gap-y" => "row-gap",
                    _ => "gap",
                };
                Body::new(&[(property, self.spacing(value?)?)])
            }
            "space-x" | "space-y" => {
                let value = spaced(self.spacing(value?)?);
                let axis = &root[6..];
                let (start, end) = if axis == "x" {
                    ("margin-inline-start", "margin-inline-end")
                } else {
                    ("margin-block-start", "margin-block-end")
                };
                let reverse = format!("--tw-space-{axis}-reverse");
                let (start_value, end_value) = if value == "0px" {
                    ("0".into(), "0".into())
                } else {
                    (
                        format!("calc({value} * var({reverse}))"),
                        format!("calc({value} * calc(1 - var({reverse})))"),
                    )
                };
                Some(Body {
                    nested: vec![(
                        Vec::new(),
                        ":where(& > :not(:last-child))".into(),
                        vec![
                            (reverse, "0".into()),
                            (start.into(), start_value),
                            (end.into(), end_value),
                        ],
                    )],
                    sort: Some(if axis == "x" { "column-gap" } else { "row-gap" }),
                    ..Body::default()
                })
            }
            "space-x-reverse" | "space-y-reverse" | "divide-x-reverse" | "divide-y-reverse"
                if value.is_none() =>
            {
                let name = root.strip_suffix("-reverse")?;
                Some(Body::default().nest(
                    &[],
                    ":where(& > :not(:last-child))",
                    &format!("--tw-{name}-reverse:1"),
                ))
            }
            "divide" | "divide-x" | "divide-y" => {
                let nested = ":where(& > :not(:last-child))";
                if root == "divide" {
                    if let Some(style) = bare.filter(|style| {
                        matches!(*style, "solid" | "dashed" | "dotted" | "double" | "none")
                    }) {
                        return Some(Body::default().nest(
                            &[],
                            nested,
                            &format!("--tw-border-style:{style};border-style:{style}"),
                        ));
                    }
                    let color = self.color(value?, modifier)?;
                    return Some(Body::default().nest(
                        &[],
                        nested,
                        &format!("border-color:{color}"),
                    ));
                }
                let width = width("1px").filter(|_| modifier.is_none())?;
                let axis = &root[7..];
                let reverse = format!("--tw-divide-{axis}-reverse");
                let source = if axis == "x" {
                    format!(
                        "{reverse}:0;border-inline-style:var(--tw-border-style);border-inline-start-width:calc({width} * var({reverse}));border-inline-end-width:calc({width} * calc(1 - var({reverse})))"
                    )
                } else {
                    format!(
                        "{reverse}:0;border-bottom-style:var(--tw-border-style);border-top-style:var(--tw-border-style);border-top-width:calc({width} * var({reverse}));border-bottom-width:calc({width} * calc(1 - var({reverse})))"
                    )
                };
                let mut body = Body::default().nest(&[], nested, &source);
                body.sort = Some(if axis == "x" {
                    "divide-x-width"
                } else {
                    "divide-y-width"
                });
                Some(body)
            }
            "translate" | "translate-x" | "translate-y" | "translate-z" => {
                let value = value?;
                if root == "translate" && bare == Some("none") {
                    return Body::new(&[("translate", "none".into())]);
                }
                if root == "translate" && bare == Some("3d") {
                    return Body::new(&[(
                        "translate",
                        "var(--tw-translate-x) var(--tw-translate-y) var(--tw-translate-z)".into(),
                    )]);
                }
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => negate(raw.clone()),
                    (_, Some(_)) => negate(fraction(true)?),
                    _ if value == "full" => spaced("100%".into()),
                    _ => spaced(self.spacing(value)?),
                };
                let axes: &[&str] = match root {
                    "translate" => &["x", "y"],
                    "translate-x" => &["x"],
                    "translate-y" => &["y"],
                    _ => &["z"],
                };
                let mut body: Pairs = axes
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
                if root == "scale" {
                    match bare {
                        Some("none") => return Body::new(&[("scale", "none".into())]),
                        Some("3d") => {
                            return Body::new(&[(
                                "scale",
                                "var(--tw-scale-x) var(--tw-scale-y) var(--tw-scale-z)".into(),
                            )]);
                        }
                        _ => {}
                    }
                    if let Some(raw) = &raw {
                        return Body::new(&[("scale", negate(raw.clone()))]);
                    }
                }
                let resolved = negate(match &raw {
                    Some(raw) => raw.clone(),
                    None => integer(value).then(|| format!("{value}%"))?,
                });
                let axes: &[&str] = match root {
                    "scale" => &["x", "y", "z"],
                    "scale-x" => &["x"],
                    "scale-y" => &["y"],
                    _ => &["z"],
                };
                let mut body: Pairs = axes
                    .iter()
                    .map(|axis| (format!("--tw-scale-{axis}"), resolved.clone()))
                    .collect();
                body.push((
                    "scale".into(),
                    if root == "scale-z" {
                        "var(--tw-scale-x) var(--tw-scale-y) var(--tw-scale-z)"
                    } else {
                        "var(--tw-scale-x) var(--tw-scale-y)"
                    }
                    .into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "rotate" | "rotate-x" | "rotate-y" | "rotate-z" | "skew" | "skew-x" | "skew-y" => {
                if root == "rotate" && bare == Some("none") {
                    return Body::new(&[("rotate", "none".into())]);
                }
                let resolved = negate(match &raw {
                    Some(raw) => raw.clone(),
                    None => number("deg")?,
                });
                if root == "rotate" {
                    return Body::new(&[("rotate", resolved)]);
                }
                let mut body: Pairs = match root {
                    "skew" => vec![
                        ("--tw-skew-x".into(), format!("skewX({resolved})")),
                        ("--tw-skew-y".into(), format!("skewY({resolved})")),
                    ],
                    _ => {
                        let axis = &root[root.len() - 1..];
                        let function = if root.starts_with("skew") {
                            "skew"
                        } else {
                            "rotate"
                        };
                        vec![(
                            format!("--tw-{root}"),
                            format!("{function}{}({resolved})", axis.to_ascii_uppercase()),
                        )]
                    }
                };
                body.push(("transform".into(), TRANSFORM.into()));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "transform" => match (value, &raw) {
                (None, _) => Body::new(&[("transform", TRANSFORM.into())]),
                (_, Some(raw)) => Body::new(&[("transform", raw.clone())]),
                _ => None,
            },
            "perspective" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "none".into(),
                    (Some(value), _) => self.theme("--perspective", value)?,
                    _ => return None,
                };
                Body::new(&[("perspective", resolved)])
            }
            "origin" | "perspective-origin" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => ORIGINS
                        .iter()
                        .find(|(name, _)| *name == value)
                        .map(|(_, position)| (*position).to_owned())?,
                    _ => return None,
                };
                let property = if root == "origin" {
                    "transform-origin"
                } else {
                    root
                };
                Body::new(&[(property, resolved)])
            }
            "z" | "order" | "col" | "col-start" | "col-end" | "row" | "row-start" | "row-end"
            | "col-span" | "row-span" | "columns" | "tab" | "line-clamp" | "grow" | "shrink"
            | "flex-grow" | "flex-shrink" => {
                let span = root.ends_with("-span");
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) if span => format!("span {raw} / span {raw}"),
                    (_, Some(raw)) => raw.clone(),
                    (None, _)
                        if matches!(root, "grow" | "shrink" | "flex-grow" | "flex-shrink") =>
                    {
                        "1".into()
                    }
                    (Some("auto"), _)
                        if matches!(
                            root,
                            "z" | "col"
                                | "col-start"
                                | "col-end"
                                | "row"
                                | "row-start"
                                | "row-end"
                                | "columns"
                        ) =>
                    {
                        "auto".into()
                    }
                    (Some("first"), _) if root == "order" => "-9999".into(),
                    (Some("last"), _) if root == "order" => "9999".into(),
                    (Some("none"), _) if root == "order" => "0".into(),
                    (Some("full"), _) if span => "1 / -1".into(),
                    (Some(value), _) if integer(value) => {
                        if span {
                            format!("span {value} / span {value}")
                        } else {
                            value.to_owned()
                        }
                    }
                    (Some(value), _) if root == "columns" => self.theme("--container", value)?,
                    _ => return None,
                };
                if root == "line-clamp" {
                    if bare == Some("none") {
                        return Body::new(&[
                            ("overflow", "visible".into()),
                            ("display", "block".into()),
                            ("-webkit-box-orient", "horizontal".into()),
                            ("-webkit-line-clamp", "unset".into()),
                        ]);
                    }
                    return Body::new(&[
                        ("overflow", "hidden".into()),
                        ("display", "-webkit-box".into()),
                        ("-webkit-box-orient", "vertical".into()),
                        ("-webkit-line-clamp", resolved),
                    ]);
                }
                let property = match root {
                    "z" => "z-index",
                    "col" | "col-span" => "grid-column",
                    "col-start" => "grid-column-start",
                    "col-end" => "grid-column-end",
                    "row" | "row-span" => "grid-row",
                    "row-start" => "grid-row-start",
                    "row-end" => "grid-row-end",
                    "tab" => "tab-size",
                    "grow" | "flex-grow" => "flex-grow",
                    "shrink" | "flex-shrink" => "flex-shrink",
                    _ => root,
                };
                Body::new(&[(property, negate(resolved))])
            }
            "grid-cols" | "grid-rows" | "auto-cols" | "auto-rows" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none" | "subgrid"), _) if root.starts_with("grid") => bare?.into(),
                    (Some(value), _)
                        if root.starts_with("grid") && integer(value) && value != "0" =>
                    {
                        format!("repeat({value}, minmax(0, 1fr))")
                    }
                    (Some("auto"), _) if root.starts_with("auto") => "auto".into(),
                    (Some("min"), _) if root.starts_with("auto") => "min-content".into(),
                    (Some("max"), _) if root.starts_with("auto") => "max-content".into(),
                    (Some("fr"), _) if root.starts_with("auto") => "minmax(0, 1fr)".into(),
                    _ => return None,
                };
                let property = match root {
                    "grid-cols" => "grid-template-columns",
                    "grid-rows" => "grid-template-rows",
                    "auto-cols" => "grid-auto-columns",
                    _ => "grid-auto-rows",
                };
                Body::new(&[(property, resolved)])
            }
            "flex" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    _ if modifier.is_some() => fraction(false)?,
                    (Some(value), _) if integer(value) => value.to_owned(),
                    _ => return None,
                };
                Body::new(&[("flex", resolved)])
            }
            "basis" => {
                let value = value?;
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "full" => "100%".into(),
                        "auto" => "auto".into(),
                        _ => self
                            .theme("--container", value)
                            .or_else(|| self.spacing(value))?,
                    },
                };
                Body::new(&[("flex-basis", resolved)])
            }
            "border-spacing" | "border-spacing-x" | "border-spacing-y" => {
                let resolved = self.spacing(value?)?;
                let mut body: Pairs = match root {
                    "border-spacing-x" => vec![("--tw-border-spacing-x".into(), resolved)],
                    "border-spacing-y" => vec![("--tw-border-spacing-y".into(), resolved)],
                    _ => vec![
                        ("--tw-border-spacing-x".into(), resolved.clone()),
                        ("--tw-border-spacing-y".into(), resolved),
                    ],
                };
                body.push((
                    "border-spacing".into(),
                    "var(--tw-border-spacing-x) var(--tw-border-spacing-y)".into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "indent" => Body::new(&[("text-indent", spaced(self.spacing(value?)?))]),
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
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (None, _) => self.raw("--radius")?,
                    (Some("none"), _) => "0".into(),
                    (Some("full"), _) => "calc(infinity * 1px)".into(),
                    (Some(value), _) => self.theme("--radius", value)?,
                };
                Body::new(
                    &corners
                        .iter()
                        .map(|corner| (*corner, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "border" | "border-x" | "border-y" | "border-s" | "border-e" | "border-bs"
            | "border-be" | "border-t" | "border-r" | "border-b" | "border-l" => {
                let side = match &root[6..] {
                    "" => "",
                    "-x" => "-inline",
                    "-y" => "-block",
                    "-s" => "-inline-start",
                    "-e" => "-inline-end",
                    "-bs" => "-block-start",
                    "-be" => "-block-end",
                    "-t" => "-top",
                    "-r" => "-right",
                    "-b" => "-bottom",
                    _ => "-left",
                };
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        (
                            &format!("border{side}-style"),
                            "var(--tw-border-style)".into(),
                        ),
                        (&format!("border{side}-width"), width),
                    ]);
                }
                Body::new(&[(
                    &format!("border{side}-color"),
                    self.color(value?, modifier)?,
                )])
            }
            "bg" => {
                let value = value?;
                if let Some(raw) = &raw {
                    let property = match hint {
                        Some("image" | "url") => Some("background-image"),
                        Some("length" | "size" | "bg-size") => Some("background-size"),
                        Some("position" | "percentage") => Some("background-position"),
                        Some("color") => None,
                        Some(_) => return None,
                        None if raw.starts_with("url(") || raw.contains("gradient(") => {
                            Some("background-image")
                        }
                        None if is_color(raw) || raw.starts_with("var(") => None,
                        None => Some("background-position"),
                    };
                    if let Some(property) = property {
                        return Body::new(&[(property, raw.clone())]);
                    }
                }
                Body::new(&[("background-color", self.color(value, modifier)?)])
            }
            "bg-size" | "bg-position" | "mask-size" | "mask-position" => {
                let property = match root {
                    "bg-size" => "background-size",
                    "bg-position" => "background-position",
                    _ => root,
                };
                Body::new(&[(property, raw?)])
            }
            "bg-linear" | "bg-gradient-to" | "bg-radial" | "bg-conic" => {
                let interpolation = match modifier {
                    None => None,
                    Some(modifier) if modifier.starts_with(['[', '(']) => {
                        Some(self.arbitrary(modifier)?)
                    }
                    Some(
                        modifier @ ("srgb" | "hsl" | "oklab" | "oklch" | "longer" | "shorter"
                        | "increasing" | "decreasing"),
                    ) => Some(match modifier {
                        "longer" | "shorter" | "increasing" | "decreasing" => {
                            format!("in oklch {modifier} hue")
                        }
                        _ => format!("in {modifier}"),
                    }),
                    _ => return None,
                };
                let kind = match root {
                    "bg-radial" => "radial",
                    "bg-conic" => "conic",
                    _ => "linear",
                };
                if let Some(raw) = &raw {
                    return Body::new(&[
                        ("--tw-gradient-position", raw.clone()),
                        (
                            "background-image",
                            format!("{kind}-gradient(var(--tw-gradient-stops,{raw}))"),
                        ),
                    ]);
                }
                let position = match (root, bare) {
                    ("bg-gradient-to", Some(side)) => {
                        let side = direction(side)?;
                        return Body::new(&[
                            ("--tw-gradient-position", format!("to {side} in oklab")),
                            (
                                "background-image",
                                "linear-gradient(var(--tw-gradient-stops))".into(),
                            ),
                        ]);
                    }
                    ("bg-linear", Some(side)) if side.starts_with("to-") => {
                        format!("to {}", direction(&side[3..])?)
                    }
                    ("bg-linear", Some(angle)) => {
                        negate(format!("{}deg", integer(angle).then_some(angle)?))
                    }
                    ("bg-conic", Some(angle)) => {
                        format!(
                            "from {}",
                            negate(format!("{}deg", integer(angle).then_some(angle)?))
                        )
                    }
                    ("bg-radial" | "bg-conic", None) => String::new(),
                    _ => return None,
                };
                let image = (
                    "background-image",
                    format!("{kind}-gradient(var(--tw-gradient-stops))"),
                );
                if kind == "linear" {
                    let interpolation = interpolation.unwrap_or_else(|| "in oklab".into());
                    let mut body = Body::new(&[("--tw-gradient-position", position.clone())])?;
                    body = body.nest(
                        &["@supports (background-image: linear-gradient(in lab, red, red))"],
                        "&",
                        &format!("--tw-gradient-position:{position} {interpolation}"),
                    );
                    body.nested
                        .push((Vec::new(), "&".into(), vec![(image.0.into(), image.1)]));
                    return Some(body);
                }
                let interpolation = interpolation.unwrap_or_else(|| "in oklab".into());
                let position = if position.is_empty() {
                    interpolation
                } else {
                    format!("{position} {interpolation}")
                };
                Body::new(&[("--tw-gradient-position", position), image])
            }
            "from" | "via" | "to" => {
                let value = value?;
                let position = match (bare, hint, &raw) {
                    (Some(value), ..) if value.ends_with('%') => {
                        let number = value.strip_suffix('%')?;
                        integer(number).then(|| value.to_owned())
                    }
                    (_, Some("length" | "percentage"), Some(raw)) => Some(raw.clone()),
                    (_, None, Some(raw)) if is_length(raw) => Some(raw.clone()),
                    _ => None,
                };
                if let Some(position) = position {
                    return Body::new(&[(&format!("--tw-gradient-{root}-position"), position)]);
                }
                if root == "via" && bare == Some("none") {
                    return Body::new(&[("--tw-gradient-via-stops", "initial".into())]);
                }
                let color = self.color(value, modifier)?;
                if root == "via" {
                    return Body::new(&[
                        ("--tw-gradient-via", color),
                        ("--tw-gradient-via-stops", "var(--tw-gradient-position), var(--tw-gradient-from) var(--tw-gradient-from-position), var(--tw-gradient-via) var(--tw-gradient-via-position), var(--tw-gradient-to) var(--tw-gradient-to-position)".into()),
                        ("--tw-gradient-stops", "var(--tw-gradient-via-stops)".into()),
                    ]);
                }
                Body::new(&[
                    (&format!("--tw-gradient-{root}"), color),
                    ("--tw-gradient-stops", STOPS.into()),
                ])
            }
            "mask" => Body::new(&[("mask-image", raw?)]),
            "mask-linear" | "mask-conic" | "mask-radial" => {
                let kind = &root[5..];
                let position = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(angle @ ("0" | "1")), None) if kind != "radial" => {
                        format!(
                            "{}{angle}deg",
                            if negative && angle == "1" { "-" } else { "" }
                        )
                    }
                    (Some(angle), None) if kind != "radial" && integer(angle) => {
                        format!("calc(1deg * {}{angle})", if negative { "-" } else { "" })
                    }
                    _ => return None,
                };
                let name = if kind == "radial" { "size" } else { "position" };
                let mut body = Body::parse(MASK);
                body.declarations.push((
                    format!("--tw-mask-{kind}"),
                    format!(
                        "{kind}-gradient(var(--tw-mask-{kind}-stops, var(--tw-mask-{kind}-{name})))"
                    ),
                ));
                body.declarations
                    .push((format!("--tw-mask-{kind}-{name}"), position));
                Some(body)
            }
            _ if root.starts_with("mask-")
                && (root.ends_with("-from") || root.ends_with("-to")) =>
            {
                let (edge, end) = root[5..].rsplit_once('-')?;
                let value = value?;
                let position = match (bare, hint, &raw) {
                    (_, Some("length" | "percentage"), Some(raw)) => Some(raw.clone()),
                    (_, None, Some(raw)) if is_length(raw) => Some(raw.clone()),
                    (Some(value), ..) if value.ends_with('%') => {
                        integer(value.strip_suffix('%')?).then(|| value.to_owned())
                    }
                    (Some(value), ..) => self.spacing(value),
                    _ => None,
                };
                let (field, assigned) = match position {
                    Some(position) => ("position", position),
                    None => ("color", self.color(value, modifier)?),
                };
                let mut body = Body::parse(MASK);
                let mut push = |name: String, value: String| body.declarations.push((name, value));
                match edge {
                    "linear" | "radial" | "conic" => {
                        let stops = match edge {
                            "linear" => "var(--tw-mask-linear-position), ",
                            "radial" => {
                                "var(--tw-mask-radial-shape) var(--tw-mask-radial-size) at var(--tw-mask-radial-position), "
                            }
                            _ => "from var(--tw-mask-conic-position), ",
                        };
                        push(
                            format!("--tw-mask-{edge}-stops"),
                            format!(
                                "{stops}var(--tw-mask-{edge}-from-color) var(--tw-mask-{edge}-from-position), var(--tw-mask-{edge}-to-color) var(--tw-mask-{edge}-to-position)"
                            ),
                        );
                        push(
                            format!("--tw-mask-{edge}"),
                            format!("{edge}-gradient(var(--tw-mask-{edge}-stops))"),
                        );
                        push(format!("--tw-mask-{edge}-{end}-{field}"), assigned);
                    }
                    _ => {
                        let sides: &[&str] = match edge {
                            "x" => &["right", "left"],
                            "y" => &["top", "bottom"],
                            "t" => &["top"],
                            "r" => &["right"],
                            "b" => &["bottom"],
                            "l" => &["left"],
                            _ => return None,
                        };
                        push("--tw-mask-linear".into(), EDGES.into());
                        for side in sides {
                            push(
                                format!("--tw-mask-{side}"),
                                format!(
                                    "linear-gradient(to {side}, var(--tw-mask-{side}-from-color) var(--tw-mask-{side}-from-position), var(--tw-mask-{side}-to-color) var(--tw-mask-{side}-to-position))"
                                ),
                            );
                            push(format!("--tw-mask-{side}-{end}-{field}"), assigned.clone());
                        }
                    }
                }
                Some(body)
            }
            "text" => {
                let value = value?;
                let size = match (hint, &raw) {
                    (
                        Some("length" | "percentage" | "absolute-size" | "relative-size"),
                        Some(raw),
                    ) => Some(raw.clone()),
                    (None, Some(raw))
                        if is_length(raw)
                            || matches!(
                                raw.as_str(),
                                "xx-small"
                                    | "x-small"
                                    | "small"
                                    | "medium"
                                    | "large"
                                    | "x-large"
                                    | "xx-large"
                                    | "xxx-large"
                                    | "larger"
                                    | "smaller"
                            ) =>
                    {
                        Some(raw.clone())
                    }
                    (None, None) => self.theme("--text", value),
                    _ => None,
                };
                let Some(size) = size else {
                    return Body::new(&[("color", self.color(value, modifier)?)]);
                };
                let mut body = vec![("font-size", size)];
                match modifier {
                    Some(modifier) => body.push((
                        "line-height",
                        self.arbitrary(modifier)
                            .or_else(|| self.theme("--leading", modifier))
                            .or_else(|| self.spacing(modifier))?,
                    )),
                    None if raw.is_none() => {
                        if let Some(height) = self.theme("--text", &format!("{value}--line-height"))
                        {
                            body.push(("line-height", format!("var(--tw-leading, {height})")));
                        }
                    }
                    None => {}
                }
                Body::new(&body)
            }
            "font" => {
                if let Some(raw) = &raw {
                    let weight = match hint {
                        Some("number") => true,
                        Some("family-name" | "generic-name") => false,
                        Some(_) => return None,
                        None => raw.parse::<u16>().is_ok() || raw.starts_with("var("),
                    };
                    if weight {
                        return Body::new(&[
                            ("--tw-font-weight", raw.clone()),
                            ("font-weight", raw.clone()),
                        ]);
                    }
                    return Body::new(&[("font-family", raw.clone())]);
                }
                let value = bare?;
                if let Some(family) = self.theme("--font", value) {
                    return Body::new(&[("font-family", family)]);
                }
                let weight = self.theme("--font-weight", value)?;
                Body::new(&[
                    ("--tw-font-weight", weight.clone()),
                    ("font-weight", weight),
                ])
            }
            "font-stretch" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => {
                        let number = value.strip_suffix('%')?;
                        let number: f64 = number.parse().ok()?;
                        (50. ..=200.).contains(&number).then(|| value.to_owned())?
                    }
                    _ => return None,
                };
                Body::new(&[("font-stretch", resolved)])
            }
            "leading" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "1".into(),
                    (Some(value), _) => self
                        .theme("--leading", value)
                        .or_else(|| self.spacing(value))?,
                    _ => return None,
                };
                Body::new(&[
                    ("--tw-leading", resolved.clone()),
                    ("line-height", resolved),
                ])
            }
            "tracking" => {
                let resolved = negate(match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => self.theme("--tracking", value)?,
                    _ => return None,
                });
                Body::new(&[
                    ("--tw-tracking", resolved.clone()),
                    ("letter-spacing", resolved),
                ])
            }
            "opacity" | "zoom" => {
                let resolved = match &raw {
                    Some(raw) => raw.clone(),
                    None => {
                        let value = bare?;
                        let number: f64 = value.parse().ok()?;
                        ((number >= 0. && root == "zoom" || (0. ..=100.).contains(&number))
                            && (number * 4.).fract() == 0.)
                            .then(|| format!("{value}%"))?
                    }
                };
                Body::new(&[(root, resolved)])
            }
            "shadow" | "inset-shadow" | "text-shadow" | "drop-shadow" => {
                let shadow = match (bare, &raw) {
                    (None, None) if value.is_none() => match root {
                        "shadow" => self.raw("--shadow"),
                        "drop-shadow" => self.raw("--drop-shadow"),
                        _ => None,
                    },
                    (Some("none"), _) => Some("none".into()),
                    (Some("inner"), _) if root == "shadow" => self.raw("--shadow-inner"),
                    (Some(value), _) => self.raw(&format!("--{root}-{value}")),
                    (_, Some(raw)) if hint.is_some_and(|hint| hint != "color") => Some(raw.clone()),
                    (_, Some(raw)) if hint.is_none() && !is_color(raw) => Some(raw.clone()),
                    _ => None,
                };
                let Some(shadow) = shadow else {
                    let color = self.shadow(value?, modifier, root)?;
                    if root == "drop-shadow" {
                        return Body::new(&[
                            ("--tw-drop-shadow-color", color),
                            ("--tw-drop-shadow", "var(--tw-drop-shadow-size)".into()),
                        ]);
                    }
                    return Body::new(&[(&format!("--tw-{root}-color"), color)]);
                };
                let alpha = match modifier {
                    Some(modifier) => Some(self.opacity(modifier)?),
                    None => None,
                };
                let name = format!("--tw-{root}-color");
                let colored = |shadow: &str| shadow_color(shadow, &name, alpha.as_deref());
                let mut body = Vec::new();
                if let Some(alpha) = &alpha {
                    body.push((format!("--tw-{root}-alpha"), alpha.clone()));
                }
                let variable = raw.is_some() && shadow.starts_with("var(");
                match root {
                    "shadow" | "inset-shadow" => {
                        let value = match (root, shadow.as_str()) {
                            ("shadow", "none") => "0 0 #0000".into(),
                            (_, "none") => "inset 0 0 #0000".into(),
                            ("inset-shadow", shadow) if raw.is_some() => {
                                colored(&format!("inset {shadow}"))
                            }
                            (_, shadow) if variable => shadow.to_owned(),
                            (_, shadow) => colored(shadow),
                        };
                        body.push((format!("--tw-{root}"), value));
                        body.push(("box-shadow".into(), SHADOW.into()));
                    }
                    "text-shadow" => {
                        let value = if shadow == "none" || variable {
                            shadow.clone()
                        } else {
                            colored(&shadow)
                        };
                        body.push(("text-shadow".into(), value));
                    }
                    _ if shadow == "none" => {
                        body.push(("--tw-drop-shadow".into(), " ".into()));
                        body.push(("filter".into(), FILTER.into()));
                    }
                    _ => {
                        let layers = |colored: &dyn Fn(&str) -> String| {
                            split(&shadow, ',')
                                .iter()
                                .map(|layer| format!("drop-shadow({})", colored(layer.trim())))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        let size = if variable {
                            format!("drop-shadow({shadow})")
                        } else {
                            layers(&colored)
                        };
                        let plain = match (bare, value) {
                            (Some(value), _) if alpha.is_none() => {
                                format!("drop-shadow(var(--drop-shadow-{value}))")
                            }
                            (None, None) => layers(&|layer: &str| layer.to_owned()),
                            _ => "var(--tw-drop-shadow-size)".into(),
                        };
                        body.push(("--tw-drop-shadow-size".into(), size));
                        body.push(("--tw-drop-shadow".into(), plain));
                        body.push(("filter".into(), FILTER.into()));
                    }
                }
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "ring" | "inset-ring" => {
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    let shadow = if root == "ring" {
                        format!(
                            "var(--tw-ring-inset,) 0 0 0 calc({width} + var(--tw-ring-offset-width)) var(--tw-ring-color, currentcolor)"
                        )
                    } else {
                        format!("inset 0 0 0 {width} var(--tw-inset-ring-color, currentcolor)")
                    };
                    return Body::new(&[
                        (&format!("--tw-{root}-shadow"), shadow),
                        ("box-shadow", SHADOW.into()),
                    ]);
                }
                Body::new(&[(&format!("--tw-{root}-color"), self.color(value?, modifier)?)])
            }
            "ring-offset" => {
                let value = value?;
                if let Some(width) = width("0px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        ("--tw-ring-offset-width", width),
                        ("--tw-ring-offset-shadow", "var(--tw-ring-inset,) 0 0 0 var(--tw-ring-offset-width) var(--tw-ring-offset-color)".into()),
                    ]);
                }
                Body::new(&[("--tw-ring-offset-color", self.color(value, modifier)?)])
            }
            "outline" => {
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        ("outline-style", "var(--tw-outline-style)".into()),
                        ("outline-width", width),
                    ]);
                }
                Body::new(&[("outline-color", self.color(value?, modifier)?)])
            }
            "outline-offset" | "underline-offset" => {
                let resolved = match (&raw, bare) {
                    (Some(raw), _) => raw.clone(),
                    (_, Some(value)) if integer(value) => format!("{value}px"),
                    _ => return None,
                };
                let property = if root == "outline-offset" {
                    "outline-offset"
                } else {
                    "text-underline-offset"
                };
                Body::new(&[(property, negate(resolved))])
            }
            "decoration" => {
                let value = value?;
                let thickness = match (hint, &raw, bare) {
                    (Some("length" | "percentage"), Some(raw), _) => Some(raw.clone()),
                    (None, Some(raw), _) if is_length(raw) => Some(raw.clone()),
                    (_, None, Some(value)) if integer(value) => Some(format!("{value}px")),
                    _ => None,
                };
                if let Some(thickness) = thickness {
                    return Body::new(&[("text-decoration-thickness", thickness)]);
                }
                Body::new(&[("text-decoration-color", self.color(value, modifier)?)])
            }
            "stroke" => {
                let value = value?;
                let width = match (hint, &raw, bare) {
                    (Some("length" | "number" | "percentage"), Some(raw), _) => Some(raw.clone()),
                    (None, Some(raw), _) if is_length(raw) => Some(raw.clone()),
                    (_, None, Some(value)) if integer(value) => Some(value.to_owned()),
                    _ => None,
                };
                if let Some(width) = width {
                    return Body::new(&[("stroke-width", width)]);
                }
                Body::new(&[("stroke", self.color(value, modifier)?)])
            }
            "fill" | "accent" | "caret" | "bg-glass" => {
                let property = match root {
                    "fill" => "fill",
                    "accent" => "accent-color",
                    "bg-glass" => "-macos-glass-tint-color",
                    _ => "caret-color",
                };
                let mut body = Body::new(&[(property, self.color(value?, modifier)?)])?;
                if root == "bg-glass" && cfg!(target_os = "macos") {
                    body = body
                        .nest(
                            &[],
                            ":where(&)",
                            "appearance:none;background-color:transparent;border:0",
                        )
                        .nest(
                            &[],
                            ":where(&:not(.bg-glass-clear))",
                            "-macos-glass:regular",
                        );
                    body.sort = Some("-macos-glass-tint-color");
                }
                Some(body)
            }
            "placeholder" => {
                let color = self.color(value?, modifier)?;
                Some(Body::default().nest(&[], "&::placeholder", &format!("color:{color}")))
            }
            "scrollbar-thumb" | "scrollbar-track" => {
                let color = self.color(value?, modifier)?;
                Body::new(&[
                    (&format!("--tw-{root}"), color),
                    (
                        "scrollbar-color",
                        "var(--tw-scrollbar-thumb) var(--tw-scrollbar-track)".into(),
                    ),
                ])
            }
            "duration" | "delay" | "ease" => {
                if bare == Some("initial") && root != "delay" {
                    return Body::new(&[(&format!("--tw-{root}"), "initial".into())]);
                }
                let resolved = match (&raw, bare) {
                    (Some(raw), _) => raw.clone(),
                    (_, Some("linear")) if root == "ease" => "linear".into(),
                    (_, Some(value)) if root == "ease" => self.theme("--ease", value)?,
                    (_, Some(value)) if integer(value) => format!("{value}ms"),
                    _ => return None,
                };
                match root {
                    "delay" => Body::new(&[("transition-delay", resolved)]),
                    "duration" => Body::new(&[
                        ("--tw-duration", resolved.clone()),
                        ("transition-duration", resolved),
                    ]),
                    _ => Body::new(&[
                        ("--tw-ease", resolved.clone()),
                        ("transition-timing-function", resolved),
                    ]),
                }
            }
            "transition" => {
                let properties = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (None, None) => format!(
                        "{COLORS}, opacity, box-shadow, transform, translate, scale, rotate, filter, -webkit-backdrop-filter, backdrop-filter, display, content-visibility, overlay, pointer-events"
                    ),
                    (Some("all"), _) => "all".into(),
                    (Some("colors"), _) => COLORS.into(),
                    (Some("opacity"), _) => "opacity".into(),
                    (Some("shadow"), _) => "box-shadow".into(),
                    (Some("transform"), _) => "transform, translate, scale, rotate".into(),
                    (Some("none"), _) => {
                        return Body::new(&[("transition-property", "none".into())]);
                    }
                    _ => return None,
                };
                let mut body = Body::parse(TRANSITION);
                body.declarations
                    .insert(0, ("transition-property".into(), properties));
                Some(body)
            }
            "blur" | "backdrop-blur" => {
                let resolved = match (value, bare, &raw) {
                    (None, ..) => "blur(8px)".into(),
                    (_, Some("none"), _) => " ".into(),
                    (_, _, Some(raw)) => format!("blur({raw})"),
                    (_, Some(value), _) => format!("blur({})", self.theme("--blur", value)?),
                    _ => return None,
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
            "brightness"
            | "contrast"
            | "saturate"
            | "backdrop-brightness"
            | "backdrop-contrast"
            | "backdrop-saturate"
            | "backdrop-opacity" => filter(root.trim_start_matches("backdrop-"), "%", None),
            "grayscale" | "invert" | "sepia" | "backdrop-grayscale" | "backdrop-invert"
            | "backdrop-sepia" => filter(root.trim_start_matches("backdrop-"), "%", Some("100%")),
            "hue-rotate" | "backdrop-hue-rotate" => filter("hue-rotate", "deg", None),
            "filter" | "backdrop-filter" if value.is_none() => {
                if root == "filter" {
                    return Body::new(&[("filter", FILTER.into())]);
                }
                Body::new(&[
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ])
            }
            "filter" | "backdrop-filter" => {
                let raw = raw?;
                if root == "filter" {
                    return Body::new(&[("filter", raw)]);
                }
                Body::new(&[
                    ("-webkit-backdrop-filter", raw.clone()),
                    ("backdrop-filter", raw),
                ])
            }
            "animate" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "none".into(),
                    (Some(value), _) => self.theme("--animate", value)?,
                    _ => return None,
                };
                Body::new(&[("animation", resolved)])
            }
            "content" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => {
                        return Body::new(&[
                            ("--tw-content", "none".into()),
                            ("content", "none".into()),
                        ]);
                    }
                    _ => return None,
                };
                Body::new(&[
                    ("--tw-content", resolved),
                    ("content", "var(--tw-content)".into()),
                ])
            }
            "aspect" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("auto"), _) => "auto".into(),
                    (Some("square"), _) => "1 / 1".into(),
                    (Some("video"), _) => self.theme("--aspect", "video")?,
                    (Some(value), _) if integer(value) && modifier.is_some_and(integer) => {
                        format!("{value} / {}", modifier?)
                    }
                    _ => return None,
                };
                Body::new(&[("aspect-ratio", resolved)])
            }
            "cursor" | "will-change" | "contain" | "list" | "list-image" | "align" | "object" => {
                let property = match root {
                    "list" => "list-style-type",
                    "list-image" => "list-style-image",
                    "align" => "vertical-align",
                    "object" => "object-position",
                    _ => root,
                };
                Body::new(&[(property, raw?)])
            }
            "@container" => {
                let kind = match bare {
                    None => "inline-size",
                    Some(kind @ ("normal" | "size")) => kind,
                    _ => return None,
                };
                let mut body = vec![("container-type", kind.to_owned())];
                if let Some(name) = modifier {
                    body.push((
                        "container-name",
                        named_value(name).then(|| name.to_owned())?,
                    ));
                }
                Body::new(&body)
            }
            "outline-hidden" if value.is_none() => Some(
                Body::parse("--tw-outline-style:none;outline-style:none").nest(
                    &["@media (forced-colors: active)"],
                    "&",
                    "outline:2px solid transparent;outline-offset:2px",
                ),
            ),
            "container" if value.is_none() => {
                let mut breakpoints: Vec<_> = DEFAULTS.with(|defaults| {
                    defaults
                        .keys()
                        .filter_map(|name| name.strip_prefix("--breakpoint-"))
                        .map(str::to_owned)
                        .collect()
                });
                breakpoints.extend(
                    self.theme
                        .keys()
                        .filter_map(|name| name.strip_prefix("--breakpoint-"))
                        .map(str::to_owned),
                );
                let mut widths: Vec<_> = breakpoints
                    .into_iter()
                    .filter_map(|name| self.raw(&format!("--breakpoint-{name}")))
                    .collect();
                widths.sort_by(|a, b| {
                    let pixels = |width: &String| {
                        let ((_, key), _) = self.media(">=", width, "min").unwrap_or_default();
                        key
                    };
                    pixels(a).cmp(&pixels(b))
                });
                widths.dedup();
                let mut body = Body::new(&[("width", "100%".into())])?;
                for width in widths {
                    body = body.nest(
                        &[&format!("@media (width >= {width})")],
                        "&",
                        &format!("max-width:{width}"),
                    );
                }
                Some(body)
            }
            _ => None,
        }
    }
}

fn template(source: &str) -> Variant {
    let mut variant = Variant::default();
    if source.contains("@slot") {
        slots(source, &[], None, &mut variant.branches, 0);
    } else if source.starts_with('@') {
        variant.branches.push((vec![source.to_owned()], None));
    } else {
        variant.branches.push((Vec::new(), Some(source.to_owned())));
    }
    variant.never = variant.branches.iter().any(|(at, selector)| {
        selector
            .as_ref()
            .is_some_and(|selector| selector.contains("::"))
            || at.iter().any(|rule| {
                !["@media", "@supports", "@container"]
                    .iter()
                    .any(|name| rule.starts_with(name))
            })
    });
    variant
}

fn slots(
    source: &str,
    at: &[String],
    selector: Option<&str>,
    out: &mut Vec<(Vec<String>, Option<String>)>,
    depth: u8,
) {
    if depth > 8 || out.len() > 8 {
        return;
    }
    for (prelude, block) in statements(source) {
        match block {
            None if prelude == "@slot" => out.push((at.to_vec(), selector.map(str::to_owned))),
            None => {}
            Some(inner) if prelude.starts_with('@') => {
                let mut at = at.to_vec();
                at.push(prelude.to_owned());
                slots(inner, &at, selector, out, depth + 1);
            }
            Some(inner) => {
                let nested = match selector {
                    Some(outer) if prelude.contains('&') => prelude.replace('&', outer),
                    Some(outer) => format!("{outer} {prelude}"),
                    None if prelude.contains('&') => prelude.to_owned(),
                    None => format!("& {prelude}"),
                };
                slots(inner, at, Some(&nested), out, depth + 1);
            }
        }
    }
}

fn statements(source: &str) -> Vec<(&str, Option<&str>)> {
    let mut items = Vec::new();
    let (mut start, mut open, mut depth, mut parens) = (0, 0, 0usize, 0usize);
    let mut chars = source.char_indices();
    while let Some((index, character)) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' | '\'' => {
                let mut escaped = false;
                for (_, other) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if other == '\\' {
                        escaped = true;
                    } else if other == character {
                        break;
                    }
                }
            }
            '(' | '[' => parens += 1,
            ')' | ']' => parens = parens.saturating_sub(1),
            '{' => {
                if depth == 0 {
                    open = index;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    items.push((source[start..open].trim(), Some(&source[open + 1..index])));
                    start = index + 1;
                }
            }
            ';' if depth == 0 && parens == 0 => {
                let item = source[start..index].trim();
                if !item.is_empty() {
                    items.push((item, None));
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let rest = source[start..].trim();
    if depth == 0 && !rest.is_empty() {
        items.push((rest, None));
    }
    items
}

fn call(text: &str, name: &str) -> Option<(usize, usize, usize)> {
    let (mut found, mut quote, mut escaped, mut depth) = (None, None, false, 0);
    for (index, character) in text.char_indices() {
        if let Some((_, open)) = found
            && index < open
        {
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quoted) = quote {
            if character == quoted {
                quote = None;
            }
            continue;
        }
        match (character, found) {
            ('"' | '\'', _) => quote = Some(character),
            ('(', Some(_)) => depth += 1,
            (')', Some((start, open))) if depth == 0 => return Some((start, open, index)),
            (')', Some(_)) => depth -= 1,
            (_, None)
                if text[index..].starts_with(name)
                    && text[index + name.len()..].starts_with('(') =>
            {
                found = Some((index, index + name.len() + 1));
            }
            _ => {}
        }
    }
    found.map(|(start, open)| (start, open, text.len()))
}

fn calls(value: &str) -> bool {
    value.contains("theme(") || value.contains("--spacing(") || value.contains("--alpha(")
}

fn split(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut stack = Vec::new();
    let mut start = 0;
    let mut chars = value.char_indices();
    while let Some((index, character)) = chars.next() {
        if character == separator && stack.is_empty() {
            parts.push(&value[start..index]);
            start = index + character.len_utf8();
            continue;
        }
        match character {
            '\\' => {
                chars.next();
            }
            '"' | '\'' => {
                let mut escaped = false;
                for (_, other) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if other == '\\' {
                        escaped = true;
                    } else if other == character {
                        break;
                    }
                }
            }
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            '{' => stack.push('}'),
            ')' | ']' | '}' if stack.last() == Some(&character) => {
                stack.pop();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn modifier(value: &str) -> (&str, Option<&str>) {
    match split(value, '/').as_slice() {
        [value, modifier] if !value.is_empty() && !modifier.is_empty() => (value, Some(modifier)),
        _ => (value, None),
    }
}

fn valid(value: &str) -> bool {
    let mut stack = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if chars.next().is_none() => return false,
            '"' | '\'' => loop {
                match chars.next() {
                    None | Some('\n') => return false,
                    Some('\\') if chars.next().is_none() => return false,
                    Some(other) if other == character => break,
                    _ => {}
                }
            },
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            '{' => stack.push('}'),
            ')' | ']' | '}' if stack.pop() != Some(character) => return false,
            ';' if stack.is_empty() => return false,
            '/' if chars.peek() == Some(&'*') => return false,
            _ => {}
        }
    }
    stack.is_empty()
}

fn named_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'%' | b'-'))
}

fn integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn decode(raw: &str) -> String {
    let mut value = String::with_capacity(raw.len());
    let mut stack: Vec<bool> = Vec::new();
    let (mut word, mut first) = (0, false);
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if stack.last() == Some(&true) {
            match character {
                '(' => stack.push(true),
                ')' => {
                    stack.pop();
                }
                _ => {}
            }
            value.push(character);
            continue;
        }
        match character {
            '\\' if chars.peek() == Some(&'_') => {
                chars.next();
                value.push('_');
            }
            '\\' => {
                value.push(character);
                value.extend(chars.next());
            }
            '_' => value.push(if first { '_' } else { ' ' }),
            '"' | '\'' => {
                value.push(character);
                while let Some(other) = chars.next() {
                    match other {
                        '\\' if chars.peek() == Some(&'_') => {
                            chars.next();
                            value.push('_');
                        }
                        '\\' => {
                            value.push(other);
                            value.extend(chars.next());
                        }
                        '_' => value.push(' '),
                        _ => value.push(other),
                    }
                    if other == character {
                        break;
                    }
                }
            }
            '(' => {
                let name = &value[word..];
                let url = name == "url" || name.ends_with(" url");
                first = matches!(name, "var" | "theme")
                    || name.ends_with(" var")
                    || name.ends_with(" theme");
                stack.push(url);
                value.push(character);
                word = value.len();
            }
            ')' | ':' | ',' | '=' | '>' | '<' | '\n' | ' ' | '\t' | '/' => {
                if character == ')' {
                    stack.pop();
                }
                value.push(character);
                word = value.len();
                first = false;
            }
            _ => value.push(character),
        }
    }
    if !value.contains('(') {
        return value;
    }
    math(&value)
}

fn math(value: &str) -> String {
    const MATH: &[&str] = &[
        "calc", "min", "max", "clamp", "mod", "rem", "sin", "cos", "tan", "asin", "acos", "atan",
        "atan2", "pow", "sqrt", "hypot", "log", "exp", "round",
    ];
    if !MATH.iter().any(|name| value.contains(name)) {
        return value.to_owned();
    }
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len() + 8);
    let mut stack: Vec<bool> = Vec::new();
    let (mut number, mut previous) = (None, None);
    for (index, &character) in chars.iter().enumerate() {
        if character.is_ascii_digit()
            || number.is_some() && (character == '%' || character.is_ascii_alphabetic())
        {
            number = Some(index);
        } else {
            previous = number;
            number = None;
        }
        let math = stack.last() == Some(&true);
        match character {
            '(' => {
                result.push(character);
                let start = chars[..index]
                    .iter()
                    .rposition(|c| !(c.is_ascii_digit() || c.is_ascii_lowercase()))
                    .map_or(0, |position| position + 1);
                let name: String = chars[start..index].iter().collect();
                stack.push(MATH.contains(&name.as_str()) || math && name.is_empty());
            }
            ')' => {
                result.push(character);
                stack.pop();
            }
            ',' if math => result.push_str(", "),
            ' ' if math && result.ends_with(' ') => {}
            '+' | '*' | '/' | '-' if math => {
                let trimmed = result.trim_end();
                let mut tail = trimmed.chars().rev();
                let (last, before) = (tail.next(), tail.next());
                let next = chars.get(index + 1).copied();
                if matches!(last, Some('e' | 'E')) && before.is_some_and(|c| c.is_ascii_digit())
                    || matches!(last, Some('+' | '*' | '/' | '-' | '(' | ','))
                {
                    result.push(character);
                } else if index > 0 && chars[index - 1] == ' ' {
                    result.push(character);
                    result.push(' ');
                } else if last.is_some_and(|c| c.is_ascii_digit() || c == ')')
                    || next.is_some_and(|c| {
                        c.is_ascii_digit() || matches!(c, '(' | '+' | '*' | '/' | '-')
                    })
                    || index > 0 && previous == Some(index - 1)
                {
                    result.push(' ');
                    result.push(character);
                    result.push(' ');
                } else {
                    result.push(character);
                }
            }
            _ => result.push(character),
        }
    }
    result
}

fn with_alpha(color: &str, alpha: &str) -> String {
    let alpha = match alpha.parse::<f64>() {
        Ok(number) => format!("{}%", number * 100.),
        Err(_) => alpha.to_owned(),
    };
    if alpha == "100%" {
        return color.to_owned();
    }
    format!("color-mix(in oklab, {color} {alpha}, transparent)")
}

fn universal(selector: &str, all: bool) -> String {
    let mut result = String::with_capacity(selector.len() + 2);
    let mut chars = selector.chars().peekable();
    let mut done = false;
    while let Some(character) = chars.next() {
        if character != '&' || done {
            result.push(character);
            continue;
        }
        done = !all;
        if !matches!(chars.peek(), Some(':' | '[' | '.' | '#')) {
            result.push('*');
        }
    }
    result
}

fn negate(rule: &str) -> Option<String> {
    let (name, params) = rule.split_once(' ')?;
    let params = params.trim();
    if !matches!(name, "@media" | "@supports" | "@container") || split(params, ',').len() > 1 {
        return None;
    }
    let words: Vec<_> = split(params, ' ')
        .into_iter()
        .filter(|word| !word.is_empty())
        .collect();
    Some(match words.as_slice() {
        ["not", rest @ ..] => format!("{name} {}", rest.join(" ")),
        [container, "not", rest @ ..] if name == "@container" && !container.starts_with('(') => {
            format!("{name} {container} {}", rest.join(" "))
        }
        [container, rest @ ..] if name == "@container" && !container.starts_with('(') => {
            format!("{name} {container} not {}", rest.join(" "))
        }
        _ => format!("{name} not {params}"),
    })
}

fn removes(cleared: &str, name: &str) -> bool {
    match cleared.strip_suffix("-*") {
        Some(namespace) => name.starts_with(namespace) && !kept(name, namespace),
        None => cleared == name,
    }
}

fn kept(name: &str, namespace: &str) -> bool {
    KEPT.iter().any(|(other, names)| {
        *other == namespace
            && names.iter().any(|kept| {
                name.strip_prefix(kept)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
            })
    })
}

fn direction(side: &str) -> Option<&'static str> {
    Some(match side {
        "t" => "top",
        "tr" => "top right",
        "r" => "right",
        "br" => "bottom right",
        "b" => "bottom",
        "bl" => "bottom left",
        "l" => "left",
        "tl" => "top left",
        _ => return None,
    })
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
                || value
                    .strip_suffix('s')
                    .is_some_and(|number| number.parse::<f64>().is_ok())
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

fn shadow_color(shadow: &str, name: &str, alpha: Option<&str>) -> String {
    let replacement = |color: &str| match alpha {
        Some(alpha) => format!("var({name}, oklab(from {color} l a b / {alpha}))"),
        None => format!("var({name}, {color})"),
    };
    split(shadow, ',')
        .into_iter()
        .map(|layer| {
            let layer = layer.trim();
            let (mut color, mut offsets) = (None, 0);
            for part in split(layer, ' ') {
                if part.is_empty()
                    || matches!(part, "inset" | "inherit" | "initial" | "revert" | "unset")
                {
                    continue;
                }
                let digits = part.strip_prefix('-').unwrap_or(part);
                if digits.starts_with(|c: char| c.is_ascii_digit())
                    || digits.starts_with('.')
                        && digits[1..].starts_with(|c: char| c.is_ascii_digit())
                {
                    offsets += 1;
                } else if color.is_none() {
                    color = Some(part);
                }
            }
            match (offsets, color) {
                (0 | 1, _) => layer.to_owned(),
                (_, Some(color)) => layer.replacen(color, &replacement(color), 1),
                _ => format!("{layer} {}", replacement("currentcolor")),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

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

fn css(rules: &[Nested]) -> String {
    let mut css = String::new();
    for (at, selector, pairs) in rules {
        for rule in at {
            let _ = write!(css, "{rule}{{");
        }
        let _ = write!(css, "{selector}{{");
        for (key, value) in pairs {
            let _ = write!(css, "{key}:{value};");
        }
        css.push_str(&"}".repeat(at.len() + 1));
    }
    css
}

fn chunk(utilities: &[(Sort, u64, Vec<Rule>)]) -> Rc<Stylesheet> {
    let rules = utilities
        .iter()
        .flat_map(|(.., rules)| rules.iter().cloned())
        .collect();
    Rc::new(Stylesheet(vec![Rc::new(Rule::Layer(
        vec!["utilities".into()],
        rules,
    ))]))
}

fn selectors(rule: &Rule, visit: &mut impl FnMut(&super::SelectorList)) {
    match rule {
        Rule::Style(list, children) => {
            visit(list);
            for child in children {
                selectors(child, visit);
            }
        }
        Rule::Media(_, children)
        | Rule::Layer(_, children)
        | Rule::Supports(_, _, children)
        | Rule::Container(.., children) => {
            for child in children {
                selectors(child, visit);
            }
        }
        _ => {}
    }
}

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
