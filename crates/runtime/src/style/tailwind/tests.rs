use super::super::{Prop, StyleCache, compute_styles, length};
use crate::dom::Document;
use std::collections::HashMap;

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
        "#ff000080"
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
    assert!(style("html")["font-family"].starts_with("ui-sans-serif"));
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
