use super::{Declaration, Declarations, MediaFeature, Rule, Selector, SelectorList, Stylesheet};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, Delimiter, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
    TokenSerializationType, parse_important,
};
use std::rc::Rc;

#[derive(Default)]
struct Rules {
    parent: Option<SelectorList>,
    top_level: bool,
    declarations_only: bool,
}

enum Item {
    Rule(Rule),
    Declaration(Declaration),
}

impl Stylesheet {
    pub(crate) fn parse(source: &str) -> Self {
        let mut input = Parser::new(source);
        input.set_nested_block_limit(128);
        let mut parser = Rules {
            top_level: true,
            ..Rules::default()
        };
        Self(
            StyleSheetParser::new(&mut input, &mut parser)
                .filter_map(|item| match item.ok()? {
                    Item::Rule(rule) => Some(rule),
                    Item::Declaration(_) => None,
                })
                .collect(),
        )
    }
}

impl Declarations {
    pub(crate) fn parse(source: &str) -> Self {
        Self::parse_input(&mut Parser::new(source))
    }

    fn parse_input(input: &mut Parser<'_>) -> Self {
        let mut parser = Rules {
            declarations_only: true,
            ..Rules::default()
        };
        Self(
            RuleBodyParser::new(input, &mut parser)
                .filter_map(|item| match item.ok()? {
                    Item::Declaration(declaration) => Some(declaration),
                    Item::Rule(_) => None,
                })
                .collect(),
        )
    }
}

impl Rules {
    fn body(&mut self, input: &mut Parser<'_>) -> Vec<Rule> {
        let mut rules = Vec::new();
        for item in RuleBodyParser::new(input, self).flatten() {
            match item {
                Item::Rule(rule) => rules.push(rule),
                Item::Declaration(declaration) => {
                    if let Some(Rule::Declarations(values)) = rules.last_mut() {
                        values.0.push(declaration);
                    } else {
                        rules.push(Rule::Declarations(Declarations(vec![declaration])));
                    }
                }
            }
        }
        rules
    }
}

impl<'i> DeclarationParser<'i> for Rules {
    type Declaration = Item;
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _: &ParserState,
    ) -> Result<Item, ParseError<()>> {
        let custom = name.starts_with("--");
        input.skip_whitespace();
        let mut value = String::new();
        let delimiters = if custom {
            Delimiter::Bang
        } else {
            Delimiter::Bang | Delimiter::CurlyBracketBlock
        };
        input.parse_until_before(delimiters, |input| source(input, &mut value))?;
        value.truncate(value.trim_end().len());
        if value.is_empty() && !custom {
            return Err(ParseError::unexpected_token());
        }
        if input.try_parse(parse_important).is_ok() {
            value.push_str(" !important");
        }
        input.expect_exhausted()?;
        Ok(Item::Declaration(Declaration::Property(
            if custom {
                name.to_string()
            } else if name.eq_ignore_ascii_case("-webkit-user-select") {
                "user-select".into()
            } else {
                name.to_ascii_lowercase()
            },
            value,
        )))
    }
}

impl<'i> QualifiedRuleParser<'i> for Rules {
    type Prelude = SelectorList;
    type QualifiedRule = Item;
    type Error = ();

    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<SelectorList, ParseError<()>> {
        let mut value = String::new();
        source(input, &mut value)?;
        let selectors = Selector::parse(&value, self.parent.as_ref());
        if selectors.is_empty() {
            return Err(ParseError::unexpected_token());
        }
        Ok(Rc::new(selectors))
    }

    fn parse_block(
        &mut self,
        selectors: SelectorList,
        _: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Item, ParseError<()>> {
        let children = Rules {
            parent: Some(selectors.clone()),
            ..Rules::default()
        }
        .body(input);
        Ok(Item::Rule(Rule::Style(selectors, children)))
    }
}

impl<'i> AtRuleParser<'i> for Rules {
    type Prelude = Item;
    type AtRule = Item;
    type Error = ();

