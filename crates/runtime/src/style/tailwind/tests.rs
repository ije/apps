use super::super::{Prop, StyleCache, compute_styles, length};
use crate::dom::Document;
use std::{collections::HashMap, fmt::Write, rc::Rc};

fn computed(source: &str, hovered: Option<&str>) -> (Document, super::super::ComputedStyles) {
    let mut doc = Document::parse(source);
    if let Some(selector) = hovered {
        let id = (0..doc.nodes.len())
            .find(|&id| doc.matches(id, selector))
            .unwrap();
        doc.nodes[id].state = crate::dom::HOVER;
    }
    let styles = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    (doc, styles)
}

fn id(doc: &Document, selector: &str) -> usize {
    (0..doc.nodes.len())
        .find(|&id| doc.matches(id, selector))
        .unwrap()
}

/// The CSS Tailwind generates for each class, with `author` CSS for themes and utilities.
fn generated(author: &str, classes: &[&str]) -> Vec<String> {
    let mut compiler = super::Compiler::default();
    let mut sheets = vec![Rc::new(super::Stylesheet::parse(&format!(
        "@import 'tailwindcss/utilities';{author}"
    )))];
    compiler.update(&mut sheets, &Document::parse(""), &[]);
    let config = compiler.0.last().unwrap();
    classes
        .iter()
        .map(|class| {
            config
                .compile(class, &super::escape(class))
                .map(|compiled| super::css(&compiled.rules))
                .unwrap_or_default()
        })
        .collect()
}

/// Writes the CSS of the classes in `$TW_LIST` to `$TW_OUT` as JSON, to diff with Tailwind.
#[test]
#[ignore]
fn dump() {
    let list = std::fs::read_to_string(std::env::var("TW_LIST").unwrap()).unwrap();
    let author = std::env::var("TW_CSS")
        .ok()
        .filter(|path| !path.is_empty())
        .map(|path| std::fs::read_to_string(path).unwrap())
        .unwrap_or_default();
    let classes: Vec<_> = list.lines().filter(|line| !line.is_empty()).collect();
    let out: serde_json::Map<_, _> = classes
        .iter()
        .zip(generated(&author, &classes))
        .map(|(class, css)| ((*class).to_owned(), css.into()))
        .collect();
    std::fs::write(
        std::env::var("TW_OUT").unwrap(),
        serde_json::to_string(&out).unwrap(),
    )
    .unwrap();
}

/// Tailwind 4.3.3's output, written in the compiler's compact form.
fn assert_generated(author: &str, expected: &[(&str, &str)]) {
    let classes: Vec<_> = expected.iter().map(|(class, _)| *class).collect();
    for ((class, css), actual) in expected.iter().zip(generated(author, &classes)) {
        assert_eq!(actual, *css, "{class}");
    }
}

#[test]
fn custom_utility_bodies_with_strings_and_unicode_do_not_panic() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';
        @utility tab-* { font-family: \")\" --value( \"(\" ) é; tab-size: --value(integer); }
        @utility open-* { content: --value(; }</style>
        <body><p class='tab-7 open-1'>a</p></body></html>",
        None,
    );
    let p = &styles.values[id(&doc, "p")];
    assert_eq!(&p["tab-size"], "7");
    assert_eq!(
        p.get(Prop::FONT_FAMILY).map(|f| f.contains("--value")),
        Some(false)
    );
}

#[test]
fn hostile_candidates_never_panic() {
    let parts = [
        "w-",
        "[",
        "]",
        "(",
        ")",
        "-",
        "/",
        "!",
        ":",
        "_",
        "\\",
        "é",
        "→",
        "😀",
        "calc(",
        "var(--",
        "--value(",
        "--spacing(",
        "theme(",
        "%",
        ".",
        "#",
        "*",
        "&",
        "@",
        ">",
        "\"",
        "'",
        ",",
        "=",
        "0",
        "1.5",
        "px",
        "group-",
        "peer-",
        "not-",
        "has-",
        "in-",
        "data-",
        "aria-",
        "supports-",
        "min-",
        "max-",
        "nth-",
        "@min-",
        "text-",
        "bg-",
        "shadow-",
        "ring-",
        "content-",
        "font-",
        "mask-",
        "from-",
        "*:",
        "**:",
        "[&",
        "{",
        "}",
        ";",
        "/*",
        "@slot",
        " ",
    ];
    let author = "@utility tab-* { tab-size: --value(integer, [*]); font: --modifier(--text-*); }
        @custom-variant hocus { &:hover { @slot; } &:focus { @slot; } }";
    let heads = [
        "",
        "hover:",
        "not-",
        "has-[",
        "[&_",
        "data-[",
        "supports-[",
        "*:",
        "@sm:",
        "group-",
        "w-[",
        "bg-[",
        "[color:",
        "content-[",
        "p-(",
        "tab-[",
        "text-[",
        "shadow-[",
        "from-[",
        "font-[",
        "grid-cols-[",
        "bg-red-500/[",
        "[--x:",
    ];
    let tails = ["]", ")", "]/50", "]:p-1", "", ")]", "]]", "]:flex"];
    let mut seed: u64 = 7;
    let mut next = |count: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize % count
    };
    let mut classes = Vec::new();
    for _ in 0..4000 {
        let mut class = String::from(heads[next(heads.len())]);
        class.push_str(heads[next(heads.len())]);
        for _ in 0..next(6) {
            class.push_str(parts[next(parts.len())]);
        }
        class.push_str(tails[next(tails.len())]);
        classes.push(class);
    }
    let classes: Vec<_> = classes.iter().map(String::as_str).collect();
    let mut count = 0;
    for css in generated(author, &classes) {
        count += usize::from(!css.is_empty());
        let sheet = super::Stylesheet::parse(&format!("{css}.sentinel{{color:red}}"));
        assert!(
            sheet
                .0
                .last()
                .is_some_and(|rule| rule.source() == ".sentinel{color:red;}"),
            "{css}"
        );
    }
    assert!(count > 100);
    let mut html = format!("<html><style>@import 'tailwindcss';{author}</style><body>");
    for class in &classes {
        let _ = write!(
            html,
            "<i class=\"{}\"></i>",
            class
                .replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('<', "&lt;")
        );
    }
    html.push_str("<b class='pl-[29px] text-[rgb(0,0,255)]'></b></body></html>");
    let (doc, styles) = computed(&html, None);
    let b = &styles.values[id(&doc, "b")];
    assert_eq!(length(&b["padding-left"], 0., 16., 800., 600.), 29.);
    assert_eq!(&b["color"], "rgb(0,0,255)");
}

