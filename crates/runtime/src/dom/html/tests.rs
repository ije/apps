use super::*;

fn find(doc: &Document, tag: &str) -> usize {
  doc.nodes.iter().position(|node| node.tag == tag).unwrap()
}

fn parent(doc: &Document, tag: &str) -> String {
  doc.nodes[doc.nodes[find(doc, tag)].parent.unwrap()]
    .tag
    .clone()
}

fn tree(doc: &Document, id: usize) -> String {
  let node = &doc.nodes[id];
  match node.tag.as_str() {
    "#text" => format!("{:?}", node.text),
    "#comment" => format!("<!--{}-->", node.text),
    "#doctype" => format!("<!DOCTYPE {}>", node.text.replace('\0', "|")),
    tag => {
      let attrs: String = node
        .attrs
        .iter()
        .map(|(key, value)| format!(" {key}={value:?}"))
        .collect();
      let children: String = node
        .children
        .iter()
        .map(|&child| tree(doc, child))
        .collect();
      if tag.starts_with('#') {
        children
      } else {
        format!("<{tag}{attrs}>{children}</{tag}>")
      }
    }
  }
}

#[test]
fn closing_tags_do_not_cross_table_scope() {
  let doc =
    Document::parse("<div><table><tr><td></div><span>inside</span></td></tr></table></div>");
  assert_eq!(parent(&doc, "span"), "td");
}

#[test]
fn unmatched_end_tags_are_linear() {
  let started = std::time::Instant::now();
  for source in [
    format!("{}{}", "<div>".repeat(50000), "</x>".repeat(50000)),
    format!(
      "<div><table>{}{}",
      "<span>".repeat(50000),
      "</div>".repeat(50000)
    ),
    format!(
      "<p><button>{}{}",
      "<span>".repeat(50000),
      "<div>".repeat(50000)
    ),
    format!(
      "<table><tr><td>{}{}",
      "<b>".repeat(50000),
      "</tr>".repeat(50000)
    ),
    format!("<svg>{}{}", "<g>".repeat(50000), "</x>".repeat(50000)),
  ] {
    assert!(Document::parse(&source).nodes.len() > 50000);
  }
  let names: String = (0..50000).map(|n| format!(" a{n}=x")).collect();
  let doc = Document::parse(&format!("<p{names} a7=y a49999=y>"));
  let p = find(&doc, "p");
  assert_eq!(doc.nodes[p].attrs.keys().count(), 50000);
  assert_eq!(doc.nodes[p].attrs["a49999"], "x");
  assert!(started.elapsed() < std::time::Duration::from_secs(10));
  let doc = Document::parse(
    "<div><b>x</div>y<svg><linearGradient><stop/></LINEARGRADIENT><g></g></svg><table><tr><td><p>in</table><i>out</i>",
  );
  let y = doc.nodes.iter().position(|n| n.text == "y").unwrap();
  let b = doc.nodes[y].parent.unwrap();
  assert_eq!(
    (doc.nodes[b].tag.as_str(), parent(&doc, "div").as_str()),
    ("b", "body")
  );
  assert_eq!(doc.nodes[doc.nodes[b].parent.unwrap()].tag, "body");
  assert_eq!(parent(&doc, "g"), "svg");
  assert_eq!(parent(&doc, "stop"), "linearGradient");
  assert_eq!(parent(&doc, "i"), "b");
}

#[test]
fn raw_text_and_attributes() {
  let doc = Document::parse(
    "<!doctype html><html><body><h1 id='g' data-x=\"a>b\">你好 &amp; &#65;</h1><script>if (a < b) x = '<div>';</script></body></html>",
  );
  let h = find(&doc, "h1");
  assert_eq!(doc.text(h), "你好 & A");
  assert_eq!(doc.nodes[h].attrs["data-x"], "a>b");
  assert!(crate::dom::matches(&doc, h, "body #g"));
  let script = find(&doc, "script");
  assert!(doc.nodes[script].text.is_empty());
  assert_eq!(doc.nodes[script].children.len(), 1);
  assert_eq!(doc.text(script), "if (a < b) x = '<div>';");
  assert_eq!(Document::parse(&doc.serialize(0)).text(h), doc.text(h));
  let doc = Document::parse(
    "<title>a &amp; <b></title><style>a>b{}</style><textarea>\n<i>&lt;</textarea><xmp><p></xmp><noscript><p></noscript><style></style>",
  );
  assert_eq!(
    tree(&doc, 0),
    r#"<html><head><title>"a & <b>"</title><style>"a>b{}"</style></head><body><textarea>"<i><"</textarea><xmp>"<p>"</xmp><noscript>"<p>"</noscript><style></style></body></html>"#
  );
  let style = find(&doc, "style");
  assert_eq!(doc.serialize(style), "<style>a>b{}</style>");
  assert_eq!(
    Document::parse("<p>x <a < b</p>").serialize(0),
    "<html><head></head><body><p>x <a <=\"\" b<=\"\" p=\"\"></a></p></body></html>"
  );
  let doc = Document::parse("<p>a < b &c d<</p>");
  assert_eq!(doc.nodes[find(&doc, "p")].children.len(), 1);
  assert_eq!(doc.text(find(&doc, "p")), "a < b &c d<");
}

