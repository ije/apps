use super::*;

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
    assert!(wide.values[div].contains(Prop::ANIMATION));
    let motion = |value: &str| {
        MediaQuery::Feature("prefers-reduced-motion".into(), ":".into(), value.into()).matches(
            800.,
            600.,
            true,
            0,
            Lengths::new(16., 800., 600.),
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
fn huge_font_sizes_are_clamped() {
    let doc = Document::parse(
        "<p id=a style='font-size:100000000px'>a</p><p id=b style='font-size:calc(1e38px * 100)'>b</p>",
    );
    let styles = styles(&doc, 800., 600.);
    assert_eq!(styles[element(&doc, "#a")]["font-size"], "10000px");
    assert_eq!(styles[element(&doc, "#b")]["font-size"], "10000px");
}

#[test]
fn inherited_lengths_blockification_and_counters_compute_like_browsers() {
    let doc = Document::parse(
        "<html style='display:inline'><style>#a{font-size:10px;letter-spacing:1em;word-spacing:calc(2px + 1em);text-shadow:1em 0 red,0 0 2ex blue;width:calc(10px + 2em);border-top:7px none}#a *{font-size:40px}#w{width:inherit;border-top-width:inherit}#x{position:absolute;float:left;display:inline-table}#f{display:flex}#f::before{content:'x'}#f::after{content:'y';display:inline-grid}#u::after{content:'z';float:left}.r{counter-reset:c}.r span::before{content:counters(c,'.')}.k{counter-increment:q}.k::before{content:counter(q)}</style><body><div id=a><b id=c>x</b><i id=w></i></div><div id=x></div><div id=f></div><div id=u></div><div class=r></div><div class=r><span id=n></span></div><div><p class=k></p></div><div><p class=k id=k></p></div></body></html>",
    );
    let out = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    let get = |selector: &str, prop: Prop| {
        out.values[element(&doc, selector)]
            .get(prop)
            .map(str::to_owned)
    };
    for (prop, value) in [
        (Prop::LETTER_SPACING, "10px"),
        (Prop::WORD_SPACING, "12px"),
        (Prop::TEXT_SHADOW, "10px 0 red, 0 0 10px blue"),
    ] {
        assert_eq!(get("#c", prop).as_deref(), Some(value), "{prop:?}");
    }
    assert_eq!(get("#w", Prop::WIDTH).as_deref(), Some("30px"));
    assert_eq!(get("#w", Prop::BORDER_TOP_WIDTH).as_deref(), Some("0px"));
    assert_eq!(get("html", Prop::DISPLAY).as_deref(), Some("block"));
    // A table lays out as a grid, and its role keeps the display that APIs report.
    assert_eq!(get("#x", Prop::DISPLAY).as_deref(), Some("grid"));
    assert_eq!(
        TABLE_DISPLAYS[role(&out, element(&doc, "#x")) as usize],
        "table"
    );
    assert_eq!(get("#x", Prop::FLOAT), None);
    let pseudo = |selector: &str, slot: usize, prop: Prop| {
        out.pseudos[&element(&doc, selector)][slot]
            .get(prop)
            .map(str::to_owned)
    };
    assert_eq!(pseudo("#f", 0, Prop::DISPLAY).as_deref(), Some("block"));
    assert_eq!(pseudo("#f", 1, Prop::DISPLAY).as_deref(), Some("grid"));
    assert_eq!(pseudo("#u", 1, Prop::DISPLAY).as_deref(), Some("block"));
    assert_eq!(pseudo("#n", 0, Prop::CONTENT).as_deref(), Some("\"0\""));
    assert_eq!(pseudo("#k", 0, Prop::CONTENT).as_deref(), Some("\"1\""));
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
    // Like Chrome, a list with its own `counter-reset` still starts its `list-item` counter.
    assert_eq!([item("#x"), item("#y")], [Some(1), Some(2)]);
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
fn counters_stay_right_after_unrelated_and_list_changes() {
    let mut doc = Document::parse(
        "<style>h2{counter-increment:s}h2::before{content:counter(s)}.big{color:red}</style><h2 id=a>a</h2><p id=p>p</p><h2 id=b>b</h2><ol id=o start=3><li id=l1>x</li><li id=l2>y</li></ol>",
    );
    let [a, p, b, o, l1, l2] =
        ["#a", "#p", "#b", "#o", "#l1", "#l2"].map(|selector| element(&doc, selector));
    let mut cache = StyleCache::default();
    let mut last = None;
    let mut check = |doc: &Document, cache: &mut StyleCache, number: &str, items: [i64; 2]| {
        let out = incremental(doc, cache, &mut last, [800., 600.]);
        assert_eq!(
            out.pseudos[&b][0].get(Prop::CONTENT),
            Some(format!("\"{number}\"").as_str())
        );
        assert_eq!([out.list_items[&l1], out.list_items[&l2]], items);
    };
    check(&doc, &mut cache, "2", [3, 4]);
    doc.nodes[p].attrs.insert("class".into(), "big".into());
    check(&doc, &mut cache, "2", [3, 4]);
    doc.nodes[o].attrs.insert("start".into(), "10".into());
    check(&doc, &mut cache, "2", [10, 11]);
    doc.nodes[l2].attrs.insert("value".into(), "20".into());
    check(&doc, &mut cache, "2", [10, 20]);
    doc.nodes[o].attrs.insert("reversed".into(), String::new());
    doc.nodes[l2].attrs.remove("value");
    check(&doc, &mut cache, "2", [10, 9]);
    doc.nodes[a]
        .attrs
        .insert("style".into(), "counter-increment:s 5".into());
    check(&doc, &mut cache, "6", [10, 9]);
    doc.nodes[o]
        .attrs
        .insert("start".into(), i64::MAX.to_string());
    doc.nodes[o].attrs.remove("reversed");
    check(&doc, &mut cache, "6", [i64::MAX, i64::MAX]);
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
fn parser_keeps_nested_font_faces_prefixed_keyframes_and_media_types() {
    let sheet = Stylesheet::parse(
        "@layer base{@font-face{font-family:B;src:local(Arial);unicode-range:U+0400-04FF}}@supports (display:flex){@font-face{font-family:D;src:local(Arial)}}@supports (display:nope){@font-face{font-family:E;src:local(Arial)}}@-webkit-keyframes spin{to{opacity:0}}@media screen and (min-width:100px){}@media print{}@media only screen{}",
    );
    let faces = sheet.font_faces();
    assert_eq!(
        faces
            .iter()
            .map(|(family, _, range)| (family.as_str(), range.as_deref()))
            .collect::<Vec<_>>(),
        [("B", Some("U+0400-04FF")), ("D", None)]
    );
    assert!(
        matches!(&*sheet.0[3], Rule::Keyframes(name, frames) if name == "spin" && frames.len() == 1)
    );
    let source = sheet.source();
    for text in [
        "@media screen and (min-width: 100px)",
        "@media print{",
        "@media only screen{",
    ] {
        assert!(source.contains(text), "{text} in {source}");
    }
    let style = declarations(
        r#"margin:1px/**/2px;color:r\65 d;width:\31 0px;content:"a env(safe-area-inset-top) b";padding-top:env(safe-area-inset-top,9px);background:url(x.png) fixed content-box"#,
    );
    assert_eq!(&style["margin-top"], "1px");
    assert_eq!(&style["margin-right"], "2px");
    assert_eq!(&style["color"], "red");
    assert!(style.get(Prop::WIDTH).is_none());
    assert_eq!(&style["content"], r#""a env(safe-area-inset-top) b""#);
    assert_eq!(&style["padding-top"], "0px");
    assert_eq!(&style["background-attachment"], "fixed");
    assert_eq!(&style["background-origin"], "content-box");
    assert_eq!(&style["background-clip"], "content-box");
    for (value, valid) in [
        ("rgb(10px 0 0)", false),
        ("rgb(0 0 0 / 5px)", false),
        ("rgb(255, 50%, 0)", false),
        ("hsl(120, 100, 50)", false),
        ("hsl(120, 100%, 50%)", true),
        ("rgb(100%, 50%, 0%)", true),
        ("rgb(255 50% 0 / 50%)", true),
    ] {
        assert_eq!(color(value).is_some(), valid, "{value}");
    }
}

#[test]
fn list_counters_count_like_chrome() {
    for (kind, number, text) in [
        ("lower-greek", 7, "η"),
        ("lower-greek", 25, "αα"),
        ("hiragana", 9, "け"),
        ("katakana", 2, "イ"),
        ("cjk-decimal", 205, "二〇五"),
        ("cjk-ideographic", 8, "八"),
        ("cjk-ideographic", 15, "十五"),
        ("cjk-ideographic", 105, "一百零五"),
        ("armenian", 10, "Ժ"),
        ("lower-armenian", 1, "ա"),
        ("georgian", 11, "ია"),
        ("hebrew", 12, "יב"),
        ("hebrew", 15, "טו"),
    ] {
        assert_eq!(counter_text(kind, number), text, "{kind} {number}");
    }
}

#[test]
fn review_cases_follow_chrome() {
    let doc = Document::parse(
        "<style>#a{min-width:calc(50% * -1);font-weight:999.9999;line-height:calc(1e40);background-size:12345678px auto;transform:translate(1e-7px, 1234567px);transition-duration:0.1234567s}#b{position:static;top:calc((10% + 5px) * 2);min-width:calc(min(10%, 5px) + 1px);max-width:calc(10% + max(1px, 2px));background-position:calc(1em - 10%) 0;display:none;width:calc(50% * -1)}</style><div id=a></div><div id=b></div><svg><circle id=c r=5 cx=10%></circle><rect id=d x=1 y=2em rx=3 width=10 height=20></rect><rect id=e width=-10 rx=auto></rect><use id=u width=7></use></svg><div id=f style='border:0.3px solid;outline:7.7px solid;outline-offset:-0.3px'></div>",
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
    for (selector, name, expected) in [
        ("#a", "min-width", "0%"),
        ("#a", "font-weight", "999.75"),
        ("#a", "line-height", "3.40282e+38px"),
        ("#a", "background-size", "1.23457e+07px auto"),
        ("#a", "transition-duration", "0.123457s"),
        ("#b", "top", "calc(20% + 10px)"),
        ("#b", "min-width", "calc(1px + min(10%, 5px))"),
        ("#b", "max-width", "calc(10% + 2px)"),
        ("#b", "background-position", "calc(-10% + 16px) 0px"),
        ("#b", "width", "0%"),
        ("#c", "r", "5px"),
        ("#c", "cx", "10%"),
        ("#d", "x", "1px"),
        ("#d", "y", "32px"),
        ("#d", "rx", "3px"),
        ("#d", "width", "10px"),
        ("#e", "rx", "0px"),
        ("#u", "width", "auto"),
        ("#f", "border-top-width", "1px"),
        ("#f", "outline-width", "7px"),
        ("#f", "outline-offset", "-1px"),
        ("#f", "border", "1px solid rgb(0, 0, 0)"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
    for (name, value, expected) in [
        ("width", "calc(1e7px + 1em)", "calc(1em + 1e+07px)"),
        ("width", "calc(1e-10px + 1em)", "calc(1em + 1e-10px)"),
        ("width", "calc(1234567px + 1px)", "calc(1.23457e+06px)"),
        ("width", "calc(1px\t+\t1em)", "calc(1em + 1px)"),
        ("width", "calc(e * 1px + 1em)", "calc(1em + 2.71828px)"),
        (
            "width",
            "calc(min(1em,5px)*-1 + 2px)",
            "calc(2px + (-1 * min(1em, 5px)))",
        ),
        ("aspect-ratio", "calc(16 / 9)", "calc(1.77778) / 1"),
        ("box-shadow", "calc(1px + 1px) 0 red", "red calc(2px) 0px"),
        ("src", "local(Arial)", "local(\"Arial\")"),
    ] {
        assert_eq!(specified(name, value), expected, "{name}: {value}");
    }
    for (name, value, valid) in [
        ("width", "calc(0)", false),
        ("grid-template-columns", "repeat(2, calc(1fr))", false),
        (
            "grid-template-columns",
            "minmax(calc(10px + 5px), 1fr)",
            true,
        ),
        ("transform", "matrix(1,0,0,1,calc(1px + 2px),0)", false),
        ("transform", "translate(10%, 0) rotate(0) scale(50%)", true),
    ] {
        assert_eq!(supports(name, value), Some(valid), "{name}: {value}");
    }
    for (value, weight, size) in [
        ("0/0 a", "normal", "0"),
        ("bold 0 serif", "bold", "0"),
        ("calc(2000) 12px serif", "calc(2000)", "12px"),
    ] {
        let mut out = Vec::new();
        crate::css::expand("font", value, &mut out);
        let get = |name| {
            out.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(
            (get("font-weight"), get("font-size")),
            (Some(weight), Some(size)),
            "{value}"
        );
    }
    let mut out = Vec::new();
    crate::css::expand("font", "bold calc(1px) a", &mut out);
    assert!(
        out.iter()
            .any(|(key, value)| key == "font-size" && value == "calc(1px)")
    );
    for (weight, kept) in [
        ("calc(100 + 50)", true),
        ("calc(100 + 50) 900", true),
        ("bolder", false),
        ("var(--x)", false),
        ("400 auto", false),
    ] {
        let face = Declarations::descriptors(&format!("font-weight:{weight}"));
        assert_eq!(face.0.len(), usize::from(kept), "{weight}");
    }
    for value in ["12px 1001", "bold 900 12px a", "900 bold 12px a"] {
        let mut out = Vec::new();
        crate::css::expand("font", value, &mut out);
        assert!(out.is_empty(), "{value}");
    }
}
