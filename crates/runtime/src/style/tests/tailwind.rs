use super::*;

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
    let shadows = crate::css::box_shadows(&units, &style["box-shadow"], false);
    assert_eq!(shadows[0].offset.y, renderer::px(30.));
    assert_eq!(shadows[0].blur_radius, renderer::px(4.));
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
    let paint =
        crate::css::box_shadows(&Lengths::new(16., 800., 600.), &style["box-shadow"], false);
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
        crate::css::box_shadows(&Lengths::new(32., 799., 600.), &style["box-shadow"], false)
            .iter()
            .filter(|shadow| shadow.color.a > 0.)
            .count(),
        1
    );
    let wide = styles(&doc, 800., 600.).pop().unwrap();
    assert_eq!(length(&wide["width"], 800., 32., 800., 600.), 40.);
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
        doc.nodes[card].state = if hovered { crate::css::HOVER } else { 0 };
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