#[test]
fn character_references_follow_the_standard_table() {
  for (source, text) in [
    (
      "&notin; &NotEqualTilde; &amp &ampx &copy2023 &notit; &fjlig;",
      "∉ \u{2242}\u{338} & &x ©2023 ¬it; fj",
    ),
    (
      "&#x80;&#150;&#0;&#xD800;&#x110000;&#99999999999;&#x41&#65x",
      "€–\u{fffd}\u{fffd}\u{fffd}\u{fffd}AAx",
    ),
    (
      "&#; &#x; &; &unknown; &AMP; &CounterClockwiseContourIntegral;",
      "&#; &#x; &; &unknown; & ∳",
    ),
    ("a&ltb&gt;&nbsp;&NewLine;&Tab;", "a<b>\u{a0}\n\t"),
  ] {
    assert_eq!(decode(source, false), text, "{source}");
  }
  assert_eq!(decode("?a=1&copy=2&amp;b&lt", true), "?a=1&copy=2&b<");
  assert_eq!(decode("&notit &not &amp;x", true), "&notit ¬ &x");
  let doc = Document::parse("<a href='?x=1&lang=en&amp;y' title=&quot;q&quot;>&lt;&#x26;</a>");
  let a = find(&doc, "a");
  assert_eq!(doc.nodes[a].attrs["href"], "?x=1&lang=en&y");
  assert_eq!(doc.nodes[a].attrs["title"], "\"q\"");
  assert_eq!(doc.text(a), "<&");
}

