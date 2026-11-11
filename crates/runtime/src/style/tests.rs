use super::*;

#[test]
fn sf_symbol_elements_have_inherited_color_and_native_defaults() {
    let doc = Document::parse(
        "<div style='color:#123456;font-size:20px'><sf-symbol-chevron-left id=icon></sf-symbol-chevron-left><span id=after></span><sf-symbol- id=empty></sf-symbol-><sf-symbol--folder id=invalid></sf-symbol--folder><span id=legacy class=sf-icon-folder></span><sf-symbol-folder id=hidden hidden></sf-symbol-folder></div>",
    );
    let values = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values;
    let icon = &values[element(&doc, "#icon")];
    for (prop, value) in [
        (Prop::DISPLAY, "inline-block"),
        (Prop::WIDTH, "1em"),
        (Prop::HEIGHT, "1em"),
        (Prop::VERTICAL_ALIGN, "middle"),
        (Prop::FLEX_SHRINK, "0"),
        (Prop::COLOR, "#123456"),
        (Prop::FONT_SIZE, "20px"),
    ] {
        assert_eq!(icon.get(prop), Some(value), "{prop:?}");
    }
    for selector in ["#after", "#empty", "#invalid", "#legacy"] {
        let style = &values[element(&doc, selector)];
        assert_eq!(style.get(Prop::DISPLAY).unwrap_or("inline"), "inline");
        assert!(!style.contains(Prop::WIDTH));
        assert!(!style.contains(Prop::HEIGHT));
        assert!(!style.contains(Prop::FLEX_SHRINK));
        assert_eq!(style.get(Prop::COLOR), Some("#123456"));
    }
    assert_eq!(
        values[element(&doc, "#hidden")].get(Prop::DISPLAY),
        Some("none")
    );
    assert!(Stylesheet::parse("@import 'sf-symbols';").0.is_empty());
    assert_eq!(supports("-apps-sf-symbol", "folder"), None);
    assert!(declare("-apps-sf-symbol:folder").is_empty());
}

#[test]
fn sf_symbol_defaults_follow_author_cascade_and_revert() {
    let doc = Document::parse(
        "<style>@import 'tailwindcss';@layer components{.authored{display:inline;width:21px;height:13px;vertical-align:top;flex-shrink:2}}</style><sf-symbol-folder id=utility class='size-16 shrink inline-flex align-bottom'></sf-symbol-folder><sf-symbol-folder id=author class=authored></sf-symbol-folder><sf-symbol-folder id=inline class=authored style='width:23px'></sf-symbol-folder><sf-symbol-folder id=revert class=authored style='all:revert'></sf-symbol-folder>",
    );
    let values = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values;
    let utility = &values[element(&doc, "#utility")];
    assert_eq!(length(&utility["width"], 0., 16., 800., 600.), 64.);
    assert_eq!(length(&utility["height"], 0., 16., 800., 600.), 64.);
    assert_eq!(utility.get(Prop::DISPLAY), Some("inline-flex"));
    assert_eq!(utility.get(Prop::VERTICAL_ALIGN), Some("bottom"));
    assert_eq!(utility.get(Prop::FLEX_SHRINK), Some("1"));
    let author = &values[element(&doc, "#author")];
    for (prop, value) in [
        (Prop::DISPLAY, "inline"),
        (Prop::WIDTH, "21px"),
        (Prop::HEIGHT, "13px"),
        (Prop::VERTICAL_ALIGN, "top"),
        (Prop::FLEX_SHRINK, "2"),
    ] {
        assert_eq!(author.get(prop), Some(value), "{prop:?}");
    }
    assert_eq!(
        values[element(&doc, "#inline")].get(Prop::WIDTH),
        Some("23px")
    );
    let reverted = &values[element(&doc, "#revert")];
    for (prop, value) in [
        (Prop::DISPLAY, "inline-block"),
        (Prop::WIDTH, "1em"),
        (Prop::HEIGHT, "1em"),
        (Prop::VERTICAL_ALIGN, "middle"),
        (Prop::FLEX_SHRINK, "0"),
    ] {
        assert_eq!(reverted.get(prop), Some(value), "{prop:?}");
    }
}

