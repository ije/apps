use super::*;

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
    let parts = &computed
        .extras(element(&doc, "div"))
        .scrollbars
        .as_ref()
        .unwrap();
    assert_eq!(&parts[0].as_ref().unwrap()["width"], "8px");
    assert_eq!(&parts[1].as_ref().unwrap()["background-color"], "#eee");
    let [thumb, hovered] = [&parts[2], &parts[4]].map(|part| part.as_ref().unwrap());
    assert_eq!(
        (&thumb["background-color"], &hovered["background-color"]),
        ("red", "blue")
    );
    assert_eq!(&hovered["border-top-left-radius"], "4px");
    assert!(computed.extras(element(&doc, "p")).scrollbars.is_none());
}

#[test]
fn shadows_split_inset_and_outer_lists() {
    let units = Lengths::new(16., 800., 600.);
    let value = "inset 0 0 0 2px red, 0 4px 8px blue, inset 1px 2px 6px green";
    let outer = crate::css::value::box_shadows(&units, value, false);
    let inset = crate::css::value::box_shadows(&units, value, true);
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
fn hosts_take_pseudo_elements_from_their_shadow_tree() {
    let mut doc = Document::parse(
        "<style>x-a::before{color:blue;background-color:blue!important}</style><x-a id=host class=on></x-a><div id=root><style>:host::before{content:'A';color:red;background-color:red!important}:host(.on)::after{content:'B';color:green}:host(:hover)::after{color:purple}@starting-style{:host{opacity:0}}</style></div>",
    );
    let host = element(&doc, "#host");
    let shadows = attach(&mut doc);
    let (mut cache, mut last) = (StyleCache::default(), None);
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    let pseudos = out.generated(host).cloned().unwrap();
    assert_eq!(pseudos[0].get(Prop::CONTENT), Some("'A'"));
    assert_eq!(pseudos[0].get(Prop::COLOR), Some("blue"));
    assert_eq!(pseudos[0].get(Prop::BACKGROUND_COLOR), Some("red"));
    assert_eq!(pseudos[1].get(Prop::CONTENT), Some("'B'"));
    assert_eq!(pseudos[1].get(Prop::COLOR), Some("green"));
    assert_eq!(
        out.extras(host)
            .starting
            .as_ref()
            .unwrap()
            .get(Prop::OPACITY),
        Some("0")
    );
    doc.nodes[host].attrs.set("class", "off");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    let pseudos = out.generated(host).cloned().unwrap();
    assert_eq!(pseudos[0].get(Prop::CONTENT), Some("'A'"));
    assert!(!pseudos[1].contains(Prop::CONTENT));
    doc.nodes[host].attrs.set("class", "on");
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(
        out.generated(host).unwrap()[1].get(Prop::CONTENT),
        Some("'B'")
    );
    doc.nodes[host].state = crate::css::selector::HOVER;
    let out = shadowed(&doc, &shadows, &mut cache, &mut last);
    assert_eq!(
        out.generated(host).unwrap()[1].get(Prop::COLOR),
        Some("purple")
    );
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
            out.generated(host)
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
        assert_eq!(out.generated(p).unwrap()[0].get(Prop::CONTENT), Some("'P'"));
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
        assert!(
            out.values == reference.values
                && (0..out.values.len()).all(|id| out.generated(id) == reference.generated(id))
        );
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
    assert_eq!(cache.incremental, 4);
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
            out.generated(id)
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
        assert!(
            out.values == reference.values
                && (0..out.values.len()).all(|id| out.generated(id) == reference.generated(id))
        );
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
        out.generated(id)
            .and_then(|pair| pair[slot].get(prop).map(str::to_owned))
    };
    let text = |out: &ComputedStyles, id: usize, slot: usize| {
        out.extras(id)
            .text
            .as_ref()
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
    assert_eq!(
        out.extras(p).selection.as_ref().unwrap().get(Prop::COLOR),
        Some("green")
    );
    assert_eq!(text(&out, i, 3).as_deref(), Some("green"));
    assert_ne!(out.values[p].get(Prop::COLOR), Some("red"));
    assert_eq!(
        out.extras(a).starting.as_ref().unwrap().get(Prop::OPACITY),
        Some("0")
    );
    assert_eq!(
        out.extras(p).starting.as_ref().unwrap().get(Prop::OPACITY),
        Some("0.5")
    );
    assert!(out.extras(b).starting.is_none());
    doc.nodes[a].state = crate::css::selector::HOVER;
    doc.nodes[p].state = crate::css::selector::HOVER;
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
            out.generated(p)
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
    doc.nodes[p].state = crate::css::selector::HOVER;
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
fn range_progress_and_file_parts_take_their_pseudo_elements_like_chrome() {
    let doc = Document::parse(
        "<style>input::-webkit-slider-runnable-track{height:8px}input:disabled::-webkit-slider-thumb{background:gray}progress::-webkit-progress-value{background:red}::file-selector-button{padding:6px}</style><input type=range disabled><progress></progress><input type=file><div></div>",
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
    let part = |tag: &str, file: bool, slot: usize| {
        let id = (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == tag)
            .nth(usize::from(file))
            .unwrap();
        computed.extras(id).scrollbars.as_ref().unwrap()[slot]
            .clone()
            .unwrap()
    };
    assert_eq!(&part("input", false, 1)["height"], "8px");
    assert_eq!(&part("input", false, 2)["background-color"], "gray");
    assert_eq!(&part("progress", false, 2)["background-color"], "red");
    assert_eq!(&part("input", true, 1)["padding-top"], "6px");
    assert!(computed.extras(element(&doc, "div")).scrollbars.is_none());
}

#[test]
fn table_parts_take_roles_and_stay_cached() {
    let mut doc = Document::parse(
        "<table id=t><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table><div id=d style='display:table'><div id=r style='display:table-row'><div id=c style='display:table-cell'></div></div></div><div id=o style='display:table-row'></div><div id=f style='display:flex'><div id=i style='display:table-cell'></div></div><table id=b style='display:block'></table>",
    );
    let mut sheets = HashMap::new();
    let mut cache = StyleCache::default();
    let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    let shown = |styles: &ComputedStyles, selector: &str| {
        let id = element(&doc, selector);
        let display = styles.values[id]
            .get(Prop::DISPLAY)
            .unwrap_or("inline")
            .to_owned();
        (TABLE_DISPLAYS[role(styles, id) as usize], display)
    };
    for (selector, reported, used) in [
        ("#t", "table", "grid"),
        ("thead", "table-header-group", "contents"),
        ("tbody", "table-row-group", "contents"),
        ("tr", "table-row", "contents"),
        ("td", "table-cell", "block"),
        ("#d", "table", "grid"),
        ("#r", "table-row", "contents"),
        ("#c", "table-cell", "block"),
        // A row outside a table makes a table of its own, and a flex item is a block.
        ("#o", "table-row", "grid"),
        ("#i", "", "block"),
        ("#b", "block", "grid"),
    ] {
        assert_eq!(
            shown(&first, selector),
            (reported, used.to_owned()),
            "{selector}"
        );
    }
    let table = element(&doc, "#t");
    let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
    assert!(Rc::ptr_eq(&first.values[table], &second.values[table]));
    let row = doc.nodes[doc.nodes[table].children[1]].children[0];
    let cell = doc.nodes.len();
    doc.nodes.push(crate::dom::Node {
        tag: "td".into(),
        parent: Some(row),
        ..Default::default()
    });
    doc.nodes[row].children.push(cell);
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
    assert_eq!(cached.flags[cell], fresh.flags[cell]);
    assert_eq!(TABLE_DISPLAYS[role(&cached, cell) as usize], "table-cell");
}
