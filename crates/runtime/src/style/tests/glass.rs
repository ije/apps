use super::*;

#[test]
fn glass_variants_follow_native_support_and_cached_states() {
  let supported = renderer::supports_glass();
  let mut doc = Document::parse(
    "<style>@import 'tailwindcss';</style><button class='bg-glass not-supports-bg-glass:bg-[#123456] supports-bg-glass:w-[31px] not-supports-bg-glass:w-[47px] h-[11px] md:supports-bg-glass:hover:h-[33px] md:not-supports-bg-glass:hover:h-[49px]'></button>",
  );
  let button = element(&doc, "button");
  let mut sheets = HashMap::new();
  let mut cache = StyleCache::default();
  for (width, hovered) in [(800., false), (800., true), (600., true), (800., false)] {
    doc.nodes[button].state = if hovered {
      crate::css::selector::HOVER
    } else {
      0
    };
    let cached = compute_styles(&doc, width, 600., None, &mut sheets, &mut cache, None);
    let fresh = compute_styles(
      &doc,
      width,
      600.,
      None,
      &mut sheets,
      &mut StyleCache::default(),
      None,
    );
    assert_eq!(cached.values, fresh.values);
    let style = &cached.values[button];
    assert_eq!(
      style.get(Prop::BACKGROUND_COLOR),
      Some(if supported { "transparent" } else { "#123456" })
    );
    assert_eq!(
      style.get(Prop::WIDTH),
      Some(if supported { "31px" } else { "47px" })
    );
    assert_eq!(
      style.get(Prop::HEIGHT),
      Some(if hovered && width >= 768. {
        if supported { "33px" } else { "49px" }
      } else {
        "11px"
      })
    );
  }
}

#[test]
fn glass_capability_remains_a_runtime_condition_in_bundles() {
  let sheet = Stylesheet::parse(
    "@media (-macos-glass){div{color:#112233}}@media not (-macos-glass){div{color:#445566}}",
  );
  let bytes = postcard::to_allocvec(&sheet).unwrap();
  let sheet: Stylesheet = postcard::from_bytes(&bytes).unwrap();
  assert!(
    matches!(&*sheet.0[0], Rule::Media(queries, _) if matches!(queries.as_slice(), [MediaQuery::Feature(name, op, value)] if name == "-macos-glass" && op.is_empty() && value.is_empty()))
  );
  let mut doc = Document::parse("<style></style><div></div>");
  let style = element(&doc, "style");
  let div = element(&doc, "div");
  doc.nodes[style].stylesheet = Some(Rc::new(sheet));
  let computed = compute_styles(
    &doc,
    800.,
    600.,
    None,
    &mut HashMap::new(),
    &mut StyleCache::default(),
    None,
  );
  assert_eq!(
    computed.values[div].get(Prop::COLOR),
    Some(if renderer::supports_glass() {
      "#112233"
    } else {
      "#445566"
    })
  );
}

#[test]
fn glass_properties_are_known_declarations() {
  for (name, value, prop) in [
    ("-macos-glass", "regular", Prop::MACOS_GLASS),
    (
      "-macos-glass-interactive",
      "true",
      Prop::MACOS_GLASS_INTERACTIVE,
    ),
    (
      "-macos-glass-tint-color",
      "#123456",
      Prop::MACOS_GLASS_TINT_COLOR,
    ),
  ] {
    assert_eq!(Prop::find(name), Some(prop));
    assert_eq!(supports(name, value), Some(true));
    assert_eq!(
      declare(&format!("{name}:{value}")),
      vec![[name.to_owned(), value.to_owned(), String::new()]]
    );
  }
  assert!(declare("unknown-property: red").is_empty());
}