#[test]
fn nested_variants_are_bounded_and_follow_tailwind_rules() {
    let deep = format!("{}hover:flex", "not-".repeat(30000));
    let start = std::time::Instant::now();
    assert_eq!(generated("", &[&deep]), [""]);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    let chain = format!("{}flex", "hover:".repeat(30000));
    assert_eq!(generated("", &[&chain]), [""]);
    assert_generated(
        "@custom-variant hocus { &:hover { @slot; } &:focus { @slot; } }",
        &[
            ("not-not-hover:flex", ""),
            ("group-not-hover:flex", ""),
            ("not-before:flex", ""),
            ("not-*:flex", ""),
            ("not-[>svg]:flex", ""),
            ("[>svg]:flex", ""),
            ("not-hocus:flex", ""),
            ("group-md:flex", ""),
            (
                "not-not-focus:flex",
                ".not-not-focus\\:flex:not(:not(:focus)){display:flex;}",
            ),
            (
                "group-hocus:flex",
                ".group-hocus\\:flex:is(:where(.group):hover *){display:flex;}.group-hocus\\:flex:is(:where(.group):focus *){display:flex;}",
            ),
            (
                "peer-not-checked:flex",
                ".peer-not-checked\\:flex:is(:where(.peer):not(:checked) ~ *){display:flex;}",
            ),
        ],
    );
}

#[test]
fn invalid_arbitrary_values_stay_out_of_the_sheet() {
    assert_generated(
        "",
        &[
            ("w-[calc(100%-2rem]", ""),
            ("bg-[url(]", ""),
            ("w-[1px}]", ""),
            ("w-[1px{]", ""),
            ("w-[calc(1px{)]", ""),
            ("content-['a]", ""),
            ("w-[1px/*]", ""),
            ("[color:red;padding-left:11px]", ""),
            ("[color:red}]", ""),
            ("[Color:red]", ""),
            ("origin-a;b}c", ""),
            ("[&{}]:p-1", ""),
            ("data-[x]{a]:flex", ""),
            (
                "content-['a;b']",
                ".content-\\[\\'a\\;b\\'\\]{--tw-content:'a;b';content:var(--tw-content);}",
            ),
        ],
    );
    for bad in [
        "w-[calc(100%-2rem]",
        "bg-[url(]",
        "[color:red;padding-left:11px]",
    ] {
        let (doc, styles) = computed(
            &format!(
                "<html><style>@import 'tailwindcss';</style><body><a class='{bad}'></a><b class='pl-[29px] text-[rgb(0,0,255)] p-4'></b></body></html>"
            ),
            None,
        );
        let b = &styles.values[id(&doc, "b")];
        assert_eq!(length(&b["padding-left"], 0., 16., 800., 600.), 29.);
        assert_eq!(length(&b["padding-top"], 0., 16., 800., 600.), 16.);
        assert_eq!(&b["color"], "rgb(0,0,255)");
        assert_eq!(
            length(
                &styles.values[id(&doc, "a")]["padding-left"],
                0.,
                16.,
                800.,
                600.
            ),
            0.
        );
    }
}

#[test]
fn css_variable_shorthand_works_for_values_hints_and_modifiers() {
    assert_generated(
        "",
        &[
            (
                "w-(--sidebar-width)",
                ".w-\\(--sidebar-width\\){width:var(--sidebar-width);}",
            ),
            (
                "max-h-(--radix-dropdown-menu-content-available-height)",
                ".max-h-\\(--radix-dropdown-menu-content-available-height\\){max-height:var(--radix-dropdown-menu-content-available-height);}",
            ),
            (
                "size-(--cell-size)",
                ".size-\\(--cell-size\\){width:var(--cell-size);height:var(--cell-size);}",
            ),
            (
                "bg-(--brand)",
                ".bg-\\(--brand\\){background-color:var(--brand);}",
            ),
            ("z-(--z)", ".z-\\(--z\\){z-index:var(--z);}"),
            (
                "leading-(--x)",
                ".leading-\\(--x\\){--tw-leading:var(--x);line-height:var(--x);}",
            ),
            (
                "text-(length:--x)",
                ".text-\\(length\\:--x\\){font-size:var(--x);}",
            ),
            (
                "bg-(image:--x)",
                ".bg-\\(image\\:--x\\){background-image:var(--x);}",
            ),
            (
                "bg-red-500/(--alpha)",
                ".bg-red-500\\/\\(--alpha\\){background-color:color-mix(in oklab, var(--color-red-500) var(--alpha), transparent);}",
            ),
            (
                "text-sm/(--lh)",
                ".text-sm\\/\\(--lh\\){font-size:var(--text-sm);line-height:var(--lh);}",
            ),
            (
                "origin-(--x)",
                ".origin-\\(--x\\){transform-origin:var(--x);}",
            ),
            ("w-(--a_b)", ".w-\\(--a_b\\){width:var(--a_b);}"),
            ("w-(x)", ""),
            ("data-(--x):flex", ""),
        ],
    );
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body><div style='--w: 77px; --brand: #123456' class='w-(--w) bg-(--brand)'></div></body></html>",
        None,
    );
    let div = &styles.values[id(&doc, "div")];
    assert_eq!(length(&div["width"], 0., 16., 800., 600.), 77.);
    assert_eq!(&div["background-color"], "#123456");
}