#[test]
fn sf_symbol_cached_styles_follow_classes_and_inheritance() {
    let mut doc = Document::parse(
        "<style>@import 'tailwindcss';</style><div><sf-symbol-folder id=icon></sf-symbol-folder></div>",
    );
    let sheet = element(&doc, "style");
    let parent = element(&doc, "div");
    let icon = element(&doc, "#icon");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (class, disabled, color, expected) in [
        ("size-16", false, "#123456", 64.),
        ("size-8", false, "#abcdef", 32.),
        ("size-8", true, "#112233", 20.),
        ("size-16", false, "#123456", 64.),
        ("sf-icon-trash", false, "#abcdef", 20.),
        ("", false, "#abcdef", 20.),
    ] {
        doc.nodes[icon].attrs.set("class", class);
        doc.nodes[parent]
            .attrs
            .set("style", &format!("color:{color};font-size:20px"));
        if disabled {
            doc.nodes[sheet].attrs.set("disabled", "");
        } else {
            doc.nodes[sheet].attrs.remove("disabled");
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
        assert_eq!(cached.values, fresh.values);
        let style = &cached.values[icon];
        assert_eq!(length(&style["width"], 0., 20., 800., 600.), expected);
        assert_eq!(length(&style["height"], 0., 20., 800., 600.), expected);
        assert_eq!(style.get(Prop::COLOR), Some(color));
        assert_eq!(style.get(Prop::DISPLAY), Some("inline-block"));
        let same = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert!(Rc::ptr_eq(&cached.values[icon], &same.values[icon]));
    }
}

#[test]
fn glass_variants_follow_native_support_and_cached_states() {
    let supported = renderer::supports_glass();
    let mut doc = Document::parse(
        "<style>@import 'tailwindcss';</style><button class='bg-glass not-supports-bg-glass:bg-[#123456] supports-bg-glass:w-[31px] not-supports-bg-glass:w-[47px] h-[11px] md:supports-bg-glass:hover:h-[33px] md:not-supports-bg-glass:hover:h-[49px]'></button>",
    );
    let button = element(&doc, "button");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (width, hovered) in [(800., false), (800., true), (600., true), (800., false)] {
        doc.nodes[button].state = if hovered { crate::dom::HOVER } else { 0 };
        let cached = compute_styles(&doc, width, 600., None, &mut sheets, &mut cache, None);
        let fresh = compute_styles(
            &doc,
            width,
            600.,
            None,
            &mut sheets,
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values);
        let style = &cached.values[button];
        assert_eq!(
            style.get(Prop::BACKGROUND_COLOR),
            Some(if supported { "transparent" } else { "#123456" })
        );
        assert_eq!(
            style.get(Prop::WIDTH),
            Some(if supported { "31px" } else { "47px" })
        );
        assert_eq!(
            style.get(Prop::HEIGHT),
            Some(if hovered && width >= 768. {
                if supported { "33px" } else { "49px" }
            } else {
                "11px"
            })
        );
    }
}

#[test]
fn glass_capability_remains_a_runtime_condition_in_bundles() {
    let sheet = Stylesheet::parse(
        "@media (-macos-glass){div{color:#112233}}@media not (-macos-glass){div{color:#445566}}",
    );
    let bytes = postcard::to_allocvec(&sheet).unwrap();
    let sheet: Stylesheet = postcard::from_bytes(&bytes).unwrap();
    assert!(
        matches!(&*sheet.0[0], Rule::Media(queries, _) if matches!(queries.as_slice(), [MediaQuery::Feature(name, op, value)] if name == "-macos-glass" && op.is_empty() && value.is_empty()))
    );
    let mut doc = Document::parse("<style></style><div></div>");
    let style = element(&doc, "style");
    let div = element(&doc, "div");
    doc.nodes[style].stylesheet = Some(Rc::new(sheet));
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    assert_eq!(
        computed.values[div].get(Prop::COLOR),
        Some(if renderer::supports_glass() {
            "#112233"
        } else {
            "#445566"
        })
    );
}

#[test]
fn glass_properties_are_known_declarations() {
    for (name, value, prop) in [
        ("-macos-glass", "regular", Prop::MACOS_GLASS),
        (
            "-macos-glass-interactive",
            "true",
            Prop::MACOS_GLASS_INTERACTIVE,
        ),
        (
            "-macos-glass-tint-color",
            "#123456",
            Prop::MACOS_GLASS_TINT_COLOR,
        ),
    ] {
        assert_eq!(Prop::find(name), Some(prop));
        assert_eq!(supports(name, value), Some(true));
        assert_eq!(
            declare(&format!("{name}:{value}")),
            vec![[name.to_owned(), value.to_owned(), String::new()]]
        );
    }
    assert!(declare("unknown-property: red").is_empty());
}

#[test]
fn glass_tints_follow_theme_changes_and_color_utilities() {
    let mut doc = Document::parse(
        r#"<style>
            @import 'tailwindcss';
            @theme { --color-brand: #123456; }
            @theme inline { --color-accent: var(--accent); }
            .theme { --accent: #112233; color: #fedcba; }
            .theme.dark { --color-brand: #abcdef; --accent: #654321; }
            #applied { @apply bg-glass-brand/50; }
            #reset { @apply bg-glass-brand/50 bg-white border-2 appearance-auto; }
        </style><section class=theme>
            <button id=brand class='bg-glass-brand'></button>
            <button id=alpha class='bg-glass-brand/50'></button>
            <button id=inline class='bg-glass-accent'></button>
            <button id=current class='bg-glass-current/50'></button>
            <button id=applied></button>
            <button id=reset></button>
            <button id=explicit class='bg-glass-brand bg-white border-2 appearance-auto'></button>
            <button id=variant class='bg-glass-[#123abc] hover:bg-glass-brand'></button>
            <button id=hoveronly class='hover:bg-glass-brand bg-glass-interactive'></button>
            <button id=legacy class='bg-glass-blue'></button>
        </section>"#,
    );
    let host = element(&doc, "section");
    let variant = element(&doc, "#variant");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for dark in [false, true, false] {
        doc.nodes[host]
            .attrs
            .set("class", if dark { "theme dark" } else { "theme" });
        doc.nodes[variant].state = if dark { crate::dom::HOVER } else { 0 };
        let hoveronly = element(&doc, "#hoveronly");
        doc.nodes[hoveronly].state = if dark { crate::dom::HOVER } else { 0 };
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
        assert_eq!(cached.values, fresh.values);
        let brand = if dark { 0xabcdef } else { 0x123456 };
        for (selector, expected) in [
            ("#brand", (brand, 255)),
            ("#alpha", (brand, 128)),
            ("#inline", (if dark { 0x654321 } else { 0x112233 }, 255)),
            ("#current", (0xfedcba, 128)),
            ("#applied", (brand, 128)),
            ("#variant", (if dark { brand } else { 0x123abc }, 255)),
        ] {
            let values = &cached.values[element(&doc, selector)];
            assert_eq!(
                values.get(Prop::MACOS_GLASS_TINT_COLOR).and_then(color),
                Some(expected),
                "{selector}, dark={dark}"
            );
            assert_eq!(
                values.get(Prop::MACOS_GLASS),
                cfg!(target_os = "macos").then_some("regular")
            );
            if cfg!(target_os = "macos") {
                assert_eq!(values.get(Prop::BACKGROUND_COLOR), Some("transparent"));
                assert_eq!(values.get(Prop::APPEARANCE), Some("none"));
                assert_eq!(values.get(Prop::BORDER_TOP_WIDTH), Some("0"));
            }
        }
        assert!(!cached.values[element(&doc, "#legacy")].contains(Prop::MACOS_GLASS_TINT_COLOR));
        assert!(!cached.values[element(&doc, "#legacy")].contains(Prop::MACOS_GLASS));
        for selector in ["#reset", "#explicit"] {
            let values = &cached.values[element(&doc, selector)];
            assert_eq!(values.get(Prop::BACKGROUND_COLOR), Some("#fff"));
            assert_eq!(values.get(Prop::BORDER_TOP_WIDTH), Some("2px"));
            assert_eq!(values.get(Prop::APPEARANCE), Some("auto"));
        }
        assert_eq!(
            cached.values[hoveronly].get(Prop::MACOS_GLASS),
            (cfg!(target_os = "macos") && dark).then_some("regular")
        );
    }
}

#[test]
fn glass_classes_are_builtin_and_independent_of_tailwind() {
    for imports in [
        "",
        "@import 'tailwindcss';",
        "span{color:red}</style><style>@import 'tailwindcss';",
        "@import 'tailwindcss';</style><style>span{color:red}",
    ] {
        let doc = Document::parse(&format!(
            "<style>{imports}</style><button class='bg-glass bg-glass-interactive bg-glass-blue-500'><span>Save</span></button><div class=bg-glass-clear></div><button id=utility class='bg-glass bg-white rounded-lg'></button><button id=tinted class='bg-glass-blue-500 bg-glass-interactive'></button><button id=modifier class=bg-glass-interactive></button>"
        ));
        let values = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        )
        .values;
        let active = cfg!(target_os = "macos");
        assert_eq!(
            values[element(&doc, "#tinted")].get(Prop::MACOS_GLASS),
            (active && imports.contains("tailwindcss")).then_some("regular")
        );
        assert!(!values[element(&doc, "#modifier")].contains(Prop::MACOS_GLASS));
        let button = &values[element(&doc, "button")];
        assert_eq!(button.get(Prop::MACOS_GLASS), active.then_some("regular"));
        assert_eq!(
            button.get(Prop::MACOS_GLASS_INTERACTIVE),
            active.then_some("true")
        );
        assert_eq!(
            button.get(Prop::MACOS_GLASS_TINT_COLOR).and_then(color),
            imports
                .contains("tailwindcss")
                .then(|| color("oklch(62.3% 0.214 259.815)").unwrap())
        );
        assert_eq!(
            values[element(&doc, "div")].get(Prop::MACOS_GLASS),
            active.then_some("clear")
        );
        for prop in [
            Prop::MACOS_GLASS,
            Prop::MACOS_GLASS_INTERACTIVE,
            Prop::MACOS_GLASS_TINT_COLOR,
        ] {
            assert!(!values[element(&doc, "span")].contains(prop));
        }
        if active {
            assert_eq!(button.get(Prop::APPEARANCE), Some("none"));
            assert_eq!(button.get(Prop::BACKGROUND_COLOR), Some("transparent"));
            assert_eq!(button.get(Prop::BORDER_TOP_WIDTH), Some("0"));
            assert_eq!(button.get(Prop::BORDER_TOP_LEFT_RADIUS).unwrap_or("0"), "0");
        }
        if imports.contains("tailwindcss") {
            let utility = &values[element(&doc, "#utility")];
            assert_eq!(utility.get(Prop::BACKGROUND_COLOR), Some("#fff"));
            assert_eq!(utility.get(Prop::BORDER_TOP_LEFT_RADIUS), Some("0.5rem"));
        }
    }
}

#[test]
fn glass_classes_follow_cascade_and_cached_changes() {
    let mut doc = Document::parse(
        "<style>@import 'tailwindcss';@layer components{.component{appearance:auto;background-color:red}}#override{-macos-glass:clear;-macos-glass-tint-color:var(--tint);background-color:red}</style><button id=changing></button><button id=override class='bg-glass bg-glass-blue-500' style='--tint:#123456'></button><button id=utility class='bg-glass rounded-lg bg-white'></button><button id=component class='bg-glass component'></button>",
    );
    let changing = element(&doc, "#changing");
    let overridden = element(&doc, "#override");
    let utility = element(&doc, "#utility");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (class, effect, tint) in [
        (
            "bg-glass bg-glass-interactive bg-glass-blue-500",
            Some("regular"),
            "#123456",
        ),
        ("bg-glass-clear", Some("clear"), "#abcdef"),
        (
            "bg-glass-blue-500 bg-glass-interactive",
            Some("regular"),
            "#112233",
        ),
        (
            "bg-glass-clear bg-glass-blue-500 bg-glass-interactive",
            Some("clear"),
            "#223344",
        ),
        ("bg-glass-interactive", None, "#334455"),
        ("glass glass-clear glass-tint-blue-500", None, "#654321"),
        ("", None, "#fedcba"),
    ] {
        doc.nodes[changing].attrs.set("class", class);
        doc.nodes[overridden]
            .attrs
            .set("style", &format!("--tint:{tint}"));
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
        assert_eq!(cached.values, fresh.values);
        assert_eq!(
            cached.values[changing].get(Prop::MACOS_GLASS),
            effect.filter(|_| cfg!(target_os = "macos"))
        );
        assert_eq!(
            cached.values[changing].get(Prop::MACOS_GLASS_INTERACTIVE),
            (cfg!(target_os = "macos")
                && class
                    .split_ascii_whitespace()
                    .any(|class| class == "bg-glass-interactive"))
            .then_some("true")
        );
        assert_eq!(
            cached.values[overridden].get(Prop::MACOS_GLASS),
            Some("clear")
        );
        assert_eq!(
            cached.values[overridden].get(Prop::MACOS_GLASS_TINT_COLOR),
            Some(tint)
        );
        assert_eq!(
            cached.values[overridden].get(Prop::BACKGROUND_COLOR),
            Some("red")
        );
        assert_eq!(
            cached.values[utility].get(Prop::BACKGROUND_COLOR),
            Some("#fff")
        );
        assert_eq!(
            cached.values[utility].get(Prop::BORDER_TOP_LEFT_RADIUS),
            Some("0.5rem")
        );
        let component = &cached.values[element(&doc, "#component")];
        assert_eq!(component.get(Prop::BACKGROUND_COLOR), Some("red"));
        assert_eq!(component.get(Prop::APPEARANCE), Some("auto"));
    }
}

#[test]
fn builtin_glass_survives_cssom_edits_and_serialization() {
    let source = "div{-macos-glass:clear;-macos-glass-interactive:false}";
    let sheet = Stylesheet::parse(source);
    let bytes = postcard::to_allocvec(&sheet).unwrap();
    let mut sheet: Stylesheet = postcard::from_bytes(&bytes).unwrap();
    sheet.bind_parents().unwrap();
    let mut doc =
        Document::parse("<style></style><div class='bg-glass bg-glass-interactive'></div>");
    let style = element(&doc, "style");
    let glass = element(&doc, "div");
    doc.nodes[style].stylesheet = Some(Rc::new(sheet.clone()));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let before = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(before.values[glass].get(Prop::MACOS_GLASS), Some("clear"));
    assert_eq!(
        before.values[glass].get(Prop::MACOS_GLASS_INTERACTIVE),
        Some("false")
    );
    sheet.edit(&[0], Edit::Delete).unwrap();
    doc.nodes[style].stylesheet = Some(Rc::new(sheet));
    let after = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        after.values[glass].get(Prop::MACOS_GLASS),
        cfg!(target_os = "macos").then_some("regular")
    );
    assert_eq!(
        after.values[glass].get(Prop::MACOS_GLASS_INTERACTIVE),
        cfg!(target_os = "macos").then_some("true")
    );
}

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
        .map(|rule| match &**rule {
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
    assert!(compute(&mut cache)[outer].get(Prop::COLOR) != Some("red"));
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [500., 50.]), (b, [200., 100.])]),
        &doc
    ));
    let values = compute(&mut cache);
    assert_eq!(&values[outer]["color"], "red");
    assert_eq!(&values[outer]["order"], "5");
    assert!(!values[outer].contains(Prop::WIDTH) && !values[outer].contains(Prop::Z_INDEX));
    assert_eq!(&values[inner]["color"], "red");
    assert_eq!(&values[inner]["width"], "1px");
    assert!(!values[inner].contains(Prop::ORDER) && !values[inner].contains(Prop::HEIGHT));
    assert!(!cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [450., 50.]), (b, [200., 100.])]),
        &doc
    ));
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [90., 50.]), (b, [200., 100.])]),
        &doc
    ));
    let values = compute(&mut cache);
    assert!(values[outer].get(Prop::COLOR) != Some("red"));
    assert_eq!(&values[outer]["z-index"], "3");
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
        "<style>.on span{color:red}i+span{font-weight:700}span:nth-child(2){background-color:yellow}div:has(.mark) span{color:blue}span:empty::before{content:'empty'}div[style*='--on'] span{outline-style:solid}span{width:var(--w)}</style><div><i>head</i><span id=a>alpha</span><span id=b></span></div>",
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
            9 => {
                doc.nodes[parent]
                    .attrs
                    .insert("style".into(), "--w:3px".into());
            }
            10 => {
                doc.nodes[parent]
                    .attrs
                    .insert("style".into(), "--w:4px;--on:1".into());
            }
            11 => {
                doc.nodes[parent]
                    .attrs
                    .insert("style".into(), "--w:4px".into());
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
fn custom_property_changes_keep_the_styles_that_dont_read_them() {
    let mut doc = Document::parse(
        "<style>p{color:var(--ink)}p::before{content:'+';color:var(--ink)}b{color:var(--t)}</style><div id=a><p>x</p><span>y</span><i style='--t:var(--w)'>z</i><b style='--t:red'>q</b></div>",
    );
    let [a, p, span, i, b] = ["#a", "p", "span", "i", "b"].map(|selector| element(&doc, selector));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut last: Option<Rc<ComputedStyles>> = None;
    for (step, inline) in [
        "--w:1px;--ink:blue",
        "--w:2px;--ink:blue",
        "--w:2px;--ink:red",
    ]
    .into_iter()
    .enumerate()
    {
        doc.nodes[a].inline = Some(Declarations::parse(inline));
        let context = StyleContext {
            previous: last.clone(),
            ..Default::default()
        };
        let cached = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut cache,
            Some(&context),
        );
        let fresh = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values, "step {step}");
        assert_eq!(cached.vars, fresh.vars, "step {step}");
        assert_eq!(cached.pseudos, fresh.pseudos, "step {step}");
        let var = |id: usize, name: &str| {
            let prop = Prop::find(name).unwrap();
            cached.vars[id]
                .iter()
                .find(|(key, _)| *key == prop)
                .map(|(_, value)| value.to_string())
        };
        assert_eq!(var(i, "--t").as_deref(), Some(&inline[4..7]));
        assert_eq!(var(b, "--t").as_deref(), Some("red"));
        if let Some(last) = &last {
            let kept = |id: usize| Rc::ptr_eq(&last.values[id], &cached.values[id]);
            assert!(kept(span) && kept(b) && kept(i), "step {step}");
            assert_eq!(kept(p), step == 1, "step {step}");
            assert_eq!(
                Rc::ptr_eq(&last.pseudos[&p], &cached.pseudos[&p]),
                step == 1,
                "step {step}"
            );
        }
        last = Some(Rc::new(cached));
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
    assert_eq!(inline.boxes.len(), 1);
    assert_eq!(inline.boxes[0].fill, 0xffee00ff);
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
        .insert("style".into(), "opacity:0.5".into());
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
fn inline_boxes_ignore_properties_that_do_not_apply_to_them() {
    let doc = Document::parse(
        "<style>a{justify-self:start;width:10px;margin-top:-10px;-webkit-text-decoration-color:red;text-underline-offset:4px}</style><p>the <a href=x>/hairline-create</a> skill</p>",
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
    let inline = inline_text(&doc, &computed, element(&doc, "p")).unwrap();
    assert_eq!(&*inline.text, "the /hairline-create skill");
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
                .boxes
                .iter()
                .map(|inline_box| (inline.ranges[inline_box.index].1.clone(), inline_box.fill))
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
fn scrollbar_parts_reach_scroll_containers_only() {
    let doc = Document::parse(
        "<style>::-webkit-scrollbar{width:8px}.s::-webkit-scrollbar-thumb{background:red;border-radius:4px}.s::-webkit-scrollbar-thumb:hover{background:blue}::-webkit-scrollbar-thumb:vertical{background:green}::-webkit-scrollbar-track-piece{background:#eee}.s{overflow-y:auto}</style><div class=s></div><p></p>",
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
    let parts = &computed.scrollbars[&element(&doc, "div")];
    assert_eq!(&parts[0].as_ref().unwrap()["width"], "8px");
    assert_eq!(&parts[1].as_ref().unwrap()["background-color"], "#eee");
    let [thumb, hovered] = [&parts[2], &parts[4]].map(|part| part.as_ref().unwrap());
    assert_eq!(
        (&thumb["background-color"], &hovered["background-color"]),
        ("red", "blue")
    );
    assert_eq!(&hovered["border-top-left-radius"], "4px");
    assert!(!computed.scrollbars.contains_key(&element(&doc, "p")));
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
    assert_eq!(&marker["color"], "blue");
    assert!(marker.contains(Prop::CONTENT) && marker.contains(Prop::FONT_SIZE));
    assert!(computed.text_pseudos[&p][0].is_none());
    let inline = inline_text(&doc, &computed, p).unwrap();
    assert_eq!(&*inline.text, "\"Hi there x");
    assert_eq!(
        inline.runs.iter().map(|run| run.len).sum::<usize>(),
        inline.text.len()
    );
    let ink = |color: u32| renderer::Color::hex(color);
    assert_eq!(
        (inline.runs[0].len, inline.runs[0].color),
        (2, ink(0xff0000ff))
    );
    assert_eq!(inline.runs[1].color, ink(0x111111ff));
    assert_eq!(
        *inline.shadows,
        [(0., 0., 3., 0x111111ff), (1., 2., 0., 0xff0000ff)]
    );
    let (base, pseudo, mode) = inline.first_line.as_deref().unwrap();
    assert_eq!(mode, &None);
    let runs = restyle(&inline.runs, 0..inline.text.len(), base, pseudo);
    let colors: Vec<_> = runs.iter().map(|run| run.color).collect();
    assert_eq!(
        colors[..3],
        [ink(0xff0000ff), ink(0x0000ffff), ink(0x00ff00ff)]
    );
    assert!(runs.iter().all(|run| run.font.weight == 700.));
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
        &computed.values[element(&doc, "td")]["vertical-align"],
        "middle"
    );
    assert_eq!(
        &computed.values[element(&doc, "td+td")]["vertical-align"],
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
    assert_eq!(&inherited.values[children[0]]["color"], "green");
    assert_eq!(
        &inherited.selections[&children[0]]["background-color"],
        "green"
    );
    doc.nodes[children[1]].tag = "b".into();
    let matched = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(&matched.values[children[0]]["color"], "blue");
    let sheet = doc
        .nodes
        .iter()
        .position(|node| node.tag == "style")
        .unwrap();
    let text = doc.nodes[sheet].children[0];
    doc.nodes[text].text = "span{color:purple}".into();
    doc.nodes[sheet].stylesheet = None;
    let changed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(&changed.values[children[0]]["color"], "purple");
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
        assert!(!computed.selections[&id].contains(Prop::FONT_SIZE));
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
    assert_eq!(&narrow.values[id]["caret-color"], "red");
    assert_eq!(&wide.values[id]["caret-color"], "#0000");
    assert_eq!(&wide.selections[&id]["background-color"], "#0000");
    assert_eq!(
        selection_colors(&wide.values[id], wide.selections.get(&id)),
        (None, 0)
    );
    let div = doc.nodes[id]
        .parent
        .and_then(|parent| doc.nodes[parent].parent)
        .unwrap();
    assert_eq!(
        wide.values[div].contains(Prop::ANIMATION),
        !reduced_motion()
    );
    let motion = |value: &str| {
        MediaQuery::Feature("prefers-reduced-motion".into(), ":".into(), value.into()).matches(
            800.,
            600.,
            true,
            0,
            FontUnits::new(16.),
        )
    };
    assert_eq!(motion("reduce"), Some(true));
    assert_eq!(motion("no-preference"), Some(false));
}

#[test]
fn app_region_is_inherited_and_accepts_the_webkit_name() {
    let doc = Document::parse(
        r#"<style>
            header { -webkit-app-region: drag; }
            button { app-region: no-drag; }
        </style><header><span id="title">Title</span><button id="close"><b id="icon">x</b></button></header><main id="main"></main>"#,
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
    let region = |selector| {
        computed.values[element(&doc, selector)]
            .get(Prop::APP_REGION)
            .map(str::to_owned)
    };
    assert_eq!(region("header").as_deref(), Some("drag"));
    assert_eq!(region("#title").as_deref(), Some("drag"));
    assert_eq!(region("#icon").as_deref(), Some("no-drag"));
    assert_eq!(region("#main"), None);
}

#[test]
fn inline_boxes_and_atoms_stay_in_one_text_layout() {
    let doc = Document::parse(
        r#"<style>code { padding: 1px 4px; border: 1px solid red; background: #111 }
            .b { display: inline-block; width: 10px } .n { white-space: nowrap } img { display: inline }
            .flex { display: flex }</style>
        <p id="p">a <code id="c">b</code> <span class="b"></span> <span class="n"><img>d</span> e</p>
        <p id="alone"><span class="b"></span></p><div id="flex" class="flex"><img>text</div>"#,
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
    let inline = inline_text(&doc, &computed, element(&doc, "#p")).unwrap();
    assert_eq!(&*inline.text, "a b \u{fffc} \u{fffc}d e");
    assert_eq!(inline.atoms.len(), 2);
    assert_eq!(inline.boxes.len(), 1);
    assert_eq!(inline.boxes[0].edges, [2., 5., 2., 5.]);
    assert_eq!(inline.boxes[0].lead, 5.);
    assert_eq!(inline.boxes[0].trail, 0.);
    let run_of = |text: &str| {
        let at = inline.text.find(text).unwrap();
        let mut end = 0;
        inline
            .runs
            .iter()
            .find(|run| {
                end += run.len;
                end > at
            })
            .unwrap()
    };
    assert_eq!(run_of("b").inset[0], renderer::px(5.));
    assert_eq!(run_of("b").inset[1], renderer::px(5.));
    assert!(run_of("d").nowrap);
    assert!(!run_of("e").nowrap);
    assert!(inline_text(&doc, &computed, element(&doc, "#alone")).is_none());
    assert!(inline_text(&doc, &computed, element(&doc, "#flex")).is_none());
}

#[test]
fn a_stronger_transition_shorthand_resets_the_longhands() {
    let doc = Document::parse(
        r#"<style>
            div { transition-property: transform; transition-duration: 1s; }
            #c { transition: opacity 2s; transition-duration: 3s; }
        </style><div id="a" style="transition: none"></div><div id="b"></div><div id="c"></div>"#,
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
    let style = |selector| &computed.values[element(&doc, selector)];
    assert_eq!(style("#a").get(Prop::TRANSITION), Some("none"));
    assert_eq!(style("#a").get(Prop::TRANSITION_PROPERTY), None);
    assert_eq!(
        style("#b").get(Prop::TRANSITION_PROPERTY),
        Some("transform")
    );
    assert_eq!(style("#c").get(Prop::TRANSITION), Some("opacity 2s"));
    assert_eq!(style("#c").get(Prop::TRANSITION_DURATION), Some("3s"));
    assert_eq!(style("#c").get(Prop::TRANSITION_PROPERTY), None);
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
    assert_eq!(&computed.values[id]["color"], "blue");
    assert_eq!(&computed.values[id]["background-color"], "red");
    assert_eq!(&computed.values[id]["display"], "grid");
    assert_eq!(&computed.values[id]["width"], "100cqi");
    assert_eq!(&computed.values[id]["cursor"], "pointer");
    assert!(!computed.values[id].contains(Prop::CONTENT));
    assert_eq!(&computed.pseudos[&id][0]["background-color"], "red");
    assert_eq!(&computed.pseudos[&id][0]["width"], "4px");
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
            .wrap { --edge:2px solid #fff; --bg:#fff; --ink:light-dark(#f00,#0f0); color-scheme:dark; }
            .item { border-right:var(--edge); border-left:1px solid var(--bg); padding-inline:1ch 2ch; margin-block:8px; color:var(--ink); background-color:color-mix(in lab, white 50%, black); }
            .item span { background-color:light-dark(inherit,inherit); }
        </style><div class="wrap"><div class="item"><span>text</span></div></div>"#,
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
    assert_eq!(
        [
            &computed[item]["color"],
            &computed[item]["border-top-color"],
            &computed[item]["background-color"],
            &computed[span]["color"],
            &computed[span]["fill"]
        ],
        [
            "color(srgb 1 0 0)",
            "color(srgb 1 0 0 / 0.5)",
            "oklch(0.627955 0.257683 29.2339 / 0.5)",
            "color(srgb 1 0.5 0.5)",
            "lab(68.6724 49.4694 23.9289)"
        ]
    );
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
    assert_eq!(&style["color"], "red !important");
    assert_eq!(&style["padding-top"], "2px");
    assert_eq!(&style["padding-right"], "4px");
    assert_eq!(&style["--Case"], r#"{ key: [a;b]; nested: fn(";}"); }"#);
    assert_eq!(&style["--case"], r#""/* text */;escaped\"}""#);
    assert_eq!(&style["--tokens"], "one/**/two");
    assert_eq!(&style["content"], r#""!important""#);
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
    assert_eq!(&style["color"], "red");
    assert_eq!(&style["height"], "7px");
    assert_eq!(&style["padding-left"], "3px");
    assert_eq!(&style["margin-bottom"], "4px");
    assert_eq!(&style["border-top-width"], "2px");
    assert_eq!(&style["--empty"], "");
    for key in ["width", "content", "background", "opacity"] {
        assert!(
            Prop::find(key).is_none_or(|prop| !style.contains(prop)),
            "{key}"
        );
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
        assert_eq!(
            Lengths::new(16., width, 600.).edges(&typed(style), PADDING, width),
            [3.; 4]
        );
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
        computed.lengths[section].edges(&computed.values[section], PADDING, 800.),
        [30., 40., 30., 40.]
    );
    assert_eq!(computed.lengths[absolute].font.em, 20.);
    assert_eq!(computed.lengths[absolute].font.lh, 60.);
    assert_eq!(&computed.values[absolute]["line-height"], "60px");
    assert_eq!(computed.lengths[number].font.lh, 40.);
    assert_eq!(computed.lengths[child].font.em, 10.);
    assert_eq!(computed.lengths[child].font.lh, 20.);
    assert_eq!(&computed.values[child]["line-height"], "2");
    assert_eq!(computed.lengths[relative].font.lh, 120.);
    assert_eq!(
        computed.lengths[relative].edges(&computed.values[relative], MARGIN, 800.),
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
    let doc =
        Document::parse("<html style='line-height:2rlh'><div style='font-size:1lh'></div></html>");
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
    assert_eq!(units.edges(style, PADDING, 800.), [22., 30., 22., 30.]);
    assert_eq!(units.resolve(&style["height"], 600.), Some(560.));
    assert_eq!(units.resolve(&style["width"], 800.), Some(400.));
    let shadows = units.shadows(&style["box-shadow"], false);
    assert_eq!(shadows[0].offset.y, renderer::px(30.));
    assert_eq!(shadows[0].blur_radius, renderer::px(4.));
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
    assert_eq!(
        length(&style["border-bottom-left-radius"], 0., 16., 800., 600.),
        8.
    );
    assert_eq!(
        length(&style["border-bottom-right-radius"], 0., 16., 800., 600.),
        8.
    );
    assert!(!style.contains_key("border-top-left-radius"));
    let paint = Lengths::new(16., 800., 600.).shadows(&style["box-shadow"], false);
    let paint: Vec<_> = paint.iter().filter(|shadow| shadow.color.a > 0.).collect();
    assert_eq!(paint.len(), 2);
    assert_eq!(
        paint[1].offset,
        renderer::point(renderer::px(0.), renderer::px(4.))
    );
    assert_eq!(paint[1].blur_radius, renderer::px(3.));
    assert_eq!(paint[1].spread_radius, renderer::px(-1.));
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
    assert_eq!(length(&style["font-size"], 0., 16., 799., 600.), 32.);
    assert_eq!(style["font-family"], "Helvetica");
    assert_eq!(style["font-weight"], "850");
    assert_eq!(
        Lengths::new(32., 799., 600.).edges(&typed(&style), PADDING, 799.),
        [20., 20., 20., 10.]
    );
    assert_eq!(
        Lengths::new(32., 799., 600.).edges(&typed(&style), MARGIN, 799.),
        [-23., 0., -10., 0.]
    );
    assert_eq!(length(&style["width"], 799., 32., 799., 600.), 23.);
    assert_eq!(
        Lengths::new(32., 799., 600.)
            .shadows(&style["box-shadow"], false)
            .iter()
            .filter(|shadow| shadow.color.a > 0.)
            .count(),
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
    assert_eq!(outer[0].blur_radius, renderer::px(4.));
    assert_eq!(inset.len(), 2);
    assert_eq!(
        inset[0].offset,
        renderer::point(renderer::px(1.), renderer::px(2.))
    );
    assert_eq!(inset[1].spread_radius, renderer::px(2.));
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
    assert_eq!(
        shadows[0].offset,
        renderer::point(renderer::px(2.), renderer::px(3.))
    );
    assert_eq!(shadows[0].color, renderer::Color::hex(0x00ffffff));
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
        for (width, expected) in [(639., 16.), (640., 32.), (768., 48.), (1024., 64.)] {
            let style = styles(&doc, width, 600.).pop().unwrap();
            let px = |key: &str| length(&style[key], 0., 16., width, 600.);
            assert_eq!(px("width"), expected);
            assert_eq!(px("height"), 32.);
            assert_eq!(
                Lengths::new(16., width, 600.).edges(&typed(&style), PADDING, width),
                [32., 16., 32., 8.]
            );
            assert_eq!(px("row-gap"), 16.);
            assert_eq!(px("column-gap"), 8.);
        }
    }
}

#[test]
fn css_and_inline_shorthands_override_utilities() {
    let doc = Document::parse(
        "<style>@import 'tailwindcss'; div { padding: 2px 3px; gap: 4px; background: blue; width: 20px !important; } .box { padding-left: 5px; }</style><div class='box p-8 bg-white gap-8 w-4' style='padding: 6px; gap: 7px 8px; background: black; width: 30px'></div>",
    );
    let style = styles(&doc, 800., 600.).pop().unwrap();
    assert_eq!(
        Lengths::new(16., 800., 600.).edges(&typed(&style), PADDING, 800.),
        [6.; 4]
    );
    assert_eq!(style["background-color"], "black");
    assert_eq!(style["row-gap"], "7px");
    assert_eq!(style["column-gap"], "8px");
    assert_eq!(style["width"], "20px");
    let style = declarations(
        "padding: 1px !important; padding: 2px; padding-left: 3px; padding-right: 4px !important",
    );
    assert_eq!(&style["padding-left"], "1px !important");
    assert_eq!(&style["padding-right"], "4px !important");
    let doc = Document::parse(
        "<html><style>@import 'tailwindcss'; :root { --space: 2px 4px; } div { padding: var(--space); gap: var(--missing, 5px 6px); margin: var(--space); margin-left: 9px; }</style><div class='p-8 gap-8 m-8'></div></html>",
    );
    let style = styles(&doc, 800., 600.).pop().unwrap();
    assert_eq!(
        Lengths::new(16., 800., 600.).edges(&typed(&style), PADDING, 800.),
        [2., 4., 2., 4.]
    );
    assert_eq!(
        Lengths::new(16., 800., 600.).edges(&typed(&style), MARGIN, 800.),
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
    let px = |key: &str| length(&style[key], 800., 16., 800., 600.);
    assert_eq!(style["display"], "grid");
    assert_eq!(style["grid-template-columns"], "repeat(2, minmax(0, 1fr))");
    assert_eq!(style["min-height"], "100vh");
    assert_eq!(style["max-width"], "100%");
    assert_eq!(px("width"), 400.);
    assert_eq!(style["height"], "30px");
    assert_eq!(px("margin-top"), -8.);
    assert_eq!(style["margin-left"], "auto");
    assert_eq!(style["margin-right"], "auto");
    assert_eq!(style["border-left-width"], "2px");
    assert_eq!(style["border-bottom-color"], "#123456");
    assert_eq!(px("border-bottom-left-radius"), 12.);
    assert_eq!(style["background-color"], "#abcdef");
    assert_eq!(px("font-size"), 18.);
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
        doc.nodes[card].state = if hovered { crate::dom::HOVER } else { 0 };
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
            &style["background-color"],
            if hovered { "#222222" } else { "#111111" }
        );
        assert_eq!(
            style.get(Prop::COLOR),
            Some(if hovered { "#333333" } else { "#000000" })
        );
        assert_eq!(
            cached.values[span].get(Prop::FONT_WEIGHT),
            hovered.then_some("700")
        );
        assert_eq!(&cached.values[span]["padding-top"], "3px");
        assert_eq!(&cached.values[bold]["padding-top"], "0");
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

#[test]
fn table_columns_follow_rows_and_stay_cached() {
    let mut doc = Document::parse(
        "<table id=t><thead><tr><th>a</th><th>b</th><th>c</th></tr></thead><tbody><tr><td>1</td><td><table id=inner><tr><td></td><td></td><td></td><td></td><td></td></tr></table></td></tr></tbody></table><table id=one><tr><td>x</td></tr></table><table id=own style='grid-template-columns:1fr 2fr'><tr><td></td><td></td></tr></table>",
    );
    let [table, inner, one, own] = ["#t", "#inner", "#one", "#own"].map(|s| element(&doc, s));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        &first.values[table]["grid-template-columns"],
        "repeat(3,auto)"
    );
    assert_eq!(
        &first.values[inner]["grid-template-columns"],
        "repeat(5,auto)"
    );
    assert!(!first.values[one].contains(Prop::GRID_TEMPLATE_COLUMNS));
    assert_eq!(&first.values[own]["grid-template-columns"], "1fr 2fr");
    let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert!(Rc::ptr_eq(&first.values[table], &second.values[table]));
    let row = doc.nodes[doc.nodes[table].children[1]].children[0];
    for _ in 0..3 {
        let cell = doc.nodes.len();
        doc.nodes.push(crate::dom::Node {
            tag: "td".into(),
            parent: Some(row),
            ..Default::default()
        });
        doc.nodes[row].children.push(cell);
    }
    let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        &cached.values[table]["grid-template-columns"],
        "repeat(5,auto)"
    );
    let last = *doc.nodes[row].children.last().unwrap();
    doc.nodes[last].tag = "#text".into();
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
    assert_eq!(cached.values, fresh.values);
    assert_eq!(
        &cached.values[table]["grid-template-columns"],
        "repeat(4,auto)"
    );
}

#[test]
fn structural_selectors_follow_sibling_changes() {
    let mut doc = Document::parse(
        "<style>li:nth-child(2n){color:red} li:nth-last-of-type(1){font-weight:700} li:only-child{padding-top:1px} li+li{margin-top:2px} li:nth-child(odd of .k){border-top-width:3px} p~li{margin-left:4px}</style><ul><li id=a class=k></li><li id=b></li><li id=c class=k></li></ul>",
    );
    let list = element(&doc, "ul");
    let [a, b, c] = ["#a", "#b", "#c"].map(|s| element(&doc, s));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for step in 0..7 {
        match step {
            1 => doc.nodes[list].children.swap(0, 2),
            2 => doc.nodes[b].tag = "p".into(),
            3 => {
                doc.nodes[c].attrs.remove("class");
            }
            4 => doc.nodes[list].children.retain(|&id| id == a),
            5 => doc.nodes[b].tag = "li".into(),
            6 => doc.nodes[list].children = vec![b, a, c],
            _ => {}
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
        assert_eq!(cached.values, fresh.values, "step {step}");
    }
}

#[test]
fn huge_font_sizes_are_clamped() {
    let doc = Document::parse(
        "<p id=a style='font-size:100000000px'>a</p><p id=b style='font-size:calc(1e38px * 100)'>b</p>",
    );
    let styles = styles(&doc, 800., 600.);
    assert_eq!(styles[element(&doc, "#a")]["font-size"], "10000px");
    assert_eq!(styles[element(&doc, "#b")]["font-size"], "10000px");
}

#[test]
fn passes_drop_unused_scopes_and_report_interaction_states() {
    let mut doc = Document::parse("<style></style><div class=a></div>");
    let sheet = element(&doc, "style");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (source, states) in [
        (".a:hover{color:red}", crate::dom::HOVER),
        (
            ".a{color:blue} .b:active{&>i{color:red}}",
            crate::dom::ACTIVE,
        ),
        (".a{color:green}", 0),
    ] {
        doc.nodes[sheet].text = source.into();
        compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        cache.finish();
        assert_eq!(cache.scopes.len(), 1);
        assert!(
            cache
                .values
                .keys()
                .all(|key| key.scope == cache.scopes[0].id)
        );
        assert_eq!(cache.states & !crate::dom::FOCUS, states, "{source}");
    }
}

#[test]
fn media_results_restyle_their_nodes_and_rebuild_scopes_only_for_other_rules() {
    let doc = Document::parse(
        "<style>.a{font-size:2vw} @media (max-width: 500px){.a{color:red} .a::before{content:'x'}} @media (max-width: 350px){@keyframes k{to{color:blue}}}</style><div class=a></div><p></p>",
    );
    let (div, p) = (element(&doc, "div"), element(&doc, "p"));
    let mut cache = StyleCache::default();
    let mut scopes = Vec::new();
    let mut last: Option<ComputedStyles> = None;
    for width in [800., 700., 400., 600., 300.] {
        let computed = compute_styles(
            &doc,
            width,
            600.,
            None,
            &mut HashMap::new(),
            &mut cache,
            None,
        );
        cache.finish();
        scopes.push(cache.scopes[0].id);
        let fresh = styles(&doc, width, 600.);
        assert_eq!(
            computed.values[div].get(Prop::COLOR),
            fresh[div].get("color").map(String::as_str)
        );
        assert_eq!(computed.pseudos.contains_key(&div), width <= 500.);
        assert_eq!(
            &computed.values[div]["font-size"],
            format!("{}px", width / 50.)
        );
        if let Some(last) = last.filter(|_| width != 300.) {
            assert!(Rc::ptr_eq(&last.values[p], &computed.values[p]));
            assert!(!Rc::ptr_eq(&last.values[div], &computed.values[div]));
            assert_eq!(computed.lengths[p].viewport, [width, 600.]);
        }
        last = Some(computed);
    }
    assert_eq!(scopes[..4], [scopes[0]; 4]);
    assert_ne!(scopes[0], scopes[4]);
}

#[test]
fn collapsed_table_borders_straddle_grid_lines() {
    let doc = Document::parse(
        "<style>table { border-collapse: collapse } td { border: 2px solid } .own { border: 6px solid }</style>
        <table id=cells><tr><td>a</td><td>b</td></tr></table>
        <table id=own class=own style='margin: 10px'><tr><td>a</td></tr></table>
        <table id=separate style='border-collapse: separate; border-spacing: 3px'><tr><td>a</td></tr></table>",
    );
    let computed = styles(&doc, 800., 600.);
    let edges = |selector, props| {
        Lengths::new(16., 800., 600.).edges(&typed(&computed[element(&doc, selector)]), props, 0.)
    };
    assert_eq!(edges("#cells td", MARGIN), [-1.; 4]);
    assert_eq!(edges("#cells", PADDING), [1.; 4]);
    assert_eq!(computed[element(&doc, "#cells")]["border-top-width"], "0");
    assert_eq!(edges("#own", MARGIN), [10.; 4]);
    assert_eq!(edges("#own", PADDING), [3.; 4]);
    assert_eq!(edges("#own td", MARGIN), [-3.; 4]);
    assert_eq!(
        computed[element(&doc, "#own td")]["border-top-width"],
        "6px"
    );
    assert_eq!(edges("#separate", PADDING), [3.; 4]);
}

fn typed(values: &BBTreeMap<String, String>) -> Style {
    values
        .iter()
        .map(|(name, value)| (Prop::new(name), value.as_str().into()))
        .collect()
}

#[test]
fn nested_parent_selector_inside_pseudo_class_arguments() {
    let doc = Document::parse(
        "<style>.p{:where(& > span){color:red}}.q{:is(&:hover,&.on){color:blue}}.space-x-4{:where(& > :not(:last-child)){margin-right:4px}}.n{:not(& > .x){width:1px}}</style><div class=p><span id=a></span><i><span id=b></span></i></div><div class='q on' id=q></div><div class=space-x-4><b id=c></b><b id=d></b></div><div class=n><b id=e class=x></b><b id=f></b></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "color").as_deref(), Some("red"));
    assert_ne!(get("#b", "color").as_deref(), Some("red"));
    assert_eq!(get("#q", "color").as_deref(), Some("blue"));
    assert_eq!(get("#c", "margin-right").as_deref(), Some("4px"));
    assert_eq!(get("#d", "margin-right"), None);
    assert_eq!(get("#e", "width"), None);
    assert_eq!(get("#f", "width").as_deref(), Some("1px"));
}

#[test]
fn invalid_variables_act_as_unset_and_cycles_use_fallbacks() {
    let fan = vec!["var(--x)"; 85].join(" ");
    let chain: String = (1..=24)
        .map(|i| format!("--l{i}:var(--l{0}) var(--l{0});", i - 1))
        .collect();
    let doc = Document::parse(&format!(
        "<style>#c3{{--a:var(--b);--b:var(--a);color:var(--a,red)}}#c4{{--a:var(--b,1px);--b:var(--a,2px)}}#c7{{--b:var(--a) var(--d);--a:var(--b);--d:3px}}#c8{{border:var(--nope) solid}}#c6{{--x:1px;--y:{fan};width:var(--nope2,var(--y2,3px))}}#c1{{--a:var(--nope)}}#e{{--l0:{};{chain}}}</style><div style=color:red><span id=s style=color:var(--nope)></span></div><div id=c3></div><div id=c4></div><div id=c7></div><div id=c8></div><div id=c6></div><div style=--a:red><p id=c1></p></div><div id=e></div>",
        "x".repeat(1024)
    ));
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#s", "color").as_deref(), Some("red"));
    assert_eq!(get("#c3", "color").as_deref(), Some("red"));
    assert_eq!(get("#c3", "--a"), None);
    assert_eq!((get("#c4", "--a"), get("#c4", "--b")), (None, None));
    assert_eq!(get("#c7", "--d").as_deref(), Some("3px"));
    assert_eq!(get("#c7", "--a"), None);
    assert_eq!(get("#c8", "border-top-style"), None);
    let fanned = get("#c6", "--y").unwrap();
    assert_eq!(fanned, vec!["1px"; 85].join(" "));
    assert_eq!(get("#c6", "width").as_deref(), Some("3px"));
    assert_eq!(get("#c1", "--a"), None);
    assert!(get("#e", "--l9").is_some_and(|value| value.len() > 500_000));
    assert_eq!(get("#e", "--l24"), None);
}

#[test]
fn registered_properties_follow_syntax_inheritance_and_initial_values() {
    let doc = Document::parse(
        "<style>@property --r{syntax:'<length>';inherits:false;initial-value:5px}@property --tw-border-style{syntax:'*';inherits:false;initial-value:solid}@property --q{syntax:'<color>';inherits:true;initial-value:blue}@property --bad{syntax:'<length>'}@property --tw-shadow{syntax:'*';inherits:false;initial-value:0 0 #0000}
        #p9{--r:7px}#c9,#p9{width:var(--r)}.border{border-style:var(--tw-border-style);border-width:1px}#c10{--r:red;width:var(--r)}#p11{--q:green}#c11{--q:var(--nope)}#c12{--q:2px}#p13{--bad:1px}.shadow{--tw-shadow:0 1px 2px red;box-shadow:var(--tw-shadow)}.child{box-shadow:var(--tw-shadow)}</style><div id=p9><div id=c9></div></div><div id=b class=border></div><div id=c10></div><div id=p11><div id=c11></div></div><div id=c12></div><div id=p13><i id=c13></i></div><div class=shadow id=sh><div class=child id=ch></div></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#p9", "width").as_deref(), Some("7px"));
    assert_eq!(get("#c9", "width").as_deref(), Some("5px"));
    assert_eq!(get("#c9", "--r"), None);
    assert_eq!(get("#b", "border-top-style").as_deref(), Some("solid"));
    assert_eq!(get("#c10", "width").as_deref(), Some("5px"));
    assert_eq!(get("#c11", "--q").as_deref(), Some("green"));
    assert_eq!(get("#c12", "--q").as_deref(), Some("blue"));
    assert_eq!(get("#c13", "--bad").as_deref(), Some("1px"));
    assert_eq!(get("#sh", "box-shadow").as_deref(), Some("0 1px 2px red"));
    assert_eq!(get("#ch", "box-shadow").as_deref(), Some("0 0 #0000"));
    assert!(
        Stylesheet::parse("@property --x{syntax:'<length>';inherits:true;initial-value:1em}")
            .source()
            .contains("@property --x")
    );
}

#[test]
fn revert_rolls_back_to_user_agent_and_lower_layers() {
    let doc = Document::parse(
        "<style>:where(.reset *){all:unset;display:revert}@layer a{.x{color:red}}@layer b{.x{color:revert-layer}}.y{color:blue}.y{color:revert}a{color:green}a.r{color:revert}</style><div class=reset><div id=d></div><span id=s></span><p id=p></p></div><div style=color:orange><div class='x' id=x></div><div class=y id=y></div></div><a href=# class=r id=a>a</a>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#d", "display").as_deref(), Some("block"));
    assert_eq!(get("#s", "display"), None);
    assert_eq!(get("#p", "display").as_deref(), Some("block"));
    assert_eq!(get("#p", "margin-top"), None);
    assert_eq!(get("#x", "color").as_deref(), Some("red"));
    assert_eq!(get("#y", "color").as_deref(), Some("orange"));
    assert_eq!(get("#a", "color").as_deref(), Some("#0000ee"));
}

#[test]
fn generated_content_inherits_every_inherited_property() {
    let doc = Document::parse(
        "<style>p{font-weight:700;text-transform:uppercase;letter-spacing:2px;width:10px}p::before{content:'x'}</style><p>a</p>",
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
    let before = &computed.pseudos[&element(&doc, "p")][0];
    assert_eq!(before.get(Prop::FONT_WEIGHT), Some("700"));
    assert_eq!(before.get(Prop::TEXT_TRANSFORM), Some("uppercase"));
    assert_eq!(before.get(Prop::LETTER_SPACING), Some("2px"));
    assert_eq!(before.get(Prop::WIDTH), None);
}

#[test]
fn inherited_custom_properties_are_shared_until_a_node_sets_one() {
    let vars: String = (0..300).map(|i| format!("--v{i}:{i}px;")).collect();
    let doc = Document::parse(&format!(
        "<html><style>:root{{{vars}}}.own{{--v7:1px}}</style><body><div id=a></div><div id=b><p id=c></p></div><div id=d class=own></div></body></html>"
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
    let vars = |selector| computed.vars[element(&doc, selector)].clone();
    assert_eq!(vars("html").len(), 300);
    assert!(Rc::ptr_eq(&vars("html"), &vars("#a")));
    assert!(Rc::ptr_eq(&vars("#a"), &vars("#c")));
    assert!(!Rc::ptr_eq(&vars("#a"), &vars("#d")));
    let v7 = Prop::find("--v7").unwrap();
    assert_eq!(
        vars("#d")
            .iter()
            .find(|(prop, _)| *prop == v7)
            .map(|(_, value)| &**value),
        Some("1px")
    );
    assert!(computed.values[element(&doc, "#d")].vars().is_empty());
}

#[test]
fn color_scheme_changes_system_colors_default_text_and_form_controls() {
    let dark = Document::parse(
        "<html><style>:root{color-scheme:dark}div{background-color:Canvas;border-color:ButtonBorder}.light{color-scheme:light;color:CanvasText}</style><body><div id=d>x</div><a href=# id=a>a</a><input id=i><p class=light id=l>l</p></body></html>",
    );
    let styles = styles(&dark, 800., 600.);
    let get = |selector, name| styles[element(&dark, selector)].get(name).cloned();
    assert_eq!(get("html", "color").as_deref(), Some("#ffffff"));
    assert_eq!(get("#d", "color").as_deref(), Some("#ffffff"));
    assert_eq!(get("#d", "background-color").as_deref(), Some("#121212"));
    assert_eq!(get("#a", "color").as_deref(), Some("#9e9eff"));
    assert_eq!(get("#i", "background-color").as_deref(), Some("#3b3b3b"));
    assert_eq!(get("#i", "color").as_deref(), Some("#fff"));
    assert_eq!(get("#l", "color").as_deref(), Some("#000000"));
    let meta = Document::parse(
        "<html><head><meta name=color-scheme content=dark></head><body><p>x</p></body></html>",
    );
    assert_eq!(styles_of(&meta, "p", "color"), Some("#ffffff".into()));
    let light = Document::parse("<html><body><p>x</p><a href=#>a</a></body></html>");
    assert_eq!(styles_of(&light, "p", "color"), Some("#000000".into()));
    assert_eq!(styles_of(&light, "a", "color"), Some("#0000ee".into()));
    let field = Document::parse("<html><body><textarea>x</textarea></body></html>");
    assert_eq!(
        styles_of(&field, "textarea", "overflow-y"),
        Some("auto".into())
    );
}

fn styles_of(doc: &Document, selector: &str, name: &str) -> Option<String> {
    styles(doc, 800., 600.)[element(doc, selector)]
        .get(name)
        .cloned()
}

#[test]
fn style_elements_in_templates_noscript_media_and_disabled_are_skipped() {
    let doc = Document::parse(
        "<html><head><template><style>p{color:red}</style></template><noscript><style>p{width:1px}</style></noscript><style media=print>p{height:1px}</style><style media='(max-width:10px)'>p{order:1}</style><style media='(min-width:10px)'>p{opacity:.5}</style><style disabled>p{z-index:1}</style></head><body><p>x</p></body></html>",
    );
    let styles = styles(&doc, 800., 600.);
    let p = &styles[element(&doc, "p")];
    assert_eq!(p.get("color").map(String::as_str), Some("#000000"));
    for name in ["width", "height", "order", "z-index"] {
        assert_eq!(p.get(name), None, "{name}");
    }
    assert_eq!(p.get("opacity").map(String::as_str), Some(".5"));
}

#[test]
fn inserted_rules_update_the_scope_in_place_and_keep_cascade_order() {
    let mut doc = Document::parse(
        "<html><head><style id=s>.x{color:red}.y{padding:1px}</style></head><body><p class=x id=p>a</p><p class=y id=q>b</p><i class=z id=z>c</i></body></html>",
    );
    let style = element(&doc, "#s");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let check = |doc: &Document,
                 cache: &mut StyleCache,
                 sheets: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
                 step: &str| {
        let cached = compute_styles(doc, 800., 600., None, sheets, cache, None);
        cache.finish();
        let mut baseline = StyleCache {
            cssom: cache.cssom.clone(),
            ..StyleCache::default()
        };
        let fresh = compute_styles(
            doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut baseline,
            None,
        );
        assert_eq!(cached.values, fresh.values, "{step}");
        assert_eq!(cache.scopes.len(), 1, "{step}");
        cache.scopes[0].id
    };
    let id = check(&doc, &mut cache, &mut sheets, "initial");
    let text = doc.nodes[style].children.first().copied().unwrap_or(style);
    doc.nodes[text].text.push_str(".z{color:blue}");
    assert_eq!(check(&doc, &mut cache, &mut sheets, "append"), id);
    for (index, rule) in [
        (0, ".x{color:green}"),
        (3, ".x{color:purple}"),
        (1, ".z{width:1px}"),
    ] {
        cache
            .edit_sheet(&doc, style, &mut sheets, Some((index, true)), |sheet, _| {
                sheet.edit(&[index], Edit::Insert(rule))
            })
            .unwrap();
        assert_eq!(check(&doc, &mut cache, &mut sheets, rule), id, "{rule}");
    }
    let computed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        computed.values[element(&doc, "#p")].get(Prop::COLOR),
        Some("purple")
    );
    assert_eq!(
        computed.values[element(&doc, "#z")].get(Prop::WIDTH),
        Some("1px")
    );
    let head = element(&doc, "head");
    let added = doc.nodes.len();
    doc.nodes.push(crate::dom::Node {
        tag: "style".into(),
        text: ".y{color:orange}".into(),
        parent: Some(head),
        ..Default::default()
    });
    doc.nodes.get_mut(head).unwrap().children.push(added);
    assert_eq!(check(&doc, &mut cache, &mut sheets, "new sheet"), id);
    for (index, step) in [
        (0, "delete first"),
        (2, "delete middle"),
        (3, "delete last"),
    ] {
        cache
            .edit_sheet(
                &doc,
                style,
                &mut sheets,
                Some((index, false)),
                |sheet, _| sheet.edit(&[index], Edit::Delete),
            )
            .unwrap();
        assert_eq!(check(&doc, &mut cache, &mut sheets, step), id, "{step}");
    }
    for (top, edit) in [
        (Some((0, true)), Edit::Insert(".y{color:teal}")),
        (Some((1, false)), Edit::Delete),
        (Some((0, false)), Edit::Delete),
    ] {
        cache
            .edit_sheet(&doc, style, &mut sheets, top, |sheet, _| {
                sheet.edit(&[top.unwrap().0], edit)
            })
            .unwrap();
    }
    assert_eq!(
        check(&doc, &mut cache, &mut sheets, "insert and delete"),
        id
    );
    let computed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        computed.values[element(&doc, "#q")].get(Prop::COLOR),
        Some("orange")
    );
}

#[test]
fn text_decorations_propagate_with_the_decorating_color() {
    let doc = Document::parse(
        "<style>a{color:blue;text-decoration:underline}.none{text-decoration:none}.badge{display:inline-block}.red{color:red}s{color:green}</style><a href=#><span class=none id=n>x</span><span class=badge id=b>b</span><span class=red id=r>r</span><s id=s>s</s></a>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(
        get("#n", "text-decoration-line").as_deref(),
        Some("underline")
    );
    assert_eq!(get("#n", "text-decoration-color").as_deref(), Some("blue"));
    assert_eq!(get("#b", "text-decoration-line"), None);
    assert_eq!(get("#r", "text-decoration-color").as_deref(), Some("blue"));
    assert_eq!(get("#r", "color").as_deref(), Some("red"));
    assert_eq!(
        get("#s", "text-decoration-line").as_deref(),
        Some("underline line-through")
    );
    assert_eq!(get("#s", "text-decoration-color").as_deref(), Some("green"));
    let text = doc.nodes[element(&doc, "#r")].children[0];
    assert_eq!(
        styles[text]
            .get("text-decoration-color")
            .map(String::as_str),
        Some("blue")
    );
}

#[test]
fn container_units_in_font_sizes_follow_the_nearest_container() {
    let doc = Document::parse(
        "<div id=c style='container-type:inline-size'><p id=p style='font-size:10cqw;line-height:20cqi'>x</p></div><p id=v style='font-size:1cqw'>y</p>",
    );
    let container = element(&doc, "#c");
    let mut cache = StyleCache::default();
    let compute = |cache: &mut StyleCache| {
        let computed = compute_styles(&doc, 800., 600., None, &mut HashMap::new(), cache, None);
        cache.finish();
        computed
    };
    compute(&mut cache);
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(container, [500., 100.])]),
        &doc
    ));
    let computed = compute(&mut cache);
    assert_eq!(
        computed.values[element(&doc, "#p")].get(Prop::FONT_SIZE),
        Some("50px")
    );
    assert_eq!(
        computed.values[element(&doc, "#p")].get(Prop::LINE_HEIGHT),
        Some("100px")
    );
    assert_eq!(
        computed.values[element(&doc, "#v")].get(Prop::FONT_SIZE),
        Some("8px")
    );
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(container, [300., 100.])]),
        &doc
    ));
    let computed = compute(&mut cache);
    assert_eq!(
        computed.values[element(&doc, "#p")].get(Prop::FONT_SIZE),
        Some("30px")
    );
}

#[test]
fn inline_logical_properties_follow_the_direction() {
    let doc = Document::parse(
        "<style>.m{margin-inline-start:50px;padding-inline:1px 2px;border-inline-start:3px solid red;inset-inline-end:4px;border-start-start-radius:5px}.override{margin-inline-start:50px}.override{margin-right:7px}</style><div id=l class=m></div><div dir=rtl><div id=r class=m></div><p id=o class=override></p></div><div style='direction:rtl'><span id=s class=m></span></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#l", "margin-left").as_deref(), Some("50px"));
    assert_eq!(get("#l", "padding-left").as_deref(), Some("1px"));
    assert_eq!(get("#l", "border-left-width").as_deref(), Some("3px"));
    assert_eq!(get("#l", "right").as_deref(), Some("4px"));
    assert_eq!(get("#l", "border-top-left-radius").as_deref(), Some("5px"));
    assert_eq!(get("#r", "direction").as_deref(), Some("rtl"));
    assert_eq!(get("#r", "margin-right").as_deref(), Some("50px"));
    assert_eq!(get("#r", "margin-left"), None);
    assert_eq!(get("#r", "padding-right").as_deref(), Some("1px"));
    assert_eq!(get("#r", "padding-left").as_deref(), Some("2px"));
    assert_eq!(get("#r", "border-right-style").as_deref(), Some("solid"));
    assert_eq!(get("#r", "left").as_deref(), Some("4px"));
    assert_eq!(get("#r", "border-top-right-radius").as_deref(), Some("5px"));
    assert_eq!(get("#s", "margin-right").as_deref(), Some("50px"));
    assert_eq!(get("#o", "margin-right").as_deref(), Some("7px"));
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    let r = element(&doc, "#r");
    assert_eq!(
        resolved(
            &computed.values[r],
            &computed.lengths[r],
            "margin-inline-start",
            None,
            &|| [200., 100.],
            &Registry::default()
        ),
        "50px"
    );
}

#[test]
fn substituted_values_the_property_rejects_are_invalid_at_computed_value_time() {
    let doc = Document::parse(
        "<style>@property --tw-gradient-stops{syntax:'*';inherits:false}#a{--s:;background-image:linear-gradient(var(--s))}#b{background-image:linear-gradient(var(--tw-gradient-stops))}#c{--w:red;width:var(--w);color:var(--w)}#d{--g:red,blue;background-image:linear-gradient(var(--g))}</style><div id=a></div><div id=b></div><div id=c></div><div id=d></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "background-image"), None);
    assert_eq!(get("#b", "background-image"), None);
    assert_eq!(get("#c", "width"), None);
    assert_eq!(get("#c", "color").as_deref(), Some("red"));
    assert_eq!(
        get("#d", "background-image").as_deref(),
        Some("linear-gradient(red,blue)")
    );
}

#[test]
fn scope_rules_stop_at_limits_and_scope_matches_the_root() {
    let doc = Document::parse(
        "<style>@scope (.card) to (.content) { img { width: 1px } :scope { height: 2px } & > p { order: 3 } } @scope (.box) { color: red }</style><div class=card id=c><img id=a><p id=p></p><div class=content><img id=b></div></div><img id=o><div class=box id=x></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "width").as_deref(), Some("1px"));
    assert_eq!(get("#b", "width"), None);
    assert_eq!(get("#o", "width"), None);
    assert_eq!(get("#c", "height").as_deref(), Some("2px"));
    assert_eq!(get("#p", "order").as_deref(), Some("3"));
    assert_eq!(get("#x", "color").as_deref(), Some("red"));
}

#[test]
fn conditional_rules_keep_their_scope_and_starting_style() {
    let doc = Document::parse(
        "<style>@scope (.card) to (.content) { @media all { img { width: 1px } :scope { height: 2px } } @supports (width: 1px) { img { order: 3 } } @layer l { img { z-index: 4 } } } @starting-style { @media all { #a { opacity: 0 } } }</style><div class=card id=c><img id=a><div class=content><img id=b></div></div><img id=o>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "width").as_deref(), Some("1px"));
    assert_eq!(get("#a", "order").as_deref(), Some("3"));
    assert_eq!(get("#a", "z-index").as_deref(), Some("4"));
    assert_eq!(get("#c", "height").as_deref(), Some("2px"));
    for outside in ["#b", "#o"] {
        assert_eq!(get(outside, "width"), None);
        assert_eq!(get(outside, "order"), None);
        assert_eq!(get(outside, "z-index"), None);
    }
    assert_eq!(get("#a", "opacity"), None);
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    assert_eq!(
        computed.starting[&element(&doc, "#a")].get(Prop::OPACITY),
        Some("0")
    );
}

#[test]
fn shorthands_keep_vendor_keywords_out_of_lengths() {
    let doc = Document::parse(
        "<style>#a{outline:5px auto -webkit-focus-ring-color}#b{outline:-1px solid red;border:+2px solid red}</style><div id=a></div><div id=b></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "outline-width").as_deref(), Some("5px"));
    assert_eq!(get("#a", "outline-style").as_deref(), Some("auto"));
    assert_eq!(get("#b", "outline-width").as_deref(), Some("-1px"));
    assert_eq!(get("#b", "border-top-width").as_deref(), Some("+2px"));
}

#[test]
fn computed_list_shorthands_count_items_by_their_longhands() {
    let doc = Document::parse(
        "<style>#a{transition-property:opacity,transform;transition-duration:1s;background-image:url(a.png),url(b.png);animation-name:x,y;animation-duration:1s}#b{background:url(a.png),url(b.png) red}</style><div id=a></div><div id=b></div>",
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
    let get = |selector, name| {
        let id = element(&doc, selector);
        resolved(
            &computed.values[id],
            &computed.lengths[id],
            name,
            None,
            &|| [800., 600.],
            &Registry::default(),
        )
    };
    assert_eq!(get("#a", "transition"), "opacity 1s, transform 1s");
    assert_eq!(get("#a", "animation"), "1s x, 1s y");
    assert_eq!(
        get("#a", "background"),
        "url(a.png) repeat scroll 0% 0% / auto padding-box border-box, rgba(0, 0, 0, 0) url(b.png) repeat scroll 0% 0% / auto padding-box border-box"
    );
    assert_eq!(
        get("#b", "background"),
        "url(a.png) repeat scroll 0% 0% / auto padding-box border-box, rgb(255, 0, 0) url(b.png) repeat scroll 0% 0% / auto padding-box border-box"
    );
}

#[test]
fn counters_number_generated_content_and_list_items() {
    let doc = Document::parse(
        "<style>body{counter-reset:section}h2{counter-increment:section}h2::before{content:counter(section) '. '}ol.c{counter-reset:item;list-style:none}ol.c li{counter-increment:item}ol.c li::before{content:counters(item, '.') ' ' counter(item, upper-roman)}.hidden{display:none}</style><body><h2 id=a>A</h2><h2 class=hidden>H</h2><h2 id=b>B</h2><ol class=c><li id=x>x<ol class=c><li>w</li><li id=y>y</li></ol></li></ol><ol start=5><li id=l1>a</li><li id=l2 value=10>b</li><li id=l3>c</li></ol><ol reversed><li id=r1>a</li><li id=r2>b</li></ol></body>",
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
    let before = |selector| {
        computed.pseudos[&element(&doc, selector)][0]
            .get(Prop::CONTENT)
            .map(str::to_owned)
    };
    assert_eq!(before("#a").as_deref(), Some("\"1\" '. '"));
    assert_eq!(before("#b").as_deref(), Some("\"2\" '. '"));
    assert_eq!(before("#x").as_deref(), Some("\"1\" ' ' \"I\""));
    assert_eq!(before("#y").as_deref(), Some("\"1.2\" ' ' \"II\""));
    let own = Document::parse(
        "<style>div{counter-reset:n 5}div::before{content:counter(n);counter-increment:n 2}div::after{content:counter(n);counter-increment:n}p::after{content:counter(n);counter-reset:n 9}</style><div id=d><p id=p></p></div>",
    );
    let pseudo = compute_styles(
        &own,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .pseudos;
    let text = |selector, index: usize| {
        pseudo[&element(&own, selector)][index]
            .get(Prop::CONTENT)
            .map(str::to_owned)
    };
    assert_eq!(text("#d", 0).as_deref(), Some("\"7\""));
    assert_eq!(text("#p", 1).as_deref(), Some("\"9\""));
    assert_eq!(text("#d", 1).as_deref(), Some("\"8\""));
    let item = |selector| computed.list_items.get(&element(&doc, selector)).copied();
    assert_eq!(
        [item("#l1"), item("#l2"), item("#l3")],
        [Some(5), Some(10), Some(11)]
    );
    assert_eq!([item("#r1"), item("#r2")], [Some(2), Some(1)]);
}

#[test]
fn computed_values_serialize_shorthands_and_color_spaces_like_webkit() {
    let doc = Document::parse(
        "<style>#a{font:italic bold 14px/20px Georgia,serif;background:red url(x.png) no-repeat center / 10px 20px;border:2px solid blue;outline:1px dashed green;transition:opacity 1s ease-in 2s, color .5s;animation:spin 2s linear infinite;list-style:square inside;columns:3 100px;flex-flow:row}#g{grid-template-areas:'a b' 'c d';grid-template-rows:auto 50px;grid-template-columns:100px 1fr;grid-auto-flow:column}#gi{grid-area:a;grid-row:1 / 3;grid-column:2}#c1{color:oklch(0.6 0.2 30)}#c2{color:lab(50% 40 30)}#c3{color:color(display-p3 1 0 0 / .25)}#c4{color:hsl(120 50% 50%)}#c5{color:color-mix(in srgb, red 50%, blue)}#c6{color:color(xyz 0.5 0.2 0.1)}</style><div id=a></div><div id=g></div><div id=gi></div><p id=c1></p><p id=c2></p><p id=c3></p><p id=c4></p><p id=c5></p><p id=c6></p>",
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
    let get = |selector, name| {
        let id = element(&doc, selector);
        resolved(
            &computed.values[id],
            &computed.lengths[id],
            name,
            None,
            &|| [800., 600.],
            &Registry::default(),
        )
    };
    for (name, expected) in [
        ("font", "italic 700 14px / 20px Georgia, serif"),
        (
            "background",
            "rgb(255, 0, 0) url(x.png) no-repeat scroll 50% 50% / 10px 20px padding-box border-box",
        ),
        ("border", "2px solid rgb(0, 0, 255)"),
        ("outline", "1px dashed rgb(0, 128, 0)"),
        ("transition", "opacity 1s ease-in 2s, color 0.5s"),
        ("animation", "2s linear infinite spin"),
        ("list-style", "inside none square"),
        ("columns", "100px 3"),
        ("flex-flow", "row"),
    ] {
        assert_eq!(get("#a", name), expected, "{name}");
    }
    assert_eq!(
        get("#g", "grid-template"),
        "\"a b\" \"c d\" 50px / 100px 1fr"
    );
    assert_eq!(get("#gi", "grid-area"), "1 / 2 / 3");
    assert_eq!(get("#gi", "grid-row"), "1 / 3");
    for (selector, expected) in [
        ("#c1", "oklch(0.6 0.2 30)"),
        ("#c2", "lab(50 40 30)"),
        ("#c3", "color(display-p3 1 0 0 / 0.25)"),
        ("#c4", "rgb(64, 191, 64)"),
        ("#c5", "color(srgb 0.5 0 0.5)"),
        ("#c6", "color(xyz-d65 0.5 0.2 0.1)"),
    ] {
        assert_eq!(get(selector, "color"), expected, "{selector}");
    }
}

#[test]
fn elliptical_border_radii_keep_both_axes() {
    let doc = Document::parse(
        "<style>#a{border-radius:10px 60px 20px 40px / 40px 10px 30px 20px}#b{border-radius:50%/25%}#c{border-radius:calc(4px / 2) 3px}</style><p id=a></p><p id=b></p><p id=c></p>",
    );
    let computed = styles(&doc, 800., 600.);
    let corners = |selector| {
        let style = &computed[element(&doc, selector)];
        [
            "border-top-left-radius",
            "border-top-right-radius",
            "border-bottom-right-radius",
            "border-bottom-left-radius",
        ]
        .map(|name| style[name].clone())
    };
    assert_eq!(
        corners("#a"),
        ["10px 40px", "60px 10px", "20px 30px", "40px 20px"]
    );
    assert_eq!(corners("#b")[0], "50% 25%");
    assert_eq!(corners("#c")[..2], ["calc(4px / 2)", "3px"]);
}

#[test]
fn overscroll_behavior_sets_both_axes() {
    let axes = |source: &str| {
        let style = declarations(source);
        [Prop::OVERSCROLL_BEHAVIOR_X, Prop::OVERSCROLL_BEHAVIOR_Y]
            .map(|prop| style.get(prop).map(str::to_owned))
    };
    let pair = |x: &str, y: &str| [Some(x.to_owned()), Some(y.to_owned())];
    assert_eq!(axes("overscroll-behavior: none"), pair("none", "none"));
    assert_eq!(
        axes("overscroll-behavior: contain none"),
        pair("contain", "none")
    );
    assert_eq!(
        axes("overscroll-behavior-inline: contain; overscroll-behavior-block: none"),
        pair("contain", "none")
    );
    assert_eq!(supports("overscroll-behavior", "auto contain"), Some(true));
    assert_eq!(supports("overscroll-behavior-y", "none"), Some(true));
    assert_eq!(supports("overscroll-behavior-block", "contain"), Some(true));
    assert_eq!(supports("overscroll-behavior", "scroll"), Some(false));
    assert_eq!(
        declare("overscroll-behavior-block: none")[0][0],
        "overscroll-behavior-y"
    );
}

#[test]
fn syntax_checks_never_split_multibyte_text() {
    assert!(!syntax_matches("x(", "aéé"));
    assert!(!syntax_matches("<length> | fit-content(", "aaéé"));
    assert!(syntax_matches("x(", "X(é)"));
    assert_eq!(supports("width", "aaéé"), Some(false));
    assert!(!supports_condition("(width: aaéé)"));
}

fn attach(doc: &mut Document) -> Rc<Shadows> {
    let mut pairs = Vec::new();
    for (host, root) in [("#host", "#root"), ("#inner", "#root2")] {
        let Some(root) = (0..doc.nodes.len()).find(|&id| doc.matches(id, root)) else {
            continue;
        };
        if let Some(parent) = doc.nodes[root].parent {
            doc.nodes[parent].children.retain(|&id| id != root);
        }
        pairs.push((element(doc, host), root));
        let node = &mut doc.nodes[root];
        node.parent = None;
        node.tag = "#fragment".into();
        node.attrs.clear();
    }
    let host = pairs[0].0;
    let mut composed: FxHashMap<usize, Vec<usize>> = pairs
        .iter()
        .map(|&(host, root)| (host, vec![root]))
        .collect();
    if let Some(slot) = doc.nodes.iter().position(|node| node.tag == "slot") {
        let assigned = doc.nodes[host].children.iter();
        composed.insert(
            slot,
            assigned.chain(&doc.nodes[slot].children).copied().collect(),
        );
    }
    Rc::new(Shadows {
        roots: pairs.iter().copied().collect(),
        hosts: Rc::new(pairs.iter().map(|&(host, root)| (root, host)).collect()),
        composed,
    })
}

#[test]
fn slotted_and_part_declarations_are_part_of_the_cache_key() {
    let mut doc = Document::parse(
        "<style>#host::part(label){color:blue}</style><div id=host><span id=a class=on></span><span id=b></span></div><div id=root><style>::slotted(.on){color:red}</style><slot></slot><p part=label id=p></p><p id=q></p></div>",
    );
    let [a, b, p, q] = ["#a", "#b", "#p", "#q"].map(|selector| element(&doc, selector));
    let context = StyleContext {
        shadows: attach(&mut doc),
        ..StyleContext::default()
    };
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        Some(&context),
    );
    assert_eq!(computed.values[a].get(Prop::COLOR), Some("red"));
    assert_ne!(computed.values[b].get(Prop::COLOR), Some("red"));
    assert_eq!(computed.values[p].get(Prop::COLOR), Some("blue"));
    assert_ne!(computed.values[q].get(Prop::COLOR), Some("blue"));
}

#[test]
fn hosts_are_featureless_in_their_shadow_tree() {
    let mut doc = Document::parse(
        "<style>x-a{color:green}</style><x-a id=host class=k style='color:blue'><em id=e>e</em></x-a><div id=root><style>*{padding-top:3px}.k{margin-left:5px}:host{color:red;--tone:teal}:host(.k){padding-left:6px}:host(.j){padding-right:7px}:is(:host) b{margin-top:8px}slot{background-color:var(--tone)}</style><slot id=s></slot><b id=b>b</b></div>",
    );
    let [host, e, s, b] = ["#host", "#e", "#s", "#b"].map(|selector| element(&doc, selector));
    let context = StyleContext {
        shadows: attach(&mut doc),
        ..StyleContext::default()
    };
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        Some(&context),
    );
    let get = |id: usize, prop: Prop| computed.values[id].get(prop).map(str::to_owned);
    for prop in [
        Prop::DISPLAY,
        Prop::PADDING_TOP,
        Prop::MARGIN_LEFT,
        Prop::PADDING_RIGHT,
    ] {
        assert_eq!(get(host, prop), None, "{}", prop.name());
    }
    assert_eq!(get(host, Prop::PADDING_LEFT).as_deref(), Some("6px"));
    assert_eq!(get(host, Prop::COLOR).as_deref(), Some("blue"));
    assert_eq!(get(s, Prop::COLOR).as_deref(), Some("blue"));
    assert_eq!(get(e, Prop::COLOR).as_deref(), Some("blue"));
    assert_eq!(get(s, Prop::BACKGROUND_COLOR).as_deref(), Some("teal"));
    assert_eq!(get(b, Prop::MARGIN_TOP).as_deref(), Some("8px"));
    assert_eq!(get(b, Prop::PADDING_TOP).as_deref(), Some("3px"));
}

#[test]
fn keyframes_added_or_removed_later_restart_animations() {
    let doc = Document::parse("<style id=s>.a{animation:x 1s infinite}</style><div class=a></div>");
    let (style, div) = (element(&doc, "#s"), element(&doc, "div"));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let animated = |cache: &mut StyleCache, sheets: &mut HashMap<Rc<str>, Rc<Stylesheet>>| {
        let computed = compute_styles(&doc, 800., 600., None, sheets, cache, None);
        cache.finish();
        computed.animations.contains_key(&div)
    };
    assert!(!animated(&mut cache, &mut sheets));
    for (top, edit, expected) in [
        ((1, true), Edit::Insert("@keyframes x{to{opacity:0}}"), true),
        ((1, false), Edit::Delete, false),
    ] {
        cache
            .edit_sheet(&doc, style, &mut sheets, Some(top), |sheet, _| {
                sheet.edit(&[top.0], edit)
            })
            .unwrap();
        assert_eq!(animated(&mut cache, &mut sheets), expected);
    }
}

#[test]
fn container_units_follow_each_container_and_the_viewport() {
    let doc = Document::parse(
        "<style>.c{font-size:4cqi}.v{font-size:5cqw}</style><div id=a style='container-type:inline-size'><p class=c></p></div><div id=b style='container-type:inline-size'><p class=c></p></div><p class=v></p>",
    );
    let [a, b, v] = ["#a", "#b", ".v"].map(|selector| element(&doc, selector));
    let [first, second] = [a, b].map(|id| doc.nodes[id].children[0]);
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |cache: &mut StyleCache, width: f32| {
        let computed = compute_styles(&doc, width, 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&mut cache, 800.);
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [500., 50.]), (b, [200., 50.])]),
        &doc
    ));
    let computed = compute(&mut cache, 800.);
    assert_eq!(&computed.values[first]["font-size"], "20px");
    assert_eq!(&computed.values[second]["font-size"], "8px");
    assert_eq!(&computed.values[v]["font-size"], "40px");
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [500., 50.]), (b, [300., 50.])]),
        &doc
    ));
    let computed = compute(&mut cache, 400.);
    assert_eq!(&computed.values[first]["font-size"], "20px");
    assert_eq!(&computed.values[second]["font-size"], "12px");
    assert_eq!(&computed.values[v]["font-size"], "20px");
}

#[test]
fn a_container_query_flip_restyles_only_inside_its_container() {
    let doc = Document::parse(
        "<style>@container (width > 400px){.x{color:red}}</style><div id=a style='container-type:inline-size'><p class=x></p></div><div id=b style='container-type:inline-size'><p class=x></p></div>",
    );
    let [a, b] = ["#a", "#b"].map(|selector| element(&doc, selector));
    let [p, q] = [a, b].map(|id| doc.nodes[id].children[0]);
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |cache: &mut StyleCache| {
        let computed = compute_styles(&doc, 800., 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&mut cache);
    cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [300., 50.]), (b, [300., 50.])]),
        &doc,
    );
    let before = compute(&mut cache);
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [500., 50.]), (b, [300., 50.])]),
        &doc
    ));
    let after = compute(&mut cache);
    assert_eq!(after.values[p].get(Prop::COLOR), Some("red"));
    assert_ne!(after.values[q].get(Prop::COLOR), Some("red"));
    assert!(Rc::ptr_eq(&before.values[q], &after.values[q]));
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(a, [500., 50.]), (b, [500., 50.])]),
        &doc
    ));
    assert_eq!(compute(&mut cache).values[q].get(Prop::COLOR), Some("red"));
}

