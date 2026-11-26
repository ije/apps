use super::*;
use crate::css::selector::{ACTIVE, FOCUS, FOCUS_WITHIN, HOVER};
use std::collections::HashMap;

#[test]
fn descendant_failures_are_bounded() {
    let doc = Document::parse(&format!(
        "{}<span></span>{}",
        "<div class='a'>".repeat(1000),
        "</div>".repeat(1000)
    ));
    let selector = Selector::parse(&format!(".missing {}span", ".a ".repeat(29)), None);
    let leaf = doc
        .nodes
        .iter()
        .position(|node| node.tag == "span")
        .unwrap();
    let started = std::time::Instant::now();
    assert!(!SelectorCache::default().matches(&selector[0], &doc, leaf, None));
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn retagged_children_record_their_parent_once() {
    let mut doc = Document::parse("<ul><li>a</li><li>b</li><li>c</li></ul>");
    let [ul, li] =
        ["ul", "li"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
    let mut walk = crate::dom::Walk::default();
    let parent = |cache: &SelectorCache| {
        cache.changes.back().map_or(0, |(_, changes)| {
            changes
                .iter()
                .filter(|&&change| change as usize == ul * KINDS + 2)
                .count()
        })
    };
    let mut cache = SelectorCache::default();
    walk.update(&doc, &Default::default(), None);
    cache.update(&doc, &walk);
    assert_eq!(parent(&cache), 1);
    doc.nodes[li].tag = "p".into();
    walk.update(&doc, &Default::default(), None);
    cache.update(&doc, &walk);
    assert_eq!(parent(&cache), 1);
}

#[test]
fn ancestor_filter_rejections_read_the_walk_kinds() {
    let doc = Document::parse("<div class=y><p><span class=x></span></p></div>");
    let x = doc
        .nodes
        .iter()
        .position(|node| node.tag == "span")
        .unwrap();
    let mut walk = crate::dom::Walk::default();
    walk.update(&doc, &Default::default(), None);
    let above: Vec<usize> =
        std::iter::successors(doc.nodes[x].parent, |&id| doc.nodes[id].parent).collect();
    let mut cache = SelectorCache::default();
    cache.update(&doc, &walk);
    let bloom = cache.ancestors(&doc, x);
    for (source, steps, rest) in [
        ("[data-k] > .x", &[&[0, 1][..]][..], &[][..]),
        ("#i > .y > .x", &[&[1, 8][..], &[0, 1]], &[]),
        (".a .x", &[], &[1, 8]),
        ("div.b .x", &[], &[0, 1, 8]),
        (".c > .d .x", &[], &[1, 8]),
        ("[style] .b > .x", &[&[1, 8][..]], &[0, 1, 6]),
    ] {
        let ancestry = Selector::parse(source, None)[0]
            .ancestry()
            .cloned()
            .unwrap();
        assert!(
            ancestry
                .0
                .iter()
                .zip(&bloom)
                .any(|(need, have)| need & !have != 0),
            "{source}"
        );
        cache.dependencies.clear();
        cache.reject(&doc, x, &ancestry, ancestry.2);
        let mut expected = vec![(x, 1)];
        for (index, &at) in above.iter().enumerate() {
            let kinds = steps.get(index).copied().unwrap_or(rest);
            expected.extend(kinds.iter().map(|&kind| (at, kind)));
        }
        let read: Vec<(usize, usize)> = cache
            .dependencies
            .iter()
            .map(|&(key, _)| (key as usize / KINDS, key as usize % KINDS))
            .collect();
        assert_eq!(read, expected, "{source}");
    }
}

#[test]
fn has_handles_relative_selectors_and_nested_lists() {
    let doc = Document::parse(
        "<main><code><div><i class='separator'></i></div></code></main><main id='empty'></main>",
    );
    let id = doc.nodes.iter().position(|n| n.tag == "main").unwrap();
    assert!(crate::dom::matches(
        &doc,
        id,
        ":has(>code > * > :is(.annotation,.separator))"
    ));
    assert!(crate::dom::matches(&doc, id, ":not(:has(> input))"));
    assert!(!crate::dom::matches(
        &doc,
        id,
        ":not(:has(>code > * > :is(.annotation,.separator)))"
    ));
    let empty = doc
        .nodes
        .iter()
        .position(|n| n.attrs.get("id").is_some_and(|v| v == "empty"))
        .unwrap();
    assert!(!crate::dom::matches(&doc, empty, ":has(.separator)"));
    assert!(
        crate::dom::matches(&doc, id, ":has(+ #empty)")
            && crate::dom::matches(&doc, id, ":has(~ main:empty)")
    );
    assert!(
        !crate::dom::matches(&doc, empty, ":has(+ main)")
            && !crate::dom::matches(&doc, id, ":has(+ code)")
    );
    let doc = Document::parse(
        "<ul><li id=a><b></b></li><li id=b class=x><i><b></b></i></li><li id=c></li></ul>",
    );
    for (selector, expected) in [
        ("li:has(> b)", "a"),
        ("li:has(b)", "a,b"),
        ("li:has(> i b)", "b"),
        ("li:has(+ .x)", "a"),
        ("li:has(+ li > i)", "a"),
        ("li:has(+ li b)", "a"),
        ("li:has(+ li + li)", "a"),
        ("li:has(~ #c)", "a,b"),
        ("li:has(> b, + #c)", "a,b"),
    ] {
        assert_eq!(matched(&doc, selector), expected, "{selector}");
    }
    let doc = Document::parse("<html><body><p></p></body></html>");
    let body = doc.nodes.iter().position(|n| n.tag == "body").unwrap();
    let html = doc.nodes.iter().position(|n| n.tag == "html").unwrap();
    let selector = &Selector::parse(":scope > body", None)[0];
    let mut memo = SelectorCache::default();
    assert!(memo.matches(selector, &doc, body, Some(0)));
    assert!(memo.matches(&Selector::parse(":scope", None)[0], &doc, html, Some(0)));
}

#[test]
fn nth_child_of_selector_follows_class_and_child_changes() {
    let mut doc = Document::parse(&format!(
        "<style>li:nth-child(odd of :not(.hidden)){{color:red}}li:nth-last-child(1 of :not(.hidden)){{width:1px}}li:nth-child(even of :has(> b)){{height:2px}}</style><ul><li><b></b></li><li><b></b></li>{}</ul>",
        "<li></li>".repeat(6)
    ));
    let [items, marks] = ["li", "b"].map(|tag| {
        (0..doc.nodes.len())
            .filter(|&id| doc.nodes[id].tag == tag)
            .collect::<Vec<_>>()
    });
    let (mut sheets, mut cache) = (HashMap::new(), crate::style::StyleCache::default());
    for (step, (hidden, owners)) in [
        (&[][..], [0, 1]),
        (&[0, 3], [0, 4]),
        (&[3], [2, 4]),
        (&[7], [2, 5]),
        (&[], [6, 7]),
    ]
    .into_iter()
    .enumerate()
    {
        for (index, &id) in items.iter().enumerate() {
            let value = if hidden.contains(&index) {
                "hidden"
            } else {
                ""
            };
            if doc.nodes[id].attrs.get("class").map_or("", String::as_str) != value {
                doc.nodes[id].attrs.set("class", value);
            }
        }
        for (&mark, owner) in marks.iter().zip(owners) {
            let (old, new) = (doc.nodes[mark].parent.unwrap(), items[owner]);
            if old != new {
                doc.nodes[old].children.retain(|&child| child != mark);
                doc.nodes[new].children.push(mark);
                doc.nodes[mark].parent = Some(new);
            }
        }
        let cached =
            crate::style::compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let fresh = crate::style::compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut crate::style::StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values, "step {step}");
        let shown: Vec<_> = (0..8).filter(|index| !hidden.contains(index)).collect();
        for (index, &id) in items.iter().enumerate() {
            let style = &cached.values[id];
            let position = shown.iter().position(|&shown| shown == index);
            assert_eq!(
                style.get(crate::css::properties::Prop::COLOR) == Some("red"),
                position.is_some_and(|position| position % 2 == 0),
                "step {step}: item {index}"
            );
            assert_eq!(
                style.get(crate::css::properties::Prop::WIDTH) == Some("1px"),
                shown.last() == Some(&index),
                "step {step}: last {index}"
            );
            assert_eq!(
                style.get(crate::css::properties::Prop::HEIGHT) == Some("2px"),
                index == owners[1],
                "step {step}: marked {index}"
            );
        }
    }
}

#[test]
fn selectors_follow_element_relationships() {
    let doc = Document::parse(
        "<html><main><section class='middle'><div class='middle'><span id='leaf'></span></div></section><i></i>text<b id='last'></b></main></html>",
    );
    let leaf = doc.nodes.iter().position(|n| n.tag == "span").unwrap();
    let last = doc.nodes.iter().position(|n| n.tag == "b").unwrap();
    assert!(crate::dom::matches(&doc, leaf, "main>.middle #leaf"));
    assert!(!crate::dom::matches(&doc, leaf, "main>.middle>#leaf"));
    for selector in ["section+i+b", "section~b", "#absent, #last", "*#last"] {
        assert!(crate::dom::matches(&doc, last, selector), "{selector}");
    }
    for selector in ["main>>b", "main>", ".", ":unknown", "#last,"] {
        assert!(!crate::dom::matches(&doc, last, selector), "{selector}");
    }
    for (id, node) in doc.nodes.iter().enumerate() {
        if node.tag.starts_with('#') {
            assert!(!crate::dom::matches(&doc, id, "*"));
        }
    }
    let selector = &Selector::parse("b:not(#last)", None)[0];
    let mut decoded: Selector =
        postcard::from_bytes(&postcard::to_allocvec(selector).unwrap()).unwrap();
    decoded.bind_parent(None).unwrap();
    assert!(
        !SelectorCache::default().matches(selector, &doc, last, None)
            && !SelectorCache::default().matches(&decoded, &doc, last, None)
    );
    assert!(!crate::dom::matches(
        &doc,
        last,
        &format!("{}b{}", ":is(".repeat(40), ")".repeat(40))
    ));
}

fn matched(doc: &Document, selector: &str) -> String {
    (0..doc.nodes.len())
        .filter(|&id| crate::dom::matches(doc, id, selector))
        .filter_map(|id| doc.nodes[id].attrs.get("id").cloned())
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn structural_pseudo_classes_follow_an_plus_b() {
    let doc = Document::parse(
        "<ul><li id=a class=k></li><li id=b></li><li id=c class=k></li><li id=d></li><li id=e class=k></li></ul><div><span id=s1></span><i id=i1></i><span id=s2></span><i id=i2></i><span id=s3></span></div><p><b id=only></b></p>",
    );
    for (selector, expected) in [
        ("li:nth-child(odd)", "a,c,e"),
        ("li:nth-child(EVEN)", "b,d"),
        ("li:nth-child(2n+1)", "a,c,e"),
        ("li:nth-child( 2n + 1 )", "a,c,e"),
        ("li:nth-child(2n- 1)", "a,c,e"),
        ("li:nth-child(-n+3)", "a,b,c"),
        ("li:nth-child(-n/**/+3)", "a,b,c"),
        ("li:nth-child(2n/**/+1)", "a,c,e"),
        ("li:nth-child(n+4)", "d,e"),
        ("li:nth-child(+3)", "c"),
        ("li:nth-child(0n+2)", "b"),
        ("li:nth-child(3n)", "c"),
        ("li:nth-child(-2n+5)", "a,c,e"),
        ("li:nth-child(n)", "a,b,c,d,e"),
        ("li:nth-last-child(2)", "d"),
        ("li:nth-last-child(-n+2)", "d,e"),
        ("li:nth-child(odd of .k)", "a,e"),
        ("li:nth-child(2 of .k)", "c"),
        ("li:nth-last-child(1 of .k)", "e"),
        ("li:nth-child(1 of :not(.k))", "b"),
        ("span:nth-of-type(2)", "s2"),
        ("div > :nth-of-type(odd)", "s1,i1,s3"),
        ("div > :nth-last-of-type(1)", "i2,s3"),
        ("div > :first-of-type", "s1,i1"),
        ("div > :last-of-type", "i2,s3"),
        ("div > :only-of-type", ""),
        ("p > :only-of-type, b:only-child", "only"),
        ("li:first-child, li:last-child", "a,e"),
        ("li:nth-child(n+2):nth-child(-n+3)", "b,c"),
    ] {
        assert_eq!(matched(&doc, selector), expected, "{selector}");
    }
}

#[test]
fn attribute_selectors_handle_flags_quotes_and_escapes() {
    let doc = Document::fragment(
        "<div id=x data-k='ABC def' lang='en-US' title='a]b c' class='hover:bg md:w-1/2 123'></div>",
        "body",
    );
    for selector in [
        "[data-k='abc def' i]",
        "[data-k=\"ABC def\" S]",
        "[ data-k ~= \"def\" ]",
        "[data-k~=DEF i]",
        "[data-k^='ab' i]",
        "[lang|=en]",
        "[lang|='EN' i]",
        "[lang=EN-us]",
        "[title=\"a]b c\"]",
        "[title='a\\]b c']",
        "[title=\"a\\5d b c\"]",
        "[|title]",
        "[*|title]",
        "div[id=x]",
        ".hover\\:bg.md\\:w-1\\/2",
        ".\\31 23",
    ] {
        assert!(crate::dom::matches(&doc, 1, selector), "{selector}");
        let source = Selector::parse(selector, None)[0].source();
        assert!(
            crate::dom::matches(&doc, 1, &source),
            "{selector} -> {source}"
        );
    }
    for selector in [
        "[data-k='abc def']",
        "[data-k='abc def' s]",
        "[data-k$='']",
        "[data-k*='']",
        "[data-k^='']",
        "[data-k~='']",
        "[data-k~='C d']",
        "[lang|=e]",
        "[lang=EN-us s]",
    ] {
        assert!(!crate::dom::matches(&doc, 1, selector), "{selector}");
    }
}

#[test]
fn invalid_selectors_reject_the_whole_list() {
    for selector in [
        "li:nth-child(2 n)",
        "li:nth-child(n-)",
        "li:nth-child(+ 2)",
        "li:nth-child(2n+-1)",
        "li:nth-child()",
        "li:nth-child(foo)",
        "li:nth-child(1.5)",
        "li:nth-of-type(1 of .k)",
        "[data-k=1]",
        "[data-k='x' q]",
        "[data-k='x]",
        "[=x]",
        "[a b]",
        "[ns|a]",
        ".1a",
        "#-1",
        ":not()",
        ":has()",
        ":has(:has(a))",
        ":has(:not(:has(a)))",
        "a::",
        ":lang()",
        ":dir()",
        "a, :not(.b",
        ":unknown",
        "::-moz-selection",
        ":-moz-focusring",
        "li:foo(1)",
        "a::before b",
        "a::before > b",
        "p:not(::before)",
        ":has(::after)",
    ] {
        assert!(
            Selector::parse(&format!("p, {selector}"), None).is_empty(),
            "{selector}"
        );
    }
    for selector in [
        ":is()",
        ":is(#1, .a)",
        ":where(.a, 1x)",
        "::-webkit-scrollbar",
        "::-webkit-scrollbar-thumb:vertical",
        ":-webkit-any-link",
        ":is(:has(a)):not(:has(b))",
        "dialog:-internal-dialog-in-top-layer",
        "a::starting-style",
        "select, ::picker(select)",
        "a::before:hover",
    ] {
        assert!(!Selector::parse(selector, None).is_empty(), "{selector}");
    }
    assert!(Selector::parse(&"a ".repeat(2000), None).is_empty());
}

#[test]
fn page_queries_reject_invalid_selectors() {
    for selector in [
        "",
        " ",
        "div[",
        "li,",
        ",li",
        "li >",
        "> li",
        "+ li",
        ":foo",
        "::foo",
        "li:foo(1)",
        "li::before()",
        ":hover()",
        ":root()",
        ":is",
        ":has(:has(a))",
        ":has(:is(:has(a)))",
        ":not(:foo)",
        ":nth-child(2 of :foo)",
        "a)",
        "(a",
        ":not(a",
        "a[x=\"y]",
        "'a'",
        "a..b",
        "#1a",
        "svg|rect",
        "*|.a",
        "|",
        "|.a",
        "a|",
        "div|*",
        "*|",
        "[ns|a]",
        "a > > b",
        "li:nth-child",
        "a!",
        "a::before b",
        "p:not(::before)",
    ] {
        let error = Selector::try_parse(selector).unwrap_err().to_string();
        assert!(error.starts_with("SyntaxError: "), "{selector}: {error}");
    }
    for selector in [
        "li",
        ":is()",
        ":is(:foo, li)",
        ":where(.a, 1x)",
        "#\\31 a",
        "*|rect",
        "*|*",
        "|rect",
        "|*",
        "[*|title]",
        "[|title]",
        "a::before",
        "a:before",
        "::-webkit-scrollbar-thumb:hover",
        "::-webkit-anything",
        "::slotted(span)",
        "::part(label)",
        ":host",
        ":host(.a)",
        ":host-context(.dark) b",
        ":focus-visible",
        ":visited",
        ":has(> img, + p)",
        "li:nth-child(2n+1 of .k)",
        ":-webkit-any(a, b)",
        "DIV > Span.A",
    ] {
        assert!(Selector::try_parse(selector).is_ok(), "{selector}");
    }
    let deep = format!("{}li{}", ":not(".repeat(1000), ")".repeat(1000));
    assert!(Selector::try_parse(&deep).unwrap().is_empty());
    assert!(Selector::try_parse(&format!(":not({})", "a ".repeat(300))).is_err());
}

#[test]
fn html_and_svg_names_match_case_insensitively_like_chrome() {
    let doc = Document::parse(
        "<ul><li id=a data-v=1></li></ul><svg id=s viewBox='0 0 1 1'><linearGradient id=g></linearGradient><rect id=r></rect></svg>",
    );
    let query = |selector: &str| {
        let selectors = Selector::try_parse(selector).unwrap();
        (0..doc.nodes.len())
            .filter(|&id| {
                selectors
                    .iter()
                    .any(|s| SelectorCache::default().matches(s, &doc, id, None))
            })
            .filter_map(|id| doc.nodes[id].attrs.get("id").cloned())
            .collect::<Vec<_>>()
            .join(",")
    };
    for (selector, expected) in [
        ("LI", "a"),
        ("Ul > lI", "a"),
        ("[DATA-V]", "a"),
        ("[Data-V='1']", "a"),
        ("[data-v='1' i]", "a"),
        ("linearGradient", "g"),
        ("lineargradient", "g"),
        ("LINEARGRADIENT", "g"),
        ("rect", "r"),
        ("[viewBox]", "s"),
        ("[viewbox]", "s"),
        ("*|rect", "r"),
        ("*|li", "a"),
        ("|rect, |li, |*", ""),
        ("*|*[id=g]", "g"),
        ("[*|data-v]", "a"),
    ] {
        assert_eq!(query(selector), expected, "{selector}");
    }
    assert_eq!(Selector::try_parse("|rect").unwrap()[0].source(), "|rect");
    let mut doc = Document::parse(
        "<style>LI{color:red}[DATA-V]{margin-top:1px}lineargradient{color:red}[viewbox]{margin-top:1px}[viewbox='0 0 1 1']{color:blue}[foobar]{margin-top:2px}</style><li data-v></li><lineargradient></lineargradient><svg viewBox='0 0 1 1'><linearGradient/></svg>",
    );
    let gradient = doc
        .nodes
        .iter()
        .position(|n| n.tag == "linearGradient")
        .unwrap();
    doc.nodes[gradient]
        .attrs
        .insert("fooBar".into(), String::new());
    let styles = crate::style::styles(&doc, 800., 600.);
    let at = |tag: &str| doc.nodes.iter().position(|n| n.tag == tag).unwrap();
    for (tag, color, margin) in [
        ("li", "red", Some("1px")),
        ("lineargradient", "red", None),
        ("linearGradient", "red", Some("2px")),
        ("svg", "blue", Some("1px")),
    ] {
        assert_eq!(styles[at(tag)]["color"], color, "{tag}");
        assert_eq!(
            styles[at(tag)].get("margin-top").map(String::as_str),
            margin,
            "{tag}"
        );
    }
}

#[test]
fn hostile_selectors_never_panic() {
    let alphabet: Vec<char> = "ab*|#.:()[]=~^$'\"\\>+, -_1nof&!é😀i".chars().collect();
    let doc = Document::parse("<div id=x class=a><p lang=en><b>t</b></p></div><svg><rect/></svg>");
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..2000 {
        let mut text = String::new();
        for _ in 0..(seed % 24) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            text.push(alphabet[(seed % alphabet.len() as u64) as usize]);
        }
        for prefix in ["", ":is(", ":has(", ":not(", "[", ":nth-child("] {
            let source = format!("{prefix}{text}");
            for selectors in [
                Selector::try_parse(&source).unwrap_or_default(),
                Selector::parse(&source, None),
            ] {
                for selector in &selectors {
                    selector.source();
                    for id in 0..doc.nodes.len() {
                        SelectorCache::default().matches(selector, &doc, id, None);
                    }
                }
            }
        }
    }
}

#[test]
fn specificity_follows_selectors_level_4() {
    for (selector, expected) in [
        ("*", [0, 0, 0]),
        ("li", [0, 0, 1]),
        ("li::before", [0, 0, 2]),
        ("li:before", [0, 0, 2]),
        (".a:hover", [0, 2, 0]),
        ("#a .b > c", [1, 1, 1]),
        (":is(#a, .b)", [1, 0, 0]),
        (":where(#a, .b) c", [0, 0, 1]),
        (":not(.a, #b)", [1, 0, 0]),
        (":has(> img, .x)", [0, 1, 0]),
        (":nth-child(2n of #a, .b)", [1, 1, 0]),
        (":nth-child(2n)", [0, 1, 0]),
        ("[data-k='v' i]", [0, 1, 0]),
        (":root", [0, 1, 0]),
        (":host(.a)", [0, 2, 0]),
    ] {
        assert_eq!(
            Selector::parse(selector, None)[0].specificity,
            expected,
            "{selector}"
        );
    }
}

#[test]
fn language_direction_link_and_form_pseudo_classes() {
    let doc = Document::parse(
        "<html lang=en-US><body><div lang=fr><p id=p1></p></div><p id=p2></p><div dir=rtl><span id=r></span><span id=l dir=ltr></span><span id=auto dir=auto></span></div><a id=link href='#x'></a><a id=plain></a><input id=t placeholder=x><input id=v placeholder=x value=1><input id=ro readonly><input id=cb type=checkbox checked required><input id=dis disabled><textarea id=ta required></textarea><div id=ce contenteditable><b id=inner></b></div><progress id=pr></progress><details id=det open></details></body></html>",
    );
    for (selector, expected) in [
        ("p:lang(en)", "p2"),
        ("p:lang(fr)", "p1"),
        ("p:lang(\"EN\", de)", "p2"),
        ("p:lang(en-US)", "p2"),
        ("p:lang(en-u)", ""),
        ("p:lang(*)", "p1,p2"),
        ("span:dir(rtl)", "r"),
        ("span:dir(ltr)", "l,auto"),
        ("a:link", "link"),
        ("a:any-link", "link"),
        ("a:visited, :target", ""),
        ("input:placeholder-shown", "t"),
        ("input:read-only", "ro,cb,dis"),
        ("input:read-write, textarea:read-write", "t,v,ta"),
        ("div:read-write, b:read-write", "ce,inner"),
        ("input:checked", "cb"),
        ("input:required", "cb"),
        ("input:optional", "t,v,ro,dis"),
        (":invalid", ""),
        ("input:valid", "t,v,cb"),
        ("textarea:valid", "ta"),
        ("input:disabled", "dis"),
        ("input:enabled", "t,v,ro,cb"),
        ("div:enabled, div:disabled", ""),
        ("progress:indeterminate", "pr"),
        ("details:open", "det"),
        ("p:defined", "p1,p2"),
    ] {
        assert_eq!(matched(&doc, selector), expected, "{selector}");
    }
    let html = doc.nodes.iter().position(|n| n.tag == "html").unwrap();
    assert!(SelectorCache::default().matches(
        &Selector::parse(":scope", None)[0],
        &doc,
        html,
        None
    ));
}

#[test]
fn form_pseudo_classes_follow_owners_groups_and_states() {
    let mut doc = Document::parse(
        "<form id=f><fieldset id=s><input id=a></fieldset><input id=o form=g><button id=b></button></form><form id=g><input id=i></form><button id=x form=f></button><button id=y form=f type=button></button><input id=z form=none><form id=r><input type=radio name=n id=r1><input type=radio name=n id=r2><input type=radio name=m id=r3></form><input type=radio name=n id=r4 form=r><input type=radio id=r5><x-a id=w></x-a><progress id=p></progress>",
    );
    let find = |doc: &Document, id: &str| {
        (0..doc.nodes.len())
            .find(|&node| doc.nodes[node].attrs.get("id").is_some_and(|v| v == id))
            .unwrap()
    };
    let check = |doc: &Document, cases: &[(&str, &str)]| {
        for (selector, expected) in cases {
            assert_eq!(matched(doc, selector), *expected, "{selector}");
        }
    };
    check(
        &doc,
        &[
            (":is(form, fieldset):valid", "f,s,g,r"),
            (":default", "b"),
            (":indeterminate", "r1,r2,r3,r4,r5,p"),
            (":user-valid, :state(on)", ""),
        ],
    );
    let [a, o, b, r4, w] = ["a", "o", "b", "r4", "w"].map(|id| find(&doc, id));
    doc.nodes[a].state = super::super::INVALID | super::super::USER;
    doc.nodes[o].state = super::super::INVALID;
    doc.nodes[r4].state = super::super::DIRTY | super::super::CHECKED;
    doc.nodes[w].state = super::super::FORM_ASSOCIATED | super::super::INVALID;
    doc.nodes[w].value = Some("on\0off".into());
    doc.nodes[b].attrs.set("type", "reset");
    check(
        &doc,
        &[
            (":is(form, fieldset):invalid", "f,s,g"),
            (":default", "x"),
            (":indeterminate", "r3,r5,p"),
            (":user-invalid", "a"),
            (":state(off)", "w"),
            ("x-a:invalid:enabled", "w"),
        ],
    );
    let g = find(&doc, "g");
    doc.nodes[g].attrs.remove("id");
    check(&doc, &[("form:invalid", "f")]);
}

#[test]
fn form_states_follow_fieldsets_and_pushed_bits() {
    let mut doc = Document::parse(
        "<style>:disabled{color:red}:invalid{color:blue}:not(:defined){color:green}:indeterminate,:default{color:gray}</style><form><fieldset id=fs disabled><legend id=lg><input id=a></legend><input id=b><legend id=lg2><input id=c></legend></fieldset><select><optgroup id=og disabled><option id=o1>x</option></optgroup><option id=o2 selected>y</option></select><input id=cb type=CheckBox checked><input id=sub type=submit><button id=btn></button><x-el id=x></x-el><input id=t readonly></form><button id=out></button><p dir=RTL id=p></p><svg><feColorMatrix id=m type=Saturate></feColorMatrix></svg>",
    );
    let find = |doc: &Document, id: &str| {
        (0..doc.nodes.len())
            .find(|&node| doc.nodes[node].attrs.get("id").is_some_and(|v| v == id))
            .unwrap()
    };
    let [fs, a, b, c, cb, x] = ["fs", "a", "b", "c", "cb", "x"].map(|id| find(&doc, id));
    for (selector, expected) in [
        (":disabled", "fs,b,c,og,o1"),
        ("input:enabled", "a,cb,sub,t"),
        ("input:read-write", "a"),
        (":default", "o2,cb,sub"),
        (":valid", "fs,a,cb,sub,btn,out"),
        (":invalid, :indeterminate", ""),
        (":not(:defined)", "x"),
        ("[type=checkbox], [dir=rtl], [type=saturate]", "cb,p"),
        ("[dir=rtl s], [type=Saturate]", "m"),
    ] {
        assert_eq!(matched(&doc, selector), expected, "{selector}");
    }
    doc.nodes[x].state = super::super::DEFINED;
    doc.nodes[cb].state = super::super::INDETERMINATE;
    doc.nodes[a].state = super::super::INVALID;
    doc.nodes[b].state = super::super::INVALID;
    for (selector, expected) in [
        (":not(:defined)", ""),
        (":indeterminate", "cb"),
        (":invalid", "fs,a"),
        (":has(> legend > :invalid)", "fs"),
    ] {
        assert_eq!(matched(&doc, selector), expected, "{selector}");
    }
    let mut sheets = HashMap::new();
    let mut cache = crate::style::StyleCache::default();
    let legend = find(&doc, "lg2");
    for step in 0..5 {
        match step {
            1 => drop(doc.nodes[fs].attrs.remove("disabled")),
            2 => doc.nodes[fs].attrs.set("disabled", ""),
            3 => {
                let children = &mut doc.nodes[fs].children;
                let at = children.iter().position(|&kid| kid == legend).unwrap();
                children.remove(at);
                children.insert(0, legend);
            }
            4 => {
                doc.nodes[a].state = 0;
                drop(doc.nodes[fs].attrs.remove("disabled"));
            }
            _ => {}
        }
        let cached =
            crate::style::compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let fresh = crate::style::compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut crate::style::StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values, "step {step}");
        let color = |id: usize| {
            let color = cached.values[id].get(crate::css::properties::Prop::new("color"));
            ["red", "blue"]
                .into_iter()
                .find(|&name| color.is_some_and(|value| value == name))
                .unwrap_or("-")
        };
        let expected = [
            ["blue", "red", "red"],
            ["blue", "blue", "-"],
            ["blue", "red", "red"],
            ["red", "red", "-"],
            ["-", "blue", "-"],
        ];
        assert_eq!([a, b, c].map(color), expected[step], "step {step}");
    }
    let parent = Rc::new(Selector::parse(".a", None));
    let nested =
        Document::parse("<div class=a><i id=in title='R&D'></i></div><i id=out title='R&D'></i>");
    for (source, expected) in [
        ("[title=\"R&D\"]", "in"),
        (".R\\&D, i", "in"),
        (":is(&) i", "in"),
    ] {
        let selectors = Selector::parse(source, Some(&parent));
        let found: Vec<_> = (0..nested.nodes.len())
            .filter(|&id| {
                selectors
                    .iter()
                    .any(|selector| SelectorCache::default().matches(selector, &nested, id, None))
            })
            .filter_map(|id| nested.nodes[id].attrs.get("id").cloned())
            .collect();
        assert_eq!(found.join(","), expected, "{source}");
    }
    let div = nested
        .nodes
        .iter()
        .position(|node| node.tag == "div")
        .unwrap();
    let scoped = Selector::try_parse("& > i").unwrap();
    let inside = find(&nested, "in");
    assert!(SelectorCache::default().matches(&scoped[0], &nested, inside, Some(div)));
    assert_eq!(scoped[0].specificity, [0, 0, 1]);
}

