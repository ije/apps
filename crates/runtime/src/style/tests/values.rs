use super::*;

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
fn a_stronger_transition_shorthand_resets_the_longhands() {
    let doc = Document::parse(
        r#"<style>
            div { transition-property: transform; transition-duration: 1s; }
            #c { transition: opacity 2s; transition-duration: 3s; }
        </style><div id="a" style="transition: none"></div><div id="b"></div><div id="c"></div>"#,
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
    let style = |selector| &computed.values[element(&doc, selector)];
    assert_eq!(style("#a").get(Prop::TRANSITION), Some("none"));
    assert_eq!(style("#a").get(Prop::TRANSITION_PROPERTY), None);
    assert_eq!(
        style("#b").get(Prop::TRANSITION_PROPERTY),
        Some("transform")
    );
    assert_eq!(style("#c").get(Prop::TRANSITION), Some("opacity 2s"));
    assert_eq!(style("#c").get(Prop::TRANSITION_DURATION), Some("3s"));
    assert_eq!(style("#c").get(Prop::TRANSITION_PROPERTY), None);
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
fn filters_parse_into_color_matrices() {
    let units = Lengths::new(16., 800., 600.);
    let parse = |value: &str| Filter::parse(value, &units);
    let (gray, opacity) = parse("grayscale(100%)");
    let gray = gray.unwrap();
    assert!(gray.0.iter().all(|row| row[..3] == gray.0[0][..3]));
    assert_eq!(opacity, 1.);
    assert_eq!(parse("opacity(50%)"), (None, 0.5));
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
        parse("contrast(0)").0.unwrap().color((0x123456, 255)),
        (0x808080, 255)
    );
    assert_eq!(
        parse("brightness(2)").0.unwrap().color((0x123456, 255)),
        (0x2468ac, 255)
    );
    assert_eq!(parse("hue-rotate(0deg)").0, None);
    assert_eq!(parse("saturate(1)").0, None);
    assert_eq!(parse("grayscale(-1)").0, None);
    assert_eq!(parse("unknown(2)"), (None, 1.));
    assert_eq!(parse("blur(2px)"), (None, 1.));
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
fn declarations_of_one_block_keep_their_order() {
    let doc = Document::parse(
        "<style>#l1{margin-inline-start:10px;margin-left:20px} #l2{margin-left:1px;margin-inline-start:5px} #r1{direction:rtl;padding-inline-end:7px;padding-left:3px} #a1{color:red;all:unset} #a2{all:unset;color:red} #t1{transition-duration:2s;transition:opacity 1s} #n1{animation-name:x;animation:none} #b1{border-inline-start:4px solid red;border-left:1px solid blue} #s1{inset-inline:5px;left:2px} #g1{grid-row-start:3;all:initial} #p1::before{content:'';margin-inline-start:9px;margin-left:8px}</style><body style='color:green'><div id=l1></div><div id=l2></div><div id=r1></div><div id=a1></div><div id=a2></div><div id=t1></div><div id=n1></div><div id=b1></div><div id=s1></div><div id=g1></div><div id=p1></div><div id=i1 style='margin-inline-start:10px;margin-left:20px;color:red;all:unset'></div><div id=i2 style='margin-left:20px !important;margin-inline-start:10px !important'></div></body>",
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
    let get = |selector: &str, prop: Prop| computed.values[element(&doc, selector)].get(prop);
    assert_eq!(get("#l1", Prop::MARGIN_LEFT), Some("20px"));
    assert_eq!(get("#l2", Prop::MARGIN_LEFT), Some("5px"));
    assert_eq!(get("#r1", Prop::PADDING_LEFT), Some("3px"));
    assert_ne!(get("#a1", Prop::COLOR), Some("red"));
    assert_eq!(get("#a2", Prop::COLOR), Some("red"));
    assert_ne!(get("#t1", Prop::TRANSITION_DURATION), Some("2s"));
    assert_ne!(get("#n1", Prop::ANIMATION_NAME), Some("x"));
    assert_eq!(get("#b1", Prop::BORDER_LEFT_WIDTH), Some("1px"));
    assert_eq!(get("#s1", Prop::LEFT), Some("2px"));
    assert_ne!(get("#g1", Prop::GRID_ROW_START), Some("3"));
    assert_ne!(get("#i1", Prop::MARGIN_LEFT), Some("20px"));
    assert_ne!(get("#i1", Prop::COLOR), Some("red"));
    assert_eq!(get("#i2", Prop::MARGIN_LEFT), Some("10px"));
    assert_eq!(
        computed.generated(element(&doc, "#p1")).unwrap()[0].get(Prop::MARGIN_LEFT),
        Some("8px")
    );
    assert_eq!(std::mem::size_of::<CascadeRank>(), 48);
}

#[test]
fn registered_values_compute_strings_keep_var_and_rules_follow_layers() {
    let doc = Document::parse(
        r#"<style>@property --p{syntax:'<percentage>';inherits:false;initial-value:1%}@property --t{syntax:'<time>';inherits:false;initial-value:1s}@property --i{syntax:'<integer>';inherits:false;initial-value:1}@property --l{syntax:'<length>+';inherits:true;initial-value:1px}@property --c{syntax:'<color>';inherits:true;initial-value:red}@property --n{syntax:'<number>';inherits:false;initial-value:0}@property --e{syntax:'<length>';inherits:false;initial-value:1em}@property --u{syntax:'*';inherits:false;initial-value:a  b}
        @property --z{syntax:'<length>';inherits:false;initial-value:1px}@layer x{@property --z{syntax:'<length>';inherits:false;initial-value:2px}@keyframes k{to{width:2px}}}@keyframes k{to{width:1px}}@layer y{@keyframes k{to{width:3px}}}
        #a{--p:calc(10% * 2);--t:calc(1s + 500ms);--i:calc(3 / 2);--l:1in  calc(2px + 1px);--c:rgb(0 0 255 / 50%);--n:1e2;--u:var(--n)  x;--k:x;content:"var(--k)";quotes:var(--none, ")");width:var(--z)}
        #b{--p:10px;--t:1;--i:2.5;--l:red;--c:currentcolor;--n:3px}
        #f{font-kerning:none;font-variant-numeric:tabular-nums;font-optical-sizing:none;font:12px serif}</style><div id=a><p id=c></p></div><div id=b></div><div id=f></div>"#,
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    let all = |selector| {
        ["--p", "--t", "--i", "--l", "--c", "--n"]
            .map(|name| get(selector, name).unwrap_or_default())
    };
    assert_eq!(
        all("#a"),
        [
            "20%",
            "1.5s",
            "2",
            "96px 3px",
            "rgba(0, 0, 255, 0.5)",
            "100"
        ]
    );
    assert_eq!(all("#b"), ["1%", "1s", "1", "1px", "currentcolor", "0"]);
    assert_eq!(get("#c", "--l").as_deref(), Some("96px 3px"));
    assert_eq!(get("#c", "--c").as_deref(), Some("rgba(0, 0, 255, 0.5)"));
    assert_eq!(get("#a", "--e"), None);
    assert_eq!(get("#a", "--u").as_deref(), Some("100  x"));
    assert_eq!(get("#a", "width").as_deref(), Some("1px"));
    assert_eq!(get("#a", "content").as_deref(), Some(r#""var(--k)""#));
    assert_eq!(get("#a", "quotes").as_deref(), Some(r#"")""#));
    for (name, value) in [
        ("font-kerning", "auto"),
        ("font-variant-numeric", "normal"),
        ("font-optical-sizing", "auto"),
        ("font-size", "12px"),
    ] {
        assert_eq!(get("#f", name).as_deref(), Some(value), "{name}");
    }
    let mut cache = StyleCache::default();
    compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut cache,
        None,
    );
    let frames = &cache.scopes.last().unwrap().keyframes["k"];
    assert_eq!(frames[0].1.resolve()["width"], *"1px");
    let style = declarations("transform-origin:top");
    for (value, expected) in [
        ("top", "100px 0px"),
        ("bottom left", "0px 100px"),
        ("right", "200px 50px"),
        ("left 10px", "0px 10px"),
    ] {
        let mut style = style.clone();
        style.insert(Prop::TRANSFORM_ORIGIN, value);
        assert_eq!(
            resolved(
                &style,
                &Lengths::new(16., 800., 600.),
                "transform-origin",
                Some([200., 100.]),
                &|| [200., 100.],
                &Registry::default()
            ),
            expected
        );
    }
}

#[test]
fn animations_leave_important_declarations() {
    let doc = Document::parse(
        "<style>@keyframes w{from{width:0px;opacity:0}to{width:100px;opacity:1}}div{animation:w 1s linear;width:7px !important}</style><div></div>",
    );
    let id = element(&doc, "div");
    let styles = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    assert_eq!(
        styles.extras(id).important.as_ref().unwrap()[..],
        [Prop::WIDTH]
    );
    let start = std::time::Instant::now();
    let mut motion = animation::Motion::default();
    let visible = vec![true; doc.nodes.len()];
    let sample = |motion: &mut animation::Motion, seconds: f64| {
        let animated = motion
            .update(
                &doc,
                &styles,
                &visible,
                &|_| true,
                &|_| f32::INFINITY,
                start + std::time::Duration::from_secs_f64(seconds),
                false,
                &mut Vec::new(),
            )
            .animated;
        let style = animated.into_iter().find(|entry| entry.0 == id).unwrap().1;
        [Prop::WIDTH, Prop::OPACITY].map(|prop| style.get(prop).map(str::to_owned))
    };
    sample(&mut motion, 0.);
    assert_eq!(
        sample(&mut motion, 0.5),
        [Some("7px".into()), Some("0.5".into())]
    );
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
    assert_eq!(get("#c11", "--q").as_deref(), Some("rgb(0, 128, 0)"));
    assert_eq!(get("#c12", "--q").as_deref(), Some("rgb(0, 0, 255)"));
    assert_eq!(get("#c13", "--bad").as_deref(), Some("1px"));
    assert_eq!(get("#sh", "box-shadow").as_deref(), Some("0 1px 2px red"));
    assert_eq!(get("#ch", "box-shadow").as_deref(), Some("0 0 #0000"));
    assert!(
        !Stylesheet::parse("@property --x{syntax:'<length>';inherits:true;initial-value:1em}")
            .source()
            .contains("@property --x")
    );
}

#[test]
fn scroll_spacing_and_carets_parse_like_chrome() {
    for (name, value, valid) in [
        ("scroll-margin-top", "-5px", true),
        ("scroll-margin-top", "5%", false),
        ("scroll-margin-block", "5px 6px", true),
        ("scroll-margin-inline-start", "auto", false),
        ("scroll-padding-top", "10%", true),
        ("scroll-padding-top", "-2px", false),
        ("scroll-padding-inline", "auto 5px", true),
        ("caret-shape", "underscore", true),
        ("caret-shape", "box", false),
        ("caret-animation", "manual", true),
        ("caret-animation", "none", false),
    ] {
        assert_eq!(supports(name, value), Some(valid), "{name}: {value}");
    }
    let doc = Document::parse(
        "<style>div{scroll-margin-inline-start:5px;scroll-margin-block:3px 4px;scroll-padding-inline:1px 2px;caret-shape:block}</style><div id=l></div><div id=r dir=rtl><p id=p></p></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    for (id, name, value) in [
        ("#l", "scroll-margin-left", "5px"),
        ("#l", "scroll-margin-top", "3px"),
        ("#l", "scroll-margin-bottom", "4px"),
        ("#l", "scroll-padding-left", "1px"),
        ("#r", "scroll-margin-right", "5px"),
        ("#r", "scroll-padding-right", "1px"),
        ("#r", "scroll-padding-left", "2px"),
        ("#p", "caret-shape", "block"),
    ] {
        assert_eq!(get(id, name).as_deref(), Some(value), "{id} {name}");
    }
    assert_eq!(get("#p", "scroll-margin-top"), None);
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
            &|| [200., 100.],
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
fn shorthands_keep_vendor_keywords_out_of_lengths() {
    let doc = Document::parse(
        "<style>#a{outline:5px auto -webkit-focus-ring-color}#b{outline:-1px solid red;border:+2px solid red}</style><div id=a></div><div id=b></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "outline-width").as_deref(), Some("5px"));
    assert_eq!(get("#a", "outline-style").as_deref(), Some("auto"));
    assert_eq!(get("#b", "outline-width"), None);
    assert_eq!(get("#b", "border-top-width").as_deref(), Some("+2px"));
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
            &|| [800., 600.],
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
        ("outline", "rgb(0, 128, 0) dashed 1px"),
        ("transition", "opacity 1s ease-in 2s, color 0.5s"),
        ("animation", "2s linear infinite spin"),
        ("list-style", "inside none square"),
        ("columns", "100px 3"),
        ("flex-flow", "row nowrap"),
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
fn overscroll_behavior_sets_both_axes() {
    let axes = |source: &str| {
        let style = declarations(source);
        [Prop::OVERSCROLL_BEHAVIOR_X, Prop::OVERSCROLL_BEHAVIOR_Y]
            .map(|prop| style.get(prop).map(str::to_owned))
    };
    let pair = |x: &str, y: &str| [Some(x.to_owned()), Some(y.to_owned())];
    assert_eq!(axes("overscroll-behavior: none"), pair("none", "none"));
    assert_eq!(
        axes("overscroll-behavior: contain none"),
        pair("contain", "none")
    );
    assert_eq!(
        axes("overscroll-behavior-inline: contain; overscroll-behavior-block: none"),
        pair("contain", "none")
    );
    assert_eq!(supports("overscroll-behavior", "auto contain"), Some(true));
    assert_eq!(supports("overscroll-behavior-y", "none"), Some(true));
    assert_eq!(supports("overscroll-behavior-block", "contain"), Some(true));
    assert_eq!(supports("overscroll-behavior", "scroll"), Some(false));
    assert_eq!(
        declare("overscroll-behavior-block: none")[0][0],
        "overscroll-behavior-y"
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
fn invalid_declarations_are_dropped_at_parse_time() {
    let doc = Document::parse(
        "<style>#a{color:red;color:notacolor;width:10px;width:-5px;opacity:.5;opacity:abc;z-index:3;z-index:1.5;padding:2px;padding:red;margin:1px;margin:1px 2px 3px 4px 5px;border:1px solid green;border:1px solid red blue;flex:2;flex:1 2 3px 4}
        #b{width:fit-content;width:-moz-fit-content;position:sticky;position:-webkit-sticky;display:flex;display:-moz-box;padding-left:5px;padding-left:-3px}
        #b{color:green !important} #b{color:bogus !important}
        #c{display:-webkit-box;outline:5px auto -webkit-focus-ring-color;color:var(--nope, red);--raw:not a color;unknown-thing:1;-webkit-user-select:none}
        #d{transition-duration:calc(1s * 2);background-color:oklch(from currentcolor l c h / .5);display:table-cell}
        #d{transition-timing-function:cubic-bezier(1,0,0,1);animation-timing-function:ease;animation-timing-function:cubic-bezier(2,0,0,1)}</style>
        <div id=a style='color:blue;color:notacolor;margin:1px 2px 3px 4px 5px'></div><div id=b></div><div id=c></div><div id=d></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    for (selector, name, value) in [
        ("#a", "color", Some("blue")),
        ("#a", "width", Some("10px")),
        ("#a", "opacity", Some(".5")),
        ("#a", "z-index", Some("3")),
        ("#a", "padding-top", Some("2px")),
        ("#a", "margin-left", Some("1px")),
        ("#a", "border-top-color", Some("green")),
        ("#a", "flex-grow", Some("2")),
        ("#b", "width", Some("fit-content")),
        ("#b", "position", Some("sticky")),
        ("#b", "display", Some("flex")),
        ("#b", "padding-left", Some("5px")),
        ("#b", "color", Some("green")),
        ("#c", "display", Some("-webkit-box")),
        ("#c", "outline-width", Some("5px")),
        ("#c", "color", Some("red")),
        ("#c", "--raw", Some("not a color")),
        ("#c", "unknown-thing", Some("1")),
        ("#c", "user-select", Some("none")),
        ("#d", "transition-duration", Some("calc(1s * 2)")),
        // A cell lays out as a block, and `getComputedStyle()` reports `table-cell` from its role.
        ("#d", "display", Some("block")),
        (
            "#d",
            "transition-timing-function",
            Some("cubic-bezier(1,0,0,1)"),
        ),
        ("#d", "animation-timing-function", Some("ease")),
    ] {
        assert_eq!(get(selector, name).as_deref(), value, "{selector} {name}");
    }
    assert!(get("#d", "background-color").is_some());
    for (name, value, expected) in [
        ("margin", "1px 2px 3px 4px 5px", Some(false)),
        ("margin", "1px 2px 3px 4px", Some(true)),
        ("border", "1px solid red blue", Some(false)),
        ("flex", "1 2 3px 4", Some(false)),
        ("flex", "1 1 0%", Some(true)),
        ("width", "-5px", Some(false)),
        ("margin-left", "-5px", Some(true)),
        ("display", "table", Some(true)),
        ("display", "-moz-box", Some(false)),
        ("animation-duration", "calc(1s + 1s)", Some(true)),
        ("corner-shape", "squircle", Some(true)),
        (
            "corner-shape",
            "bevel scoop notch superellipse(-0.5)",
            Some(true),
        ),
        ("corner-shape", "round round round round round", Some(false)),
        ("corner-shape", "superellipse(2px)", Some(false)),
        (
            "corner-top-left-shape",
            "superellipse(-infinity)",
            Some(true),
        ),
        ("corner-top-left-shape", "bevel round", Some(false)),
        ("corner-top-shape", "bevel round", Some(true)),
    ] {
        assert_eq!(supports(name, value), expected, "{name}: {value}");
    }
}

#[test]
fn integer_and_number_math_folds_to_plain_numbers() {
    let doc = Document::parse(
        "<html><style>@import 'tailwindcss'; #v{--n:3; z-index:calc(var(--n) * -1); flex-shrink:calc(var(--n) / 2)}</style><body>
        <div id=a class='-z-10 -order-1 relative -col-start-1 -row-end-2'></div>
        <div id=b class='z-[calc(1+1)] opacity-[calc(0.25*2)] grow-[calc(1+1)]'></div>
        <div id=v></div></body></html>",
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
        ("#a", "z-index", "-10"),
        ("#a", "order", "-1"),
        ("#a", "grid-row-end", "-2"),
        ("#a", "grid-column-start", "-1"),
        ("#b", "z-index", "2"),
        ("#b", "opacity", "0.5"),
        ("#b", "flex-grow", "2"),
        ("#v", "z-index", "-3"),
        ("#v", "flex-shrink", "1.5"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
}

#[test]
fn grid_column_and_mask_shorthands_expand_to_longhands() {
    let doc = Document::parse(
        "<html><style>@import 'tailwindcss';
        #a{display:grid;grid-template-columns:1fr 1fr} #a{grid-template:'x y' 10px / 100px 1fr}
        #b{grid-row-start:2} #b{grid-row:5 / 6} #c{grid-area:main}
        #d{column-count:2;column-rule-style:solid} #d{columns:3;column-rule:2px dotted red}
        #e{mask:url(m.png) no-repeat center / 10px} #f{grid:auto-flow dense 20px / 1fr 1fr}
        #g{grid-row:span 2} #h{grid-column:1 / 3;grid-column-end:4}</style><body>
        <div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div><div id=g></div><div id=h></div>
        <div class='grid grid-cols-3'><i id=t class='col-start-2 md:col-span-full'></i></div></body></html>",
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
        ("#a", "grid-template-columns", "100px 1fr"),
        ("#a", "grid-template-rows", "10px"),
        ("#a", "grid-template-areas", "\"x y\""),
        ("#b", "grid-row-start", "5"),
        ("#b", "grid-row-end", "6"),
        ("#b", "grid-row", "5 / 6"),
        ("#c", "grid-column-end", "main"),
        // Like Chrome, lines that the first line implies are left out.
        ("#c", "grid-area", "main"),
        ("#d", "column-count", "3"),
        ("#d", "column-width", "auto"),
        ("#d", "columns", "3"),
        ("#d", "column-rule-style", "dotted"),
        ("#d", "column-rule-width", "2px"),
        ("#e", "mask-image", "url(m.png)"),
        ("#e", "mask-repeat", "no-repeat"),
        ("#e", "mask-size", "10px auto"),
        ("#f", "grid-auto-flow", "dense"),
        ("#f", "grid-auto-rows", "20px"),
        ("#f", "grid-template-columns", "1fr 1fr"),
        ("#g", "grid-row-start", "span 2"),
        ("#g", "grid-row-end", "auto"),
        ("#h", "grid-column-start", "1"),
        ("#h", "grid-column-end", "4"),
        ("#t", "grid-column-start", "1"),
        ("#t", "grid-column-end", "-1"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
    assert_eq!(
        computed.values[element(&doc, "#e")].get(Prop::MASK),
        Some("url(m.png) no-repeat center / 10px")
    );
}

#[test]
fn invalid_background_shorthands_keep_the_earlier_value_like_chrome() {
    let doc = Document::parse(
        "<style>#a{background:red;background:lolwut(1 2 3)}#b{background:red;background:linear-gradient(red,blue) space / 30px 20px}\
         #c{background:red;background:bogus}#d{background:url(a.png) center / cover, color-mix(in srgb, currentColor 30%, white)}\
         #e{background:red;background:url(a.png) red, blue}</style>\
         <div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    for selector in ["#a", "#b", "#c", "#e"] {
        assert_eq!(
            get(selector, "background-color").as_deref(),
            Some("red"),
            "{selector}"
        );
        assert_eq!(
            get(selector, "background-image").as_deref(),
            Some("none"),
            "{selector}"
        );
    }
    assert_eq!(get("#d", "background-size").as_deref(), Some("cover, auto"));
    assert_eq!(
        get("#d", "background-color").as_deref(),
        Some("color(srgb 0.7 0.7 0.7)")
    );
}

#[test]
fn svg_width_and_height_attributes_are_css_lengths_like_chrome() {
    let doc = Document::parse(
        r#"<style>#c { width: 30px }</style><svg id="a" width="1em" height="50%"></svg><svg id="b" width="20" height="bad"></svg><svg id="c" width="10"></svg>"#,
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
    let size = |selector| {
        [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
            computed.values[element(&doc, selector)]
                .get(prop)
                .map(str::to_owned)
        })
    };
    assert_eq!(size("#a"), [Some("1em".into()), Some("50%".into())]);
    assert_eq!(size("#b"), [Some("20px".into()), None]);
    assert_eq!(size("#c")[0].as_deref(), Some("30px"));
}

#[test]
fn computed_defaults_and_logical_names_read_like_chrome() {
    let doc = Document::parse(
        "<style>#p{color:rgb(1,2,3)} #c{color:initial;text-decoration:underline dotted;outline:2px dashed red;border-block-end:3px solid;font-stretch:condensed;caret-color:auto}</style><div id=p><p id=c></p></div>",
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
        ("#p", "transition", "all"),
        ("#p", "animation", "none"),
        ("#p", "text-decoration", "none"),
        ("#p", "outline", "rgb(1, 2, 3) none 3px"),
        ("#p", "caret-color", "rgb(1, 2, 3)"),
        ("#p", "font-stretch", "100%"),
        ("#p", "border-spacing", "0px"),
        ("#p", "caption-side", "top"),
        ("#p", "margin-block", "0px"),
        ("#c", "color", "rgb(0, 0, 0)"),
        ("#c", "text-decoration", "underline dotted"),
        ("#c", "outline", "rgb(255, 0, 0) dashed 2px"),
        ("#c", "border-block-end-width", "3px"),
        ("#c", "font-stretch", "75%"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
}

#[test]
fn border_shorthands_set_and_reset_border_images_like_chrome() {
    let doc = Document::parse(
        "<style>#a{border-image:linear-gradient(red,blue) 1;border:1px solid}#b{border:1px solid;border-image:url(a.png) 30 / 2px round}\
         #c{border-image:url(a.png) 30;border-image:red 1}#d{border-image:linear-gradient(red,blue) 1;border-image-slice:20 fill}</style>\
         <div id=a></div><div id=b></div><div id=c></div><div id=d></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#a", "border-image-source"), None);
    assert_eq!(
        get("#b", "border-image-source").as_deref(),
        Some("url(a.png)")
    );
    assert_eq!(get("#b", "border-image-width").as_deref(), Some("2px"));
    assert_eq!(get("#b", "border-image-repeat").as_deref(), Some("round"));
    assert_eq!(get("#c", "border-image-slice").as_deref(), Some("30"));
    assert_eq!(get("#d", "border-image-slice").as_deref(), Some("20 fill"));
}

#[test]
fn computed_times_resolve_math_like_chrome() {
    let doc = Document::parse(
        "<style>i{--i:2;animation:spin calc(420ms + var(--i) * 37ms) infinite;animation-delay:calc(var(--i) * -91ms);transition-duration:max(1s, 200ms)}</style><i id=a></i>",
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
    let id = element(&doc, "#a");
    let get = |name| {
        resolved(
            &computed.values[id],
            &computed.lengths[id],
            name,
            None,
            &|| [800., 600.],
            &Registry::default(),
        )
    };
    assert_eq!(
        [
            "animation-duration",
            "animation-delay",
            "transition-duration"
        ]
        .map(get),
        ["0.494s", "-0.182s", "1s"]
    );
}

#[test]
fn registered_properties_in_var_cycles_are_unset() {
    let doc = Document::parse(
        "<style>@property --a{syntax:'<length>';inherits:true;initial-value:1px}@property --b{syntax:'<length>';inherits:false;initial-value:2px}#p{--a:5px;--b:6px}#c{--a:var(--b);--b:var(--a);--u:var(--v);--v:var(--u);width:var(--a);height:var(--b);min-width:var(--u,7px)}</style><div id=p><div id=c></div></div>",
    );
    assert_eq!(styles_of(&doc, "#c", "width").as_deref(), Some("5px"));
    assert_eq!(styles_of(&doc, "#c", "height").as_deref(), Some("2px"));
    assert_eq!(styles_of(&doc, "#c", "min-width").as_deref(), Some("7px"));
}

#[test]
fn computed_lengths_svg_lengths_weights_and_math_print_like_chrome() {
    let doc = Document::parse(
        "<style>#a{line-height:1e6;stroke-width:2;stroke-dasharray:1 2;r:5;font-weight:300;font-weight:1001;min-width:calc((50% + 10px) * 2);max-width:min(10%, 20px)}#b{font-size:13px;line-height:1.15;stroke-dasharray:10%,2px;stroke-dashoffset:3;font-weight:calc(2000);width:calc(100% - (2 * 10px));margin-left:calc(10% + 5px);transform:rotate(90deg);display:none}#c{line-height:1e40;font-weight:300;font-weight:0.5;column-gap:calc(1em + 10%)}#d{line-height:1e-10;font-weight:calc(0.5)}</style><div id=a></div><div id=b></div><div id=c></div><div id=d></div>",
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
        ("#a", "line-height", "1.6e+07px"),
        ("#a", "stroke-width", "2px"),
        ("#a", "stroke-dasharray", "1px, 2px"),
        ("#a", "r", "5px"),
        ("#a", "cx", "0px"),
        ("#a", "rx", "auto"),
        ("#a", "font-weight", "300"),
        ("#a", "min-width", "calc(100% + 20px)"),
        ("#a", "max-width", "min(10%, 20px)"),
        ("#b", "line-height", "14.95px"),
        ("#b", "stroke-dasharray", "10%, 2px"),
        ("#b", "stroke-dashoffset", "3px"),
        ("#b", "font-weight", "1000"),
        ("#b", "width", "calc(100% - 20px)"),
        ("#b", "margin-left", "calc(10% + 5px)"),
        ("#b", "transform", "none"),
        ("#c", "line-height", "3.40282e+38px"),
        ("#c", "font-weight", "300"),
        ("#c", "column-gap", "calc(10% + 16px)"),
        ("#d", "line-height", "1.6e-09px"),
        ("#d", "font-weight", "1"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
    for (name, value, expected) in [
        ("width", "calc(1e1px + 2e0%)", "calc(2% + 10px)"),
        ("width", "calc(2px + 1em + 3px)", "calc(1em + 5px)"),
        (
            "width",
            "calc(10px + 2% - 1vw + 1em + 1rem)",
            "calc(2% + 1em + 10px + 1rem - 1vw)",
        ),
        ("width", "calc((50% + 10px) * 2)", "calc(100% + 20px)"),
        ("width", "calc(1em - 1em)", "calc(0em)"),
        ("width", "calc(1in + 1em)", "calc(1em + 96px)"),
        ("width", "calc(10px/3 + 1em)", "calc(1em + 3.33333px)"),
        (
            "width",
            "calc(0.5 * (1vw + 1vh) - 1px)",
            "calc(-1px + 0.5vh + 0.5vw)",
        ),
        (
            "width",
            "calc(10px - min(1em, 5px))",
            "calc(10px - min(1em, 5px))",
        ),
        (
            "width",
            "calc(2 * min(1em, 5px))",
            "calc(2 * min(1em, 5px))",
        ),
        (
            "width",
            "clamp(1em, 10px + 2%, 3em)",
            "clamp(1em, 2% + 10px, 3em)",
        ),
        (
            "width",
            "round(up, 10px + 1em, 1em)",
            "round(up, 1em + 10px, 1em)",
        ),
        ("margin-left", "calc(-1 * (1em + 2px))", "calc(-1em - 2px)"),
        (
            "transform",
            "translate(calc(2px + 1em), calc(10% + 10%))",
            "translate(calc(1em + 2px), calc(20%))",
        ),
        ("padding-left", "CALC(1EM + 2PX)", "calc(1em + 2px)"),
    ] {
        assert_eq!(specified(name, value), expected, "{name}: {value}");
    }
    for (weight, kept) in [
        ("1001", false),
        ("0", false),
        ("100 1001", false),
        ("200 900", true),
        ("1000", true),
    ] {
        let face = Declarations::descriptors(&format!("font-weight:{weight}"));
        assert_eq!(face.0.len(), usize::from(kept), "{weight}");
    }
}

#[test]
fn computed_numbers_print_like_chrome() {
    let doc = Document::parse(
        "<style>#a{z-index:1.5e1;order:+5;column-count:03;flex-grow:2e0;flex-shrink:.25;animation-iteration-count:2,3.0}#b{z-index:-0;column-count:2E1;flex-grow:1.0;order:99999999999;opacity:1e-10}#c{z-index:-2147483649;flex-grow:1e40;flex-shrink:1e-10;opacity:1e40}#d{flex-grow:3.4e38;flex-shrink:1234567;order:1e1}#e{flex-grow:1e7;flex-shrink:1e-7;tab-size:0.1234567}#f{flex-grow:0.00001;flex-shrink:0.0001}</style><div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div>",
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
        ("#a", "z-index", "auto"),
        ("#a", "order", "5"),
        ("#a", "column-count", "3"),
        ("#a", "flex-grow", "2"),
        ("#a", "flex-shrink", "0.25"),
        ("#a", "animation-iteration-count", "2, 3"),
        ("#b", "z-index", "0"),
        ("#b", "column-count", "auto"),
        ("#b", "flex-grow", "1"),
        ("#b", "order", "2147483647"),
        ("#b", "opacity", "1e-10"),
        ("#c", "z-index", "-2147483648"),
        ("#c", "flex-grow", "3.40282e+38"),
        ("#c", "flex-shrink", "1e-10"),
        ("#c", "opacity", "1"),
        ("#d", "flex-grow", "3.4e+38"),
        ("#d", "flex-shrink", "1.23457e+06"),
        ("#d", "order", "0"),
        ("#e", "flex-grow", "1e+07"),
        ("#e", "flex-shrink", "1e-07"),
        ("#e", "tab-size", "0.123457"),
        ("#f", "flex-grow", "1e-05"),
        ("#f", "flex-shrink", "0.0001"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
}

#[test]
fn content_visibility_and_intrinsic_sizes_parse_like_chrome() {
    for (name, value, valid) in [
        ("content-visibility", "auto", true),
        ("content-visibility", "foo", false),
        ("contain-intrinsic-size", "auto 10px", true),
        ("contain-intrinsic-size", "auto none", true),
        ("contain-intrinsic-size", "10px 20px 30px", false),
        ("contain-intrinsic-size", "auto 1px auto 2px", true),
        ("contain-intrinsic-size", "auto", false),
        ("contain-intrinsic-width", "auto none", true),
        ("contain-intrinsic-width", "10px 20px", false),
        ("contain-intrinsic-height", "-1px", false),
    ] {
        assert_eq!(supports(name, value), Some(valid), "{name}: {value}");
    }
    let doc = Document::parse(
        "<style>#a{contain-intrinsic-size:auto 70px}#b{contain-intrinsic-size:80px 40px}#c{contain-intrinsic-block-size:auto 5px;contain-intrinsic-inline-size:none}</style><div id=a></div><div id=b></div><div id=c hidden=until-found></div><div id=d hidden></div><div id=e style='--s:10px 20px;contain-intrinsic-size:var(--s)'></div>",
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
        ("#a", "contain-intrinsic-size", "auto 70px"),
        ("#a", "contain-intrinsic-height", "auto 70px"),
        ("#b", "contain-intrinsic-size", "80px 40px"),
        ("#c", "contain-intrinsic-size", "none auto 5px"),
        ("#c", "display", "block"),
        ("#c", "content-visibility", "hidden"),
        ("#d", "display", "none"),
        ("#d", "content-visibility", "visible"),
        ("#e", "contain-intrinsic-width", "10px"),
        ("#e", "contain-intrinsic-height", "20px"),
    ] {
        assert_eq!(get(selector, name), expected, "{selector} {name}");
    }
}

#[test]
fn registered_syntax_idents_are_case_sensitive() {
    let doc = Document::parse(
        "<style>@property --a{syntax:'Foo';inherits:false;initial-value:Foo}@property --b{syntax:'foo+';inherits:false;initial-value:foo}@property --c{syntax:'<length> | Auto';inherits:false;initial-value:1px}#t{--a:foo;--b:foo FOO;--c:auto}#u{--a:Foo;--b:foo foo;--c:Auto}</style><div id=t></div><div id=u></div>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("#t", "--a").as_deref(), Some("Foo"));
    assert_eq!(get("#t", "--b").as_deref(), Some("foo"));
    assert_eq!(get("#t", "--c").as_deref(), Some("1px"));
    assert_eq!(get("#u", "--a").as_deref(), Some("Foo"));
    assert_eq!(get("#u", "--b").as_deref(), Some("foo foo"));
    assert_eq!(get("#u", "--c").as_deref(), Some("Auto"));
    for (syntax, initial, valid) in [
        ("Foo", "foo", false),
        ("foo", "FOO", false),
        ("Foo+", "Foo foo", false),
        ("foo | <length>", "FOO", false),
        ("Foo-Bar", "Foo-Bar", true),
        ("Foo#", "Foo,Foo", true),
    ] {
        let rule =
            format!("@property --x{{syntax:'{syntax}';inherits:false;initial-value:{initial}}}");
        assert_eq!(
            Stylesheet::parse(&rule).source().contains("@property --x"),
            valid,
            "{rule}"
        );
    }
}

#[test]
fn registered_values_compute_as_their_first_matching_type() {
    use crate::css::value::grammar::computed;
    let of = |syntax: &str, value: &str| computed(syntax, value, None).0;
    assert_eq!(of("<custom-ident> | <color>", "red"), "red");
    assert_eq!(of("<color> | <custom-ident>", "red"), "rgb(255, 0, 0)");
    assert_eq!(of("<length> | <color>", "red"), "rgb(255, 0, 0)");
    assert_eq!(of("<length-percentage>", "max(1px, 5%)"), "max(1px, 5%)");
    assert_eq!(
        of("<length-percentage>", "calc(5% - 1px)"),
        "calc(5% - 1px)"
    );
}

#[test]
fn text_and_font_values_parse_and_compute_like_chrome() {
    for (name, value, supported) in [
        ("font-variant", "all-small-caps tabular-nums", true),
        ("font-variant", "normal none", false),
        ("font-variant-caps", "super", false),
        ("font-variant-numeric", "lining-nums oldstyle-nums", false),
        ("font-size-adjust", "cap-height .7", true),
        ("font-size-adjust", "from-font", false),
        ("hyphens", "auto", true),
        ("hyphens", "all", false),
        ("hyphenate-character", "'='", true),
        ("hyphenate-character", "'=' '-'", false),
        ("text-indent", "hanging 2em each-line", true),
        ("text-indent", "hanging", false),
        ("text-underline-position", "under left", true),
        ("text-underline-position", "left right", false),
        ("text-decoration-skip-ink", "none", true),
        ("text-emphasis", "filled red", true),
        ("text-emphasis", "red open circle", true),
        ("text-emphasis", "filled red dot", false),
        ("text-emphasis", "red blue", false),
        ("text-emphasis-style", "'x'", true),
        ("text-emphasis-style", "filled 'x'", false),
        ("text-emphasis-style", "dot circle", false),
        ("text-emphasis-position", "left under", true),
        ("text-emphasis-position", "left", false),
        ("-webkit-text-emphasis-color", "red", true),
    ] {
        assert_eq!(
            crate::css::value::grammar::supports(name, value),
            Some(supported),
            "{name}: {value}"
        );
    }
    let doc = Document::parse(
        "<p style='font:italic small-caps 20px Times;font-variant-position:super'><b style='font-variant:ruby unicase'></b></p>\
         <i style='font:20px Times;text-indent:each-line 1em'></i>",
    );
    let styles = styles(&doc, 800., 600.);
    let get = |selector, name| styles[element(&doc, selector)].get(name).cloned();
    assert_eq!(get("p", "font-variant-caps").as_deref(), Some("small-caps"));
    assert_eq!(get("p", "font-variant-position").as_deref(), Some("super"));
    assert_eq!(get("b", "font-variant-caps").as_deref(), Some("unicase"));
    assert_eq!(get("b", "font-variant-east-asian").as_deref(), Some("ruby"));
    assert_eq!(get("b", "font-variant-position").as_deref(), Some("normal"));
    assert_eq!(get("i", "font-variant-caps").as_deref(), Some("normal"));
    assert_eq!(get("i", "text-indent").as_deref(), Some("20px each-line"));
}

#[test]
fn zoom_multiplies_along_the_tree_and_scales_lengths() {
    let doc = Document::parse(
        r#"<style>
            #a { zoom: 200%; font-size: 10px; line-height: 12px; }
            #b { zoom: 1.5; font-size: 2em; }
            #c { zoom: 0; }
        </style><div id="a"><div id="b"><div id="c"></div></div></div>"#,
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
    let (a, b, c) = (
        element(&doc, "#a"),
        element(&doc, "#b"),
        element(&doc, "#c"),
    );
    let style = |id: usize, prop| computed.values[id].get(prop);
    assert_eq!(style(a, Prop::ZOOM), Some("2"));
    assert_eq!(style(c, Prop::ZOOM), Some("1"));
    assert_eq!(style(a, Prop::FONT_SIZE), Some("10px"));
    assert_eq!(style(a, Prop::LINE_HEIGHT), Some("12px"));
    assert_eq!(style(b, Prop::FONT_SIZE), Some("20px"));
    let units = |id: usize| computed.lengths[id];
    assert_eq!(units(a).font.em, 20.);
    assert_eq!(units(a).font.lh, 24.);
    assert_eq!(units(b).font.em, 60.);
    assert_eq!(units(c).font.zoom, 3.);
    assert_eq!(units(b).resolve("10px", 0.), Some(30.));
    assert_eq!(units(b).resolve("1rem", 0.), Some(48.));
    assert_eq!(units(b).resolve("50%", 100.), Some(50.));
    assert_eq!(units(b).unzoomed().resolve("1em", 0.), Some(20.));
}

#[test]
fn zoom_takes_non_negative_numbers_and_percentages() {
    for (value, valid) in [
        ("normal", true),
        ("0", true),
        ("1.5", true),
        ("50%", true),
        ("calc(2)", true),
        ("-1", false),
        ("-50%", false),
        ("reset", false),
        ("1px", false),
        ("auto", false),
    ] {
        assert_eq!(supports("zoom", value), Some(valid), "{value}");
    }
}

#[test]
fn zoom_prints_like_chrome_and_inherits_css_pixels() {
    let doc = Document::parse(
        r#"<style>
            #a { zoom: 2; font-size: 10px; width: 3em; }
            #b { width: inherit; }
            #c { zoom: 1e20; }
        </style><div id="a"><div id="b"></div></div><div id="c"><div id="d"></div></div>"#,
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
    let style = |selector| &computed.values[element(&doc, selector)];
    assert_eq!(style("#b").get(Prop::WIDTH), Some("30px"));
    assert_eq!(style("#c").get(Prop::ZOOM), Some("1e+20"));
    assert_eq!(computed.lengths[element(&doc, "#d")].font.zoom, 5e5);
}

#[test]
fn shorthand_keywords_match_without_case() {
    let doc = Document::parse(
        r#"<style>#b{border:1px SOLID red}#g{background:url(a.png) NO-REPEAT}#q{background:url("a).PNG") RED}#f{font:BOLD 12px Arial}#o{outline:2px DOTTED blue}</style><p id=b></p><p id=g></p><p id=q></p><p id=f></p><p id=o></p>"#,
    );
    let get = |selector, name| styles_of(&doc, selector, name);
    assert_eq!(get("#b", "border-top-style").as_deref(), Some("solid"));
    assert_eq!(get("#g", "background-repeat").as_deref(), Some("no-repeat"));
    let image = get("#q", "background-image").unwrap_or_default();
    assert!(image.contains("a).PNG"), "{image}");
    assert_eq!(get("#f", "font-weight").as_deref(), Some("700"));
    assert_eq!(get("#f", "font-family").as_deref(), Some("Arial"));
    assert_eq!(get("#o", "outline-style").as_deref(), Some("dotted"));
}

#[test]
fn animation_math_times_stay_times() {
    assert_eq!(
        specified("animation", "calc(1s) spin"),
        "calc(1s) ease 0s 1 normal none running spin"
    );
    assert_eq!(
        specified("animation", "spin 1s calc(2s)"),
        "1s ease calc(2s) 1 normal none running spin"
    );
}