#[test]
fn glass_tints_follow_theme_changes_and_color_utilities() {
  let mut doc = Document::parse(
    r#"<style>
            @import 'tailwindcss';
            @theme { --color-brand: #123456; }
            @theme inline { --color-accent: var(--accent); }
            .theme { --accent: #112233; color: #fedcba; }
            .theme.dark { --color-brand: #abcdef; --accent: #654321; }
            #applied { @apply bg-glass-brand/50; }
            #reset { @apply bg-glass-brand/50 bg-white border-2 appearance-auto; }
        </style><section class=theme>
            <button id=brand class='bg-glass-brand'></button>
            <button id=alpha class='bg-glass-brand/50'></button>
            <button id=inline class='bg-glass-accent'></button>
            <button id=current class='bg-glass-current/50'></button>
            <button id=applied></button>
            <button id=reset></button>
            <button id=explicit class='bg-glass-brand bg-white border-2 appearance-auto'></button>
            <button id=variant class='bg-glass-[#123abc] hover:bg-glass-brand'></button>
            <button id=hoveronly class='hover:bg-glass-brand bg-glass-interactive'></button>
            <button id=legacy class='bg-glass-blue'></button>
        </section>"#,
  );
  let host = element(&doc, "section");
  let variant = element(&doc, "#variant");
  let mut sheets = HashMap::new();
  let mut cache = StyleCache::default();
  for dark in [false, true, false] {
    doc.nodes[host]
      .attrs
      .set("class", if dark { "theme dark" } else { "theme" });
    doc.nodes[variant].state = if dark { crate::css::selector::HOVER } else { 0 };
    let hoveronly = element(&doc, "#hoveronly");
    doc.nodes[hoveronly].state = if dark { crate::css::selector::HOVER } else { 0 };
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
    let brand = if dark { 0xabcdef } else { 0x123456 };
    for (selector, expected) in [
      ("#brand", (brand, 255)),
      ("#alpha", (brand, 128)),
      ("#inline", (if dark { 0x654321 } else { 0x112233 }, 255)),
      ("#current", (0xfedcba, 128)),
      ("#applied", (brand, 128)),
      ("#variant", (if dark { brand } else { 0x123abc }, 255)),
    ] {
      let values = &cached.values[element(&doc, selector)];
      assert_eq!(
        values.get(Prop::MACOS_GLASS_TINT_COLOR).and_then(color),
        Some(expected),
        "{selector}, dark={dark}"
      );
      assert_eq!(
        values.get(Prop::MACOS_GLASS),
        cfg!(target_os = "macos").then_some("regular")
      );
      if cfg!(target_os = "macos") {
        assert_eq!(values.get(Prop::BACKGROUND_COLOR), Some("transparent"));
        assert_eq!(values.get(Prop::APPEARANCE), Some("none"));
        assert_eq!(values.get(Prop::BORDER_TOP_WIDTH), Some("0"));
      }
    }
    assert!(!cached.values[element(&doc, "#legacy")].contains(Prop::MACOS_GLASS_TINT_COLOR));
    assert!(!cached.values[element(&doc, "#legacy")].contains(Prop::MACOS_GLASS));
    for selector in ["#reset", "#explicit"] {
      let values = &cached.values[element(&doc, selector)];
      assert_eq!(values.get(Prop::BACKGROUND_COLOR), Some("#fff"));
      assert_eq!(values.get(Prop::BORDER_TOP_WIDTH), Some("2px"));
      assert_eq!(values.get(Prop::APPEARANCE), Some("auto"));
    }
    assert_eq!(
      cached.values[hoveronly].get(Prop::MACOS_GLASS),
      (cfg!(target_os = "macos") && dark).then_some("regular")
    );
  }
}

#[test]
fn glass_classes_are_builtin_and_independent_of_tailwind() {
  for imports in [
    "",
    "@import 'tailwindcss';",
    "span{color:red}</style><style>@import 'tailwindcss';",
    "@import 'tailwindcss';</style><style>span{color:red}",
  ] {
    let doc = Document::parse(&format!(
      "<style>{imports}</style><button class='bg-glass bg-glass-interactive bg-glass-blue-500'><span>Save</span></button><div class=bg-glass-clear></div><button id=utility class='bg-glass bg-white rounded-lg'></button><button id=tinted class='bg-glass-blue-500 bg-glass-interactive'></button><button id=modifier class=bg-glass-interactive></button>"
    ));
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
    let active = cfg!(target_os = "macos");
    assert_eq!(
      values[element(&doc, "#tinted")].get(Prop::MACOS_GLASS),
      (active && imports.contains("tailwindcss")).then_some("regular")
    );
    assert!(!values[element(&doc, "#modifier")].contains(Prop::MACOS_GLASS));
    let button = &values[element(&doc, "button")];
    assert_eq!(button.get(Prop::MACOS_GLASS), active.then_some("regular"));
    assert_eq!(
      button.get(Prop::MACOS_GLASS_INTERACTIVE),
      active.then_some("true")
    );
    assert_eq!(
      button.get(Prop::MACOS_GLASS_TINT_COLOR).and_then(color),
      imports
        .contains("tailwindcss")
        .then(|| color("oklch(62.3% 0.214 259.815)").unwrap())
    );
    assert_eq!(
      values[element(&doc, "div")].get(Prop::MACOS_GLASS),
      active.then_some("clear")
    );
    for prop in [
      Prop::MACOS_GLASS,
      Prop::MACOS_GLASS_INTERACTIVE,
      Prop::MACOS_GLASS_TINT_COLOR,
    ] {
      assert!(!values[element(&doc, "span")].contains(prop));
    }
    if active {
      assert_eq!(button.get(Prop::APPEARANCE), Some("none"));
      assert_eq!(button.get(Prop::BACKGROUND_COLOR), Some("transparent"));
      assert_eq!(button.get(Prop::BORDER_TOP_WIDTH), Some("0"));
      assert_eq!(button.get(Prop::BORDER_TOP_LEFT_RADIUS).unwrap_or("0"), "0");
    }
    if imports.contains("tailwindcss") {
      let utility = &values[element(&doc, "#utility")];
      assert_eq!(utility.get(Prop::BACKGROUND_COLOR), Some("#fff"));
      assert_eq!(utility.get(Prop::BORDER_TOP_LEFT_RADIUS), Some("0.5rem"));
    }
  }
}

#[test]
fn glass_classes_follow_cascade_and_cached_changes() {
  let mut doc = Document::parse(
    "<style>@import 'tailwindcss';@layer components{.component{appearance:auto;background-color:red}}#override{-macos-glass:clear;-macos-glass-tint-color:var(--tint);background-color:red}</style><button id=changing></button><button id=override class='bg-glass bg-glass-blue-500' style='--tint:#123456'></button><button id=utility class='bg-glass rounded-lg bg-white'></button><button id=component class='bg-glass component'></button>",
  );
  let changing = element(&doc, "#changing");
  let overridden = element(&doc, "#override");
  let utility = element(&doc, "#utility");
  let mut sheets = HashMap::new();
  let mut cache = StyleCache::default();
  for (class, effect, tint) in [
    (
      "bg-glass bg-glass-interactive bg-glass-blue-500",
      Some("regular"),
      "#123456",
    ),
    ("bg-glass-clear", Some("clear"), "#abcdef"),
    (
      "bg-glass-blue-500 bg-glass-interactive",
      Some("regular"),
      "#112233",
    ),
    (
      "bg-glass-clear bg-glass-blue-500 bg-glass-interactive",
      Some("clear"),
      "#223344",
    ),
    ("bg-glass-interactive", None, "#334455"),
    ("glass glass-clear glass-tint-blue-500", None, "#654321"),
    ("", None, "#fedcba"),
  ] {
    doc.nodes[changing].attrs.set("class", class);
    doc.nodes[overridden]
      .attrs
      .set("style", &format!("--tint:{tint}"));
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
    assert_eq!(
      cached.values[changing].get(Prop::MACOS_GLASS),
      effect.filter(|_| cfg!(target_os = "macos"))
    );
    assert_eq!(
      cached.values[changing].get(Prop::MACOS_GLASS_INTERACTIVE),
      (cfg!(target_os = "macos")
        && class
          .split_ascii_whitespace()
          .any(|class| class == "bg-glass-interactive"))
      .then_some("true")
    );
    assert_eq!(
      cached.values[overridden].get(Prop::MACOS_GLASS),
      Some("clear")
    );
    assert_eq!(
      cached.values[overridden].get(Prop::MACOS_GLASS_TINT_COLOR),
      Some(tint)
    );
    assert_eq!(
      cached.values[overridden].get(Prop::BACKGROUND_COLOR),
      Some("red")
    );
    assert_eq!(
      cached.values[utility].get(Prop::BACKGROUND_COLOR),
      Some("#fff")
    );
    assert_eq!(
      cached.values[utility].get(Prop::BORDER_TOP_LEFT_RADIUS),
      Some("0.5rem")
    );
    let component = &cached.values[element(&doc, "#component")];
    assert_eq!(component.get(Prop::BACKGROUND_COLOR), Some("red"));
    assert_eq!(component.get(Prop::APPEARANCE), Some("auto"));
  }
}

#[test]
fn builtin_glass_survives_cssom_edits_and_serialization() {
  let source = "div{-macos-glass:clear;-macos-glass-interactive:false}";
  let sheet = Stylesheet::parse(source);
  let bytes = postcard::to_allocvec(&sheet).unwrap();
  let mut sheet: Stylesheet = postcard::from_bytes(&bytes).unwrap();
  sheet.bind_parents().unwrap();
  let mut doc = Document::parse("<style></style><div class='bg-glass bg-glass-interactive'></div>");
  let style = element(&doc, "style");
  let glass = element(&doc, "div");
  doc.nodes[style].stylesheet = Some(Rc::new(sheet.clone()));
  let mut sheets = HashMap::new();
  let mut cache = StyleCache::default();
  let before = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
  assert_eq!(before.values[glass].get(Prop::MACOS_GLASS), Some("clear"));
  assert_eq!(
    before.values[glass].get(Prop::MACOS_GLASS_INTERACTIVE),
    Some("false")
  );
  sheet.edit(&[0], Edit::Delete).unwrap();
  doc.nodes[style].stylesheet = Some(Rc::new(sheet));
  let after = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
  assert_eq!(
    after.values[glass].get(Prop::MACOS_GLASS),
    cfg!(target_os = "macos").then_some("regular")
  );
  assert_eq!(
    after.values[glass].get(Prop::MACOS_GLASS_INTERACTIVE),
    cfg!(target_os = "macos").then_some("true")
  );
}