#[test]
fn new_classes_join_the_scope_in_tailwind_order() {
    let mut doc = Document::parse(
        "<html><style>@import 'tailwindcss';</style><body><p class='px-2'>a</p><b class='pt-1'>b</b></body></html>",
    );
    let mut cache = StyleCache::default();
    let mut sheets = HashMap::new();
    let mut pass = |doc: &Document| {
        let styles = compute_styles(doc, 800., 600., None, &mut sheets, &mut cache, None);
        cache.finish();
        styles
    };
    pass(&doc);
    let p = id(&doc, "p");
    let b = id(&doc, "b");
    doc.nodes[p].attrs.set("class", "px-2 p-4 hover:p-8");
    let styles = pass(&doc);
    let left = |styles: &super::super::ComputedStyles, node: usize| {
        length(&styles.values[node]["padding-left"], 0., 16., 800., 600.)
    };
    assert_eq!(left(&styles, p), 8.);
    assert_eq!(
        length(&styles.values[p]["padding-top"], 0., 16., 800., 600.),
        16.
    );
    doc.nodes[b].attrs.set("class", "pt-1 p-[3px]");
    let styles = pass(&doc);
    assert_eq!(
        length(&styles.values[b]["padding-top"], 0., 16., 800., 600.),
        4.
    );
    assert_eq!(left(&styles, b), 3.);
    assert_eq!(left(&styles, p), 8.);
}

#[test]
fn configs_are_kept_per_theme() {
    let document = Rc::new(super::Stylesheet::parse(
        "@import 'tailwindcss'; @theme { --color-brand: #123456; }",
    ));
    let shadow = Rc::new(super::Stylesheet::parse("@import 'tailwindcss';"));
    let doc = Document::parse("<p class='bg-brand p-3'></p>");
    let order: Vec<_> = (0..doc.nodes.len()).collect();
    let mut compiler = super::Compiler::default();
    for _ in 0..3 {
        for sheet in [&document, &shadow] {
            let update = compiler
                .update(&mut vec![sheet.clone()], &doc, &order)
                .unwrap();
            assert!(update.replaced.is_empty() && update.added.is_empty());
        }
    }
    assert_eq!(compiler.0.len(), 2);
    assert!(
        compiler
            .0
            .iter()
            .all(|config| config.classes.contains_key("p-3"))
    );
}

#[test]
fn relative_and_descendant_variants_match() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body>
        <button class='has-[>svg]:px-3 [&>svg]:size-4 *:p-1 **:m-2 [&_i]:inline'><svg></svg><span><i></i></span></button>
        <a class='has-[>svg]:px-3'><span></span></a>
        </body></html>",
        None,
    );
    let button = &styles.values[id(&doc, "button")];
    assert_eq!(length(&button["padding-left"], 0., 16., 800., 600.), 12.);
    assert_eq!(
        styles.values[id(&doc, "a")]
            .get(Prop::PADDING_LEFT)
            .map(|left| length(left, 0., 16., 800., 600.)),
        Some(0.)
    );
    let svg = &styles.values[id(&doc, "svg")];
    assert_eq!(length(&svg["width"], 0., 16., 800., 600.), 16.);
    assert_eq!(length(&svg["padding-top"], 0., 16., 800., 600.), 4.);
    let i = &styles.values[id(&doc, "i")];
    assert_eq!(length(&i["margin-top"], 0., 16., 800., 600.), 8.);
    assert_eq!(&i["display"], "inline");
}