#[test]
fn implied_end_tags_and_document_structure() {
  for (source, expected) in [
    (
      "<!doctype html><title>x</title><p>",
      "<!DOCTYPE html><html><head><title>\"x\"</title></head><body><p></p></body></html>",
    ),
    (
      "<html><body>a",
      "<html><head></head><body>\"a\"</body></html>",
    ),
    (
      "<meta charset=utf-8>\n<link rel=x><body class=a><p>",
      "<html><head><meta charset=\"utf-8\"></meta>\"\\n\"<link rel=\"x\"></link></head><body class=\"a\"><p></p></body></html>",
    ),
    (
      "<head></head>\n<style>s</style><div>",
      "<html><head><style>\"s\"</style></head>\"\\n\"<body><div></div></body></html>",
    ),
    (
      "<p>a<div>b</div><p>c<ul><li>1<li>2<ol><li>3</ol></ul>",
      "<html><head></head><body><p>\"a\"</p><div>\"b\"</div><p>\"c\"</p><ul><li>\"1\"</li><li>\"2\"<ol><li>\"3\"</li></ol></li></ul></body></html>",
    ),
    (
      "<dl><dt>a<dd>b<dt>c</dl><p>x<h1>y</p>z",
      "<html><head></head><body><dl><dt>\"a\"</dt><dd>\"b\"</dd><dt>\"c\"</dt></dl><p>\"x\"</p><h1>\"y\"<p></p>\"z\"</h1></body></html>",
    ),
    (
      "</p><select><option>a<option>b<optgroup><option>c</select>",
      "<html><head></head><body><select><option>\"a\"</option><option>\"b\"</option><optgroup><option>\"c\"</option></optgroup></select></body></html>",
    ),
    (
      "<ruby>a<rb>b<rt>c<rp>d</ruby><pre>\nx</pre>",
      "<html><head></head><body><ruby>\"a\"<rb>\"b\"</rb><rt>\"c\"</rt><rp>\"d\"</rp></ruby><pre>\"x\"</pre></body></html>",
    ),
    (
      "<span><div>x</span>y</div>z",
      "<html><head></head><body><span><div>\"xy\"</div>\"z\"</span></body></html>",
    ),
    (
      "<a href=1>x<a href=2>y</a><br></br><!--c-->",
      "<html><head></head><body><a href=\"1\">\"x\"</a><a href=\"2\">\"y\"</a><br></br><br></br><!--c--></body></html>",
    ),
    (
      "<b>1<p>2</b>3</p>",
      "<html><head></head><body><b>\"1\"</b><p><b>\"2\"</b>\"3\"</p></body></html>",
    ),
    (
      "<a><p>X<a>Y</a>Z</p></a>",
      "<html><head></head><body><a></a><p><a>\"X\"</a><a>\"Y\"</a>\"Z\"</p></body></html>",
    ),
    (
      "<p><b><i>x</p>y",
      "<html><head></head><body><p><b><i>\"x\"</i></b></p><b><i>\"y\"</i></b></body></html>",
    ),
    (
      "<b><div><i>x</b>y</i>z",
      "<html><head></head><body><b></b><div><b><i>\"x\"</i></b><i>\"y\"</i>\"z\"</div></body></html>",
    ),
    (
      "<table><tr><td><b>x</td><td>y</td></tr></table>z",
      "<html><head></head><body><table><tbody><tr><td><b>\"x\"</b></td><td>\"y\"</td></tr></tbody></table>\"z\"</body></html>",
    ),
    ("", "<html><head></head><body></body></html>"),
    (
      "<!doctype html><title>t</title><script>1</script>",
      "<!DOCTYPE html><html><head><title>\"t\"</title><script>\"1\"</script></head><body></body></html>",
    ),
    (
      "<template>x",
      "<html><head><template>\"x\"</template></head><body></body></html>",
    ),
    (
      "<head></head><template id=t><p>x</p></template><body><p>y",
      "<html><head><template id=\"t\"><p>\"x\"</p></template></head><body><p>\"y\"</p></body></html>",
    ),
    (
      "<p>a<!--b--!>c<!--d--->e<!---->f</p>",
      "<html><head></head><body><p>\"a\"<!--b-->\"c\"<!--d--->\"e\"<!---->\"f\"</p></body></html>",
    ),
    (
      "<script><!--<script></script>x</script>y<script><!--<script>--></script>z",
      "<html><head><script>\"<!--<script></script>x\"</script></head><body>\"y\"<script>\"<!--<script>-->\"</script>\"z\"</body></html>",
    ),
    (
      "<p><b a=1 c=2><b c=2 a=1><b a=1 c=2><b c=2 a=1></p>x",
      "<html><head></head><body><p><b a=\"1\" c=\"2\"><b c=\"2\" a=\"1\"><b a=\"1\" c=\"2\"><b c=\"2\" a=\"1\"></b></b></b></b></p><b c=\"2\" a=\"1\"><b a=\"1\" c=\"2\"><b c=\"2\" a=\"1\">\"x\"</b></b></b></body></html>",
    ),
    (
      "<p><b>x</p><div>y</div><xmp>z</xmp></br>",
      "<html><head></head><body><p><b>\"x\"</b></p><div><b>\"y\"</b></div><b><xmp>\"z\"</xmp><br></br></b></body></html>",
    ),
    (
      "<a><table><a></table><p><a><div><a>",
      "<html><head></head><body><a><a></a><table></table></a><p><a></a></p><div><a></a></div></body></html>",
    ),
    (
      "<form id=g><div><form id=h><input name=b></form></div></form><form><p>x</form>y",
      "<html><head></head><body><form id=\"g\"><div><input name=\"b\"></input></div></form><form><p>\"x\"</p></form>\"y\"</body></html>",
    ),
  ] {
    assert_eq!(tree(&Document::parse(source), 0), expected, "{source}");
  }
  let doc =
    Document::parse("<!-- a --><!doctype html><?xml x?></3><html lang=en><body></body></html>");
  assert_eq!(
    tree(&doc, 0),
    "<!-- a --><!DOCTYPE html><!--?xml x?--><!--3--><html lang=\"en\"><head></head><body></body></html>"
  );
  for (source, expected) in [
    (
      "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\" 'http://www.w3.org/TR/html4/strict.dtd'><p>",
      "<!DOCTYPE html|-//W3C//DTD HTML 4.01//EN|http://www.w3.org/TR/html4/strict.dtd>",
    ),
    (
      "<!doctype html SYSTEM \"about:legacy-compat\">",
      "<!DOCTYPE html||about:legacy-compat>",
    ),
    ("<!doctype>", "<!DOCTYPE >"),
    ("<html><!doctype html>", ""),
  ] {
    let doc = Document::parse(source);
    let first = &doc.nodes[doc.nodes[0].children[0]];
    assert_eq!(
      if first.tag == "#doctype" {
        tree(&doc, doc.nodes[0].children[0])
      } else {
        String::new()
      },
      expected,
      "{source}"
    );
  }
  assert!(
    Document::fragment("<!doctype html><p>", "body")
      .nodes
      .iter()
      .all(|node| node.tag != "#doctype")
  );
  let legacy = Document::parse("<!DOCTYPE html PUBLIC \"a\" \"b\"><p>x");
  assert!(
    legacy
      .serialize(0)
      .starts_with("<!DOCTYPE html PUBLIC \"a\" \"b\"><html>")
  );
  assert_eq!(
    Document::parse(&legacy.serialize(0)).nodes[1].text,
    "html\0a\0b"
  );
}