#[test]
fn interaction_states_invalidate_cached_styles() {
    let mut doc = Document::parse(
        "<style>.card:hover{color:red}.card:hover .title{font-weight:700}:focus-within>.title{background-color:blue}.title:focus{color:green}button:active{color:orange}:has(>.title:hover){padding-top:1px}</style><div class=card><span class=title>t</span><button>b</button></div>",
    );
    let card = doc.nodes.iter().position(|n| n.tag == "div").unwrap();
    let title = doc.nodes.iter().position(|n| n.tag == "span").unwrap();
    let button = doc.nodes.iter().position(|n| n.tag == "button").unwrap();
    let checks = [
        (card, "color", "red"),
        (card, "padding-top", "1px"),
        (title, "font-weight", "700"),
        (title, "background-color", "blue"),
        (title, "color", "green"),
        (button, "color", "orange"),
    ];
    let mut sheets = HashMap::new();
    let mut cache = crate::style::StyleCache::default();
    for (step, (states, expected)) in [
        ([0, 0, 0], &[][..]),
        ([HOVER, 0, 0], &[0, 2][..]),
        ([HOVER, HOVER, 0], &[0, 1, 2][..]),
        ([0, 0, 0], &[][..]),
        ([FOCUS_WITHIN, FOCUS | FOCUS_WITHIN, 0], &[3, 4][..]),
        ([HOVER | ACTIVE, 0, HOVER | ACTIVE], &[0, 2, 5][..]),
        ([0, 0, 0], &[][..]),
    ]
    .into_iter()
    .enumerate()
    {
        for (id, state) in [card, title, button].into_iter().zip(states) {
            doc.nodes[id].state = state;
        }
        let cached =
            crate::style::compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let fresh = crate::style::compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut sheets,
            &mut crate::style::StyleCache::default(),
            None,
        );
        assert_eq!(cached.values, fresh.values, "step {step}");
        for (index, (id, key, value)) in checks.into_iter().enumerate() {
            assert_eq!(
                cached.values[id]
                    .get(crate::css::properties::Prop::new(key))
                    .is_some_and(|v| v == value),
                expected.contains(&index),
                "step {step}: {key}:{value}"
            );
        }
    }
}