#[test]
fn default_utilities_match_tailwind() {
    assert_generated(
        "",
        &[
            ("p-0", ".p-0{padding:0px;}"),
            ("m-1", ".m-1{margin:var(--spacing);}"),
            ("-m-px", ".-m-px{margin:-1px;}"),
            ("-w-4", ""),
            ("-opacity-50", ""),
            (
                "bg-linear-to-r/oklch",
                ".bg-linear-to-r\\/oklch{--tw-gradient-position:to right;}@supports (background-image: linear-gradient(in lab, red, red)){.bg-linear-to-r\\/oklch{--tw-gradient-position:to right in oklch;}}.bg-linear-to-r\\/oklch{background-image:linear-gradient(var(--tw-gradient-stops));}",
            ),
            (
                "bg-gradient-to-b",
                ".bg-gradient-to-b{--tw-gradient-position:to bottom in oklab;background-image:linear-gradient(var(--tw-gradient-stops));}",
            ),
            (
                "from-red-500/50",
                ".from-red-500\\/50{--tw-gradient-from:color-mix(in oklab, var(--color-red-500) 50%, transparent);--tw-gradient-stops:var(--tw-gradient-via-stops, var(--tw-gradient-position), var(--tw-gradient-from) var(--tw-gradient-from-position), var(--tw-gradient-to) var(--tw-gradient-to-position));}",
            ),
            ("via-10%", ".via-10\\%{--tw-gradient-via-position:10%;}"),
            (
                "bg-conic-90",
                ".bg-conic-90{--tw-gradient-position:from 90deg in oklab;background-image:conic-gradient(var(--tw-gradient-stops));}",
            ),
            (
                "divide-y-2",
                ":where(.divide-y-2 > :not(:last-child)){--tw-divide-y-reverse:0;border-bottom-style:var(--tw-border-style);border-top-style:var(--tw-border-style);border-top-width:calc(2px * var(--tw-divide-y-reverse));border-bottom-width:calc(2px * calc(1 - var(--tw-divide-y-reverse)));}",
            ),
            (
                "space-x-reverse",
                ":where(.space-x-reverse > :not(:last-child)){--tw-space-x-reverse:1;}",
            ),
            (
                "container",
                ".container{width:100%;}@media (width >= 40rem){.container{max-width:40rem;}}@media (width >= 48rem){.container{max-width:48rem;}}@media (width >= 64rem){.container{max-width:64rem;}}@media (width >= 80rem){.container{max-width:80rem;}}@media (width >= 96rem){.container{max-width:96rem;}}",
            ),
            (
                "auto-rows-fr",
                ".auto-rows-fr{grid-auto-rows:minmax(0, 1fr);}",
            ),
            (
                "drop-shadow-md",
                ".drop-shadow-md{--tw-drop-shadow-size:drop-shadow(0 3px 3px var(--tw-drop-shadow-color, rgb(0 0 0 / 0.12)));--tw-drop-shadow:drop-shadow(var(--drop-shadow-md));filter:var(--tw-blur,) var(--tw-brightness,) var(--tw-contrast,) var(--tw-grayscale,) var(--tw-hue-rotate,) var(--tw-invert,) var(--tw-saturate,) var(--tw-sepia,) var(--tw-drop-shadow,);}",
            ),
            (
                "inset-ring-2",
                ".inset-ring-2{--tw-inset-ring-shadow:inset 0 0 0 2px var(--tw-inset-ring-color, currentcolor);box-shadow:var(--tw-inset-shadow), var(--tw-inset-ring-shadow), var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow);}",
            ),
            (
                "shadow-lg/50",
                ".shadow-lg\\/50{--tw-shadow-alpha:50%;--tw-shadow:0 10px 15px -3px var(--tw-shadow-color, oklab(from rgb(0 0 0 / 0.1) l a b / 50%)), 0 4px 6px -4px var(--tw-shadow-color, oklab(from rgb(0 0 0 / 0.1) l a b / 50%));box-shadow:var(--tw-inset-shadow), var(--tw-inset-ring-shadow), var(--tw-ring-offset-shadow), var(--tw-ring-shadow), var(--tw-shadow);}",
            ),
            (
                "shadow-black/10",
                ".shadow-black\\/10{--tw-shadow-color:color-mix(in oklab, color-mix(in oklab, var(--color-black) 10%, transparent) var(--tw-shadow-alpha), transparent);}",
            ),
            (
                "-skew-x-6",
                ".-skew-x-6{--tw-skew-x:skewX(calc(6deg * -1));transform:var(--tw-rotate-x,) var(--tw-rotate-y,) var(--tw-rotate-z,) var(--tw-skew-x,) var(--tw-skew-y,);}",
            ),
            (
                "transform-gpu",
                ".transform-gpu{transform:translateZ(0) var(--tw-rotate-x,) var(--tw-rotate-y,) var(--tw-rotate-z,) var(--tw-skew-x,) var(--tw-skew-y,);}",
            ),
            (
                "perspective-origin-top-left",
                ".perspective-origin-top-left{perspective-origin:0 0;}",
            ),
            (
                "mix-blend-multiply",
                ".mix-blend-multiply{mix-blend-mode:multiply;}",
            ),
            (
                "mask-b-from-50%",
                ".mask-b-from-50\\%{mask-image:var(--tw-mask-linear), var(--tw-mask-radial), var(--tw-mask-conic);mask-composite:intersect;--tw-mask-linear:var(--tw-mask-left), var(--tw-mask-right), var(--tw-mask-bottom), var(--tw-mask-top);--tw-mask-bottom:linear-gradient(to bottom, var(--tw-mask-bottom-from-color) var(--tw-mask-bottom-from-position), var(--tw-mask-bottom-to-color) var(--tw-mask-bottom-to-position));--tw-mask-bottom-from-position:50%;}",
            ),
            (
                "text-shadow-sm",
                ".text-shadow-sm{text-shadow:0px 1px 0px var(--tw-text-shadow-color, rgb(0 0 0 / 0.075)), 0px 1px 1px var(--tw-text-shadow-color, rgb(0 0 0 / 0.075)), 0px 2px 2px var(--tw-text-shadow-color, rgb(0 0 0 / 0.075));}",
            ),
            ("scheme-dark", ".scheme-dark{color-scheme:dark;}"),
            (
                "snap-x",
                ".snap-x{scroll-snap-type:x var(--tw-scroll-snap-strictness);}",
            ),
            (
                "touch-pan-y",
                ".touch-pan-y{--tw-pan-y:pan-y;touch-action:var(--tw-pan-x,) var(--tw-pan-y,) var(--tw-pinch-zoom,);}",
            ),
            ("bg-top-left", ".bg-top-left{background-position:left top;}"),
            (
                "object-left-top",
                ".object-left-top{object-position:left top;}",
            ),
            (
                "hyphens-auto",
                ".hyphens-auto{-webkit-hyphens:auto;hyphens:auto;}",
            ),
            (
                "font-stretch-condensed",
                ".font-stretch-condensed{font-stretch:condensed;}",
            ),
            (
                "border-spacing-2",
                ".border-spacing-2{--tw-border-spacing-x:calc(var(--spacing) * 2);--tw-border-spacing-y:calc(var(--spacing) * 2);border-spacing:var(--tw-border-spacing-x) var(--tw-border-spacing-y);}",
            ),
            (
                "box-decoration-clone",
                ".box-decoration-clone{-webkit-box-decoration-break:clone;box-decoration-break:clone;}",
            ),
            ("float-start", ".float-start{float:inline-start;}"),
            ("max-w-prose", ".max-w-prose{max-width:65ch;}"),
            (
                "@sm:flex",
                "@container (width >= 24rem){.\\@sm\\:flex{display:flex;}}",
            ),
            (
                "@max-md/main:hidden",
                "@container main (width < 28rem){.\\@max-md\\/main\\:hidden{display:none;}}",
            ),
        ],
    );
}