    fn parse_prelude(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<Item, ParseError<()>> {
        if name.eq_ignore_ascii_case("import") && self.top_level {
            if input.expect_url_or_string()?.as_ref() != "tailwindcss" {
                return Err(ParseError::unexpected_token());
            }
            return Ok(Item::Rule(Rule::Tailwind));
        }
        if name.eq_ignore_ascii_case("keyframes") && !self.declarations_only {
            let name = input.expect_ident_or_string()?.to_string();
            input.expect_exhausted()?;
            return Ok(Item::Rule(Rule::Keyframes(name, Vec::new())));
        }
        if name.eq_ignore_ascii_case("theme") && self.top_level {
            return Ok(Item::Rule(Rule::Theme(Declarations::default())));
        }
        if name.eq_ignore_ascii_case("apply") && self.parse_declarations() {
            let mut value = String::new();
            source(input, &mut value)?;
            return Ok(Item::Declaration(Declaration::Apply(
                super::components(&value, ' ')
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            )));
        }
        if !self.declarations_only
            && (name.eq_ignore_ascii_case("layer") || name.eq_ignore_ascii_case("supports"))
        {
            let mut value = String::new();
            source(input, &mut value)?;
            return Ok(Item::Rule(if name.eq_ignore_ascii_case("layer") {
                Rule::Layer(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    Vec::new(),
                )
            } else {
                Rule::Supports(value, Vec::new())
            }));
        }
        if name.eq_ignore_ascii_case("media") && !self.declarations_only {
            let queries = input.parse_comma_separated_ignoring_errors(|input| {
                let mut features = Vec::new();
                if let Ok(medium) = input.try_parse(Parser::expect_ident_cloned) {
                    if !medium.eq_ignore_ascii_case("screen") && !medium.eq_ignore_ascii_case("all")
                    {
                        return Err(ParseError::<()>::unexpected_token());
                    }
                    if input.is_exhausted() {
                        return Ok(features);
                    }
                    input.expect_ident_matching("and")?;
                }
                loop {
                    input.expect_parenthesis_block()?;
                    features.push(input.parse_nested_block(|input| {
                        let feature = match input.expect_ident()?.to_ascii_lowercase().as_str() {
                            "min-width" => MediaFeature::MinWidth,
                            "max-width" => MediaFeature::MaxWidth,
                            "min-height" => MediaFeature::MinHeight,
                            "max-height" => MediaFeature::MaxHeight,
                            "pointer" => MediaFeature::Pointer,
                            "width" => MediaFeature::Width,
                            "height" => MediaFeature::Height,
                            "prefers-reduced-motion" => MediaFeature::ReducedMotion,
                            "prefers-color-scheme" => MediaFeature::ColorScheme,
                            "prefers-contrast" => MediaFeature::Contrast,
                            "inverted-colors" => MediaFeature::InvertedColors,
                            "prefers-reduced-transparency" => MediaFeature::ReducedTransparency,
                            "forced-colors" => MediaFeature::ForcedColors,
                            _ => return Err(ParseError::unexpected_token()),
                        };
                        let feature = if input.try_parse(Parser::expect_colon).is_ok() {
                            feature
                        } else {
                            let Token::Delim(operator) = *input.next()? else {
                                return Err(ParseError::unexpected_token());
                            };
                            let inclusive =
                                input.try_parse(|input| input.expect_delim('=')).is_ok();
                            match (feature, operator, inclusive) {
                                (MediaFeature::Width, '>', true) => MediaFeature::MinWidth,
                                (MediaFeature::Width, '<', true) => MediaFeature::MaxWidth,
                                (MediaFeature::Height, '>', true) => MediaFeature::MinHeight,
                                (MediaFeature::Height, '<', true) => MediaFeature::MaxHeight,
                                (MediaFeature::Width | MediaFeature::Height, '=', false) => feature,
                                _ => return Err(ParseError::unexpected_token()),
                            }
                        };
                        input.skip_whitespace();
                        let mut value = String::new();
                        source(input, &mut value)?;
                        value.truncate(value.trim_end().len());
                        Ok((feature, value))
                    })?);
                    if input.is_exhausted() {
                        return Ok(features);
                    }
                    input.expect_ident_matching("and")?;
                }
            });
            return Ok(Item::Rule(Rule::Media(queries, Vec::new())));
        }
        Err(ParseError::unexpected_token())
    }

    fn rule_without_block(&mut self, item: Item, _: &ParserState) -> Result<Item, ()> {
        match item {
            Item::Rule(Rule::Tailwind) | Item::Rule(Rule::Layer(_, _)) | Item::Declaration(_) => {
                Ok(item)
            }
            _ => Err(()),
        }
    }

    fn parse_block(
        &mut self,
        item: Item,
        _: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Item, ParseError<()>> {
        Ok(Item::Rule(match item {
            Item::Rule(Rule::Layer(names, _)) => Rule::Layer(
                names,
                Rules {
                    parent: self.parent.clone(),
                    ..Rules::default()
                }
                .body(input),
            ),
            Item::Rule(Rule::Supports(condition, _)) => Rule::Supports(
                condition,
                Rules {
                    parent: self.parent.clone(),
                    ..Rules::default()
                }
                .body(input),
            ),
            Item::Rule(Rule::Theme(_)) => Rule::Theme(Declarations::parse_input(input)),
            Item::Rule(Rule::Keyframes(name, _)) => Rule::Keyframes(
                name,
                StyleSheetParser::new(input, &mut Keyframes)
                    .flatten()
                    .flatten()
                    .collect(),
            ),
            Item::Rule(Rule::Media(queries, _)) => Rule::Media(
                queries,
                Rules {
                    parent: self.parent.clone(),
                    ..Rules::default()
                }
                .body(input),
            ),
            _ => return Err(ParseError::unexpected_token()),
        }))
    }
}

impl<'i> RuleBodyItemParser<'i, Item, ()> for Rules {
    fn parse_declarations(&self) -> bool {
        self.declarations_only || self.parent.is_some()
    }

    fn parse_qualified(&self) -> bool {
        !self.declarations_only
    }
}

fn source(input: &mut Parser<'_>, output: &mut String) -> Result<(), ParseError<()>> {
    let mut previous = TokenSerializationType::Nothing;
    loop {
        let start = input.position();
        let Ok(token) = input.next_including_whitespace_and_comments() else {
            return Ok(());
        };
        if matches!(token, Token::Comment(_)) {
            continue;
        }
        if token.is_parse_error() {
            return Err(ParseError::unexpected_token());
        }
        let current = token.serialization_type();
        let close = match token {
            Token::Function(_) | Token::ParenthesisBlock => Some(')'),
            Token::SquareBracketBlock => Some(']'),
            Token::CurlyBracketBlock => Some('}'),
            _ => None,
        };
        if previous.needs_separator_when_before(current) {
            output.push_str("/**/");
        }
        output.push_str(input.slice_from(start));
        previous = current;
        if let Some(close) = close {
            input.parse_nested_block(|input| source(input, output))?;
            output.push(close);
            previous = TokenSerializationType::Other;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FontUnits {
    pub em: f32,
    pub ch: f32,
    pub ex: f32,
    pub cap: f32,
    pub ic: f32,
    pub lh: f32,
}

impl FontUnits {
    pub fn new(em: f32) -> Self {
        Self {
            em,
            ch: em * 0.5,
            ex: em * 0.5,
            cap: em * 0.8,
            ic: em,
            lh: em * 1.2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Lengths {
    pub font: FontUnits,
    pub root: FontUnits,
    pub viewport: [f32; 2],
    pub container: Option<[f32; 2]>,
}

impl Lengths {
    pub fn new(font: f32, vw: f32, vh: f32) -> Self {
        Self {
            font: FontUnits::new(font),
            root: FontUnits::new(16.),
            viewport: [vw, vh],
            container: None,
        }
    }

    pub fn parse(&self, source: &str, base: f32) -> Option<(f32, bool)> {
        let mut parser = Math {
            rest: source.trim(),
            units: self,
            base,
            depth: 0,
        };
        let (value, dimension) = parser.atom()?;
        let value = value as f32;
        (parser.rest.trim().is_empty() && value.is_finite()).then_some((value, dimension))
    }

    pub fn resolve(&self, source: &str, base: f32) -> Option<f32> {
        let (value, dimensional) = self.parse(source, base)?;
        (dimensional || value == 0.).then_some(value)
    }
}

struct Math<'a> {
    rest: &'a str,
    units: &'a Lengths,
    base: f32,
    depth: usize,
}

impl Math<'_> {
    fn expression(&mut self, minimum: u8) -> Option<(f64, bool)> {
        self.depth += 1;
        if self.depth > 64 {
            return None;
        }
        let mut left = self.atom()?;
        loop {
            let rest = self.rest.trim_start();
            let operator = rest.as_bytes().first().copied().unwrap_or(0);
            let precedence = match operator {
                b'+' | b'-' => 1,
                b'*' | b'/' => 2,
                _ => break,
            };
            if precedence < minimum {
                break;
            }
            if precedence == 1
                && (rest.len() == self.rest.len()
                    || !rest.as_bytes().get(1).is_some_and(u8::is_ascii_whitespace))
            {
                return None;
            }
            self.rest = &rest[1..];
            let right = self.expression(precedence + 1)?;
            left = match operator {
                b'+' if left.1 == right.1 => (left.0 + right.0, left.1),
                b'-' if left.1 == right.1 => (left.0 - right.0, left.1),
                b'*' if !left.1 || !right.1 => (left.0 * right.0, left.1 || right.1),
                b'/' if right.0 != 0. && (!right.1 || left.1) => {
                    (left.0 / right.0, left.1 && !right.1)
                }
                _ => return None,
            };
            if !left.0.is_finite() {
                return None;
            }
        }
        self.depth -= 1;
        Some(left)
    }

    fn atom(&mut self) -> Option<(f64, bool)> {
        self.rest = self.rest.trim_start();
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(self.rest.len());
        if end > 0 || self.rest.starts_with('(') {
            let name = &self.rest[..end];
            if name.is_empty() && self.depth == 0 {
                return None;
            }
            if !["", "calc", "min", "max", "clamp", "round"]
                .iter()
                .any(|s| name.eq_ignore_ascii_case(s))
            {
                return None;
            }
            self.rest = self.rest[end..].strip_prefix('(')?;
            let mut values = [self.expression(0)?; 3];
            let mut count = 1;
            loop {
                self.rest = self.rest.trim_start();
                if let Some(rest) = self.rest.strip_prefix(')') {
                    self.rest = rest;
                    break;
                }
                self.rest = self.rest.strip_prefix(',')?;
                let value = self.expression(0)?;
                if value.1 != values[0].1 {
                    return None;
                }
                if name.eq_ignore_ascii_case("min") {
                    values[0].0 = values[0].0.min(value.0);
                } else if name.eq_ignore_ascii_case("max") {
                    values[0].0 = values[0].0.max(value.0);
                } else if name.eq_ignore_ascii_case("clamp") && count < 3 {
                    values[count] = value;
                } else {
                    return None;
                }
                count += 1;
            }
            if name.eq_ignore_ascii_case("clamp") {
                if count != 3 {
                    return None;
                }
                values[0].0 = values[0].0.max(values[1].0.min(values[2].0));
            }
            if name.eq_ignore_ascii_case("round") {
                values[0].0 = values[0].0.round();
            }
            return Some(values[0]);
        }
        let bytes = self.rest.as_bytes();
        let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let mut digits = end - start;
        if bytes.get(end) == Some(&b'.') {
            end += 1;
            let start = end;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if start == end {
                return None;
            }
            digits += end - start;
        }
        if digits == 0 {
            return None;
        }
        if bytes.get(end).is_some_and(|b| matches!(b, b'e' | b'E')) {
            let mut exponent = end + 1;
            if bytes
                .get(exponent)
                .is_some_and(|b| matches!(b, b'+' | b'-'))
            {
                exponent += 1;
            }
            if bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
                end = exponent + 1;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
            }
        }
        let value = self.rest[..end].parse::<f64>().ok()?;
        self.rest = &self.rest[end..];
        let end = if self.rest.starts_with('%') {
            1
        } else {
            self.rest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(self.rest.len())
        };
        let unit = &self.rest[..end];
        self.rest = &self.rest[end..];
        let lower;
        let unit = if unit.bytes().any(|c| c.is_ascii_uppercase()) {
            lower = unit.to_ascii_lowercase();
            lower.as_str()
        } else {
            unit
        };
        let font = self.units.font;
        let root = self.units.root;
        let [vw, vh] = self.units.viewport;
        let scale = match unit {
            "" | "px" => 1.,
            "%" => self.base / 100.,
            "em" => font.em,
            "rem" => root.em,
            "ch" => font.ch,
            "rch" => root.ch,
            "ex" => font.ex,
            "rex" => root.ex,
            "cap" => font.cap,
            "rcap" => root.cap,
            "ic" => font.ic,
            "ric" => root.ic,
            "lh" => font.lh,
            "rlh" => root.lh,
            "vw" | "svw" | "lvw" | "dvw" | "vi" | "svi" | "lvi" | "dvi" => vw / 100.,
            "vh" | "svh" | "lvh" | "dvh" | "vb" | "svb" | "lvb" | "dvb" => vh / 100.,
            "cqw" | "cqi" => self.units.container.unwrap_or(self.units.viewport)[0] / 100.,
            "cqh" | "cqb" => self.units.container.unwrap_or(self.units.viewport)[1] / 100.,
            "vmin" | "svmin" | "lvmin" | "dvmin" => vw.min(vh) / 100.,
            "vmax" | "svmax" | "lvmax" | "dvmax" => vw.max(vh) / 100.,
            "in" => 96.,
            "cm" => 96. / 2.54,
            "mm" => 96. / 25.4,
            "q" => 96. / 101.6,
            "pt" => 96. / 72.,
            "pc" => 16.,
            _ => return None,
        };
        let value = value * f64::from(scale);
        value.is_finite().then_some((value, !unit.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_font_and_absolute_units() {
        let units = Lengths {
            container: None,
            font: FontUnits {
                em: 20.,
                ch: 12.,
                ex: 9.,
                cap: 14.,
                ic: 20.,
                lh: 30.,
            },
            root: FontUnits {
                em: 24.,
                ch: 14.,
                ex: 11.,
                cap: 18.,
                ic: 24.,
                lh: 36.,
            },
            viewport: [1200., 800.],
        };
        for (source, expected) in [
            ("2em", 40.),
            ("2rem", 48.),
            ("2ch", 24.),
            ("2rch", 28.),
            ("2ex", 18.),
            ("2rex", 22.),
            ("2cap", 28.),
            ("2rcap", 36.),
            ("2ic", 40.),
            ("2ric", 48.),
            ("2lh", 60.),
            ("2rlh", 72.),
            ("10vw", 120.),
            ("10vh", 80.),
            ("10vmin", 80.),
            ("10vmax", 120.),
            ("10vi", 120.),
            ("10vb", 80.),
            ("25%", 100.),
            ("1in", 96.),
            ("2.54cm", 96.),
            ("25.4mm", 96.),
            ("101.6Q", 96.),
            ("72pt", 96.),
            ("6pc", 96.),
            ("1e2px", 100.),
            (".5EM", 10.),
        ] {
            let actual = units.resolve(source, 400.).unwrap();
            assert!(
                (actual - expected).abs() < 0.001,
                "{source}: {actual} != {expected}"
            );
        }
        for prefix in ["s", "l", "d"] {
            for (unit, expected) in [
                ("vw", 120.),
                ("vh", 80.),
                ("vi", 120.),
                ("vb", 80.),
                ("vmin", 80.),
                ("vmax", 120.),
            ] {
                assert_eq!(
                    units.resolve(&format!("10{prefix}{unit}"), 0.),
                    Some(expected)
                );
            }
        }
        assert_eq!(
            Lengths::new(16., 800., 1200.).resolve("10vmin", 0.),
            Some(80.)
        );
    }

    #[test]
    fn math_precedence_dimensions_and_comparison_functions() {
        let units = Lengths::new(20., 1200., 800.);
        for (source, expected) in [
            ("calc(100% - 2rem)", 368.),
            ("calc(2 * (3px + 4px) / 2)", 7.),
            ("calc(2px + 3px * 4 - 5px)", 9.),
            ("calc(2px - -3px)", 5.),
            ("calc(-2 * 1rem)", -32.),
            ("calc(calc(10vw - 2px) / 2)", 59.),
            ("calc(1e-2 * 100px)", 1.),
            ("min(50%, 300px, 40ch)", 200.),
            ("max(50%, 300px)", 300.),
            ("clamp(10px, 2vw, 40px)", 24.),
            ("clamp(40px, 10px, 20px)", 40.),
            ("calc(max(2em, 10px) + min(1rem, 20px))", 56.),
            ("clamp(1rem, calc(100% / 4), 20vw)", 100.),
            ("CALC(1PX + 2PX)", 3.),
        ] {
            assert_eq!(units.resolve(source, 400.), Some(expected), "{source}");
        }
        assert_eq!(units.parse("calc(6px / 2px)", 0.), Some((3., false)));
        assert_eq!(units.parse("calc(1 + .5)", 0.), Some((1.5, false)));
    }

    #[test]
    fn invalid_math_does_not_produce_layout_values() {
        let units = Lengths::new(16., 800., 600.);
        for source in [
            "",
            "NaN",
            "inf",
            "1e999px",
            "1e40px",
            "5unknown",
            "2 px",
            "1px garbage",
            "calc()",
            "calc(2px +)",
            "calc(1px+2px)",
            "calc(1px +2px)",
            "calc(1px+ 2px)",
            "calc(1px + 2)",
            "calc(2px * 3px)",
            "calc(2 / 1px)",
            "calc(2px / 0)",
            "calc(2px",
            "calc(2px))",
            "calc(2px, 3px)",
            "min()",
            "max(2px,)",
            "min(2px, 3)",
            "clamp(1px, 2px)",
            "clamp(1px, 2px, 3px, 4px)",
            "unknown(1px)",
            "1px + 2px",
            "calc(1 + 2)",
            "calc(6px / 2px)",
            "1.px",
            "(1px)",
            "calc(1e300 * 1e300)",
        ] {
            assert_eq!(units.resolve(source, 100.), None, "{source}");
        }
        assert_eq!(
            units.resolve(
                &format!("{}1px{}", "calc(".repeat(100), ")".repeat(100)),
                0.
            ),
            None
        );
    }
}

struct Keyframes;
impl<'i> AtRuleParser<'i> for Keyframes {
    type Prelude = ();
    type AtRule = Vec<(f32, Declarations)>;
    type Error = ();
}
impl<'i> QualifiedRuleParser<'i> for Keyframes {
    type Prelude = Vec<f32>;
    type QualifiedRule = Vec<(f32, Declarations)>;
    type Error = ();
    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<Vec<f32>, ParseError<()>> {
        input.parse_comma_separated(|input| match input.next()? {
            Token::Ident(name) if name.eq_ignore_ascii_case("from") => Ok(0.),
            Token::Ident(name) if name.eq_ignore_ascii_case("to") => Ok(1.),
            Token::Percentage { unit_value, .. } if (0. ..=1.).contains(unit_value) => {
                Ok(*unit_value)
            }
            _ => Err(ParseError::unexpected_token()),
        })
    }
    fn parse_block(
        &mut self,
        offsets: Vec<f32>,
        _: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Vec<(f32, Declarations)>, ParseError<()>> {
        let values = Declarations::parse_input(input);
        Ok(offsets
            .into_iter()
            .map(|offset| (offset, values.clone()))
            .collect())
    }
}
