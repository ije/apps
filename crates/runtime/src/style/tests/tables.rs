use super::*;

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
fn tables_have_border_box_sizing_like_chrome() {
    let doc = Document::parse("<table id=t><tr><td>a</td></tr></table>");
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
    assert_eq!(
        values[element(&doc, "#t")].get(Prop::BOX_SIZING),
        Some("border-box")
    );
}
