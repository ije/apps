use super::*;
use crate::dom::Document;
use crate::style::*;
use std::collections::HashMap;

fn element(doc: &Document, selector: &str) -> usize {
    (0..doc.nodes.len())
        .find(|&id| doc.matches(id, selector))
        .unwrap()
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
    let inline = inline_text(&doc, &computed, id, &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "A😀BC");
    assert_eq!(inline.boxes.len(), 1);
    assert_eq!(inline.boxes[0].fill, renderer::Color::hex(0xffee00ff));
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
        .insert("style".into(), "opacity:0.5;box-shadow:0 0 2px red".into());
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    assert!(inline_text(&doc, &computed, id, &[], &[], [true; 2]).is_none());
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
    let inline = inline_text(&doc, &computed, element(&doc, "p"), &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "the /hairline-create skill");
}

#[test]
fn inline_boxes_built_alone_keep_the_spaces_between_their_siblings() {
    let doc = Document::parse(
        "<div><mark>San</mark><span id=a> Juan </span><b>PR</b><span id=b> Rico </span><div>block</div></div>",
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
    let text = |selector| {
        inline_text(
            &doc,
            &computed,
            element(&doc, selector),
            &[],
            &[],
            [true; 2],
        )
        .unwrap()
        .text
    };
    assert_eq!(&*text("#a"), " Juan ");
    assert_eq!(&*text("#b"), " Rico");
}

#[test]
fn inline_boxes_built_alone_collapse_the_spaces_between_them() {
    let doc = Document::parse(
        "<p><span id=a1>a </span><span id=a2> b</span></p>\
         <p>a <span id=f> b </span><br></p>\
         <p><span id=l1>a </span>\n<span id=l2>b</span></p>\
         <p><span style=display:inline-block>ib</span> <span id=i> x </span> <span style=display:inline-block>ib</span></p>\
         <p>Hello <em><span id=c>world </span></em>again</p>\
         <p>one<em id=d> <span>two</span></em> three</p>",
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
    let text =
        |id: usize| inline_text(&doc, &computed, id, &[], &[], [true; 2]).map(|inline| inline.text);
    let of = |selector| text(element(&doc, selector)).unwrap();
    let pairs = [
        ("#a1", "a "),
        ("#a2", "b"),
        ("#f", "b"),
        ("#l1", "a "),
        ("#l2", "b"),
        ("#i", "x "),
        ("#c", "world "),
    ];
    for (selector, expected) in pairs {
        assert_eq!(&*of(selector), expected, "{selector}");
    }
    let space = doc.nodes[element(&doc, "#d")].children[0];
    assert_eq!(text(space).as_deref(), Some(" "));
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
        let inline = inline_text(&doc, &computed, id, &[], &[], [true; 2]).unwrap();
        let actual = (
            inline.text.clone(),
            inline
                .boxes
                .iter()
                .map(|inline_box| (inline.ranges[inline_box.index].1.clone(), inline_box.fill))
                .collect::<Vec<_>>(),
        );
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            assert_eq!(&*actual.0, "  state: ReviewState | 'all'");
            assert_eq!(actual.1, vec![(9..28, renderer::Color::hex(0xabcdefff))]);
            expected = Some(actual);
        }
    }
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
    let inline = inline_text(&doc, &computed, p, &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "\"Hi there x");
    assert_eq!(
        inline.runs.iter().map(|run| run.len).sum::<usize>(),
        inline.text.len()
    );
    let ink = |color: u32| renderer::Color::hex(color);
    assert_eq!(
        (inline.runs[0].len, inline.runs[0].color),
        (2, ink(0xff0000ff))
    );
    assert_eq!(inline.runs[1].color, ink(0x111111ff));
    assert_eq!(
        *inline.shadows,
        [
            (0., 0., 3., renderer::Color::hex(0x111111ff)),
            (1., 2., 0., renderer::Color::hex(0xff0000ff))
        ]
    );
    let (base, pseudo, mode) = inline.first_line.as_deref().unwrap();
    assert_eq!(mode, &None);
    let runs = restyle(&inline.runs, 0..inline.text.len(), base, pseudo);
    let colors: Vec<_> = runs.iter().map(|run| run.color).collect();
    assert_eq!(
        colors[..3],
        [ink(0xff0000ff), ink(0x0000ffff), ink(0x00ff00ff)]
    );
    assert!(runs.iter().all(|run| run.font.weight == 700.));
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
    assert!(inline_text(&doc, &computed, element(&doc, "p"), &[], &[], [true; 2]).is_none());
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
fn inline_boxes_and_atoms_stay_in_one_text_layout() {
    let doc = Document::parse(
        r#"<style>code { padding: 1px 4px; border: 1px solid red; background: #111 }
            .b { display: inline-block; width: 10px } .n { white-space: nowrap } img { display: inline }
            .flex { display: flex }</style>
        <p id="p">a <code id="c">b</code> <span class="b"></span> <span class="n"><img>d</span> e</p>
        <p id="alone"><span class="b"></span></p><div id="flex" class="flex"><img>text</div>"#,
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
    let inline = inline_text(&doc, &computed, element(&doc, "#p"), &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "a b \u{fffc} \u{fffc}d e");
    assert_eq!(inline.atoms.len(), 2);
    assert_eq!(inline.boxes.len(), 1);
    assert_eq!(inline.boxes[0].edges, [2., 5., 2., 5.]);
    assert_eq!(inline.boxes[0].lead, 5.);
    assert_eq!(inline.boxes[0].trail, 0.);
    let run_of = |text: &str| {
        let at = inline.text.find(text).unwrap();
        let mut end = 0;
        inline
            .runs
            .iter()
            .find(|run| {
                end += run.len;
                end > at
            })
            .unwrap()
    };
    assert_eq!(run_of("b").inset[0], renderer::px(5.));
    assert_eq!(run_of("b").inset[1], renderer::px(5.));
    assert!(run_of("d").nowrap);
    assert!(!run_of("e").nowrap);
    assert!(
        inline_text(
            &doc,
            &computed,
            element(&doc, "#alone"),
            &[],
            &[],
            [true; 2]
        )
        .is_none()
    );
    assert!(inline_text(&doc, &computed, element(&doc, "#flex"), &[], &[], [true; 2]).is_none());
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
    assert!(holds_block_box(&doc, &computed, id("h")));
    assert!(!holds_block_box(&doc, &computed, id("s")));
    assert!(makes_no_line(&doc, &computed, id("s")));
    assert!(!makes_no_line(&doc, &computed, id("t")));
    assert!(!makes_no_line(&doc, &computed, id("u")));
    let inline = inline_text(&doc, &computed, id("q"), &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "a b\nc");
    assert_eq!(
        inline.runs.iter().map(|run| run.len).sum::<usize>(),
        inline.text.len()
    );
}

#[test]
fn small_caps_are_smaller_capitals_like_chrome() {
    let doc = Document::parse("<p style='font:small-caps 20px Helvetica'>Ab cD 1</p>");
    let computed = compute_styles(
        &doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
    let inline = inline_text(&doc, &computed, p, &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "AB CD 1");
    let runs: Vec<_> = inline
        .runs
        .iter()
        .map(|run| (run.len, run.font_size.map(f32::from)))
        .collect();
    assert_eq!(
        runs,
        [
            (1, None),
            (1, Some(14.)),
            (1, None),
            (1, Some(14.)),
            (3, None)
        ]
    );
    assert!(inline.mixed);
}

#[test]
fn capitalize_follows_words_across_inline_text() {
    let doc = Document::parse(
        "<p style='text-transform:capitalize'>foo<b>bar</b> don't 3d x.y a-b _c<i> e</i>f</p>",
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
    let inline = inline_text(&doc, &computed, element(&doc, "p"), &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "Foobar Don't 3d X.y A-B _c Ef");
    let mut text = "ab cd".to_owned();
    text_transform(&mut text, 1..5, "capitalize");
    assert_eq!(text, "ab Cd");
}

#[test]
fn fallback_glyphs_measure_rows_with_a_normal_line_height() {
    let doc = Document::parse(
        "<p id=a style='font:16px Helvetica'>Hello 日本</p><p id=b style='font:16px Helvetica'>Héllo</p>\
         <p id=c style='font:16px/20px Helvetica'>Hello 日本</p>",
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
    let system = renderer::TextSystem::new();
    let mixed = |id| {
        let node = doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some(id));
        inline_text(&doc, &computed, node.unwrap(), &[], &[], [true; 2])
            .unwrap()
            .mixed(&system)
    };
    // Like Chrome, the font of `日本` grows the row, but not a fixed line height.
    assert_eq!([mixed("a"), mixed("b"), mixed("c")], [true, false, false]);
}

#[test]
fn uppercase_and_lowercase_take_full_case_mappings_like_chrome() {
    let doc = Document::parse(
        "<p style='text-transform:uppercase'>straße <b>ŉ ﬁx</b></p><div style='text-transform:lowercase'>İX ΟΔΟΣ ΣΑ ΟΔΟΣ'S ΟΔΟ<b>Σ</b></div>",
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
    let inline = inline_text(&doc, &computed, element(&doc, "p"), &[], &[], [true; 2]).unwrap();
    assert_eq!(&*inline.text, "STRASSE ʼN FIX");
    assert_eq!(inline.ranges.last().map(|(_, range)| range.len()), Some(7));
    let inline = inline_text(&doc, &computed, element(&doc, "div"), &[], &[], [true; 2]).unwrap();
    // A sigma at the end of a word takes its final form.
    assert_eq!(&*inline.text, "i\u{307}x οδος σα οδοσ's οδοσ");
}
