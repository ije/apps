use super::*;

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
        .map(|rule| match rule {
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
    assert!(compute(&mut cache)[outer].get("color") != Some(&"red".to_owned()));
    assert!(cache.set_containers(HashMap::from([(a, [500., 50.]), (b, [200., 100.])])));
    let values = compute(&mut cache);
    assert_eq!(values[outer]["color"], "red");
    assert_eq!(values[outer]["order"], "5");
    assert!(!values[outer].contains_key("width") && !values[outer].contains_key("z-index"));
    assert_eq!(values[inner]["color"], "red");
    assert_eq!(values[inner]["width"], "1px");
    assert!(!values[inner].contains_key("order") && !values[inner].contains_key("height"));
    assert!(!cache.set_containers(HashMap::from([(a, [450., 50.]), (b, [200., 100.])])));
    assert!(cache.set_containers(HashMap::from([(a, [90., 50.]), (b, [200., 100.])])));
    let values = compute(&mut cache);
    assert!(values[outer].get("color") != Some(&"red".to_owned()));
    assert_eq!(values[outer]["z-index"], "3");
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
        "<style>.on span{color:red}i+span{font-weight:700}span:nth-child(2){background-color:yellow}div:has(.mark) span{color:blue}span:empty::before{content:'empty'}</style><div><i>head</i><span id=a>alpha</span><span id=b></span></div>",
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
    assert_eq!(inline.backgrounds.len(), 1);
    assert_eq!(inline.backgrounds[0].1, 0xffee00ff);
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
        .insert("style".into(), "padding:4px".into());
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
                .backgrounds
                .iter()
                .map(|&(index, color)| (inline.ranges[index].1.clone(), color))
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
    assert_eq!(marker["color"], "blue");
    assert!(marker.contains_key("content") && marker.contains_key("font-size"));
    assert!(computed.text_pseudos[&p][0].is_none());
    let inline = inline_text(&doc, &computed, p).unwrap();
    assert_eq!(&*inline.text, "\"Hi there x");
    assert_eq!(
        inline.runs.iter().map(|run| run.len).sum::<usize>(),
        inline.text.len()
    );
    let ink = |color: u32| gpui::Hsla::from(gpui::rgba(color));
    assert_eq!(
        (inline.runs[0].len, inline.runs[0].color),
        (2, ink(0xff0000ff))
    );
    assert_eq!(inline.runs[1].color, ink(0x111111ff));
    assert_eq!(
        inline.shadows,
        vec![(0., 0., 3., 0x111111ff), (1., 2., 0., 0xff0000ff)]
    );
    let (base, pseudo, mode) = inline.first_line.as_ref().unwrap();
    assert_eq!(mode, &None);
    let runs = restyle(&inline.runs, 0..inline.text.len(), base, pseudo);
    let colors: Vec<_> = runs.iter().map(|run| run.color).collect();
    assert_eq!(
        colors[..3],
        [ink(0xff0000ff), ink(0x0000ffff), ink(0x00ff00ff)]
    );
    assert!(
        runs.iter()
            .all(|run| run.font.weight == gpui::FontWeight(700.))
    );
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
        computed.values[element(&doc, "td")]["vertical-align"],
        "middle"
    );
    assert_eq!(
        computed.values[element(&doc, "td+td")]["vertical-align"],
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
    assert_eq!(inherited.values[children[0]]["color"], "green");
    assert_eq!(
        inherited.selections[&children[0]]["background-color"],
        "green"
    );
    doc.nodes[children[1]].tag = "b".into();
    let matched = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(matched.values[children[0]]["color"], "blue");
    let sheet = doc
        .nodes
        .iter()
        .position(|node| node.tag == "style")
        .unwrap();
    doc.nodes[sheet].text = "span{color:purple}".into();
    doc.nodes[sheet].stylesheet = None;
    let changed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert_eq!(changed.values[children[0]]["color"], "purple");
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
        assert!(!computed.selections[&id].contains_key("font-size"));
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
    assert_eq!(narrow.values[id]["caret-color"], "red");
    assert_eq!(wide.values[id]["caret-color"], "#0000");
    assert_eq!(wide.selections[&id]["background-color"], "#0000");
    let div = doc.nodes[id]
        .parent
        .and_then(|parent| doc.nodes[parent].parent)
        .unwrap();
    assert_eq!(
        wide.values[div].contains_key("animation"),
        !reduced_motion()
    );
    let motion = |value: &str| {
        MediaQuery::Feature("prefers-reduced-motion".into(), ":".into(), value.into())
            .matches(800., 600., true, 0)
    };
    assert_eq!(motion("reduce"), Some(true));
    assert_eq!(motion("no-preference"), Some(false));
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
    assert_eq!(computed.values[id]["color"], "blue");
    assert_eq!(computed.values[id]["background-color"], "red");
    assert_eq!(computed.values[id]["display"], "grid");
    assert_eq!(computed.values[id]["width"], "100cqi");
    assert_eq!(computed.values[id]["cursor"], "pointer");
    assert!(!computed.values[id].contains_key("content"));
    assert_eq!(computed.pseudos[&id][0]["background-color"], "red");
    assert_eq!(computed.pseudos[&id][0]["width"], "4px");
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
            :host { --edge:2px solid #fff; --bg:#fff; --ink:light-dark(#f00,#0f0); color-scheme:dark; }
            .item { border-right:var(--edge); border-left:1px solid var(--bg); padding-inline:1ch 2ch; margin-block:8px; color:var(--ink); background-color:color-mix(in lab, white 50%, black); }
            .item span { background-color:light-dark(inherit,inherit); }
        </style><apps-shadow-root><div class="item"><span>text</span></div></apps-shadow-root>"#,
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
    assert_eq!(computed[item]["color"], "#ff0000ff");
    assert_eq!(computed[item]["border-top-color"], "#ff000080");
    assert_eq!(computed[item]["background-color"], "#ff000080");
    assert_eq!(computed[span]["color"], "#ff8080ff");
    assert_eq!(computed[span]["fill"], "#ff8080ff");
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
    assert_eq!(style["color"], "red !important");
    assert_eq!(style["padding-top"], "2px");
    assert_eq!(style["padding-right"], "4px");
    assert_eq!(style["--Case"], r#"{ key: [a;b]; nested: fn(";}"); }"#);
    assert_eq!(style["--case"], r#""/* text */;escaped\"}""#);
    assert_eq!(style["--tokens"], "one/**/two");
    assert_eq!(style["content"], r#""!important""#);
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
    assert_eq!(style["color"], "red");
    assert_eq!(style["height"], "7px");
    assert_eq!(style["padding-left"], "3px");
    assert_eq!(style["margin-bottom"], "4px");
    assert_eq!(style["border-top-width"], "2px");
    assert_eq!(style["--empty"], "");
    for key in ["width", "content", "background", "opacity"] {
        assert!(!style.contains_key(key), "{key}");
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
        assert_eq!(edges(style, "padding", width, 16., width, 600.), [3.; 4]);
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
        computed.lengths[section].edges(&computed.values[section], "padding", 800.),
        [30., 40., 30., 40.]
    );
    assert_eq!(computed.lengths[absolute].font.em, 20.);
    assert_eq!(computed.lengths[absolute].font.lh, 60.);
    assert_eq!(computed.values[absolute]["line-height"], "60px");
    assert_eq!(computed.lengths[number].font.lh, 40.);
    assert_eq!(computed.lengths[child].font.em, 10.);
    assert_eq!(computed.lengths[child].font.lh, 20.);
    assert_eq!(computed.values[child]["line-height"], "2");
    assert_eq!(computed.lengths[relative].font.lh, 120.);
    assert_eq!(
        computed.lengths[relative].edges(&computed.values[relative], "margin", 800.),
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
    assert_eq!(units.edges(style, "padding", 800.), [22., 30., 22., 30.]);
    assert_eq!(units.resolve(&style["height"], 600.), Some(560.));
    assert_eq!(units.resolve(&style["width"], 800.), Some(400.));
    let shadows = units.shadows(&style["box-shadow"], false);
    assert_eq!(shadows[0].offset.y, gpui::px(30.));
    assert_eq!(shadows[0].blur_radius, gpui::px(4.));
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
    assert_eq!(style["border-bottom-left-radius"], "8px");
    assert_eq!(style["border-bottom-right-radius"], "8px");
    assert!(!style.contains_key("border-top-left-radius"));
    let paint = Lengths::new(16., 800., 600.).shadows(&style["box-shadow"], false);
    assert_eq!(paint.len(), 2);
    assert_eq!(paint[1].offset, gpui::point(gpui::px(0.), gpui::px(4.)));
    assert_eq!(paint[1].blur_radius, gpui::px(3.));
    assert_eq!(paint[1].spread_radius, gpui::px(-1.));
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
    assert_eq!(style["font-size"], "32px");
    assert_eq!(style["font-family"], "Helvetica");
    assert_eq!(style["font-weight"], "850");
    assert_eq!(
        edges(&style, "padding", 799., 32., 799., 600.),
        [20., 20., 20., 10.]
    );
    assert_eq!(
        edges(&style, "margin", 799., 32., 799., 600.),
        [-23., 0., -10., 0.]
    );
    assert_eq!(length(&style["width"], 799., 32., 799., 600.), 23.);
    assert_eq!(
        Lengths::new(32., 799., 600.)
            .shadows(&style["box-shadow"], false)
            .len(),
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
    assert_eq!(outer[0].blur_radius, gpui::px(4.));
    assert_eq!(inset.len(), 2);
    assert_eq!(inset[0].offset, gpui::point(gpui::px(1.), gpui::px(2.)));
    assert_eq!(inset[1].spread_radius, gpui::px(2.));
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
    assert_eq!(shadows[0].offset, gpui::point(gpui::px(2.), gpui::px(3.)));
    assert_eq!(gpui::Rgba::from(shadows[0].color), gpui::rgba(0x00ffffff));
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
        for (width, expected) in [
            (639., "1rem"),
            (640., "2rem"),
            (768., "3rem"),
            (1024., "4rem"),
        ] {
            let style = styles(&doc, width, 600.).pop().unwrap();
            assert_eq!(style["width"], expected);
            assert_eq!(style["height"], "2rem");
            assert_eq!(
                edges(&style, "padding", width, 16., width, 600.),
                [32., 16., 32., 8.]
            );
            assert_eq!(style["row-gap"], "1rem");
            assert_eq!(style["column-gap"], "0.5rem");
        }
    }
}

#[test]
fn css_and_inline_shorthands_override_utilities() {
    let doc = Document::parse(
        "<style>@import 'tailwindcss'; div { padding: 2px 3px; gap: 4px; background: blue; width: 20px !important; } .box { padding-left: 5px; }</style><div class='box p-8 bg-white gap-8 w-4' style='padding: 6px; gap: 7px 8px; background: black; width: 30px'></div>",
    );
    let style = styles(&doc, 800., 600.).pop().unwrap();
    assert_eq!(edges(&style, "padding", 800., 16., 800., 600.), [6.; 4]);
    assert_eq!(style["background-color"], "black");
    assert_eq!(style["row-gap"], "7px");
    assert_eq!(style["column-gap"], "8px");
    assert_eq!(style["width"], "20px");
    let style = declarations(
        "padding: 1px !important; padding: 2px; padding-left: 3px; padding-right: 4px !important",
    );
    assert_eq!(style["padding-left"], "1px !important");
    assert_eq!(style["padding-right"], "4px !important");
    let doc = Document::parse(
        "<html><style>@import 'tailwindcss'; :root { --space: 2px 4px; } div { padding: var(--space); gap: var(--missing, 5px 6px); margin: var(--space); margin-left: 9px; }</style><div class='p-8 gap-8 m-8'></div></html>",
    );
    let style = styles(&doc, 800., 600.).pop().unwrap();
    assert_eq!(
        edges(&style, "padding", 800., 16., 800., 600.),
        [2., 4., 2., 4.]
    );
    assert_eq!(
        edges(&style, "margin", 800., 16., 800., 600.),
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
    assert_eq!(style["display"], "grid");
    assert_eq!(style["grid-template-columns"], "repeat(2,1fr)");
    assert_eq!(style["min-height"], "100vh");
    assert_eq!(style["max-width"], "100%");
    assert_eq!(style["width"], "50%");
    assert_eq!(style["height"], "30px");
    assert_eq!(style["margin-top"], "-0.5rem");
    assert_eq!(style["margin-left"], "auto");
    assert_eq!(style["margin-right"], "auto");
    assert_eq!(style["border-left-width"], "2px");
    assert_eq!(style["border-bottom-color"], "#123456");
    assert_eq!(style["border-bottom-left-radius"], "12px");
    assert_eq!(style["background-color"], "#abcdef");
    assert_eq!(style["font-size"], "18px");
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
            style["background-color"],
            if hovered { "#222222" } else { "#111111" }
        );
        assert_eq!(
            style.get("color").map(String::as_str),
            Some(if hovered { "#333333" } else { "#171717" })
        );
        assert_eq!(
            cached.values[span].get("font-weight").map(String::as_str),
            hovered.then_some("700")
        );
        assert_eq!(cached.values[span]["padding-top"], "3px");
        assert_eq!(cached.values[bold]["padding-top"], "0");
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