#[test]
fn slotted_and_part_take_what_chrome_takes_after_them() {
    let element = Some(usize::MAX);
    for (selector, target) in [
        ("::part(a)", element),
        ("x::part(a)::before", Some(0)),
        ("::part(a)::selection", Some(2)),
        ("::part(a)::marker", Some(3)),
        ("::part(a)::first-line", Some(5)),
        ("::part(a)::placeholder", Some(6)),
        ("::part(a)::-webkit-scrollbar-thumb:hover", Some(11)),
        ("::part(a)::backdrop", Some(15)),
        ("::part(a):hover:focus", element),
        ("::part(a):not(:hover)", element),
        ("::part(a):state(x)", element),
        ("::part(a):hover::after", Some(1)),
        ("::part(a):first-child", None),
        ("::part(a):not(:first-child)", None),
        ("::part(a):empty", None),
        ("::part(a):has(b)", None),
        ("::part(a):root", None),
        ("::part(a):is(.x)", None),
        ("::part(a).x", None),
        ("::part(a)::before:hover", None),
        ("::part(a)::part(b)", None),
        ("::part(a)::slotted(b)", None),
        ("::slotted(a)", element),
        ("::slotted(a)::before", Some(0)),
        ("::slotted(a)::after", Some(1)),
        ("::slotted(a)::marker", Some(3)),
        ("::slotted(a)::placeholder", Some(6)),
        ("::slotted(a)::backdrop", Some(15)),
        ("::slotted(a)::starting-style", Some(14)),
        ("::part(a)::backdrop::starting-style", Some(16)),
        ("::slotted(a)::first-line", None),
        ("::slotted(a)::selection", None),
        ("::slotted(a)::-webkit-scrollbar", None),
        ("::slotted(a):hover", None),
        ("::slotted(a)::part(b)", None),
    ] {
        let parsed = Selector::parse(selector, None);
        assert_eq!(
            parsed[0].shadow_target().map(|(kind, _)| kind),
            target,
            "{selector}"
        );
    }
}