#[test]
fn css_functions_resolve_in_values_and_author_css() {
    assert_generated(
        "",
        &[
            (
                "w-[calc(var(--a)+(--spacing(4)))]",
                ".w-\\[calc\\(var\\(--a\\)\\+\\(--spacing\\(4\\)\\)\\)\\]{width:calc(var(--a) + (calc(var(--spacing) * 4)));}",
            ),
            (
                "min-h-[calc(100svh-theme(spacing.4))]",
                ".min-h-\\[calc\\(100svh-theme\\(spacing\\.4\\)\\)\\]{min-height:calc(100svh - 1rem);}",
            ),
            (
                "[--cell-size:--spacing(8)]",
                ".\\[--cell-size\\:--spacing\\(8\\)\\]{--cell-size:calc(var(--spacing) * 8);}",
            ),
            (
                "bg-[--alpha(var(--color-red-500)/50%)]",
                ".bg-\\[--alpha\\(var\\(--color-red-500\\)\\/50\\%\\)\\]{background-color:color-mix(in oklab, var(--color-red-500) 50%, transparent);}",
            ),
            (
                "bg-[theme(colors.red.500)]",
                ".bg-\\[theme\\(colors\\.red\\.500\\)\\]{background-color:oklch(63.7% 0.237 25.331);}",
            ),
            (
                "bg-[--theme(--color-nope,blue)]",
                ".bg-\\[--theme\\(--color-nope\\,blue\\)\\]{background-color:blue;}",
            ),
        ],
    );
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';
        p { width: --spacing(4); color: --alpha(#ff0000 / 50%); height: theme(spacing.2); margin-left: --theme(--spacing); }</style>
        <body><p>a</p></body></html>",
        None,
    );
    let p = &styles.values[id(&doc, "p")];
    assert_eq!(length(&p["width"], 0., 16., 800., 600.), 16.);
    assert_eq!(length(&p["height"], 0., 16., 800., 600.), 8.);
    assert_eq!(length(&p["margin-left"], 0., 16., 800., 600.), 4.);
    assert_eq!(&p["color"], "oklab(0.627955 0.224863 0.125846 / 0.5)");
}

#[test]
fn apply_expands_in_nested_at_rules_and_utilities() {
    let source = "<html><style>@import 'tailwindcss';
        .med { @media (width >= 600px) { @apply pt-7 text-[#ff0000]; } }
        .sup { @supports (display: grid) { @apply pt-6; } }
        .hov { &:hover { @apply pt-8; } }
        @utility btn { @apply px-4 hover:text-[#00ff00]; border-top-width: 3px; }</style>
        <body><p class='med'>a</p><b class='sup'>b</b><i class='hov'>c</i><a class='btn'>d</a></body></html>";
    let (doc, styles) = computed(source, None);
    let top = |selector| {
        length(
            &styles.values[id(&doc, selector)]["padding-top"],
            0.,
            16.,
            800.,
            600.,
        )
    };
    assert_eq!(top("p"), 28.);
    assert_eq!(&styles.values[id(&doc, "p")]["color"], "#ff0000");
    assert_eq!(top("b"), 24.);
    let a = &styles.values[id(&doc, "a")];
    assert_eq!(length(&a["padding-left"], 0., 16., 800., 600.), 16.);
    assert_eq!(length(&a["border-top-width"], 0., 16., 800., 600.), 3.);
    assert_ne!(a.get(Prop::COLOR), Some("#00ff00"));
    let (doc, styles) = computed(source, Some("a"));
    assert_eq!(&styles.values[id(&doc, "a")]["color"], "#00ff00");
    let (doc, styles) = computed(source, Some("i"));
    assert_eq!(
        length(
            &styles.values[id(&doc, "i")]["padding-top"],
            0.,
            16.,
            800.,
            600.
        ),
        32.
    );
}

#[test]
fn block_variants_variant_rules_and_reference() {
    let source = "<html class='dark'><style>@import 'tailwindcss';
        @custom-variant hocus { &:hover { @slot; } &:focus { @slot; } }
        @custom-variant theme-dark { &:where(.dark, .dark *) { @media (width >= 1px) { @slot; } } }
        .x { color: #000001; @variant theme-dark { color: #000002; } }
        .y { @variant hocus { color: #000003; } }</style>
        <style>@reference 'tailwindcss'; .z { @apply pt-2; }</style>
        <body><p class='x theme-dark:pt-3'>a</p><a class='y hocus:pt-4'>b</a><b class='z'>c</b></body></html>";
    let (doc, styles) = computed(source, None);
    let p = &styles.values[id(&doc, "p")];
    assert_eq!(&p["color"], "#000002");
    assert_eq!(length(&p["padding-top"], 0., 16., 800., 600.), 12.);
    assert_eq!(
        length(
            &styles.values[id(&doc, "b")]["padding-top"],
            0.,
            16.,
            800.,
            600.
        ),
        8.
    );
    let (doc, styles) = computed(source, Some("a"));
    let a = &styles.values[id(&doc, "a")];
    assert_eq!(&a["color"], "#000003");
    assert_eq!(length(&a["padding-top"], 0., 16., 800., 600.), 16.);
    let (doc, styles) = computed(
        "<html><style>@reference 'tailwindcss'; .z { @apply pt-2; }</style><body><b class='z pt-9'>c</b></body></html>",
        None,
    );
    assert_eq!(
        length(
            &styles.values[id(&doc, "b")]["padding-top"],
            0.,
            16.,
            800.,
            600.
        ),
        8.
    );
}

#[test]
fn theme_namespaces_clear_with_initial() {
    assert_generated(
        "@theme { --color-*: initial; --color-brand: #123456; --breakpoint-*: initial; --breakpoint-tablet: 10rem; --font-*: initial; }",
        &[
            ("bg-red-500", ""),
            (
                "bg-brand",
                ".bg-brand{background-color:var(--color-brand);}",
            ),
            ("sm:flex", ""),
            (
                "tablet:flex",
                "@media (width >= 10rem){.tablet\\:flex{display:flex;}}",
            ),
            ("font-sans", ""),
            (
                "font-bold",
                ".font-bold{--tw-font-weight:var(--font-weight-bold);font-weight:var(--font-weight-bold);}",
            ),
        ],
    );
    assert_generated(
        "@theme { --*: initial; --spacing: 2px; }",
        &[
            ("p-2", ".p-2{padding:calc(var(--spacing) * 2);}"),
            ("text-sm", ""),
        ],
    );
}

#[test]
fn theme_variables_used_outside_utilities_are_defined() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body>
        <p style='color: var(--color-teal-700)'>a</p><svg><rect fill='var(--color-lime-300)'></rect></svg>
        </body></html>",
        None,
    );
    let root = &styles.values[id(&doc, "html")];
    assert!(root.get(Prop::new("--color-teal-700")).is_some());
    assert!(root.get(Prop::new("--color-lime-300")).is_some());
    assert_eq!(root.get(Prop::new("--color-indigo-600")), None);
}

#[test]
fn arbitrary_values_keep_modifiers_and_type_hints() {
    assert_generated(
        "",
        &[
            (
                "text-[13px]/[18px]",
                ".text-\\[13px\\]\\/\\[18px\\]{font-size:13px;line-height:18px;}",
            ),
            (
                "text-[13px]/5",
                ".text-\\[13px\\]\\/5{font-size:13px;line-height:calc(var(--spacing) * 5);}",
            ),
            (
                "font-[family-name:var(--x)]",
                ".font-\\[family-name\\:var\\(--x\\)\\]{font-family:var(--x);}",
            ),
            (
                "font-[number:var(--x)]",
                ".font-\\[number\\:var\\(--x\\)\\]{--tw-font-weight:var(--x);font-weight:var(--x);}",
            ),
            (
                "text-[length:var(--x)]",
                ".text-\\[length\\:var\\(--x\\)\\]{font-size:var(--x);}",
            ),
            ("text-[color:red]", ".text-\\[color\\:red\\]{color:red;}"),
            ("text-[larger]", ".text-\\[larger\\]{font-size:larger;}"),
            (
                "bg-[length:200px]",
                ".bg-\\[length\\:200px\\]{background-size:200px;}",
            ),
            (
                "bg-[position:center]",
                ".bg-\\[position\\:center\\]{background-position:center;}",
            ),
            (
                "bg-[url(/a_b.png)]",
                ".bg-\\[url\\(\\/a_b\\.png\\)\\]{background-image:url(/a_b.png);}",
            ),
            (
                "border-[length:var(--x)]",
                ".border-\\[length\\:var\\(--x\\)\\]{border-style:var(--tw-border-style);border-width:var(--x);}",
            ),
            (
                "stroke-[number:3]",
                ".stroke-\\[number\\:3\\]{stroke-width:3;}",
            ),
            (
                "content-['\\201C']",
                ".content-\\[\\'\\\\201C\\'\\]{--tw-content:'\\201C';content:var(--tw-content);}",
            ),
        ],
    );
}

#[test]
fn not_inverts_every_variant_kind() {
    assert_generated(
        "",
        &[
            (
                "not-supports-[display:grid]:flex",
                "@supports not (display:grid){.not-supports-\\[display\\:grid\\]\\:flex{display:flex;}}",
            ),
            (
                "supports-grid:flex",
                "@supports (grid: var(--tw)){.supports-grid\\:flex{display:flex;}}",
            ),
            (
                "not-supports-backdrop-filter:flex",
                "@supports not (backdrop-filter: var(--tw)){.not-supports-backdrop-filter\\:flex{display:flex;}}",
            ),
            (
                "not-data-open:flex",
                ".not-data-open\\:flex:not([data-open]){display:flex;}",
            ),
            (
                "not-aria-checked:flex",
                ".not-aria-checked\\:flex:not([aria-checked=\"true\"]){display:flex;}",
            ),
            (
                "not-hover:flex",
                ".not-hover\\:flex:not(:hover){display:flex;}@media not (hover: hover){.not-hover\\:flex{display:flex;}}",
            ),
            (
                "not-has-[>svg]:flex",
                ".not-has-\\[\\>svg\\]\\:flex:not(:has( > svg)){display:flex;}",
            ),
            (
                "not-[.x]:flex",
                ".not-\\[\\.x\\]\\:flex:not(:is(.x)){display:flex;}",
            ),
            (
                "not-md:flex",
                "@media not (width >= 48rem){.not-md\\:flex{display:flex;}}",
            ),
            (
                "not-print:flex",
                "@media not print{.not-print\\:flex{display:flex;}}",
            ),
            (
                "not-@sm/main:flex",
                "@container main not (width >= 24rem){.not-\\@sm\\/main\\:flex{display:flex;}}",
            ),
        ],
    );
}

#[test]
fn outline_hidden_only_draws_in_forced_colors() {
    assert_generated(
        "",
        &[(
            "outline-hidden",
            ".outline-hidden{--tw-outline-style:none;outline-style:none;}@media (forced-colors: active){.outline-hidden{outline:2px solid transparent;outline-offset:2px;}}",
        )],
    );
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body><p class='outline-2 outline-hidden'>a</p></body></html>",
        None,
    );
    assert_eq!(&styles.values[id(&doc, "p")]["outline-style"], "none");
}

#[test]
fn pseudo_element_variants_match_tailwind() {
    assert_generated(
        "",
        &[
            (
                "before:content-['x']",
                ".before\\:content-\\[\\'x\\'\\]::before{--tw-content:'x';content:var(--tw-content);}",
            ),
            (
                "after:inset-0",
                ".after\\:inset-0::after{content:var(--tw-content);inset:0px;}",
            ),
            (
                "placeholder:text-red-500",
                ".placeholder\\:text-red-500::placeholder{color:var(--color-red-500);}",
            ),
            (
                "marker:text-red-500",
                ".marker\\:text-red-500 *::marker,.marker\\:text-red-500::marker,.marker\\:text-red-500 *::-webkit-details-marker,.marker\\:text-red-500::-webkit-details-marker{color:var(--color-red-500);}",
            ),
            (
                "selection:bg-red-500",
                ".selection\\:bg-red-500 *::selection,.selection\\:bg-red-500::selection{background-color:var(--color-red-500);}",
            ),
            (
                "file:p-1",
                ".file\\:p-1::file-selector-button{padding:var(--spacing);}",
            ),
            (
                "backdrop:bg-black/50",
                ".backdrop\\:bg-black\\/50::backdrop{background-color:color-mix(in oklab, var(--color-black) 50%, transparent);}",
            ),
            (
                "first-letter:uppercase",
                ".first-letter\\:uppercase::first-letter{text-transform:uppercase;}",
            ),
            (
                "first-line:italic",
                ".first-line\\:italic::first-line{font-style:italic;}",
            ),
        ],
    );
}

#[test]
#[ignore]
fn bench() {
    for count in [500, 7500] {
        let html = format!(
            "<html><style>@import 'tailwindcss';</style><body>{}<div id=t></div><style id=s>.a{{color:red}}</style></body></html>",
            (0..count)
                .map(|i| format!("<i class='p-[{}px]'></i>", i + 100000))
                .collect::<String>()
        );
        let mut doc = Document::parse(&html);
        let mut cache = StyleCache::default();
        let mut sheets = HashMap::new();
        let mut pass = |doc: &Document| {
            compute_styles(doc, 800., 600., None, &mut sheets, &mut cache, None);
            cache.finish();
        };
        pass(&doc);
        let target = id(&doc, "#t");
        let start = std::time::Instant::now();
        for i in 0..10 {
            doc.nodes[target]
                .attrs
                .set("class", &format!("w-[{}px]", i + 50000));
            pass(&doc);
        }
        let class = start.elapsed() / 10;
        let start = std::time::Instant::now();
        for i in 0..10 {
            doc.nodes[target]
                .attrs
                .set("style", &format!("width:{}px", i + 1));
            pass(&doc);
        }
        let inline = start.elapsed() / 10;
        let style = id(&doc, "style#s");
        let text = doc.nodes[style].children.first().copied().unwrap_or(style);
        let start = std::time::Instant::now();
        for i in 0..10 {
            doc.nodes[text].text = format!(".b{i}{{color:red}}");
            pass(&doc);
        }
        let style = start.elapsed() / 10;
        println!(
            "{count} utilities: new class {class:?}, inline style {inline:?}, <style> text {style:?}"
        );
    }
}

#[test]
fn custom_variants_follow_classes() {
    for (class, expected) in [("dark", "#111111"), ("light", "#222222")] {
        let (doc, styles) = computed(
            &format!(
                "<html class='{class}'><style>@import 'tailwindcss';
                @custom-variant dark (&:is(.dark *, .dark));
                @custom-variant light (&:is(.light *));</style>
                <body><p class='dark:text-[#111111] light:text-[#222222]'>x</p></body></html>"
            ),
            None,
        );
        assert_eq!(&styles.values[id(&doc, "p")]["color"], expected);
    }
}

#[test]
fn variants_reach_pseudo_elements_attributes_and_descendants() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body>
        <div data-state='open' class=\"after:block after:w-[3px] after:content-[''] data-[state=open]:bg-[#123456] [&_svg]:size-4 bg-[#ff0000]/50\"><svg></svg></div>
        </body></html>",
        None,
    );
    let div = id(&doc, "div");
    assert_eq!(&styles.values[div]["background-color"], "#123456");
    assert_eq!(&styles.pseudos[&div][1]["width"], "3px");
    assert_eq!(&styles.pseudos[&div][1]["display"], "block");
    let svg = &styles.values[id(&doc, "svg")];
    assert_eq!(length(&svg["width"], 0., 16., 800., 600.), 16.);
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body><div class='bg-[#ff0000]/50'></div></body></html>",
        None,
    );
    assert_eq!(
        &styles.values[id(&doc, "div")]["background-color"],
        "oklab(0.627955 0.224863 0.125846 / 0.5)"
    );
}

#[test]
fn utilities_sit_between_base_layer_and_author_rules() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss'; @layer base { p { color: red; padding: 1px } } b { color: blue }</style>
        <body><p class='text-[#00ff00] p-2'>p</p><b class='text-[#00ff00]'>b</b></body></html>",
        None,
    );
    let p = &styles.values[id(&doc, "p")];
    assert_eq!(&p["color"], "#00ff00");
    assert_eq!(length(&p["padding-top"], 0., 16., 800., 600.), 8.);
    assert_eq!(&styles.values[id(&doc, "b")]["color"], "blue");
}

#[test]
fn apply_expands_variants_into_author_rules() {
    let source = "<html><style>@import 'tailwindcss';
        .link { @apply underline hover:text-[#123456]; }
        .card { &.open { @apply p-2; } }</style>
        <body><a class='link'>a</a><div class='card open'></div></body></html>";
    let (doc, styles) = computed(source, None);
    let link = &styles.values[id(&doc, "a")];
    assert_eq!(&link["text-decoration-line"], "underline");
    assert_ne!(link.get(Prop::COLOR), Some("#123456"));
    let card = &styles.values[id(&doc, "div")];
    assert_eq!(length(&card["padding-left"], 0., 16., 800., 600.), 8.);
    let (doc, styles) = computed(source, Some("a"));
    assert_eq!(&styles.values[id(&doc, "a")]["color"], "#123456");
}

#[test]
fn custom_utilities_resolve_values_from_inline_theme() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';
        @theme inline { --percentage-95: .95; --animate-in: enter var(--tw-duration,1s)var(--tw-ease,ease); }
        @utility fade-in { --tw-enter-opacity: 0; }
        @utility fade-in-* { --tw-enter-opacity: calc(--value(number)/100); --tw-enter-opacity: --value(--percentage-*,[*]); }
        @utility running { animation-play-state: running; }</style>
        <body><p class='fade-in-95 running animate-in'>a</p><b class='fade-in-[0.3] fade-in'>b</b></body></html>",
        None,
    );
    let p = &styles.values[id(&doc, "p")];
    assert_eq!(&p["--tw-enter-opacity"], ".95");
    assert_eq!(&p["animation-play-state"], "running");
    assert_eq!(&p["animation"], "enter 1s ease");
    assert_eq!(&styles.values[id(&doc, "b")]["--tw-enter-opacity"], "0.3");
}

#[test]
fn preflight_resets_elements_unless_left_out() {
    let body = "<body><h1>a</h1><ul><li>b</li></ul><img><button>c</button><a href='#'>d</a><div hidden class='flex'></div></body></html>";
    let (doc, styles) = computed(
        &format!(
            "<html><style>@import 'tailwindcss'; @layer base {{ h1 {{ font-size: var(--text-2xl) }} }}</style>{body}"
        ),
        None,
    );
    let style = |selector| &styles.values[id(&doc, selector)];
    assert_eq!(
        length(&style("body")["margin-top"], 0., 16., 800., 600.),
        0.
    );
    assert_eq!(&style("h1")["font-size"], "24px");
    assert_eq!(style("h1").get(Prop::FONT_WEIGHT), None);
    assert_eq!(&style("ul")["list-style-type"], "none");
    assert_eq!(&style("img")["display"], "block");
    assert_eq!(&style("button")["background-color"], "transparent");
    assert_eq!(style("a")["color"], style("body")["color"]);
    assert_eq!(&style("div")["display"], "none");
    assert!(style("html")["font-family"].starts_with("-apple-system"));
    let (doc, styles) = computed(
        &format!(
            "<html><style>@layer theme, base, components, utilities;
        @import 'tailwindcss/theme.css' layer(theme);
        @import 'tailwindcss/utilities.css' layer(utilities);</style>{body}"
        ),
        None,
    );
    assert_eq!(
        length(
            &styles.values[id(&doc, "body")]["margin-top"],
            0.,
            16.,
            800.,
            600.
        ),
        8.
    );
    assert_eq!(&styles.values[id(&doc, "ul")]["list-style-type"], "disc");
}

#[test]
fn gradients_and_divides_reach_computed_styles() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body>
        <div class='bg-gradient-to-br from-violet-600 to-fuchsia-500'></div>
        <ul class='divide-y divide-gray-100'><li>a</li><li>b</li></ul></body></html>",
        None,
    );
    assert_eq!(
        &styles.values[id(&doc, "div")]["background-image"],
        "linear-gradient(to bottom right in oklab, oklch(54.1% 0.281 293.009) 0%, oklch(66.7% 0.295 322.15) 100%)"
    );
    let width = |selector, side| {
        length(
            &styles.values[id(&doc, selector)][side],
            0.,
            16.,
            800.,
            600.,
        )
    };
    assert_eq!(width("li", "border-bottom-width"), 1.);
    assert_eq!(width("li", "border-top-width"), 0.);
    assert_eq!(width("li:last-child", "border-bottom-width"), 0.);
    let li = &styles.values[id(&doc, "li")];
    assert_eq!(&li["border-bottom-style"], "solid");
    assert_eq!(&li["border-bottom-color"], "oklch(96.7% 0.003 264.542)");
}