#[test]
fn children_keep_their_styles_when_a_parent_changes_what_it_does_not_pass_on() {
    let mut doc = Document::parse(
        "<div id=a style='height:1px;color:red'><p id=b></p><p id=c style='height:inherit'></p></div>",
    );
    let [a, b, c] = ["#a", "#b", "#c"].map(|selector| element(&doc, selector));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |doc: &Document, cache: &mut StyleCache| {
        let computed = compute_styles(doc, 800., 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    let first = compute(&doc, &mut cache);
    doc.nodes[a]
        .attrs
        .insert("style".into(), "height:2px;color:red".into());
    let second = compute(&doc, &mut cache);
    assert!(Rc::ptr_eq(&first.values[b], &second.values[b]));
    assert_eq!(second.values[c].get(Prop::HEIGHT), Some("2px"));
    doc.nodes[a]
        .attrs
        .insert("style".into(), "height:2px;color:blue".into());
    assert_eq!(
        compute(&doc, &mut cache).values[b].get(Prop::COLOR),
        Some("blue")
    );
}

#[test]
fn attributes_that_no_selector_reads_keep_matches() {
    let mut doc = Document::parse(
        "<style>[data-on] p{color:red}[aria-x=y]{color:blue}</style><div id=a><p id=b></p></div><i id=c></i>",
    );
    let [a, b, c] = ["#a", "#b", "#c"].map(|selector| element(&doc, selector));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |doc: &Document, cache: &mut StyleCache| {
        let computed = compute_styles(doc, 800., 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&doc, &mut cache);
    doc.nodes[a].attrs.insert("data-off".into(), "1".into());
    assert_ne!(
        compute(&doc, &mut cache).values[b].get(Prop::COLOR),
        Some("red")
    );
    doc.nodes[a].attrs.insert("data-on".into(), "1".into());
    assert_eq!(
        compute(&doc, &mut cache).values[b].get(Prop::COLOR),
        Some("red")
    );
    doc.nodes[c].attrs.insert("aria-x".into(), "y".into());
    assert_eq!(
        compute(&doc, &mut cache).values[c].get(Prop::COLOR),
        Some("blue")
    );
}

#[test]
fn container_queries_flip_after_a_resize_that_keeps_media_results() {
    let doc = Document::parse(
        "<style>@container (width > 400px){.x{color:red}}</style><div id=a style='container-type:inline-size'><p class=x></p></div>",
    );
    let a = element(&doc, "#a");
    let p = doc.nodes[a].children[0];
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |cache: &mut StyleCache, width: f32| {
        let computed = compute_styles(&doc, width, 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&mut cache, 800.);
    cache.set_containers(&rustc_hash::FxHashMap::from_iter([(a, [300., 50.])]), &doc);
    let red = Some("red");
    assert_ne!(compute(&mut cache, 800.).values[p].get(Prop::COLOR), red);
    assert_ne!(compute(&mut cache, 700.).values[p].get(Prop::COLOR), red);
    assert!(cache.set_containers(&rustc_hash::FxHashMap::from_iter([(a, [500., 50.])]), &doc));
    assert_eq!(compute(&mut cache, 700.).values[p].get(Prop::COLOR), red);
}

#[test]
fn collapsed_tables_take_outer_edges_by_position_and_stay_cached() {
    let doc = Document::parse(
        "<style>table{border-collapse:collapse;border:4px solid}td{border:2px solid}</style><table><tr><td id=a></td><td></td><td id=c></td></tr><tr><td></td><td id=m></td><td></td></tr><tr><td></td><td></td><td id=z></td></tr></table>",
    );
    let [a, c, m, z, table] = ["#a", "#c", "#m", "#z", "table"].map(|s| element(&doc, s));
    let margins = |computed: &ComputedStyles, id: usize| {
        [
            Prop::MARGIN_TOP,
            Prop::MARGIN_RIGHT,
            Prop::MARGIN_BOTTOM,
            Prop::MARGIN_LEFT,
        ]
        .map(|prop| computed.values[id].get(prop).unwrap_or("").to_owned())
    };
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    cache.finish();
    assert_eq!(margins(&first, a), ["-2px", "-1px", "-1px", "-2px"]);
    assert_eq!(margins(&first, c), ["-2px", "-2px", "-1px", "-1px"]);
    assert_eq!(margins(&first, m), ["-1px"; 4]);
    assert_eq!(margins(&first, z), ["-1px", "-2px", "-2px", "-1px"]);
    let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    cache.finish();
    for id in [a, c, m, z, table] {
        assert!(Rc::ptr_eq(&first.values[id], &second.values[id]));
        assert_eq!(first.authored.get(&id), second.authored.get(&id));
        assert!(second.authored.contains_key(&id));
    }
    let resized = compute_styles(&doc, 700., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(margins(&resized, a), margins(&first, a));
}

#[test]
fn appended_style_text_shares_old_rules_and_parses_like_the_whole_text() {
    for (old, added, shared) in [
        (".a{color:red}", ".b{color:blue}", true),
        (".a{color:red}/*/.c{color:green}", ".b{color:blue}", false),
    ] {
        let mut doc = Document::parse(&format!(
            "<style id=s>{old}</style><p class=a></p><p class=b></p><p class=c></p>"
        ));
        let style = element(&doc, "#s");
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        cache.finish();
        let before = cache.sheets[&style].3.clone();
        let text = doc.nodes[style].children[0];
        doc.nodes[text].text.push_str(added);
        let computed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let after = &cache.sheets[&style].3;
        assert_eq!(Rc::ptr_eq(&before.0[0], &after.0[0]), shared, "{old}");
        let fresh = styles(&doc, 800., 600.);
        for id in [".a", ".b", ".c"].map(|selector| element(&doc, selector)) {
            assert_eq!(
                computed.values[id].get(Prop::COLOR),
                fresh[id].get("color").map(String::as_str),
                "{old}"
            );
        }
    }
}

#[test]
fn pseudo_elements_match_through_buckets() {
    let doc = Document::parse(
        "<style>.a::before{content:'a'}#i::after{content:'i'}p::before{content:'p'}[data-x]::after{content:'x'}</style><div class=a id=i></div><p></p><span data-x></span><b></b>",
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
    let content = |selector: &str, index: usize| {
        computed
            .pseudos
            .get(&element(&doc, selector))
            .and_then(|pseudos| pseudos[index].get(Prop::CONTENT).map(str::to_owned))
    };
    assert_eq!(content("div", 0).as_deref(), Some("'a'"));
    assert_eq!(content("div", 1).as_deref(), Some("'i'"));
    assert_eq!(content("p", 0).as_deref(), Some("'p'"));
    assert_eq!(content("span", 1).as_deref(), Some("'x'"));
    assert_eq!(content("b", 0), None);
}

#[test]
fn light_dark_picks_nested_and_repeated_arguments_in_one_pass() {
    let many = vec!["light-dark(red, blue)"; 20_000].join(",");
    let doc = Document::parse(&format!(
        "<p id=l style='background-image:linear-gradient(light-dark(light-dark(red, blue), green), light-dark( white , \"a,b\"))'></p><p id=d style='color-scheme:dark;background-image:linear-gradient(light-dark(red, light-dark(blue, black)), light-dark(red))'></p><p id=m style='background-image:linear-gradient({many})'></p>"
    ));
    let computed = styles(&doc, 800., 600.);
    let image = |selector| computed[element(&doc, selector)]["background-image"].clone();
    assert_eq!(image("#l"), "linear-gradient(red, white)");
    assert_eq!(image("#d"), "linear-gradient(black, light-dark(red))");
    assert_eq!(
        image("#m"),
        format!("linear-gradient({})", vec!["red"; 20_000].join(","))
    );
}

#[test]
fn nested_var_fallbacks_resolve_and_stay_bounded() {
    let deep = format!("{}red{}", "var(--u, ".repeat(120), ")".repeat(120));
    let doc = Document::parse(&format!(
        "<p id=a style='color:var(--a, var(--b, var(--c, red)))'></p><p id=b style='--x:{deep};color:var(--x, blue)'></p>"
    ));
    let computed = styles(&doc, 800., 600.);
    assert_eq!(computed[element(&doc, "#a")]["color"], "red");
    assert!(computed[element(&doc, "#b")].contains_key("color"));
}

#[test]
fn env_safe_area_insets_are_zero_and_other_names_take_their_fallback() {
    let style = declarations(
        "padding-left: max(22px, env(safe-area-inset-left)); margin-top: env(titlebar-area-height, 7px); --x: var(--y, env(safe-area-inset-right)); width: env(unknown)",
    );
    assert_eq!(style.get(Prop::PADDING_LEFT), Some("max(22px, 0px)"));
    assert_eq!(style.get(Prop::MARGIN_TOP), Some("7px"));
    assert_eq!(style.get(Prop::new("--x")), Some("var(--y, 0px)"));
    assert_eq!(style.get(Prop::WIDTH), Some("env(unknown)"));
}

#[test]
fn block_in_inline_splits_and_empty_inlines_make_no_line() {
    let doc = Document::parse(
        "<div id=h style='all:unset'><aside style='display:none'>x</aside><nav style='position:absolute'></nav><div id=p></div></div>\
         <span id=s> <i></i> <b style='position:absolute'>x</b> </span><span id=t style='padding-left:1px'></span><em id=u>x</em>\
         <p id=q>a <br class=x>b <br>c</p><style>br.x{display:none}</style>",
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
    let id = |name: &str| {
        (0..doc.nodes.len())
            .find(|&id| {
                doc.nodes[id]
                    .attrs
                    .get("id")
                    .is_some_and(|value| value == name)
            })
            .unwrap()
    };
    assert!(holds_block_box(&doc, &computed, id("h")));
    assert!(!holds_block_box(&doc, &computed, id("s")));
    assert!(makes_no_line(&doc, &computed, id("s")));
    assert!(!makes_no_line(&doc, &computed, id("t")));
    assert!(!makes_no_line(&doc, &computed, id("u")));
    let inline = inline_text(&doc, &computed, id("q")).unwrap();
    assert_eq!(&*inline.text, "a b\nc");
    assert_eq!(
        inline.runs.iter().map(|run| run.len).sum::<usize>(),
        inline.text.len()
    );
}

#[test]
fn top_layer_boxes_get_ua_rules_and_backdrops() {
    let mut doc = Document::parse(
        "<style>::backdrop{color:red}#p::backdrop{background:blue}</style><dialog id=d open>d</dialog><div id=p popover>p</div><div id=f>f</div><dialog id=n open>n</dialog>",
    );
    let [d, p, f, n] = ["#d", "#p", "#f", "#n"].map(|selector| element(&doc, selector));
    let (mut sheets, mut cache) = (HashMap::new(), StyleCache::default());
    let closed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(&closed.values[p]["display"], "none");
    assert!(!closed.values[f].contains(Prop::WIDTH));
    assert!(closed.backdrops.is_empty());
    doc.nodes[d].state = crate::dom::MODAL;
    doc.nodes[p].state = crate::dom::POPOVER;
    doc.nodes[f].state = crate::dom::FULLSCREEN;
    let open = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(
        [
            &open.values[d]["position"],
            &open.values[p]["position"],
            &open.values[f]["position"]
        ],
        ["fixed"; 3]
    );
    assert_eq!(
        (
            &open.values[p]["display"],
            &open.values[f]["width"],
            &open.values[f]["margin-top"]
        ),
        ("block", "100%", "0")
    );
    let backdrop = |id: usize, prop: &str| open.backdrops[&id][prop].to_string();
    assert_eq!(color(&backdrop(d, "background-color")), Some((0, 26)));
    assert_eq!(
        (
            backdrop(p, "background-color"),
            backdrop(p, "pointer-events")
        ),
        ("blue".into(), "none".into())
    );
    assert_eq!(color(&backdrop(f, "background-color")), Some((0, 255)));
    assert_eq!(color(&backdrop(d, "color")), Some((0xff0000, 255)));
    assert!(!open.backdrops.contains_key(&n));
    doc.nodes[f].state = 0;
    let exited = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert!(!exited.values[f].contains(Prop::WIDTH) && !exited.backdrops.contains_key(&f));
}

#[test]
fn leaving_top_layer_boxes_keep_their_place_and_backdrop() {
    let mut doc = Document::parse(
        "<style>dialog{transition:overlay 1s allow-discrete}dialog[open]::backdrop{background:red}@starting-style{dialog[open]::backdrop{background:blue}}#p::backdrop{color:blue}#x{overlay:auto}</style><dialog id=d open>d</dialog><div id=p popover>p</div><div id=x style='overlay:auto'>x</div>",
    );
    let [d, p, x] = ["#d", "#p", "#x"].map(|selector| element(&doc, selector));
    let (mut sheets, mut cache) = (HashMap::new(), StyleCache::default());
    let mut styles =
        |doc: &Document| compute_styles(doc, 800., 600., None, &mut sheets, &mut cache, None);
    let closed = styles(&doc);
    assert_eq!(
        [d, p, x].map(|id| closed.values[id].get(Prop::OVERLAY)),
        [None; 3]
    );
    doc.nodes[d].state = crate::dom::MODAL;
    doc.nodes[p].state = crate::dom::POPOVER;
    let open = styles(&doc);
    assert_eq!(
        [d, p].map(|id| open.values[id].get(Prop::OVERLAY)),
        [Some("auto"); 2]
    );
    assert_eq!(&open.backdrops[&d]["background-color"], "red");
    assert_eq!(&open.backdrop_starts[&d]["background-color"], "blue");
    assert!(!open.backdrop_starts.contains_key(&p));
    doc.nodes[d].attrs.remove("open");
    doc.nodes[d].state = crate::dom::MODAL << 5;
    doc.nodes[p].state = crate::dom::POPOVER << 5;
    let leaving = styles(&doc);
    assert_eq!(
        [d, p].map(|id| leaving.values[id].get(Prop::OVERLAY)),
        [Some("none"); 2]
    );
    assert_eq!(
        [
            &leaving.values[d]["position"],
            &leaving.values[d]["display"]
        ],
        ["fixed", "none"]
    );
    assert!(leaving.backdrops.contains_key(&d) && leaving.backdrops.contains_key(&p));
    assert!(!doc.matches(d, ":modal"));
    doc.nodes[d].state = 0;
    let left = styles(&doc);
    assert_eq!(&left.values[d]["position"], "absolute");
    assert!(!left.backdrops.contains_key(&d));
}

#[test]
fn node_lengths_share_equal_values() {
    let mut lengths = super::NodeLengths::default();
    let base = Lengths::new(16., 800., 600.);
    lengths.reset(4, base);
    lengths.set(1, Lengths::new(20., 800., 600.));
    lengths.set(2, base);
    lengths.set(3, Lengths::new(20., 800., 600.));
    assert_eq!(lengths[0], base);
    assert_eq!(lengths[1].font.em, 20.);
    assert_eq!(lengths[2], base);
    assert_eq!(lengths[3], lengths[1]);
    lengths.grow(6);
    assert_eq!(lengths[5], base);
    assert!(lengths.get(6).is_none());
    assert_eq!(lengths.list_len(), 2);
}

fn incremental(
    doc: &Document,
    cache: &mut StyleCache,
    last: &mut Option<Rc<ComputedStyles>>,
    viewport: [f32; 2],
) -> Rc<ComputedStyles> {
    let context = StyleContext {
        previous: last.clone(),
        ..Default::default()
    };
    let out = Rc::new(compute_styles(
        doc,
        viewport[0],
        viewport[1],
        None,
        &mut HashMap::new(),
        cache,
        Some(&context),
    ));
    cache.finish();
    *last = Some(out.clone());
    out
}

#[test]
fn ancestor_filter_rejections_restyle_when_ancestors_change() {
    let mut doc = Document::parse(
        "<style>.a .x{color:red}[data-k]>.x{width:1px}#i .x{height:2px}div.b .x{top:3px}.c>.d .x{left:4px}</style>
        <section id=s><div id=d><p id=p><span class=x>x</span></p></div></section>",
    );
    let [s, d, p, x] = ["#s", "#d", "#p", ".x"].map(|selector| element(&doc, selector));
    let (mut cache, mut last) = (StyleCache::default(), None);
    let mut matched = |doc: &Document| {
        let out = incremental(doc, &mut cache, &mut last, [800., 600.]);
        [
            (Prop::COLOR, "red"),
            (Prop::WIDTH, "1px"),
            (Prop::HEIGHT, "2px"),
            (Prop::TOP, "3px"),
            (Prop::LEFT, "4px"),
        ]
        .map(|(prop, value)| u8::from(out.values[x].get(prop) == Some(value)))
    };
    assert_eq!(matched(&doc), [0, 0, 0, 0, 0]);
    doc.nodes[s].attrs.set("class", "a");
    assert_eq!(matched(&doc), [1, 0, 0, 0, 0]);
    doc.nodes[d].attrs.set("data-k", "");
    assert_eq!(matched(&doc), [1, 0, 0, 0, 0]);
    doc.nodes[p].attrs.set("data-k", "");
    assert_eq!(matched(&doc), [1, 1, 0, 0, 0]);
    doc.nodes[d].attrs.set("id", "i");
    assert_eq!(matched(&doc), [1, 1, 1, 0, 0]);
    doc.nodes[s].attrs.set("class", "a b");
    assert_eq!(matched(&doc), [1, 1, 1, 0, 0]);
    doc.nodes[d].attrs.set("class", "b d");
    assert_eq!(matched(&doc), [1, 1, 1, 1, 0]);
    doc.nodes[s].attrs.set("class", "c");
    assert_eq!(matched(&doc), [0, 1, 1, 1, 1]);
    let mut doc = Document::parse(
        "<style>[data-k]>.x{width:1px}</style><div><p><span class=x>x</span></p></div>",
    );
    let [div, p, x] = ["div", "p", ".x"].map(|selector| element(&doc, selector));
    let (mut cache, mut last) = (StyleCache::default(), None);
    let mut width = |doc: &Document| {
        let out = incremental(doc, &mut cache, &mut last, [800., 600.]);
        out.values[x].get(Prop::WIDTH) == Some("1px")
    };
    assert!(!width(&doc));
    doc.nodes[div].attrs.set("data-k", "");
    assert!(!width(&doc));
    doc.nodes[p].attrs.set("data-k", "");
    assert!(width(&doc));
}

#[test]
fn animated_passes_start_from_the_stored_pass() {
    let mut doc = Document::parse(
        "<style>@property --w{syntax:'<length>';inherits:false;initial-value:1px}
        @keyframes k{to{height:calc(var(--w) * 2)}}.a{width:var(--w);--c:red;animation:k 1s}.a p{color:var(--c)}</style>
        <div class=a><p>x</p></div><div class=b><p>y</p></div><i>z</i>",
    );
    let [a, b, p, i] = [".a", ".b", ".a p", "i"].map(|selector| element(&doc, selector));
    let (mut cache, mut last) = (StyleCache::default(), None);
    let base = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    doc.nodes[b].attrs.set("class", "b a");
    let context = StyleContext {
        animated: HashMap::from([(
            a,
            Style::from_iter([
                (Prop::new("--w"), "5px".into()),
                (Prop::new("--c"), "blue".into()),
            ]),
        )]),
        previous: Some(base.clone()),
        ..Default::default()
    };
    let passes = cache.incremental;
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        Some(&context),
    );
    cache.finish();
    assert_eq!(cache.incremental, passes + 1);
    assert_eq!(out.values[a].get(Prop::WIDTH), Some("5px"));
    assert_eq!(out.values[p].get(Prop::COLOR), Some("blue"));
    assert!(Rc::ptr_eq(&out.values[i], &base.values[i]));
    assert_eq!(
        format!("{:?}", out.animations.get(&a)),
        format!("{:?}", base.animations.get(&a))
    );
    let after = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert_eq!(after.values[b].get(Prop::WIDTH), Some("1px"));
    assert_eq!(after.values[a].get(Prop::WIDTH), Some("1px"));
    assert!(after.animations.contains_key(&b));
}

#[test]
fn bundled_sheets_name_their_font_lists() {
    let mut doc = Document::parse(
        "<style>@media (min-width:1px){p{font-family:'Geist', system-ui}}:root{--font-mono:Menlo,monospace;--x:1px}b{font:12px Arial}</style><style>@layer a{i{font-family:\"Inter\"}}</style>",
    );
    for id in 0..doc.nodes.len() {
        if doc.nodes[id].tag == "#text" {
            let sheet = Stylesheet::parse(&doc.nodes[id].text);
            doc.nodes.get_mut(id).unwrap().stylesheet = Some(Rc::new(sheet));
        }
    }
    let mut lists = font_lists(&doc);
    lists.sort();
    assert_eq!(
        lists,
        [
            vec!["Geist", "system-ui"],
            vec!["Inter"],
            vec!["Menlo", "monospace"]
        ]
    );
}

#[test]
fn text_nodes_follow_their_parent_units() {
    let doc = Document::parse(
        "<style>.a{font-size:2em}</style><div style=font-size:10px><p class=a>x</p><p class=a>x</p><p class=a>x</p></div><div style=font-size:20px><p class=a>y</p><p class=a>y</p><p class=a>y</p><!--z--><!--z--></div><b style=color:red>r</b><b style=color:red>r</b><b style=color:blue>u</b><b style=color:blue>u</b>",
    );
    let (mut cache, mut last) = (StyleCache::default(), None);
    let out = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    let mut fresh = StyleCache {
        unshared: true,
        ..Default::default()
    };
    let reference = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut fresh,
        None,
    );
    for id in 0..doc.nodes.len() {
        assert!(*out.values[id] == *reference.values[id], "node {id}");
        assert_eq!(out.lengths[id], reference.lengths[id], "node {id}");
    }
    let em = |data: &str| {
        (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag.starts_with('#') && doc.nodes[id].text == data)
            .map(|id| out.lengths[id].font.em)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        [em("x"), em("y"), em("z")],
        [[20.; 3].to_vec(), [40.; 3].to_vec(), [20.; 2].to_vec()]
    );
}

#[test]
fn new_containers_restyle_the_descendants_that_query_them() {
    let mut doc = Document::parse(
        "<style>.wrap.on{container-type:inline-size}.inner.on{container:card/inline-size}
        @container (max-width:300px){.item{color:red}}@container card (min-width:100px){.named{width:7px}}</style>
        <div class=wrap><section class=inner><div><p class='item named'>x</p></div></section></div>",
    );
    let [wrap, inner, item] = [".wrap", ".inner", ".item"].map(|selector| element(&doc, selector));
    let (mut cache, mut last) = (StyleCache::default(), None);
    let color = |out: &ComputedStyles| out.values[item].get(Prop::COLOR).map(str::to_owned);
    let out = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert_ne!(color(&out).as_deref(), Some("red"));
    doc.nodes[wrap].attrs.set("class", "wrap on");
    incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert!(cache.set_containers(
        &rustc_hash::FxHashMap::from_iter([(wrap, [200., 50.])]),
        &doc
    ));
    let out = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert_eq!(color(&out).as_deref(), Some("red"));
    assert!(!out.values[item].contains(Prop::WIDTH));
    doc.nodes[inner].attrs.set("class", "inner on");
    incremental(&doc, &mut cache, &mut last, [800., 600.]);
    let sizes = FxHashMap::from_iter([(wrap, [200., 50.]), (inner, [150., 40.])]);
    cache.set_containers(&sizes, &doc);
    let out = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert_eq!(color(&out).as_deref(), Some("red"));
    assert_eq!(out.values[item].get(Prop::WIDTH), Some("7px"));
    let mut fresh = StyleCache {
        containers: sizes,
        ..Default::default()
    };
    let reference = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut fresh,
        None,
    );
    assert_eq!(out.values, reference.values);
    doc.nodes[wrap].attrs.set("class", "wrap");
    doc.nodes[inner].attrs.set("class", "inner");
    let out = incremental(&doc, &mut cache, &mut last, [800., 600.]);
    assert_ne!(color(&out).as_deref(), Some("red"));
    assert!(!out.values[item].contains(Prop::WIDTH));
}

#[test]
fn absolute_parent_sizes_turn_off_the_monospace_quirk_for_cached_children() {
    let mut doc = Document::parse("<div id=d><code>x</code></div>");
    let [div, code] = ["#d", "code"].map(|selector| element(&doc, selector));
    let mut cache = StyleCache::default();
    let first = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert_eq!(first.lengths[code].font.em, 13.);
    doc.nodes[div].inline = Some(Declarations::parse("font-size:16px"));
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert_eq!(out.lengths[code].font.em, 16.);
    assert_eq!(out.values[code].get(Prop::FONT_SIZE), Some("16px"));
}

#[test]
fn filtered_custom_property_lists_are_released() {
    let mut doc = Document::parse(
        "<style>@property --q{syntax:'*';inherits:false}</style><div id=box><p><span>x</span></p></div>",
    );
    let (block, span) = (element(&doc, "#box"), element(&doc, "span"));
    let mut cache = StyleCache::default();
    for step in 0..20 {
        doc.nodes[block].inline = Some(Declarations::parse(&format!("--p:{step};--q:{step}")));
        let out = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut cache,
            None,
        );
        assert_eq!(out.vars[span].len(), 1);
        cache.finish();
    }
    cache.trim(doc.nodes.len());
    compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    cache.finish();
    assert!(cache.inheritable.len() <= 4, "{}", cache.inheritable.len());
}

#[test]
fn text_and_scrollbar_properties_inherit_like_css() {
    let doc = Document::parse(
        "<pre style='font-variant-ligatures:none;font-kerning:none;scrollbar-color:red blue;text-align-last:center;text-wrap-mode:nowrap;white-space-collapse:preserve;-webkit-text-stroke-width:1px;writing-mode:vertical-rl;caption-side:bottom;width:9px'><span>a != b</span></pre>",
    );
    let computed = styles(&doc, 800., 600.);
    let span = &computed[element(&doc, "span")];
    for (name, value) in [
        ("font-variant-ligatures", "none"),
        ("font-kerning", "none"),
        ("scrollbar-color", "red blue"),
        ("text-align-last", "center"),
        ("text-wrap-mode", "nowrap"),
        ("white-space-collapse", "preserve"),
        ("-webkit-text-stroke-width", "1px"),
        ("writing-mode", "vertical-rl"),
        ("caption-side", "bottom"),
    ] {
        assert_eq!(span.get(name).map(String::as_str), Some(value), "{name}");
    }
    assert!(!span.contains_key("width"));
}

#[test]
fn cascade_origins_layers_and_revert_follow_the_spec_order() {
    let names: Vec<_> = (0..1100).map(|index| format!("l{index}")).collect();
    let doc = Document::parse(&format!(
        "<style>@layer {};@layer l1099{{#deep{{color:red}}}}@layer l1050{{#deep{{color:blue}}}}
        @layer a.b.c{{#nest{{color:red}}}}@layer a.b.c.d{{#nest{{color:blue}}}}
        p{{color:blue!important}}#rev{{color:revert}}#user{{color:red}}</style>
        <p style='color:red!important'>a</p><i id=rev>b</i><i id=user>c</i><b id=deep>d</b><b id=nest>e</b>",
        names.join(",")
    ));
    let user = Rc::new(Stylesheet::parse(
        "p{color:green!important}#rev,#user{color:orange}",
    ));
    let context = StyleContext {
        inserted: vec![(user, true)],
        ..Default::default()
    };
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        Some(&context),
    );
    let color = |selector| {
        out.values[element(&doc, selector)]
            .get(Prop::COLOR)
            .map(str::to_owned)
    };
    assert_eq!(color("p").as_deref(), Some("green"));
    assert_eq!(color("#rev").as_deref(), Some("orange"));
    assert_eq!(color("#user").as_deref(), Some("red"));
    assert_eq!(color("#deep").as_deref(), Some("red"));
    assert_eq!(color("#nest").as_deref(), Some("red"));
}

#[test]
fn inline_styles_intern_only_custom_property_names() {
    let mut doc = Document::parse(
        "<div id=a><p style='color:var(--late-h2o-name,blue);x-unknown-h2o-property:1'>x</p></div>",
    );
    let [a, p] = ["#a", "p"].map(|selector| element(&doc, selector));
    let mut cache = StyleCache::default();
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert_eq!(out.values[p].get(Prop::COLOR), Some("blue"));
    assert!(Prop::find("x-unknown-h2o-property").is_none());
    doc.nodes[a].inline = Some(Declarations::parse(
        "--late-h2o-name:red;--other-h2o-name:1",
    ));
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert_eq!(out.values[p].get(Prop::COLOR), Some("red"));
}

#[test]
fn unstored_passes_keep_the_next_stored_pass_full() {
    let doc = Document::parse("<style>@media (max-width:500px){p{color:red}}</style><p>x</p>");
    let p = element(&doc, "p");
    let (mut cache, mut last) = (StyleCache::default(), None);
    incremental(&doc, &mut cache, &mut last, [800., 600.]);
    let animated = StyleContext {
        animated: HashMap::from([(0, Style::new())]),
        ..Default::default()
    };
    compute_styles(
        &doc,
        400.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        Some(&animated),
    );
    let out = incremental(&doc, &mut cache, &mut last, [400., 600.]);
    assert_eq!(out.values[p].get(Prop::COLOR), Some("red"));
}

#[test]
fn collapsed_borders_follow_column_spans_and_reuse_cached_tables() {
    let mut doc = Document::parse(
        "<style>table{border-collapse:collapse}td{border:2px solid}.wide{border-top:8px solid}</style>
        <table><tr><td colspan=2>a</td><td>b</td></tr><tr><td>c</td><td class=wide>d</td><td>e</td></tr></table>",
    );
    let mut cache = StyleCache::default();
    let first = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    cache.finish();
    let cells: Vec<usize> = (0..doc.nodes.len())
        .filter(|&id| doc.nodes[id].tag == "td")
        .collect();
    let bottom = |out: &ComputedStyles, cell: usize| {
        out.values[cell]
            .get(Prop::BORDER_BOTTOM_WIDTH)
            .map(str::to_owned)
    };
    assert_eq!(bottom(&first, cells[0]).as_deref(), Some("8px"));
    assert_eq!(bottom(&first, cells[1]).as_deref(), Some("2px"));
    let again = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert!(
        cells
            .iter()
            .all(|&cell| Rc::ptr_eq(&first.values[cell], &again.values[cell]))
    );
    doc.nodes[cells[2]]
        .attrs
        .set("colspan", "18446744073709551615");
    let wide = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    assert_eq!(bottom(&wide, cells[0]).as_deref(), Some("2px"));
}

fn shadowed(
    doc: &Document,
    shadows: &Rc<Shadows>,
    cache: &mut StyleCache,
    last: &mut Option<Rc<ComputedStyles>>,
) -> Rc<ComputedStyles> {
    let context = StyleContext {
        shadows: shadows.clone(),
        previous: last.clone(),
        ..Default::default()
    };
    let out = Rc::new(compute_styles(
        doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        cache,
        Some(&context),
    ));
    cache.finish();
    *last = Some(out.clone());
    out
}

#[test]
fn hosts_take_pseudo_elements_from_their_shadow_tree() {
    let mut doc = Document::parse(
        "<style>x-a::before{color:blue;background-color:blue!important}</style><x-a id=host class=on></x-a><div id=root><style>:host::before{content:'A';color:red;background-color:red!important}:host(.on)::after{content:'B';color:green}:host(:hover)::after{color:purple}@starting-style{:host{opacity:0}}</style></div>",
    );
    let host = element(&doc, "#host");
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    let pseudos = out.pseudos.get(&host).cloned().unwrap();
    assert_eq!(pseudos[0].get(Prop::CONTENT), Some("'A'"));
    assert_eq!(pseudos[0].get(Prop::COLOR), Some("blue"));
    assert_eq!(pseudos[0].get(Prop::BACKGROUND_COLOR), Some("red"));
    assert_eq!(pseudos[1].get(Prop::CONTENT), Some("'B'"));
    assert_eq!(pseudos[1].get(Prop::COLOR), Some("green"));
    assert_eq!(out.starting[&host].get(Prop::OPACITY), Some("0"));
    doc.nodes[host].attrs.set("class", "off");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    let pseudos = out.pseudos.get(&host).cloned().unwrap();
    assert_eq!(pseudos[0].get(Prop::CONTENT), Some("'A'"));
    assert!(!pseudos[1].contains(Prop::CONTENT));
    doc.nodes[host].attrs.set("class", "on");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(out.pseudos[&host][1].get(Prop::CONTENT), Some("'B'"));
    doc.nodes[host].state = crate::dom::HOVER;
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(out.pseudos[&host][1].get(Prop::COLOR), Some("purple"));
    #[cfg(debug_assertions)]
    assert_eq!(cache.incremental, 3);
}

#[test]
fn host_rules_query_the_containers_of_the_flat_tree() {
    let mut doc = Document::parse(
        "<style>#outer{container-type:inline-size}.self{container-type:inline-size}@container (width < 300px){#p::before{content:'P'}}</style>
        <div id=outer><x-a id=host><span id=s>s</span></x-a><div id=p class=self></div></div>
        <div id=root><style>:host{color:gray;font-size:10cqw;container-type:inline-size}
        @container (width > 300px){:host{color:green}}@container (width < 300px){:host{background-color:red}}
        @container (width < 300px){:host::before{content:'C'}}@container (width > 300px){:host::after{content:'D'}}
        slot{display:block;container-type:inline-size}@container (width < 150px){::slotted(span){padding-left:5px}}</style><slot id=slot></slot></div>",
    );
    let [outer, host, s, p] =
        ["#outer", "#host", "#s", "#p"].map(|selector| element(&doc, selector));
    let slot = element(&doc, "#slot");
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    shadowed(&doc, &shadows, &mut cache, &mut last);
    let mut sizes = FxHashMap::from_iter([
        (outer, [400., 50.]),
        (host, [200., 50.]),
        (slot, [100., 50.]),
        (p, [200., 50.]),
    ]);
    let mut check = |sizes: &FxHashMap<usize, [f32; 2]>| {
        assert!(cache.set_containers(sizes, &doc));
        let out = shadowed(&doc, &shadows, &mut cache, &mut last);
        let get = |id: usize, prop: Prop| out.values[id].get(prop).map(str::to_owned);
        let content = |slot: usize| {
            out.pseudos
                .get(&host)
                .and_then(|pair| pair[slot].get(Prop::CONTENT).map(str::to_owned))
        };
        let (wide, narrow, slim) = (
            sizes[&outer][0] > 300.,
            sizes[&host][0] < 300.,
            sizes[&slot][0] < 150.,
        );
        assert_eq!(
            get(host, Prop::COLOR).as_deref(),
            Some(if wide { "green" } else { "gray" })
        );
        assert_eq!(
            get(host, Prop::BACKGROUND_COLOR).as_deref(),
            (!wide).then_some("red")
        );
        assert_eq!(
            get(host, Prop::FONT_SIZE),
            Some(format!("{}px", sizes[&outer][0] / 10.))
        );
        assert_eq!(content(0).as_deref(), narrow.then_some("'C'"));
        assert_eq!(content(1).as_deref(), (!narrow).then_some("'D'"));
        assert_eq!(get(s, Prop::PADDING_LEFT).as_deref(), slim.then_some("5px"));
        assert_eq!(out.pseudos[&p][0].get(Prop::CONTENT), Some("'P'"));
        let mut fresh = StyleCache {
            containers: sizes.clone(),
            ..Default::default()
        };
        let reference = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut fresh,
            Some(&StyleContext {
                shadows: shadows.clone(),
                ..Default::default()
            }),
        );
        assert!(out.values == reference.values && out.pseudos == reference.pseudos);
    };
    check(&sizes);
    sizes.insert(host, [400., 50.]);
    sizes.insert(slot, [200., 50.]);
    check(&sizes);
    sizes.insert(outer, [250., 50.]);
    check(&sizes);
    sizes.insert(host, [100., 50.]);
    sizes.insert(slot, [50., 50.]);
    check(&sizes);
    #[cfg(debug_assertions)]
    assert_eq!(cache.incremental, 2);
}

#[test]
fn part_and_slotted_rules_query_the_containers_of_the_node_they_reach() {
    let mut doc = Document::parse(
        "<style>#outer{container-type:inline-size;container-name:out}
        @container (width > 500px){#host::part(p){color:red}}
        @container (200px < width < 500px){#host::part(p){padding-left:1px}}
        @container (width < 200px){#host::part(p){padding-right:2px}}
        @container in (width < 500px){#host::part(p){margin-left:3px}}
        @container out (width > 500px){#host::part(p){margin-right:4px}}
        @container (200px < width < 500px){#host::part(p)::before{content:'B'}}
        @container (width < 200px){#host::part(p)::after{content:'A'}}</style>
        <div id=outer><div id=host><span id=s>s</span></div></div>
        <div id=root><style>#in{container-type:inline-size;container-name:in}#p{container-type:inline-size}
        slot{display:block;container-type:inline-size}::slotted(span){display:block;container-type:inline-size}
        @container (width < 200px){::slotted(span)::before{content:'C'}}
        @container (200px < width < 500px){::slotted(span)::after{content:'D'}}
        @container (200px < width < 500px){::slotted(span){color:green}}</style>
        <slot id=slot></slot><div id=in><div><span id=p part=p>x</span></div></div></div>",
    );
    let [outer, s, slot, inner, p] =
        ["#outer", "#s", "#slot", "#in", "#p"].map(|selector| element(&doc, selector));
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    shadowed(&doc, &shadows, &mut cache, &mut last);
    let mut sizes = FxHashMap::from_iter([
        (outer, [600., 50.]),
        (inner, [300., 50.]),
        (p, [100., 50.]),
        (slot, [300., 50.]),
        (s, [100., 50.]),
    ]);
    let mut check = |sizes: &FxHashMap<usize, [f32; 2]>| {
        assert!(cache.set_containers(sizes, &doc));
        let out = shadowed(&doc, &shadows, &mut cache, &mut last);
        let get = |id: usize, prop: Prop| out.values[id].get(prop).map(str::to_owned);
        let content = |id: usize, slot: usize| {
            out.pseudos
                .get(&id)
                .and_then(|pair| pair[slot].get(Prop::CONTENT).map(str::to_owned))
        };
        let [within, own, slim, inside] = [inner, p, s, slot].map(|id| sizes[&id][0]);
        let mid = |width: f32| 200. < width && width < 500.;
        assert_eq!(get(p, Prop::COLOR).as_deref() == Some("red"), within > 500.);
        assert_eq!(
            get(p, Prop::PADDING_LEFT).as_deref() == Some("1px"),
            mid(within)
        );
        assert_eq!(
            get(p, Prop::PADDING_RIGHT).as_deref() == Some("2px"),
            within < 200.
        );
        assert_eq!(
            get(p, Prop::MARGIN_LEFT).as_deref() == Some("3px"),
            within < 500.
        );
        assert_eq!(get(p, Prop::MARGIN_RIGHT).as_deref(), Some("4px"));
        assert_eq!(content(p, 0).as_deref(), mid(own).then_some("'B'"));
        assert_eq!(content(p, 1).as_deref(), (own < 200.).then_some("'A'"));
        assert_eq!(content(s, 0).as_deref(), (slim < 200.).then_some("'C'"));
        assert_eq!(content(s, 1).as_deref(), mid(slim).then_some("'D'"));
        assert_eq!(get(s, Prop::COLOR).as_deref() == Some("green"), mid(inside));
        let mut fresh = StyleCache {
            containers: sizes.clone(),
            ..Default::default()
        };
        let reference = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut fresh,
            Some(&StyleContext {
                shadows: shadows.clone(),
                ..Default::default()
            }),
        );
        assert!(out.values == reference.values && out.pseudos == reference.pseudos);
    };
    check(&sizes);
    sizes.insert(inner, [600., 50.]);
    sizes.insert(p, [300., 50.]);
    sizes.insert(s, [300., 50.]);
    check(&sizes);
    sizes.insert(inner, [150., 50.]);
    sizes.insert(slot, [100., 50.]);
    check(&sizes);
    #[cfg(debug_assertions)]
    assert_eq!(cache.incremental, 3);
}

#[test]
fn slotted_and_part_rules_style_pseudo_elements() {
    let mut doc = Document::parse(
        "<style>#host::part(p)::before{content:'P';color:green}#host::part(p)::first-line{color:green}
        #host::part(p)::selection{color:green}#host::part(i)::placeholder{color:green}
        #host::part(p).x{color:red}#host::part(p)::before:hover{color:red}#host::part(p)::slotted(b){color:red}
        @starting-style{#host::part(p){opacity:0.5}}</style>
        <div id=host><span id=a class=a>a</span><span id=b>b</span><input id=c placeholder=x><li id=l>l</li></div>
        <div id=root><style>::slotted(span.a)::before{content:'B';background-color:green}
        ::slotted(.a)::before{background-color:red;color:green}::slotted(span)::after{content:'A'}
        ::slotted(input)::placeholder{color:green}::slotted(li)::marker{color:green}
        ::slotted(span)::first-line{color:red}::slotted(span):hover{color:red}
        ::slotted(span.a){color:green}::slotted(.a){color:red}@starting-style{::slotted(.a){opacity:0}}</style>
        <slot></slot><p id=p part=p>p</p><input id=i part=i placeholder=y></div>",
    );
    let [a, b, c, l, p, i] =
        ["#a", "#b", "#c", "#l", "#p", "#i"].map(|selector| element(&doc, selector));
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    let pseudo = |out: &ComputedStyles, id: usize, slot: usize, prop: Prop| {
        out.pseudos
            .get(&id)
            .and_then(|pair| pair[slot].get(prop).map(str::to_owned))
    };
    let text = |out: &ComputedStyles, id: usize, slot: usize| {
        out.text_pseudos
            .get(&id)
            .and_then(|list| list[slot].as_ref()?.get(Prop::COLOR).map(str::to_owned))
    };
    assert_eq!(out.values[a].get(Prop::COLOR), Some("green"));
    assert_eq!(pseudo(&out, a, 0, Prop::CONTENT).as_deref(), Some("'B'"));
    assert_eq!(pseudo(&out, a, 0, Prop::COLOR).as_deref(), Some("green"));
    assert_eq!(
        pseudo(&out, a, 0, Prop::BACKGROUND_COLOR).as_deref(),
        Some("green")
    );
    assert_eq!(pseudo(&out, a, 1, Prop::CONTENT).as_deref(), Some("'A'"));
    assert_eq!(pseudo(&out, b, 0, Prop::CONTENT), None);
    assert_eq!(pseudo(&out, b, 1, Prop::CONTENT).as_deref(), Some("'A'"));
    assert_eq!(text(&out, a, 2), None);
    assert_eq!(text(&out, c, 3).as_deref(), Some("green"));
    assert_eq!(text(&out, l, 0).as_deref(), Some("green"));
    assert_eq!(pseudo(&out, p, 0, Prop::CONTENT).as_deref(), Some("'P'"));
    assert_eq!(pseudo(&out, p, 0, Prop::COLOR).as_deref(), Some("green"));
    assert_eq!(text(&out, p, 2).as_deref(), Some("green"));
    assert_eq!(out.selections[&p].get(Prop::COLOR), Some("green"));
    assert_eq!(text(&out, i, 3).as_deref(), Some("green"));
    assert_ne!(out.values[p].get(Prop::COLOR), Some("red"));
    assert_eq!(out.starting[&a].get(Prop::OPACITY), Some("0"));
    assert_eq!(out.starting[&p].get(Prop::OPACITY), Some("0.5"));
    assert!(!out.starting.contains_key(&b));
    doc.nodes[a].state = crate::dom::HOVER;
    doc.nodes[p].state = crate::dom::HOVER;
    doc.nodes[p].attrs.set("class", "x");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(out.values[a].get(Prop::COLOR), Some("green"));
    assert_ne!(out.values[p].get(Prop::COLOR), Some("red"));
    assert_eq!(pseudo(&out, p, 0, Prop::COLOR).as_deref(), Some("green"));
    doc.nodes[a].attrs.set("class", "");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(pseudo(&out, a, 0, Prop::CONTENT), None);
    assert_eq!(pseudo(&out, a, 1, Prop::CONTENT).as_deref(), Some("'A'"));
    assert_ne!(out.values[a].get(Prop::COLOR), Some("green"));
    #[cfg(debug_assertions)]
    assert_eq!(cache.incremental, 2);
}

#[test]
fn parts_take_pseudo_classes_names_and_their_own_trees_host_rules() {
    let mut doc = Document::parse(
        "<style>#host::part(p):hover{color:green}#host::part(q){color:blue}#host::part(w  q){margin-top:6px}
        #host::part(q v){margin-bottom:7px}#host::part(x y){margin-top:8px}#host::part(x v){margin-bottom:9px}</style>
        <div id=host class=on></div>
        <div id=root><style>#p{color:red}:host::part(p){color:green;padding-left:1px}span{padding-left:2px}
        :host(.on)::part(p)::before{content:'H'}:host::part(q){color:red;padding-right:3px}
        #inner::part(r){margin-left:4px}</style>
        <span id=p part=p>p</span><span id=q part='q w'>q</span>
        <div id=inner exportparts='r:q, s:x, t:y, t:v'></div></div>
        <div id=root2><span id=r part=r>r</span><span id=t part='t s'>t</span></div>",
    );
    let [host, p, q, r, t] =
        ["#host", "#p", "#q", "#r", "#t"].map(|selector| element(&doc, selector));
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    let mut step = |doc: &Document, hover: bool, on: bool| {
        let out = shadowed(doc, &shadows, &mut cache, &mut last);
        let get = |id: usize, prop: Prop| out.values[id].get(prop).map(str::to_owned);
        assert_eq!(
            get(p, Prop::COLOR).as_deref(),
            Some(if hover { "green" } else { "red" })
        );
        assert_eq!(get(p, Prop::PADDING_LEFT).as_deref(), Some("1px"));
        assert_eq!(
            out.pseudos
                .get(&p)
                .and_then(|pair| pair[0].get(Prop::CONTENT).map(str::to_owned))
                .as_deref(),
            on.then_some("'H'")
        );
        assert_eq!(get(q, Prop::COLOR).as_deref(), Some("blue"));
        assert_eq!(get(q, Prop::PADDING_RIGHT).as_deref(), Some("3px"));
        assert_eq!(get(r, Prop::COLOR).as_deref(), Some("blue"));
        assert_eq!(get(r, Prop::MARGIN_LEFT).as_deref(), Some("4px"));
        assert_eq!(get(r, Prop::PADDING_RIGHT), None);
        assert_eq!(get(q, Prop::MARGIN_TOP).as_deref(), Some("6px"));
        assert_eq!(get(q, Prop::MARGIN_BOTTOM), None);
        assert_eq!(get(t, Prop::MARGIN_TOP).as_deref(), Some("8px"));
        assert_eq!(get(t, Prop::MARGIN_BOTTOM).as_deref(), Some("9px"));
    };
    step(&doc, false, true);
    doc.nodes[p].state = crate::dom::HOVER;
    step(&doc, true, true);
    doc.nodes[host].attrs.set("class", "off");
    step(&doc, true, false);
    doc.nodes[p].state = 0;
    doc.nodes[host].attrs.set("class", "on");
    step(&doc, false, true);
    #[cfg(debug_assertions)]
    assert_eq!(cache.incremental, 3);
}

#[test]
fn siblings_share_matches_only_when_they_read_no_position_or_neighbor() {
    let doc = Document::parse(
        "<html><head><style>li:first-child{order:1}li+li{z-index:2}li:nth-child(3){opacity:.5}b:empty{width:1px}p:has(i){height:2px}[style*=red]{left:3px}.row span{top:4px}.row span+span{right:5px}[data-n]{bottom:6px}[data-n='2']{max-width:7px}</style></head><body><div id=n><i data-n=1></i><i data-n=2></i><i data-n=2></i></div><ul id=l><li></li><li></li><li></li></ul><div id=e><b></b><b>x</b></div><div id=p><p><i></i></p><p></p></div><div id=s><s style='color:red'></s><s style='color:blue'></s></div><div id=r class=row><span></span><span></span></div></body></html>",
    );
    let values = styles(&doc, 800., 600.);
    let get = |parent: &str, index: usize, name: &str| {
        let kids: Vec<usize> = doc.nodes[element(&doc, parent)]
            .children
            .iter()
            .copied()
            .filter(|&kid| !doc.nodes[kid].tag.starts_with('#'))
            .collect();
        values[kids[index]].get(name).cloned()
    };
    for (parent, index, name, value) in [
        ("#l", 0, "order", Some("1")),
        ("#l", 1, "order", None),
        ("#l", 0, "z-index", None),
        ("#l", 1, "z-index", Some("2")),
        ("#l", 2, "opacity", Some(".5")),
        ("#l", 1, "opacity", None),
        ("#e", 0, "width", Some("1px")),
        ("#e", 1, "width", None),
        ("#p", 0, "height", Some("2px")),
        ("#p", 1, "height", None),
        ("#s", 0, "left", Some("3px")),
        ("#s", 1, "left", None),
        ("#r", 0, "top", Some("4px")),
        ("#r", 1, "top", Some("4px")),
        ("#r", 0, "right", None),
        ("#r", 1, "right", Some("5px")),
        ("#n", 0, "bottom", Some("6px")),
        ("#n", 0, "max-width", None),
        ("#n", 1, "max-width", Some("7px")),
        ("#n", 2, "max-width", Some("7px")),
    ] {
        assert_eq!(
            get(parent, index, name).as_deref(),
            value,
            "{parent} {index} {name}"
        );
    }
}

#[test]
fn cousins_share_matches_only_with_the_same_ancestors() {
    let rows =
        "<div class=row><span class=x>1</span></div><div class=row><span class=x>2</span></div>";
    for (css, groups) in [
        (
            ".a .x{color:red}",
            "<div class=g>R</div><div class='g a'>R</div>",
        ),
        (
            "[data-k=v] .x{color:red}",
            "<div class=g data-k=w>R</div><div class=g data-k=v>R</div>",
        ),
        (
            ".g[title] .x{color:red}",
            "<div class=g>R</div><div class=g title=t>R</div>",
        ),
        (
            ".g:hover .x{color:red}",
            "<div class=g>R</div><div class=g id=hovered>R</div>",
        ),
        (
            ".g:nth-child(2) .x{color:red}",
            "<div class=g>R</div><div class=g>R</div>",
        ),
        (".row:nth-child(odd) .x{color:red}", "<div class=g>R</div>"),
        (
            ".row:first-child .x{color:red}",
            "<div class=g>R</div><div class=g>R</div>",
        ),
        (
            ":not(.b)>.row .x{color:red}",
            "<div class=g>R</div><div class='g b'>R</div>",
        ),
        (
            ".g:is(.c) .x{color:red}",
            "<div class=g>R</div><div class='g c'>R</div>",
        ),
        (
            ".g:has(.y) .x{color:red}",
            "<div class=g>R</div><div class=g>R<i class=y></i></div>",
        ),
        (
            ".g+.g .x{color:red}",
            "<div class=g>R</div><div class=g>R</div>",
        ),
        (".row~.row .x{color:red}", "<div class=g>R</div>"),
        (
            ".g:empty~.g .x{color:red}",
            "<div class=g>R</div><div class=g></div><div class=g>R</div>",
        ),
    ] {
        let mut doc = Document::parse(&format!(
            "<style>{css}</style><div id=list>{}</div>",
            groups.replace('R', rows)
        ));
        if let Some(hovered) = (0..doc.nodes.len()).find(|&id| doc.matches(id, "#hovered")) {
            doc.nodes[hovered].state = crate::dom::HOVER;
        }
        let pass = |unshared: bool| {
            compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut HashMap::new(),
                &mut StyleCache {
                    unshared,
                    ..Default::default()
                },
                None,
            )
            .values
        };
        assert_eq!(pass(false), pass(true), "{css}");
    }
}

#[test]
fn children_share_values_by_what_their_parents_pass_on() {
    let doc = Document::parse(
        "<style>.c{padding-left:2em} .k{border-top-style:solid;border-top-width:inherit} .v{width:var(--w,1px)}</style>
        <div style='transform:translateX(1px)'><span class=c></span></div><div style='transform:translateX(2px)'><span class=c></span></div>
        <div style='border-top:3px solid'><span class='c k'></span></div><div style='border-top:4px solid'><span class='c k'></span></div>
        <div style='--w:5px'><span class='c v'></span></div><div style='font-size:10px'><span class=c></span></div>
        <div style='display:flex'><span class=c></span></div><div style='display:inline-grid'><span class=c></span></div>
        <div><span class=c></span></div><div><span class='c v'></span></div><div style='color:red'><span class=c></span></div>",
    );
    let values = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values;
    let spans: Vec<&Rc<Style>> = (0..doc.nodes.len())
        .filter(|&id| doc.nodes[id].tag == "span")
        .map(|id| &values[id])
        .collect();
    assert!(Rc::ptr_eq(spans[0], spans[1]));
    assert!(Rc::ptr_eq(spans[0], spans[8]));
    for (span, prop, value) in [
        (2, Prop::BORDER_TOP_WIDTH, Some("3px")),
        (3, Prop::BORDER_TOP_WIDTH, Some("4px")),
        (0, Prop::BORDER_TOP_STYLE, None),
        (4, Prop::WIDTH, Some("5px")),
        (9, Prop::WIDTH, Some("1px")),
        (10, Prop::COLOR, Some("red")),
        (5, Prop::FONT_SIZE, Some("10px")),
        (6, Prop::DISPLAY, Some("block")),
        (7, Prop::DISPLAY, Some("block")),
        (8, Prop::DISPLAY, None),
    ] {
        assert_eq!(spans[span].get(prop), value, "span {span} {prop:?}");
    }
}

#[test]
fn collapsed_cells_share_adjusted_styles_and_keep_them_when_other_rows_change() {
    let mut doc = Document::parse(&format!(
        "<style>table{{border-collapse:collapse}} td{{border:1px solid #ccc}} tr.blue{{border-top:4px solid blue}}
        tr.green{{border-top:4px solid green}} tr.on td{{background-color:red}}</style><table>{}</table>",
        "<tr><td></td><td></td></tr>".repeat(8)
    ));
    let rows: Vec<usize> = (0..doc.nodes.len())
        .filter(|&id| doc.nodes[id].tag == "tr")
        .collect();
    let cells = |row: usize, doc: &Document| -> Vec<usize> {
        doc.nodes[row]
            .children
            .iter()
            .copied()
            .filter(|&id| doc.nodes[id].tag == "td")
            .collect()
    };
    doc.nodes[rows[1]].attrs.set("class", "blue");
    doc.nodes[rows[3]].attrs.set("class", "green");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    cache.finish();
    let [blue, green] = [1, 3].map(|row| first.values[cells(rows[row], &doc)[0]].clone());
    assert_eq!(blue.get(Prop::BORDER_TOP_COLOR), Some("blue"));
    assert_eq!(green.get(Prop::BORDER_TOP_COLOR), Some("green"));
    let [middle, other] = [5, 6].map(|row| first.values[cells(rows[row], &doc)[0]].clone());
    assert!(Rc::ptr_eq(&middle, &other));
    doc.nodes[rows[4]].attrs.set("class", "on");
    let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    for row in [0, 1, 2, 3, 5, 6, 7] {
        for cell in cells(rows[row], &doc) {
            assert!(
                Rc::ptr_eq(&first.values[cell], &second.values[cell]),
                "row {row}"
            );
        }
    }
    let changed = &second.values[cells(rows[4], &doc)[0]];
    assert_eq!(changed.get(Prop::BACKGROUND_COLOR), Some("red"));
    assert_eq!(changed.get(Prop::MARGIN_TOP), middle.get(Prop::MARGIN_TOP));
}

#[test]
fn collapsed_groups_give_their_first_and_last_rows_their_borders() {
    let doc = Document::parse(
        "<style>table{border-collapse:collapse} td{border:1px solid #ccc} tbody{border-top:5px solid red;border-bottom:2px solid blue}</style>
        <table><thead><tr><td id=h></td></tr></thead><tbody><tr><td id=a></td></tr><tr><td id=b></td></tr><tr><td id=c></td></tr></tbody></table>",
    );
    let values = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values;
    let [h, a, b, c] = ["#h", "#a", "#b", "#c"].map(|selector| &values[element(&doc, selector)]);
    for (style, prop, value) in [
        (h, Prop::BORDER_BOTTOM_WIDTH, "5px"),
        (a, Prop::BORDER_TOP_WIDTH, "5px"),
        (a, Prop::BORDER_TOP_COLOR, "red"),
        (a, Prop::BORDER_BOTTOM_WIDTH, "1px"),
        (b, Prop::BORDER_TOP_WIDTH, "1px"),
        (c, Prop::BORDER_BOTTOM_WIDTH, "2px"),
        (c, Prop::BORDER_BOTTOM_COLOR, "blue"),
    ] {
        assert_eq!(style.get(prop), Some(value), "{prop:?}");
    }
}

#[test]
fn values_keep_a_style_per_inline_source_selection_and_custom_properties() {
    let doc = Document::parse(
        "<style>div::selection{color:red} .s::selection{color:blue} .c{width:var(--w,1px)}</style>
        <div id=d><span id=a></span><span id=b></span><span id=s class=s></span></div>
        <p style='--w:5px'><b class=c id=five></b></p><p style='--w:7px'><b class=c id=seven></b></p>
        <p style='--w:5px'><b class=c id=again></b></p>
        <i style='color:green' id=g1></i><i style='color:green' id=g2></i><i style='color:navy' id=n></i>",
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
    let id = |selector: &str| element(&doc, selector);
    let selection = |selector: &str| computed.selections[&id(selector)].clone();
    assert!(Rc::ptr_eq(&selection("#d"), &selection("#a")));
    assert_eq!(selection("#a").get(Prop::COLOR), Some("red"));
    assert_eq!(selection("#s").get(Prop::COLOR), Some("blue"));
    let style = |selector: &str| computed.values[id(selector)].clone();
    assert_eq!(style("#five").get(Prop::WIDTH), Some("5px"));
    assert_eq!(style("#seven").get(Prop::WIDTH), Some("7px"));
    assert!(Rc::ptr_eq(&style("#five"), &style("#again")));
    assert!(Rc::ptr_eq(&style("#g1"), &style("#g2")));
    assert_eq!(style("#n").get(Prop::COLOR), Some("navy"));
}