#[test]
fn tables_imply_sections_rows_and_foster_parent() {
  for (source, expected) in [
    (
      "<table><tr><td>a<td>b<tr><th>c</table>",
      "<table><tbody><tr><td>\"a\"</td><td>\"b\"</td></tr><tr><th>\"c\"</th></tr></tbody></table>",
    ),
    (
      "<table><td>a</td></table>",
      "<table><tbody><tr><td>\"a\"</td></tr></tbody></table>",
    ),
    (
      "<table><thead><tr><th>h<tbody><tr><td>b<tfoot><tr><td>f</table>",
      "<table><thead><tr><th>\"h\"</th></tr></thead><tbody><tr><td>\"b\"</td></tr></tbody><tfoot><tr><td>\"f\"</td></tr></tfoot></table>",
    ),
    (
      "<table><caption>c<tr><td>x</table>",
      "<table><caption>\"c\"</caption><tbody><tr><td>\"x\"</td></tr></tbody></table>",
    ),
    (
      "<table><col><col></table>",
      "<table><colgroup><col></col><col></col></colgroup></table>",
    ),
    (
      "<table>\n<tr>\n<td>a</td>\n</tr>\n</table>",
      "<table>\"\\n\"<tbody><tr>\"\\n\"<td>\"a\"</td>\"\\n\"</tr>\"\\n\"</tbody></table>",
    ),
    (
      "<table>x<b>y</b><tr><td>z</table>",
      "\"x\"<b>\"y\"</b><table><tbody><tr><td>\"z\"</td></tr></tbody></table>",
    ),
    (
      "<table><tr><td><table><tr><td>in</table>out</td></tr></table>",
      "<table><tbody><tr><td><table><tbody><tr><td>\"in\"</td></tr></tbody></table>\"out\"</td></tr></tbody></table>",
    ),
    (
      "<table><tr><td>a<p>b</table><i>c",
      "<table><tbody><tr><td>\"a\"<p>\"b\"</p></td></tr></tbody></table><i>\"c\"</i>",
    ),
    (
      "<table><input type=hidden><select><option>a</table>",
      "<select><option>\"a\"</option></select><table><input type=\"hidden\"></input></table>",
    ),
    (
      "<a href=1><table><a href=2>x</a><tr><td>cell</td></tr></table></a>",
      "<a href=\"1\"><a href=\"2\">\"x\"</a><table><tbody><tr><td>\"cell\"</td></tr></tbody></table></a>",
    ),
    (
      "<a href=1><svg><foreignObject><a href=2>x</a><div>y</div></foreignObject><rect/></svg></a>",
      "<a href=\"1\"><svg><foreignObject><a href=\"2\">\"x\"</a><div>\"y\"</div></foreignObject><rect></rect></svg></a>",
    ),
    (
      "<a><table><a></table>x",
      "<a><a></a><table></table></a><a>\"x\"</a>",
    ),
    (
      "<table><a>1<p>2</a>3</p></table>",
      "<a>\"1\"</a><p><a>\"2\"</a>\"3\"</p><table></table>",
    ),
    (
      "<table><tr><b>1<div>2</b>3</div><td>c</table>",
      "<b>\"1\"</b><div><b>\"2\"</b>\"3\"</div><table><tbody><tr><td>\"c\"</td></tr></tbody></table>",
    ),
    (
      "<table><form id=f><tr><td>a</td></tr></table><form id=g>b</form>",
      "<table><form id=\"f\"></form><tbody><tr><td>\"a\"</td></tr></tbody></table>\"b\"",
    ),
  ] {
    let doc = Document::fragment(source, "body");
    assert_eq!(tree(&doc, 0), expected, "{source}");
  }
  let doc = Document::fragment("<table><select><option>a<td>b</table>", "body");
  assert_eq!(
    tree(&doc, 0),
    "<select><option>\"a\"</option></select><table><tbody><tr><td>\"b\"</td></tr></tbody></table>"
  );
}