#[test]
fn relative_has_selectors_match_and_invalid_candidates_emit_nothing() {
    let (doc, styles) = computed(
        "<html><style>@import 'tailwindcss';</style><body>
        <div class='has-[>img]:p-2'><img></div><p class='p-1/2 -bg-red-500 [>img]:p-2'><img></p></body></html>",
        None,
    );
    let padding = |selector| {
        length(
            &styles.values[id(&doc, selector)]["padding-top"],
            0.,
            16.,
            800.,
            600.,
        )
    };
    assert_eq!(padding("div"), 8.);
    assert_eq!(padding("p"), 0.);
    assert_eq!(
        styles.values[id(&doc, "p")].get(Prop::BACKGROUND_COLOR),
        None
    );
}

#[test]
fn hostile_classes_stay_linear_and_bounded() {
    let start = std::time::Instant::now();
    let classes = [
        format!("{}flex", "marker:".repeat(12)),
        format!("{}flex", "[&&&&]:".repeat(12)),
        format!(
            "[color:{}x{}]",
            "--alpha(--theme(theme(".repeat(9),
            ")".repeat(27)
        ),
        format!("[x:{}]", "a-theme(".repeat(20_000)),
        format!(
            "[color:{}x{}]",
            "--alpha(".repeat(14_000),
            ")".repeat(14_000)
        ),
        "a-".repeat(40_000) + "a",
    ];
    let classes: Vec<_> = classes.iter().map(String::as_str).collect();
    let css = generated("", &classes);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(css[..2], ["", ""]);
    assert_eq!(
        generated("", &["marker:flex"]),
        [
            ".marker\\:flex *::marker,.marker\\:flex::marker,.marker\\:flex *::-webkit-details-marker,.marker\\:flex::-webkit-details-marker{display:flex;}"
        ]
    );
}

#[test]
fn custom_utilities_never_expand_forever() {
    assert_generated(
        "@utility tab-* { tab-size: --value(integer, [integer]); }
        @utility loop { @apply loop; color: red; }
        @utility a { @apply b; color: red; }
        @utility b { @apply a; width: 1px; }",
        &[
            (
                "tab-[integer:--value([integer])]",
                ".tab-\\[integer\\:--value\\(\\[integer\\]\\)\\]{tab-size:--value([integer]);}",
            ),
            ("loop", ".loop{color:red;}"),
            ("a", ".a{width:1px;color:red;}"),
        ],
    );
}
