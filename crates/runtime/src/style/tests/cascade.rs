use super::*;

#[test]
fn cssom_edits_replace_group_rules_and_reject_bad_selectors() {
    let mut sheet = Stylesheet::parse(
        "@keyframes k{from{opacity:0}} @media print{.a{color:red}} .b{color:blue} .n{& .c{color:red}}",
    );
    sheet
        .edit(
            &[0],
            Edit::Replace("@keyframes k2{0%{opacity:.5}50%{opacity:1}}"),
        )
        .unwrap();
    sheet.edit(&[1], Edit::Replace("@media all{}")).unwrap();
    assert_eq!(sheet.edit(&[1], Edit::Replace(".x{}")), Err("SyntaxError"));
    assert_eq!(sheet.edit(&[2], Edit::Selector("###")), Err("SyntaxError"));
    assert_eq!(
        sheet.edit(&[2], Edit::Selector("div  >  .c")),
        Ok(serde_json::json!("div > .c"))
    );
    assert_eq!(
        sheet.edit(&[3, 0], Edit::Selector("& > .d")),
        Ok(serde_json::json!("& > .d"))
    );
    assert_eq!(sheet.edit(&[], Edit::Delete), Err("IndexSizeError"));
    let tree = sheet.cssom();
    assert_eq!(
        tree[0],
        serde_json::json!([
            "keyframes",
            "k2",
            [
                ["0%", [["opacity", ".5", ""]]],
                ["50%", [["opacity", "1", ""]]]
            ]
        ])
    );
    assert_eq!(tree[1][1], serde_json::json!(["all"]));
    assert_eq!(tree[1][2][0][1], ".a");
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
        assert!(
            (0..doc.nodes.len()).all(|id| cached.generated(id) == fresh.generated(id)),
            "step {step}"
        );
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
                Rc::ptr_eq(last.generated(p).unwrap(), cached.generated(p).unwrap()),
                step == 1,
                "step {step}"
            );
        }
        last = Some(Rc::new(cached));
    }
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
    assert_eq!(
        &computed.generated(id).unwrap()[0]["background-color"],
        "red"
    );
    assert_eq!(&computed.generated(id).unwrap()[0]["width"], "4px");
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
    let before = &computed.generated(element(&doc, "p")).unwrap()[0];
    assert_eq!(before.get(Prop::FONT_WEIGHT), Some("700"));
    assert_eq!(before.get(Prop::TEXT_TRANSFORM), Some("uppercase"));
    assert_eq!(before.get(Prop::LETTER_SPACING), Some("2px"));
    assert_eq!(before.get(Prop::WIDTH), None);
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
        computed
            .extras(element(&doc, "#a"))
            .starting
            .as_ref()
            .unwrap()
            .get(Prop::OPACITY),
        Some("0")
    );
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
            .generated(element(&doc, selector))
            .and_then(|pseudos| pseudos[index].get(Prop::CONTENT).map(str::to_owned))
    };
    assert_eq!(content("div", 0).as_deref(), Some("'a'"));
    assert_eq!(content("div", 1).as_deref(), Some("'i'"));
    assert_eq!(content("p", 0).as_deref(), Some("'p'"));
    assert_eq!(content("span", 1).as_deref(), Some("'x'"));
    assert_eq!(content("b", 0), None);
}

#[test]
fn selectors_match_through_value_and_alternative_buckets() {
    let doc = Document::parse(
        "<style>[data-k=a]{color:red}[data-k=A]{color:blue}[data-k=b i]{color:green}:is([data-k=c],.d){color:navy}.p{&.q,&[data-k=e]{color:teal}}:where(x-a:not([hidden]),x-a[hidden=until-found]){color:gray}</style><i data-k=a></i><u data-k=B></u><s data-k=c></s><s class=d></s><b class='p q'></b><b class=p data-k=e></b><b class=q></b><x-a></x-a><x-a hidden=until-found></x-a>",
    );
    let computed = styles(&doc, 800., 600.);
    let color = |selector: &str| {
        (0..doc.nodes.len())
            .filter(|&id| crate::dom::matches(&doc, id, selector))
            .map(|id| computed[id].get("color").cloned().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert_eq!(color("i"), ["red"]);
    assert_eq!(color("u"), ["green"]);
    assert_eq!(color("s"), ["navy"; 2]);
    assert_eq!(color("b"), ["teal", "teal", "#000000"]);
    assert_eq!(color("x-a"), ["gray"; 2]);
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
    assert!(closed.backdrops().next().is_none());
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
    let backdrop =
        |id: usize, prop: &str| open.extras(id).backdrop.as_ref().unwrap()[prop].to_string();
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
    assert!(open.extras(n).backdrop.is_none());
    doc.nodes[f].state = 0;
    let exited = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert!(!exited.values[f].contains(Prop::WIDTH) && exited.extras(f).backdrop.is_none());
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
    assert_eq!(
        &open.extras(d).backdrop.as_ref().unwrap()["background-color"],
        "red"
    );
    assert_eq!(
        &open.extras(d).backdrop_start.as_ref().unwrap()["background-color"],
        "blue"
    );
    assert!(open.extras(p).backdrop_start.is_none());
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
    assert!(leaving.extras(d).backdrop.is_some() && leaving.extras(p).backdrop.is_some());
    assert!(!crate::dom::matches(&doc, d, ":modal"));
    doc.nodes[d].state = 0;
    let left = styles(&doc);
    assert_eq!(&left.values[d]["position"], "absolute");
    assert!(left.extras(d).backdrop.is_none());
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
fn nested_parent_selectors_record_their_attributes_once() {
    let mut doc = Document::parse(
        "<style>.box[data-on]{& span{order:3} & & &{width:1px}}</style><div class=box id=a><span id=s></span></div>",
    );
    let (a, s) = (element(&doc, "#a"), element(&doc, "#s"));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for step in 0..3 {
        match step {
            1 => {
                doc.nodes.edit(a).unwrap().attrs.set("data-on", "");
            }
            2 => {
                doc.nodes.edit(a).unwrap().attrs.remove("data-on");
            }
            _ => {}
        }
        let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(
            cached.values[s].get(Prop::ORDER),
            (step == 1).then_some("3"),
            "step {step}"
        );
        cache.finish();
    }
}