#[test]
fn fragments_follow_their_context() {
  for (context, source, expected) in [
    ("tbody", "<tr><td>a</td></tr>", "<tr><td>\"a\"</td></tr>"),
    ("tr", "<td>a<td>b", "<td>\"a\"</td><td>\"b\"</td>"),
    (
      "table",
      "<tr><td>a",
      "<tbody><tr><td>\"a\"</td></tr></tbody>",
    ),
    (
      "select",
      "<option>a<option>b<div>c</div>",
      "<option>\"a\"</option><option>\"bc\"</option>",
    ),
    ("template", "<tr><td>a</td></tr>", "<tr><td>\"a\"</td></tr>"),
    (
      "template",
      "<td>a</td><td>b",
      "<td>\"a\"</td><td>\"b\"</td>",
    ),
    ("body", "<tr><td>a</td></tr>x", "\"ax\""),
    ("div", "<p>a<p>b", "<p>\"a\"</p><p>\"b\"</p>"),
    ("style", "a>b{}<p>&amp;", "\"a>b{}<p>&amp;\""),
    ("textarea", "<b>&amp;", "\"<b>&\""),
    (
      "svg",
      "<lineargradient gradientunits='x'/><div>y</div>",
      "<linearGradient gradientUnits=\"x\"></linearGradient><div>\"y\"</div>",
    ),
    (
      "html",
      "<head><title>t</title></head><body>b",
      "<head><title>\"t\"</title></head><body>\"b\"</body>",
    ),
    ("ul", "<li>a<li>b", "<li>\"a\"</li><li>\"b\"</li>"),
  ] {
    assert_eq!(
      tree(&Document::fragment(source, context), 0),
      expected,
      "{context}: {source}"
    );
  }
  assert!(Document::fragment("", "style").nodes[0].children.is_empty());
  assert_eq!(
    tree(
      &Document::build("<form id=a><input></form><form id=b>", Some("div"), true),
      0
    ),
    "<input></input><form id=\"b\"></form>"
  );
  assert_eq!(
    Document::fragment("<template><tr><td>x</td></tr></template>", "body")
      .nodes
      .len(),
    5
  );
}

#[test]
fn svg_adjusts_case_and_integration_points() {
  let doc = Document::parse(
    r#"<svg VIEWBOX="0 0 16 16" xlink:href=x><defs><LINEARGRADIENT id="paint" GRADIENTUNITS="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/><foreignObject><div><p>html</div><svg><clippath/></svg></foreignObject><title>a<b>t</b></title><style>.a>b{}</style><![CDATA[<x>]]></svg><p>after"#,
  );
  let svg = find(&doc, "svg");
  assert_eq!(doc.nodes[svg].attrs["viewBox"], "0 0 16 16");
  assert_eq!(doc.nodes[svg].attrs["xlink:href"], "x");
  let gradient = find(&doc, "linearGradient");
  assert_eq!(doc.nodes[gradient].attrs["gradientUnits"], "userSpaceOnUse");
  assert!(doc.nodes[gradient].children.is_empty());
  assert_eq!(parent(&doc, "path"), "svg");
  assert_eq!(parent(&doc, "div"), "foreignObject");
  assert_eq!(parent(&doc, "clipPath"), "svg");
  assert_eq!(parent(&doc, "b"), "title");
  assert_eq!(doc.text(find(&doc, "style")), ".a>b{}");
  assert!(
    doc
      .nodes
      .iter()
      .any(|node| node.tag == "#text" && node.text == "<x>")
  );
  assert_eq!(parent(&doc, "p"), "div");
  let after = doc
    .nodes
    .iter()
    .position(|node| node.text == "after")
    .unwrap();
  assert_eq!(doc.nodes[doc.nodes[after].parent.unwrap()].tag, "p");
  assert_eq!(
    doc.nodes[doc.nodes[doc.nodes[after].parent.unwrap()].parent.unwrap()].tag,
    "body"
  );
  let doc = Document::parse(
    "<svg><style>a&amp;lt;b</style><foreignObject><style>a<b</style></foreignObject></svg>",
  );
  let style = find(&doc, "style");
  assert_eq!(doc.text(style), "a&lt;b");
  assert_eq!(doc.serialize(style), "<style>a&amp;lt;b</style>");
  assert!(
    doc
      .serialize(0)
      .contains("<style>a&amp;lt;b</style><foreignObject><style>a<b</style></foreignObject>")
  );
  assert_eq!(tree(&Document::parse(&doc.serialize(0)), 0), tree(&doc, 0));
  let doc = Document::fragment("<svg><g><div>x</div></g><circle/></svg>", "body");
  assert_eq!(
    tree(&doc, 0),
    "<svg><g></g></svg><div>\"x\"</div><circle></circle>"
  );
}

