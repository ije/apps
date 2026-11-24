use super::*;
use crate::style::cascade::rank::DELETED;

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
fn registered_fallbacks_and_contents_items_follow_their_ancestors() {
    let mut doc = Document::parse(
        "<style>@property --q{syntax:'<color>';inherits:true;initial-value:red}#t{color:var(--q)}</style><div id=p><span id=t style='--q:5px'></span></div><div id=g><div style='display:contents'><span id=s></span></div></div>",
    );
    let [p, t, g, s] = ["#p", "#t", "#g", "#s"].map(|selector| element(&doc, selector));
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut last: Option<Rc<ComputedStyles>> = None;
    for (step, (ink, display)) in [("blue", "grid"), ("green", "grid"), ("green", "block")]
        .into_iter()
        .enumerate()
    {
        doc.nodes[p].inline = Some(Declarations::parse(&format!("--q:{ink}")));
        doc.nodes[g].inline = Some(Declarations::parse(&format!("display:{display}")));
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
        assert_eq!(cached.values[t].get(Prop::COLOR), Some(ink), "step {step}");
        let item = if display == "grid" { "block" } else { "inline" };
        assert_eq!(
            cached.values[s].get(Prop::DISPLAY).unwrap_or("inline"),
            item,
            "step {step}"
        );
        last = Some(Rc::new(cached));
    }
}

#[test]
fn passes_drop_unused_scopes_and_report_interaction_states() {
    let mut doc = Document::parse("<style></style><div class=a></div>");
    let sheet = element(&doc, "style");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for (source, states) in [
        (".a:hover{color:red}", crate::css::HOVER),
        (
            ".a{color:blue} .b:active{&>i{color:red}}",
            crate::css::ACTIVE,
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
        assert_eq!(cache.states & !crate::css::FOCUS, states, "{source}");
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
    // Only debug builds count the incremental passes.
    #[cfg(debug_assertions)]
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
    #[cfg(debug_assertions)]
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
            doc.nodes[hovered].state = crate::css::HOVER;
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

#[test]
fn text_nodes_that_share_a_cached_style_keep_their_selection_colors() {
    let doc = Document::parse(
        "<style>::selection{background:#fc0}</style><p>one</p><p>two</p><p style='user-select:none'>three</p>",
    );
    let (mut sheets, mut cache) = (HashMap::new(), StyleCache::default());
    compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    let computed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    for (id, node) in doc.nodes.iter().enumerate() {
        if node.tag == "#text"
            && node
                .parent
                .is_some_and(|parent| doc.nodes[parent].tag == "p")
        {
            assert_eq!(
                computed
                    .selections
                    .get(&id)
                    .map(|style| style.get(Prop::BACKGROUND_COLOR)),
                Some(Some("#fc0")),
                "{}",
                node.text
            );
        }
    }
}

#[test]
fn rewritten_style_text_drops_old_rules_from_the_scope() {
    let rules = |step: usize| {
        (0..200)
            .map(|index| format!(".c{index}{{width:{}px}}", index + step))
            .collect::<String>()
    };
    let mut doc = Document::parse(&format!(
        "<html><head><style id=s>{}</style></head><body><p class=c5 id=p>a</p></body></html>",
        rules(0)
    ));
    let style = element(&doc, "#s");
    let text = doc.nodes[style].children[0];
    let (mut sheets, mut cache) = (HashMap::new(), StyleCache::default());
    compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    cache.finish();
    let live = cache.scopes[0].rules.len();
    let mut ids = vec![cache.scopes[0].id];
    for step in 1..40 {
        doc.nodes[text].text = rules(step);
        let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        cache.finish();
        let fresh = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values, "{step}");
        let width = format!("{}px", 5 + step);
        assert_eq!(
            cached.values[element(&doc, "#p")].get(Prop::WIDTH),
            Some(width.as_str())
        );
        let scope = cache.scopes.last().unwrap();
        assert!(scope.rules.len() <= live * 2 + 200, "{step}");
        let deleted =
            |entry: &usize| *entry < PSEUDO && scope.rules[entry >> SELECTOR].2.origin == DELETED;
        assert!(
            !scope
                .buckets
                .iter()
                .flat_map(|bucket| bucket.values().flatten())
                .any(deleted)
        );
        ids.push(scope.id);
    }
    ids.dedup();
    // Most rewrites update the scope in place, and some build it again.
    assert!(ids.len() > 1 && ids.len() < 30, "{ids:?}");
}

#[test]
fn siblings_share_styles_only_after_like_siblings() {
    let mut doc = Document::parse(
        "<style>span{color:blue} .a + span{color:red} section > :first-of-type{width:3px}</style>\
         <div><p class=a></p><span></span><b></b><span></span><p class=a></p><span></span></div>\
         <section><i></i><i></i></section>",
    );
    let b = element(&doc, "b");
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    for marked in [false, true, false] {
        doc.nodes[b]
            .attrs
            .set("class", if marked { "a" } else { "" });
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
        for (index, red) in [(1, true), (2, marked), (3, true)] {
            let id = format!("span:nth-of-type({index})");
            assert_eq!(
                cached.values[element(&doc, &id)]
                    .get(Prop::COLOR)
                    .and_then(color),
                Some((if red { 0xff0000 } else { 0x0000ff }, 255)),
                "{id}"
            );
        }
        for (id, width) in [("i:first-of-type", Some("3px")), ("i:last-of-type", None)] {
            assert_eq!(
                cached.values[element(&doc, id)].get(Prop::WIDTH),
                width,
                "{id}"
            );
        }
    }
}
