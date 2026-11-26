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

#[test]
fn html_attributes_give_presentational_hints() {
    let doc = Document::parse(
        "<ol type=a><li id=a><li id=b type=I></ol><ul type=SQUARE><li id=c></ul><table id=t align=right rules=all border=3 bordercolor=abc><tr valign=top><td id=d nowrap width=40 bgcolor=ff0000><td id=e colspan=3 rowspan=0></table><h2 id=h align=center></h2><div id=f align=right><table id=g></table></div><hr id=i size=8 color=red width=50%>",
    );
    let computed = styles(&doc, 800., 600.);
    let get = |id: &str, name: &str| {
        (computed[element(&doc, id)].get(name)).map_or("", |value| value.as_str())
    };
    for (id, kind) in [
        ("#a", "lower-alpha"),
        ("#b", "upper-roman"),
        ("#c", "square"),
    ] {
        assert_eq!(get(id, "list-style-type"), kind);
    }
    assert_eq!(get("#t", "float"), "right");
    assert_eq!(get("#t", "border-collapse"), "collapse");
    assert_eq!(get("#d", "white-space"), "nowrap");
    assert_eq!(get("#d", "width"), "40px");
    assert_eq!(get("#d", "vertical-align"), "top");
    assert_eq!(get("#d", "background-color"), "#ff0000");
    assert_eq!(get("#e", "grid-column-start"), "span 3");
    assert_eq!(get("#e", "grid-row-start"), "span 4095");
    assert_eq!(get("#h", "text-align"), "center");
    assert_eq!(get("#f", "text-align"), "-webkit-right");
    // Tables don't inherit the alignments that also place blocks.
    assert_eq!(get("#g", "text-align"), "start");
    assert_eq!(get("#i", "height"), "6px");
    assert_eq!(get("#i", "border-top-style"), "solid");
}

#[test]
fn collapsed_borders_follow_row_spans_hidden_borders_and_columns() {
    let doc = Document::parse(
        "<style>table{border-collapse:collapse}</style><table><colgroup><col><col style='border:4px solid blue'></colgroup><tr><td id=a rowspan=2 style='border:2px solid red'><td id=b style='border:1px solid'><tr><td id=c style='border:1px dashed green'></table><table><tr><td id=d style='border:2px hidden'><td id=e style='border:5px solid'></table>",
    );
    let computed = styles(&doc, 800., 600.);
    let get = |id: &str, name: &str| computed[element(&doc, id)][name].as_str();
    // The second row starts in the second column.
    assert_eq!(get("#c", "border-left-width"), "4px");
    assert_eq!(get("#a", "border-right-width"), "4px");
    assert_eq!(get("#a", "border-right-color"), "blue");
    assert_eq!(get("#b", "border-bottom-style"), "solid");
    assert_eq!(get("#c", "border-top-style"), "solid");
    assert_eq!(get("#a", "border-bottom-width"), "2px");
    assert_eq!(get("#e", "border-left-style"), "none");
    assert_eq!(get("#e", "border-right-width"), "5px");
}

#[test]
fn logical_properties_turn_with_the_lines_of_a_box() {
    let doc = Document::parse(
        "<style>div{padding-inline:1px 2px;padding-block:3px 4px;inline-size:50px}</style><div id=a></div><div id=b style='writing-mode:vertical-rl'></div><div id=c style='writing-mode:sideways-lr'></div><div id=d dir=rtl></div>",
    );
    let computed = styles(&doc, 800., 600.);
    let padding = |id: &str| {
        ["top", "right", "bottom", "left"]
            .map(|side| computed[element(&doc, id)][&format!("padding-{side}")].clone())
            .join(" ")
    };
    assert_eq!(padding("#a"), "3px 2px 4px 1px");
    assert_eq!(padding("#b"), "1px 3px 2px 4px");
    assert_eq!(padding("#c"), "2px 4px 1px 3px");
    assert_eq!(padding("#d"), "3px 1px 4px 2px");
    assert_eq!(computed[element(&doc, "#a")]["width"], "50px");
    assert_eq!(computed[element(&doc, "#b")]["height"], "50px");
}

#[test]
fn hints_stay_under_the_layers_of_the_author() {
    let doc = Document::parse(
        "<table id=a bordercolor=red></table><table id=b border=-3><tr><td id=c bgcolor=red style='all:revert-layer'></table><table id=d border=5 style='border-top-width:revert'></table><img id=e border>",
    );
    let computed = styles(&doc, 800., 600.);
    let get = |id: &str, name: &str| {
        (computed[element(&doc, id)].get(name)).map_or("", |value| value.as_str())
    };
    assert_eq!(get("#a", "border-top-style"), "");
    assert_eq!(get("#b", "border-top-width"), "1px");
    assert_eq!(get("#c", "background-color"), "red");
    assert_eq!(get("#c", "border-top-style"), "inset");
    assert_ne!(get("#d", "border-top-width"), "5px");
    assert_eq!(get("#d", "border-left-width"), "5px");
    assert_eq!(get("#e", "border-top-width"), "0px");
}

#[test]
fn spans_past_the_table_make_no_columns() {
    let mut rows = String::new();
    for _ in 0..300 {
        rows.push_str("<tr><td colspan=1000 rowspan=300 style='border:1px solid'>");
    }
    let doc = Document::parse(&format!(
        "<table style='border-collapse:collapse'><col style='border-right:4px solid'><tr><td id=a style='border:1px solid'><td id=b style='border:1px solid'><tr><td id=c colspan=100 style='border:1px solid'></table><table style='border-collapse:collapse'>{rows}</table><table dir=rtl style='border-collapse:collapse'><tr><td id=d style='border-left:6px solid'><td id=e></table>"
    ));
    let start = std::time::Instant::now();
    let computed = styles(&doc, 800., 600.);
    assert!(start.elapsed().as_secs() < 5);
    let get = |id: &str, name: &str| computed[element(&doc, id)][name].as_str();
    assert_eq!(get("#a", "border-right-width"), "4px");
    assert_eq!(get("#b", "border-left-width"), "4px");
    assert_eq!(get("#c", "border-right-width"), "1px");
    assert_eq!(get("#c", "border-top-width"), "1px");
    // The second cell of a right-to-left row is at the left of the first.
    assert_eq!(get("#e", "border-right-width"), "6px");
}

#[test]
fn cells_take_the_first_free_columns() {
    let mut slots = crate::style::Slots::default();
    assert_eq!(slots.place(0, 1, 2), 0);
    assert_eq!(slots.place(0, 1, 1), 1);
    assert_eq!(slots.place(0, 1, 3), 2);
    assert_eq!(slots.place(1, 3, 1), 1);
    assert_eq!(slots.place(2, 1, 1), 0);
    assert_eq!(slots.place(2, 1, 1), 1);
    assert_eq!(slots.place(2, 1, 1), 3);
    let real = slots.merge(0);
    assert_eq!([0, 2, 3, 4, 1000].map(real), [0, 2, 3, 4, 4]);
}
