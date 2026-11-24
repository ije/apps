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
            (Some("card".into()), false, "(min-width: 400px)".into()),
            (None, true, "(aspect-ratio > 1)".into()),
            (None, false, "style(--x: 1)".into()),
            (Some("card".into()), false, "not (width > 100px)".into()),
            (
                None,
                false,
                "(300px < inline-size <= 600px) or (width > 2000px)".into()
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
fn container_queries_use_the_root_font_the_viewport_and_the_container_font() {
    let mut doc = Document::parse(
        "<html style='font-size:20px'><style>@container (width > 21rem){.p{color:red}} @container (width > 50vw){.p{width:1px}} @container (width > 20em){.p{height:2px}}</style><div id=a style='container-type:inline-size;font-size:10px'><div style='font-size:16px'><p class=p></p></div></div></html>",
    );
    let a = element(&doc, "#a");
    let p = doc.nodes[doc.nodes[a].children[0]].children[0];
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |doc: &Document, cache: &mut StyleCache| {
        let computed = compute_styles(doc, 1000., 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&doc, &mut cache);
    cache.set_containers(&rustc_hash::FxHashMap::from_iter([(a, [410., 50.])]), &doc);
    let style = compute(&doc, &mut cache).values[p].clone();
    assert_ne!(style.get(Prop::COLOR), Some("red"));
    assert_ne!(style.get(Prop::WIDTH), Some("1px"));
    assert_eq!(style.get(Prop::HEIGHT), Some("2px"));
    doc.nodes[a].attrs.insert(
        "style".into(),
        "container-type:inline-size;font-size:30px".into(),
    );
    assert_ne!(
        compute(&doc, &mut cache).values[p].get(Prop::HEIGHT),
        Some("2px")
    );
    assert!(cache.set_containers(&rustc_hash::FxHashMap::from_iter([(a, [610., 50.])]), &doc));
    let style = compute(&doc, &mut cache).values[p].clone();
    assert_eq!(style.get(Prop::HEIGHT), Some("2px"));
    assert_eq!(style.get(Prop::WIDTH), Some("1px"));
    assert_eq!(style.get(Prop::COLOR), Some("red"));
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
fn nested_containers_with_many_queries_restyle_once_per_resize() {
    let rules: String = (1..=40)
        .map(|step| {
            format!(
                "@container (width > {}px){{.x{{z-index:{step}}}}}",
                step * 10
            )
        })
        .collect();
    let doc = Document::parse(&format!(
        "<style>{rules}.u{{font-size:10cqi}}</style><div id=a style='container-type:inline-size'><p class=x></p><p class=u></p><div id=b style='container-type:inline-size'><p class=x></p><p class=u></p></div></div>"
    ));
    let [a, b] = ["#a", "#b"].map(|selector| element(&doc, selector));
    let [p, u, q, v] = [
        doc.nodes[a].children[0],
        doc.nodes[a].children[1],
        doc.nodes[b].children[0],
        doc.nodes[b].children[1],
    ];
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let mut compute = |cache: &mut StyleCache| {
        let computed = compute_styles(&doc, 800., 600., None, &mut sheets, cache, None);
        cache.finish();
        computed
    };
    compute(&mut cache);
    for [outer, inner] in [[300., 150.], [405., 205.], [50., 25.]] {
        assert!(cache.set_containers(
            &rustc_hash::FxHashMap::from_iter([(a, [outer, 50.]), (b, [inner, 50.])]),
            &doc
        ));
        let computed = compute(&mut cache);
        let z = |id: usize| computed.values[id].get(Prop::Z_INDEX).map(str::to_owned);
        let level = |width: f32| ((width / 10.).ceil() as i32 - 1).clamp(0, 40);
        let expected = |width| (level(width) > 0).then(|| level(width).to_string());
        assert_eq!([z(p), z(q)], [expected(outer), expected(inner)]);
        assert_eq!(
            &computed.values[u]["font-size"],
            &format!("{}px", outer / 10.)
        );
        assert_eq!(
            &computed.values[v]["font-size"],
            &format!("{}px", inner / 10.)
        );
    }
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
