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
    assert!(cache.set_containers(HashMap::from([(a, [500., 50.]), (b, [200., 100.])])));
    let values = compute(&mut cache);
    assert_eq!(&values[outer]["color"], "red");
    assert_eq!(&values[outer]["order"], "5");
    assert!(!values[outer].contains(Prop::WIDTH) && !values[outer].contains(Prop::Z_INDEX));
    assert_eq!(&values[inner]["color"], "red");
    assert_eq!(&values[inner]["width"], "1px");
    assert!(!values[inner].contains(Prop::ORDER) && !values[inner].contains(Prop::HEIGHT));
    assert!(!cache.set_containers(HashMap::from([(a, [450., 50.]), (b, [200., 100.])])));
    assert!(cache.set_containers(HashMap::from([(a, [90., 50.]), (b, [200., 100.])])));
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
    let ink = |color: u32| renderer::rgba(color);
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
            .all(|run| run.font.weight == renderer::FontWeight(700.))
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
    assert_eq!(shadows[0].color, renderer::rgba(0x00ffffff));
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
        // Values without viewport units or rules under a new media result are kept and get the new viewport.
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

fn typed(values: &BTreeMap<String, String>) -> Style {
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
    let vars = |selector| computed.values[element(&doc, selector)].vars().clone();
    assert_eq!(vars("html").len(), 300);
    assert!(Rc::ptr_eq(&vars("html"), &vars("#a")));
    assert!(Rc::ptr_eq(&vars("#a"), &vars("#c")));
    assert!(!Rc::ptr_eq(&vars("#a"), &vars("#d")));
    assert_eq!(
        computed.values[element(&doc, "#d")].get(Prop::find("--v7").unwrap()),
        Some("1px")
    );
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
    assert!(cache.set_containers(HashMap::from([(container, [500., 100.])])));
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
    assert!(cache.set_containers(HashMap::from([(container, [300., 100.])])));
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
            [200., 100.],
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
            [800., 600.],
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
fn syntax_checks_never_split_multibyte_text() {
    assert!(!syntax_matches("x(", "aéé"));
    assert!(!syntax_matches("<length> | fit-content(", "aaéé"));
    assert!(syntax_matches("x(", "X(é)"));
    assert_eq!(supports("width", "aaéé"), Some(false));
    assert!(!supports_condition("(width: aaéé)"));
}

#[test]
fn slotted_and_part_declarations_are_part_of_the_cache_key() {
    let doc = Document::parse("<div><span id=a></span><span id=b></span></div>");
    let [a, b] = ["#a", "#b"].map(|selector| element(&doc, selector));
    let red: Vec<Declared> = vec![(Prop::COLOR, "red".into(), false)];
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (on, off) in [(a, b), (b, a)] {
        let context = StyleContext {
            slotted: FxHashMap::from_iter([(on, red.clone())]),
            ..StyleContext::default()
        };
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut cache,
            Some(&context),
        );
        cache.finish();
        assert_eq!(computed.values[on].get(Prop::COLOR), Some("red"));
        assert_ne!(computed.values[off].get(Prop::COLOR), Some("red"));
    }
    let context = StyleContext {
        parts: FxHashMap::from_iter([(a, red)]),
        ..StyleContext::default()
    };
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut sheets,
        &mut cache,
        Some(&context),
    );
    assert_eq!(computed.values[a].get(Prop::COLOR), Some("red"));
    assert_ne!(computed.values[b].get(Prop::COLOR), Some("red"));
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
    assert!(cache.set_containers(HashMap::from([(a, [500., 50.]), (b, [200., 50.])])));
    let computed = compute(&mut cache, 800.);
    assert_eq!(&computed.values[first]["font-size"], "20px");
    assert_eq!(&computed.values[second]["font-size"], "8px");
    assert_eq!(&computed.values[v]["font-size"], "40px");
    assert!(cache.set_containers(HashMap::from([(a, [500., 50.]), (b, [300., 50.])])));
    let computed = compute(&mut cache, 400.);
    assert_eq!(&computed.values[first]["font-size"], "20px");
    assert_eq!(&computed.values[second]["font-size"], "12px");
    assert_eq!(&computed.values[v]["font-size"], "20px");
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
    cache.set_containers(HashMap::from([(a, [300., 50.])]));
    let red = Some("red");
    assert_ne!(compute(&mut cache, 800.).values[p].get(Prop::COLOR), red);
    assert_ne!(compute(&mut cache, 700.).values[p].get(Prop::COLOR), red);
    assert!(cache.set_containers(HashMap::from([(a, [500., 50.])])));
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
        let before = cache.sheets[&style].2.clone();
        let text = doc.nodes[style].children[0];
        doc.nodes[text].text.push_str(added);
        let computed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let after = &cache.sheets[&style].2;
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
    assert!(splits(&doc, &computed, id("h")));
    assert!(!splits(&doc, &computed, id("s")));
    assert!(phantom(&doc, &computed, id("s")));
    assert!(!phantom(&doc, &computed, id("t")));
    assert!(!phantom(&doc, &computed, id("u")));
    // A hidden `<br>` breaks nothing, and the space before a break hangs.
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