#[test]
fn html_keeps_comments_and_svg_sources_drop_them() {
  let doc = Document::parse(
    "<p>a<!-- -->b<svg><!--x--><g style='fill:red' x='1' transform='scale(2)'></g></svg></p>",
  );
  assert!(doc.serialize(0).contains(
    "<p>a<!-- -->b<svg><!--x--><g style=\"fill:red\" x=\"1\" transform=\"scale(2)\"></g></svg></p>"
  ));
  let svg = find(&doc, "svg");
  assert_eq!(
    doc.serialize_svg(svg, false, &|node| if doc.nodes[node].tag == "g" {
      vec![
        ("style", "fill:blue".into()),
        ("x", "2".into()),
        ("transform", String::new()),
      ]
    } else {
      Vec::new()
    }),
    "<svg><g style=\"fill:blue\" x=\"2\"></g></svg>"
  );
}

#[test]
fn svg_keeps_case_and_self_closing_siblings() {
  let doc = Document::parse(
    r#"<svg viewBox="0 0 16 16"><defs><linearGradient id="paint" gradientUnits="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/></svg>"#,
  );
  let svg = find(&doc, "svg");
  assert_eq!(doc.nodes[svg].attrs["viewBox"], "0 0 16 16");
  assert_eq!(doc.nodes[svg].children, vec![svg + 1, svg + 3, svg + 4]);
  assert_eq!(doc.nodes[svg + 2].tag, "linearGradient");
  assert_eq!(doc.nodes[svg + 2].attrs["gradientUnits"], "userSpaceOnUse");
  let source = doc.serialize(svg);
  let tree = usvg::Tree::from_str(&source, &Default::default()).unwrap();
  assert_eq!(tree.root().children().len(), 2);
}

#[test]
fn hostile_markup_never_panics() {
  for source in [
    "<",
    "</",
    "<!",
    "<!-",
    "<!--",
    "<?",
    "<a",
    "<a b",
    "<a b=",
    "<a b='",
    "</a",
    "</a b='x",
    "&",
    "&#",
    "&#x",
    "&#xffffffffffffffffff;",
    "<table><caption><table>",
    "<select><select><table>",
    "<svg><foreignObject><svg><desc><table><td><svg><title>",
    "<template><template><tr>",
    "<table><template><td></template></table>",
    "<frameset><frame>",
    "é<b é=é>é</b é>",
    "<plaintext><b>",
    "<textarea>",
    "<title>&am",
    "<style></st",
    "<html><html><head><head><body><body>",
    "<colgroup><col>x</colgroup>",
    "</br></p></li></td></table></select></template></svg>",
  ] {
    for context in [
      None,
      Some("body"),
      Some("tr"),
      Some("svg"),
      Some("select"),
      Some("colgroup"),
      Some("html"),
    ] {
      let doc = match context {
        Some(context) => Document::fragment(source, context),
        None => Document::parse(source),
      };
      for (id, node) in doc.nodes.iter().enumerate() {
        for &child in &node.children {
          assert_eq!(doc.nodes[child].parent, Some(id), "{source}");
        }
      }
    }
  }
  assert_eq!(
    tree(&Document::parse("<plaintext><b>&amp;"), 0),
    "<html><head></head><body><plaintext>\"<b>&amp;\"</plaintext></body></html>"
  );
  assert_eq!(
    tree(&Document::fragment("a\r\nb\rc", "body"), 0),
    "\"a\\nb\\nc\""
  );
}

#[test]
fn packed_entities_match_the_source() {
  assert_eq!(packed!("entities"), entities::ENTITIES);
}
