use super::{Declaration, Declarations, MediaQuery, Rule, Selector, SelectorList, Stylesheet};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, Delimiter, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
    TokenSerializationType, parse_important,
};
use std::rc::Rc;

thread_local! {
    pub(crate) static DROPPED: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
}

#[derive(Default)]
struct Rules {
    parent: Option<SelectorList>,
    top_level: bool,
    declarations_only: bool,
    theme: bool,
    scope: bool,
    suffix: String,
}

enum Item {
    Rule(Rule),
    Scope(String, Option<String>),
    Starting,
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
                    Item::Rule(rule) => Some(Rc::new(rule)),
                    _ => None,
                })
                .collect(),
        )
    }
}

impl Rule {
    pub(crate) fn parse_nested(source: &str, parent: &SelectorList) -> Vec<Rule> {
        let mut input = Parser::new(source);
        input.set_nested_block_limit(128);
        Rules {
            parent: Some(parent.clone()),
            ..Rules::default()
        }
        .body(&mut input)
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
                    _ => None,
                })
                .collect(),
        )
    }
}

impl Rules {
    fn body(&mut self, input: &mut Parser<'_>) -> Vec<Rule> {
        let theme = self.theme;
        let mut rules = Vec::new();
        for item in RuleBodyParser::new(input, self) {
            let declaration = match item {
                Ok(Item::Rule(rule)) => {
                    rules.push(rule);
                    continue;
                }
                Ok(Item::Declaration(declaration)) => declaration,
                Ok(Item::Scope(..) | Item::Starting) => continue,
                // `--color-*: initial` is no declaration for the tokenizer, so it comes as an error.
                Err((_, source, _)) if theme => match source.split_once(':') {
                    Some((name, value))
                        if name.trim().starts_with("--")
                            && name.trim().ends_with("-*")
                            && value.trim().trim_end_matches(';').trim() == "initial" =>
                    {
                        Declaration::Property(name.trim().into(), "initial".into())
                    }
                    _ => continue,
                },
                Err(_) => continue,
            };
            if let Some(Rule::Declarations(values)) = rules.last_mut() {
                values.0.push(declaration);
            } else {
                rules.push(Rule::Declarations(Declarations(vec![declaration])));
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
        if value.contains("env(") {
            value = environment(&value);
        }
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
            } else if name.eq_ignore_ascii_case("-webkit-app-region") {
                "app-region".into()
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
        if self.scope {
            value = value.replace(":scope", "&");
        }
        if !self.suffix.is_empty() {
            let mut depth = 0;
            let mut parts = vec![String::new()];
            for c in value.chars() {
                depth += i32::from(matches!(c, '(' | '[')) - i32::from(matches!(c, ')' | ']'));
                match parts.last_mut() {
                    Some(_) if c == ',' && depth == 0 => parts.push(String::new()),
                    Some(part) => part.push(c),
                    None => {}
                }
            }
            value = parts
                .iter()
                .map(|part| format!("{}{}", part.trim(), self.suffix))
                .collect::<Vec<_>>()
                .join(",");
        }
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
            let url = input.expect_url_or_string()?.clone();
            let preflight = match url.strip_suffix(".css").unwrap_or(&url) {
                "tailwindcss" | "tailwindcss/index" | "tailwindcss/preflight" => true,
                "tailwindcss/theme" | "tailwindcss/utilities" => false,
                _ => return Err(ParseError::unexpected_token()),
            };
            while input.next().is_ok() {}
            return Ok(Item::Rule(Rule::Tailwind(Some(preflight))));
        }
        if name.eq_ignore_ascii_case("reference") && self.top_level {
            input.expect_url_or_string()?;
            while input.next().is_ok() {}
            return Ok(Item::Rule(Rule::Tailwind(None)));
        }
        if name.eq_ignore_ascii_case("keyframes") && !self.declarations_only {
            let name = input.expect_ident_or_string()?.to_string();
            input.expect_exhausted()?;
            return Ok(Item::Rule(Rule::Keyframes(name, Vec::new())));
        }
        if name.eq_ignore_ascii_case("custom-variant") && self.top_level {
            let name = input.expect_ident_cloned()?.to_string();
            let mut template = String::new();
            if !input.is_exhausted() {
                input.expect_parenthesis_block()?;
                input.parse_nested_block(|input| source(input, &mut template))?;
                input.expect_exhausted()?;
            }
            return Ok(Item::Rule(Rule::Variant(name, template.trim().to_owned())));
        }
        if name.eq_ignore_ascii_case("variant") && self.parent.is_some() {
            let mut name = String::new();
            source(input, &mut name)?;
            return Ok(Item::Rule(Rule::Variant(
                name.trim().to_owned(),
                String::new(),
            )));
        }
        if name.eq_ignore_ascii_case("theme") && self.top_level {
            let mut inline = false;
            while let Ok(option) = input.try_parse(|input| input.expect_ident_cloned()) {
                inline |= option.eq_ignore_ascii_case("inline");
            }
            return Ok(Item::Rule(Rule::Theme(inline, Vec::new())));
        }
        if name.eq_ignore_ascii_case("property")
            && self.parent.is_none()
            && !self.declarations_only
            && !self.theme
        {
            let name = input.expect_ident_cloned()?.to_string();
            input.expect_exhausted()?;
            if !name.starts_with("--") {
                return Err(ParseError::unexpected_token());
            }
            return Ok(Item::Rule(Rule::Property(name, String::new(), false, None)));
        }
        if name.eq_ignore_ascii_case("scope") && !self.declarations_only && !self.theme {
            let group = |input: &mut Parser<'i>| -> Result<String, ParseError<()>> {
                input.expect_parenthesis_block()?;
                let mut text = String::new();
                input.parse_nested_block(|input| source(input, &mut text))?;
                Ok(text.trim().to_owned())
            };
            let start = input.try_parse(group).ok();
            let end = input
                .try_parse(|input| {
                    input.expect_ident_matching("to")?;
                    group(input)
                })
                .ok();
            input.expect_exhausted()?;
            return Ok(Item::Scope(start.unwrap_or_else(|| ":root".into()), end));
        }
        if name.eq_ignore_ascii_case("starting-style") && !self.declarations_only && !self.theme {
            input.expect_exhausted()?;
            return Ok(Item::Starting);
        }
        if name.eq_ignore_ascii_case("font-face") && self.top_level {
            input.expect_exhausted()?;
            return Ok(Item::Rule(Rule::FontFace(Declarations::default())));
        }
        if name.eq_ignore_ascii_case("utility") && self.top_level {
            let mut name = String::new();
            source(input, &mut name)?;
            name.retain(|c| !c.is_whitespace());
            return Ok(Item::Rule(Rule::Utility(name, String::new())));
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
        if name.eq_ignore_ascii_case("layer") && !self.declarations_only {
            let mut value = String::new();
            source(input, &mut value)?;
            return Ok(Item::Rule(Rule::Layer(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                Vec::new(),
            )));
        }
        if name.eq_ignore_ascii_case("supports") && !self.declarations_only {
            input.skip_whitespace();
            let start = input.state();
            let supported =
                condition(input, true, true)?.matches(0., 0., false, 0, FontUnits::new(16.))
                    == Some(true);
            input.expect_exhausted()?;
            input.reset(&start);
            let mut value = String::new();
            source(input, &mut value)?;
            return Ok(Item::Rule(Rule::Supports(value, supported, Vec::new())));
        }
        if name.eq_ignore_ascii_case("container") && !self.declarations_only {
            let container = input
                .try_parse(|input| -> Result<String, ParseError<()>> {
                    let name = input.expect_ident_cloned()?;
                    if ["not", "and", "or", "none"]
                        .iter()
                        .any(|word| name.eq_ignore_ascii_case(word))
                    {
                        return Err(ParseError::unexpected_token());
                    }
                    Ok(name.to_string())
                })
                .ok();
            let mut query = condition(input, true, false)?;
            input.expect_exhausted()?;
            let mut block = false;
            let mut pending = vec![&mut query];
            while let Some(query) = pending.pop() {
                let replaced = match query {
                    MediaQuery::Not(inner) => {
                        pending.push(inner);
                        continue;
                    }
                    MediaQuery::And(items) | MediaQuery::Or(items) => {
                        pending.extend(items.iter_mut());
                        continue;
                    }
                    MediaQuery::Feature(name, op, value) => match name.as_str() {
                        "width" | "inline-size" => {
                            *name = "width".into();
                            continue;
                        }
                        "height" | "block-size" | "aspect-ratio" => {
                            block = true;
                            if name == "block-size" {
                                *name = "height".into();
                            }
                            continue;
                        }
                        "orientation"
                            if op == ":" && matches!(value.as_str(), "portrait" | "landscape") =>
                        {
                            block = true;
                            MediaQuery::Feature(
                                "aspect-ratio".into(),
                                if value == "portrait" { "<=" } else { ">" }.into(),
                                "1".into(),
                            )
                        }
                        _ => MediaQuery::Unknown(format!("({name}{op}{value})")),
                    },
                    _ => continue,
                };
                *query = replaced;
            }
            return Ok(Item::Rule(Rule::Container(
                container,
                block,
                query,
                Vec::new(),
            )));
        }
        if name.eq_ignore_ascii_case("media") && !self.declarations_only {
            let queries = if input.is_exhausted() {
                Vec::new()
            } else {
                input.parse_comma_separated_ignoring_errors(|input| {
                    let query = input.try_parse(|input| {
                        input.parse_entirely(|input| {
                            if let Ok(query) = input.try_parse(|input| {
                                input.parse_entirely(|input| condition(input, true, false))
                            }) {
                                return Ok(query);
                            }
                            let not = input
                                .try_parse(|input| input.expect_ident_matching("not"))
                                .is_ok();
                            if !not {
                                let _ =
                                    input.try_parse(|input| input.expect_ident_matching("only"));
                            }
                            let medium = input.expect_ident_cloned()?;
                            if ["not", "only", "and", "or", "layer"]
                                .iter()
                                .any(|word| medium.eq_ignore_ascii_case(word))
                            {
                                return Err(ParseError::unexpected_token());
                            }
                            let query = if input.is_exhausted() {
                                MediaQuery::All
                            } else {
                                input.expect_ident_matching("and")?;
                                condition(input, false, false)?
                            };
                            let matched = medium.eq_ignore_ascii_case("all")
                                || medium.eq_ignore_ascii_case("screen");
                            Ok(match (matched, not) {
                                (true, false) => query,
                                (true, true) => MediaQuery::Not(Box::new(query)),
                                (false, false) => MediaQuery::Not(Box::new(MediaQuery::All)),
                                (false, true) => MediaQuery::All,
                            })
                        })
                    });
                    Ok::<_, ParseError<()>>(query.unwrap_or_else(|_| {
                        while input.next().is_ok() {}
                        MediaQuery::Not(Box::new(MediaQuery::All))
                    }))
                })
            };
            return Ok(Item::Rule(Rule::Media(queries, Vec::new())));
        }
        if !name.eq_ignore_ascii_case("charset") {
            DROPPED.with_borrow_mut(|dropped| dropped.insert(name.to_ascii_lowercase()));
        }
        Err(ParseError::unexpected_token())
    }

    fn rule_without_block(&mut self, item: Item, _: &ParserState) -> Result<Item, ()> {
        match item {
            Item::Rule(Rule::Tailwind(_) | Rule::Layer(..)) | Item::Declaration(_) => Ok(item),
            Item::Rule(Rule::Variant(_, ref template)) if !template.is_empty() => Ok(item),
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
            Item::Scope(start, end) => {
                let root = Rc::new(Selector::parse(
                    &format!(":where({start})"),
                    self.parent.as_ref(),
                ));
                if root.is_empty() {
                    return Err(ParseError::unexpected_token());
                }
                let suffix = end.map_or_else(String::new, |end| {
                    format!(":not(:where({start}) :where({end}), :where({start}) :where({end}) *)")
                });
                let children = Rules {
                    parent: Some(root.clone()),
                    scope: true,
                    suffix,
                    ..Rules::default()
                }
                .body(input);
                Rule::Style(root, children)
            }
            Item::Starting => Rule::Starting(match &self.parent {
                Some(parent) => {
                    let own = Rc::new(Selector::parse("&::starting-style", Some(parent)));
                    let children = Rules {
                        parent: Some(own.clone()),
                        ..Rules::default()
                    }
                    .body(input);
                    vec![Rule::Style(own, children)]
                }
                None => Rules {
                    suffix: "::starting-style".into(),
                    ..Rules::default()
                }
                .body(input),
            }),
            Item::Rule(Rule::Layer(names, _)) => Rule::Layer(
                names,
                Rules {
                    parent: self.parent.clone(),
                    ..Rules::default()
                }
                .body(input),
            ),
            Item::Rule(Rule::Supports(condition, supported, _)) => Rule::Supports(
                condition,
                supported,
                Rules {
                    parent: self.parent.clone(),
                    ..Rules::default()
                }
                .body(input),
            ),
            Item::Rule(Rule::Theme(inline, _)) => Rule::Theme(
                inline,
                Rules {
                    theme: true,
                    ..Rules::default()
                }
                .body(input),
            ),
            Item::Rule(Rule::FontFace(_)) => Rule::FontFace(Declarations::parse_input(input)),
            Item::Rule(Rule::Property(name, ..)) => {
                let (mut syntax, mut inherits, mut initial) = (None, None, None);
                for declaration in Declarations::parse_input(input).0 {
                    if let Declaration::Property(key, value) = declaration {
                        match key.as_str() {
                            "syntax" => syntax = Some(value.trim_matches(['"', '\'']).to_owned()),
                            "inherits" => {
                                inherits = ["false", "true"]
                                    .iter()
                                    .position(|word| value.eq_ignore_ascii_case(word))
                            }
                            "initial-value" => initial = Some(value),
                            _ => {}
                        }
                    }
                }
                let (Some(syntax), Some(inherits)) = (syntax, inherits) else {
                    return Err(ParseError::unexpected_token());
                };
                if syntax.trim() != "*"
                    && initial.as_deref().is_none_or(|value| {
                        value.contains("var(") || !super::syntax_matches(&syntax, value)
                    })
                {
                    return Err(ParseError::unexpected_token());
                }
                Rule::Property(name, syntax, inherits == 1, initial)
            }
            Item::Rule(Rule::Utility(name, _)) => {
                let mut body = String::new();
                source(input, &mut body)?;
                Rule::Utility(name, body)
            }
            Item::Rule(Rule::Variant(name, template)) if template.is_empty() => {
                let mut body = String::new();
                source(input, &mut body)?;
                if !self.top_level {
                    body = format!("{{{body}}}");
                }
                Rule::Variant(name, body)
            }
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
            Item::Rule(Rule::Container(name, block, query, _)) => Rule::Container(
                name,
                block,
                query,
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
        self.declarations_only || self.theme || self.parent.is_some()
    }

    fn parse_qualified(&self) -> bool {
        !self.declarations_only && !self.theme
    }
}

fn condition(
    input: &mut Parser<'_>,
    or: bool,
    supports: bool,
) -> Result<MediaQuery, ParseError<()>> {
    let not = input
        .try_parse(|input| input.expect_ident_matching("not"))
        .is_ok();
    let mut items = Vec::new();
    let mut and = true;
    loop {
        let start = input.position();
        let selector = match input.next()? {
            Token::ParenthesisBlock => None,
            Token::Function(name) => Some(name.to_ascii_lowercase()),
            _ => return Err(ParseError::unexpected_token()),
        };
        let item = input.parse_nested_block(|input| {
            if selector.is_none()
                && let Ok(query) = input.try_parse(|input| {
                    input.parse_entirely(|input| condition(input, true, supports))
                })
            {
                return Ok(Some(query));
            }
            let mut text = String::new();
            source(input, &mut text)?;
            let text = text.trim();
            if supports {
                let supported = match selector.as_deref() {
                    Some("selector") => !Selector::parse(text, None).is_empty(),
                    Some("font-format") => super::FONT_FORMATS
                        .iter()
                        .any(|format| text.eq_ignore_ascii_case(format)),
                    Some("font-tech") => super::FONT_TECHS
                        .iter()
                        .any(|tech| text.eq_ignore_ascii_case(tech)),
                    Some(_) => false,
                    None => text.split_once(':').is_some_and(|(name, value)| {
                        let value = value.trim();
                        super::supports(
                            &name.trim().to_ascii_lowercase(),
                            value
                                .strip_suffix("!important")
                                .map_or(value, str::trim_end),
                        ) == Some(true)
                    }),
                };
                return Ok(Some(if supported {
                    MediaQuery::All
                } else {
                    MediaQuery::Not(Box::new(MediaQuery::All))
                }));
            }
            if selector.is_some() {
                return Ok(None);
            }
            let text = text.to_ascii_lowercase();
            let ident = |name: &str| {
                name.trim_start_matches('-')
                    .starts_with(|c: char| c.is_ascii_alphabetic())
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            };
            let feature = |name: &str, op: &str, value: &str| {
                MediaQuery::Feature(name.into(), op.into(), value.into())
            };
            if let Some((name, value)) = text.split_once(':') {
                let name = name
                    .trim()
                    .replacen("-webkit-min-", "min--webkit-", 1)
                    .replacen("-webkit-max-", "max--webkit-", 1);
                let (name, value) = (name.as_str(), value.trim());
                return Ok((ident(name) && !value.is_empty()).then(|| {
                    if let Some(name) = name.strip_prefix("min-") {
                        feature(name, ">=", value)
                    } else if let Some(name) = name.strip_prefix("max-") {
                        feature(name, "<=", value)
                    } else {
                        feature(name, ":", value)
                    }
                }));
            }
            if ident(&text) {
                return Ok(Some(feature(&text, "", "")));
            }
            let mut parts = [""; 3];
            let mut ops = [""; 2];
            let mut count = 0;
            let mut rest = text.as_str();
            while let Some(index) = rest.find(['<', '>', '=']) {
                let length = 1 + usize::from(
                    !rest[index..].starts_with('=') && rest[index + 1..].starts_with('='),
                );
                if count == 2 {
                    return Ok(None);
                }
                parts[count] = rest[..index].trim();
                ops[count] = &rest[index..index + length];
                count += 1;
                rest = &rest[index + length..];
            }
            parts[count] = rest.trim();
            let flip = |op: &str| match op {
                "<" => ">",
                "<=" => ">=",
                ">" => "<",
                ">=" => "<=",
                _ => "=",
            };
            Ok(match count {
                1 if ident(parts[0]) && !parts[1].is_empty() => {
                    Some(feature(parts[0], ops[0], parts[1]))
                }
                1 if ident(parts[1]) && !parts[0].is_empty() => {
                    Some(feature(parts[1], flip(ops[0]), parts[0]))
                }
                2 if ident(parts[1])
                    && ops[0] != "="
                    && ops[0].as_bytes().first() == ops[1].as_bytes().first()
                    && !parts[0].is_empty()
                    && !parts[2].is_empty() =>
                {
                    Some(MediaQuery::And(vec![
                        feature(parts[1], flip(ops[0]), parts[0]),
                        feature(parts[1], ops[1], parts[2]),
                    ]))
                }
                _ => None,
            })
        })?;
        let item = item.unwrap_or_else(|| MediaQuery::Unknown(input.slice_from(start).to_owned()));
        if not {
            return Ok(MediaQuery::Not(Box::new(item)));
        }
        items.push(item);
        let Ok(word) = input.try_parse(Parser::expect_ident_cloned) else {
            break;
        };
        let next = word.eq_ignore_ascii_case("and");
        if !next && !(or && word.eq_ignore_ascii_case("or")) || items.len() > 1 && next != and {
            return Err(ParseError::unexpected_token());
        }
        and = next;
    }
    if items.len() == 1
        && let Some(item) = items.pop()
    {
        return Ok(item);
    }
    Ok(if and {
        MediaQuery::And(items)
    } else {
        MediaQuery::Or(items)
    })
}

fn environment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("env(") {
        let body = &rest[start + 4..];
        let (mut depth, mut comma, mut end) = (1, None, None);
        for (index, c) in body.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                }
                ',' if depth == 1 && comma.is_none() => comma = Some(index),
                _ => {}
            }
        }
        let name = body[..comma.or(end).unwrap_or(body.len())].trim();
        let replacement =
            if rest[..start].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '-' | '_')) {
                None
            } else if [
                "safe-area-inset-",
                "safe-area-max-inset-",
                "keyboard-inset-",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            {
                Some("0px")
            } else {
                comma
                    .zip(end)
                    .map(|(comma, end)| body[comma + 1..end].trim())
            };
        match (replacement, end) {
            (Some(replacement), Some(end)) => {
                out.push_str(&rest[..start]);
                out.push_str(replacement);
                rest = &body[end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 4]);
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FontUnits {
    pub em: f32,
    pub ch: f32,
    pub ex: f32,
    pub cap: f32,
    pub ic: f32,
    pub lh: f32,
    pub absolute: bool,
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
            absolute: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lengths {
    pub font: FontUnits,
    pub root: FontUnits,
    pub viewport: [f32; 2],
    pub container: Option<[f32; 2]>,
}

#[derive(Clone, Default)]
pub(crate) struct NodeLengths {
    ids: super::Chunked<u32>,
    list: Vec<Lengths>,
    last: u32,
}

impl NodeLengths {
    pub(crate) fn reset(&mut self, count: usize, lengths: Lengths) {
        self.ids.clear();
        self.ids.resize(count, 0);
        self.list.clear();
        self.list.push(lengths);
        self.last = 0;
    }

    pub(crate) fn set(&mut self, id: usize, lengths: Lengths) {
        if self.list[self.last as usize] != lengths {
            // Only the newest values are searched, or a page with a font size per node gets quadratic.
            let mut newest = self.list.iter().enumerate().rev().take(32);
            self.last = match newest.find(|(_, old)| **old == lengths) {
                Some((index, _)) => index as u32,
                None => {
                    self.list.push(lengths);
                    self.list.len() as u32 - 1
                }
            };
        }
        self.ids[id] = self.last;
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Lengths> {
        self.ids.get(id).map(|&index| &self.list[index as usize])
    }

    pub(crate) fn grow(&mut self, count: usize) {
        self.ids.resize(count, 0);
    }

    pub(crate) fn unset(&mut self, id: usize) {
        self.ids[id] = 0;
    }

    pub(crate) fn list_len(&self) -> usize {
        self.list.len()
    }
}

impl std::ops::Index<usize> for NodeLengths {
    type Output = Lengths;

    fn index(&self, id: usize) -> &Lengths {
        &self.list[self.ids[id] as usize]
    }
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
        let (value, kind) = self.math(source, base, &[])?;
        let value = value as f32;
        (kind != Kind::Angle && value.is_finite()).then_some((value, kind == Kind::Length))
    }

    pub fn math(&self, source: &str, base: f32, vars: &[(&str, f64)]) -> Option<(f64, Kind)> {
        let mut parser = Math {
            rest: source.trim(),
            units: self,
            base,
            depth: 0,
            vars,
        };
        let value = parser.atom()?;
        (parser.rest.trim().is_empty() && !value.0.is_nan())
            .then_some((value.0.clamp(-33_554_428., 33_554_428.), value.1))
    }

    pub fn resolve(&self, source: &str, base: f32) -> Option<f32> {
        let (value, dimensional) = self.parse(source, base)?;
        (dimensional || value == 0.).then_some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Number,
    Length,
    Angle,
}

struct Math<'a> {
    rest: &'a str,
    units: &'a Lengths,
    base: f32,
    depth: usize,
    vars: &'a [(&'a str, f64)],
}

impl Math<'_> {
    fn expression(&mut self, minimum: u8) -> Option<(f64, Kind)> {
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
            left = match (operator, left.1, right.1) {
                (b'+', a, b) if a == b => (left.0 + right.0, a),
                (b'-', a, b) if a == b => (left.0 - right.0, a),
                (b'*', kind, Kind::Number) | (b'*', Kind::Number, kind) => (left.0 * right.0, kind),
                (b'/', kind, Kind::Number) if right.0 != 0. => (left.0 / right.0, kind),
                (b'/', a, b) if a == b && right.0 != 0. => (left.0 / right.0, Kind::Number),
                _ => return None,
            };
            if left.0.is_nan() {
                return None;
            }
        }
        self.depth -= 1;
        Some(left)
    }

    fn atom(&mut self) -> Option<(f64, Kind)> {
        self.rest = self.rest.trim_start();
        if let Some(rest) = self.rest.strip_prefix('-')
            && rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '(')
        {
            self.rest = rest;
            let (value, kind) = self.atom()?;
            return Some((-value, kind));
        }
        let end = if self.rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            self.rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(self.rest.len())
        } else {
            0
        };
        let name = &self.rest[..end];
        let lower;
        let name = if name.bytes().any(|c| c.is_ascii_uppercase()) {
            lower = name.to_ascii_lowercase();
            lower.as_str()
        } else {
            name
        };
        if end > 0 && !self.rest[end..].starts_with('(') {
            self.rest = &self.rest[end..];
            if let Some((_, value)) = self.vars.iter().find(|(var, _)| *var == name) {
                return Some((*value, Kind::Number));
            }
            let value = match name {
                "e" if self.depth > 0 => std::f64::consts::E,
                "pi" if self.depth > 0 => std::f64::consts::PI,
                "infinity" if self.depth > 0 => f64::INFINITY,
                "nan" if self.depth > 0 => f64::NAN,
                _ => return None,
            };
            return Some((value, Kind::Number));
        }
        if end > 0 || self.rest.starts_with('(') {
            if end == 0 && self.depth == 0 {
                return None;
            }
            self.rest = &self.rest[end + 1..];
            let strategy = ["nearest", "up", "down", "to-zero"]
                .into_iter()
                .filter(|_| name == "round")
                .find_map(|keyword| {
                    let rest = self.rest.trim_start();
                    let rest = rest
                        .get(..keyword.len())
                        .filter(|word| word.eq_ignore_ascii_case(keyword))
                        .and(rest.get(keyword.len()..))?;
                    self.rest = rest.trim_start().strip_prefix(',')?;
                    Some(keyword)
                })
                .unwrap_or("nearest");
            let mut args = [self.expression(0)?; 3];
            let mut count = 1;
            loop {
                self.rest = self.rest.trim_start();
                if let Some(rest) = self.rest.strip_prefix(')') {
                    self.rest = rest;
                    break;
                }
                self.rest = self.rest.strip_prefix(',')?;
                let value = self.expression(0)?;
                match name {
                    "min" | "max" | "hypot" if value.1 == args[0].1 => {
                        args[0].0 = match name {
                            "min" => args[0].0.min(value.0),
                            "max" => args[0].0.max(value.0),
                            _ => args[0].0.hypot(value.0),
                        }
                    }
                    "clamp" | "round" | "mod" | "rem" | "atan2" | "pow" | "log" if count < 3 => {
                        args[count] = value;
                        count += 1;
                    }
                    _ => return None,
                }
            }
            let [(a, kind), (b, other), (c, last)] = args;
            let same = kind == other;
            let number = kind == Kind::Number;
            return Some(match (name, count) {
                ("" | "calc" | "min" | "max", _) => args[0],
                ("hypot", _) => (a.abs(), kind),
                ("clamp", 3) if same && kind == last => (a.max(b.min(c)), kind),
                ("round", 1 | 2) => {
                    let step = if count == 2 && same {
                        b.abs()
                    } else if count == 1 {
                        1.
                    } else {
                        return None;
                    };
                    let value = a / step;
                    let value = match strategy {
                        "up" => value.ceil(),
                        "down" => value.floor(),
                        "to-zero" => value.trunc(),
                        _ => (value + 0.5).floor(),
                    };
                    (value * step, kind)
                }
                ("mod", 2) if same => (a - b * (a / b).floor(), kind),
                ("rem", 2) if same => (a % b, kind),
                ("abs", 1) => (a.abs(), kind),
                ("sign", 1) => (f64::from(i8::from(a > 0.) - i8::from(a < 0.)), Kind::Number),
                ("sin" | "cos" | "tan", 1) if kind != Kind::Length => {
                    let radians = if number { a } else { a.to_radians() };
                    let value = match name {
                        "sin" => radians.sin(),
                        "cos" => radians.cos(),
                        _ => radians.tan(),
                    };
                    (value, Kind::Number)
                }
                ("asin", 1) if number => (a.asin().to_degrees(), Kind::Angle),
                ("acos", 1) if number => (a.acos().to_degrees(), Kind::Angle),
                ("atan", 1) if number => (a.atan().to_degrees(), Kind::Angle),
                ("atan2", 2) if same => (a.atan2(b).to_degrees(), Kind::Angle),
                ("pow", 2) if number && same => (a.powf(b), Kind::Number),
                ("sqrt", 1) if number => (a.sqrt(), Kind::Number),
                ("exp", 1) if number => (a.exp(), Kind::Number),
                ("log", 1) if number => (a.ln(), Kind::Number),
                ("log", 2) if number && same => (a.ln() / b.ln(), Kind::Number),
                _ => return None,
            })
            .filter(|value| !value.0.is_nan());
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
        let (scale, kind) = match unit {
            "" => (1., Kind::Number),
            "deg" => (1., Kind::Angle),
            "grad" => (0.9, Kind::Angle),
            "rad" => (180. / std::f64::consts::PI, Kind::Angle),
            "turn" => (360., Kind::Angle),
            "%" => (f64::from(self.base) / 100., Kind::Length),
            _ => {
                let scale = match unit {
                    "px" => 1.,
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
                (f64::from(scale), Kind::Length)
            }
        };
        let value = value * scale;
        value.is_finite().then_some((value, kind))
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
                absolute: false,
            },
            root: FontUnits {
                em: 24.,
                ch: 14.,
                ex: 11.,
                cap: 18.,
                ic: 24.,
                lh: 36.,
                absolute: false,
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
            ("round(33px, 10px)", 30.),
            ("round(up, 31px, 10px)", 40.),
            ("round(DOWN, 39px, 10px)", 30.),
            ("round(to-zero, -39px, 10px)", -30.),
            ("round(nearest, 15px, 10px)", 20.),
            ("round(-2.5px)", -2.),
            ("round(33%, 10px)", 130.),
            ("calc(2px * round(1.5))", 4.),
            ("abs(-5px)", 5.),
            ("calc(sign(-3px) * 10px)", -10.),
            ("calc(sign(0px) * 10px)", 0.),
            ("mod(-7px, 3px)", 2.),
            ("rem(-7px, 3px)", -1.),
            ("calc(sin(30deg) * 100px)", 50.),
            ("calc(cos(0.5turn) * 10px)", -10.),
            ("calc(cos(200grad) * 10px)", -10.),
            ("calc(sin(pi / 2) * 1px)", 1.),
            ("calc(cos(acos(0.5)) * 2px)", 1.),
            ("calc(10px * tan(atan2(1px, 1px)))", 10.),
            ("calc(sin(asin(1)) * 1px)", 1.),
            ("calc(cos(atan(0)) * 1px)", 1.),
            ("calc(pow(2, 10) * 1px)", 1024.),
            ("calc(sqrt(16) * 1px)", 4.),
            ("hypot(3px, 4px)", 5.),
            ("hypot(-3px)", 3.),
            ("calc(log(e) * 1px)", 1.),
            ("calc(log(8, 2) * 1px)", 3.),
            ("calc(exp(0) * 1px)", 1.),
            ("calc(1px / infinity)", 0.),
            ("calc(-min(1px, 2px))", -1.),
            ("max(1px, -min(-5px, -6px))", 6.),
            ("clamp(1px, calc(abs(-2) * (1px + 1px)), 50px)", 4.),
        ] {
            let actual = units.resolve(source, 400.).unwrap_or(f32::NAN);
            assert!((actual - expected).abs() < 1e-4, "{source}: {actual}");
        }
        assert_eq!(units.parse("sin(30deg)", 0.).map(|v| v.1), Some(false));
        assert_eq!(
            units.parse("calc(e)", 0.).map(|v| v.0),
            Some(std::f32::consts::E)
        );
        assert_eq!(
            units.parse("calc(PI)", 0.).map(|v| v.0),
            Some(std::f32::consts::PI)
        );
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
            "round(33px, 0px)",
            "round(33px, 10)",
            "round(sideways, 33px, 10px)",
            "mod(5px, 0px)",
            "mod(5px, 2)",
            "rem(1px)",
            "abs(1px, 2px)",
            "calc(asin(2) * 1px)",
            "calc(sqrt(-1) * 1px)",
            "calc(pow(1px, 2) * 1px)",
            "sin(1px)",
            "calc(30deg)",
            "calc(1px + 30deg)",
            "calc(NaN * 1px)",
            "calc(1px * e)px",
            "pi",
            "e",
            "hypot(1px, 2)",
        ] {
            assert_eq!(units.resolve(source, 100.), None, "{source}");
        }
        for (source, value) in [
            ("1e40px", 33_554_428.),
            ("calc(infinity * 1px)", 33_554_428.),
            ("calc(-infinity * 1px)", -33_554_428.),
            ("calc(pow(10, 400) * 1px)", 33_554_428.),
            ("calc(exp(1000) * 1px)", 33_554_428.),
            ("calc(log(0) * 1px)", -33_554_428.),
        ] {
            assert_eq!(units.resolve(source, 100.), Some(value), "{source}");
        }
        assert_eq!(
            units.resolve(
                &format!("{}1px{}", "calc(".repeat(100), ")".repeat(100)),
                0.
            ),
            None
        );
    }

    #[test]
    fn media_queries_follow_level_4_grammar_and_serialize() {
        let evaluate = |query: &str| {
            let sheet = Stylesheet::parse(&format!("@media {query}{{}}"));
            let [rule] = sheet.0.as_slice() else {
                return None;
            };
            let Rule::Media(queries, _) = &**rule else {
                return None;
            };
            let matched = queries.is_empty()
                || queries.iter().any(|query| {
                    query.matches(800., 600., false, 0, FontUnits::new(16.)) == Some(true)
                });
            Some((matched, sheet.source()))
        };
        for (query, expected) in [
            ("", true),
            ("all", true),
            ("screen", true),
            ("print", false),
            ("speech", false),
            ("not print", true),
            ("not screen", false),
            ("NOT PRINT", true),
            ("only screen and (min-width: 600px)", true),
            ("screen and (max-width: 600px)", false),
            ("not screen and (max-width: 600px)", true),
            ("print and (min-width: 1px)", false),
            ("not print and (min-width: 1px)", true),
            ("print, (min-width: 600px)", true),
            ("(MIN-WIDTH: 600PX)", true),
            ("(width > 799px)", true),
            ("(width > 800px)", false),
            ("(width >= 800px)", true),
            ("(width = 800px)", true),
            ("(width: 800px)", true),
            ("(800px <= width)", true),
            ("(800px < width)", false),
            ("(400px < width < 900px)", true),
            ("(900px > width > 400px)", true),
            ("(400px <= width <= 700px)", false),
            ("(400px < width > 900px)", false),
            ("(height < 600px)", false),
            ("(height <= 600px)", true),
            ("(width >= calc(40em + 1px))", true),
            ("(min-width: calc(50em + 1px))", false),
            ("(min-width: 0)", true),
            ("(min-width: 600)", false),
            ("(min-width: 50%)", false),
            ("(orientation: landscape)", true),
            ("(orientation: portrait)", false),
            ("(min-orientation: portrait)", false),
            ("(aspect-ratio: 4/3)", true),
            ("(aspect-ratio: 8 / 6)", true),
            ("(min-aspect-ratio: 16/9)", false),
            ("(aspect-ratio > 1)", true),
            ("(min-resolution: 1dppx)", true),
            ("(resolution >= 2x)", false),
            ("(max-resolution: 96dpi)", true),
            ("(hover: hover)", true),
            ("(hover: none)", false),
            ("(any-hover: hover)", true),
            ("(pointer: fine)", true),
            ("(any-pointer: coarse)", false),
            ("(pointer)", true),
            ("(prefers-color-scheme: light)", true),
            ("(prefers-color-scheme: dark)", false),
            ("(prefers-reduced-motion: no-preference)", true),
            ("(prefers-reduced-motion)", false),
            ("(prefers-contrast: no-preference)", true),
            ("(prefers-contrast)", false),
            ("(prefers-reduced-transparency: reduce)", false),
            ("(forced-colors: none)", true),
            ("(forced-colors)", false),
            ("(inverted-colors: none)", true),
            ("(display-mode: standalone)", true),
            ("(display-mode: browser)", false),
            ("(color)", true),
            ("(min-color: 8)", true),
            ("(color-index)", false),
            ("(monochrome)", false),
            ("(monochrome: 0)", true),
            ("(update: fast)", true),
            ("(scripting: enabled)", true),
            ("(dynamic-range: standard)", true),
            ("(dynamic-range: high)", false),
            ("(color-gamut: srgb)", true),
            ("(color-gamut: p3)", false),
            ("(-webkit-min-device-pixel-ratio: 1)", true),
            ("(-webkit-min-device-pixel-ratio: 2)", false),
            ("(color-gamut)", true),
            ("(min-width: 600px) and (max-width: 900px)", true),
            ("(max-width: 600px) or (orientation: landscape)", true),
            ("not (max-width: 600px)", true),
            ("not ((min-width: 600px) and (max-width: 900px))", false),
            (
                "((min-width: 600px) or (max-width: 100px)) and (hover)",
                true,
            ),
            ("(min-width: 1px) and (not (hover: none))", true),
            ("not (not (hover))", true),
            ("(unknown-feature)", false),
            ("not (unknown-feature)", false),
            ("(unknown-feature) or (min-width: 1px)", true),
            ("(unknown-feature) and (min-width: 1px)", false),
            ("foo(bar) or (hover)", true),
            ("(prefers-color-scheme: blue)", false),
            ("not (prefers-color-scheme: blue)", false),
            ("(max-width: invalid)", false),
            ("not (max-width: invalid)", false),
            ("screen and (min-width: 1px) or (hover)", false),
            ("(min-width: 1px) and (hover) or (pointer)", false),
            ("not (hover) and (pointer)", false),
            ("garbage!!", false),
            ("screen, garbage!!", true),
            ("not", false),
            ("only", false),
            ("and", false),
            ("only (hover)", false),
        ] {
            let (matched, source) = evaluate(query).unwrap();
            assert_eq!(matched, expected, "{query}");
            let reparsed = source
                .strip_prefix("@media ")
                .and_then(|source| source.strip_suffix("{}"))
                .unwrap();
            assert_eq!(
                evaluate(reparsed).unwrap().0,
                expected,
                "{query} -> {source}"
            );
        }
        let retina = |query: &str| {
            let sheet = Stylesheet::parse(&format!("@media {query}{{}}"));
            let [rule] = sheet.0.as_slice() else {
                return None;
            };
            let Rule::Media(queries, _) = &**rule else {
                return None;
            };
            queries[0].matches(800., 600., false, 200 << 16 | 64, FontUnits::new(16.))
        };
        assert_eq!(retina("(min-resolution: 2dppx)"), Some(true));
        assert_eq!(retina("(-webkit-min-device-pixel-ratio: 2)"), Some(true));
        assert_eq!(retina("(min-resolution: 192dpi)"), Some(true));
        assert_eq!(retina("(resolution > 2x)"), Some(false));
        assert_eq!(retina("(color-gamut: p3)"), Some(true));
        assert_eq!(retina("(color-gamut: rec2020)"), Some(false));
        let deep = format!("{}(hover){}", "(".repeat(200), ")".repeat(200));
        assert!(evaluate(&deep).is_none_or(|(matched, _)| !matched));
    }

    #[test]
    fn supports_conditions_check_properties_values_and_selectors() {
        let supported = |condition: &str| match Stylesheet::parse(&format!(
            "@supports {condition} {{ a {{ color: red }} }}"
        ))
        .0
        .as_slice()
        {
            [rule] => match **rule {
                Rule::Supports(_, supported, _) => Some(supported),
                _ => None,
            },
            _ => None,
        };
        for (condition, expected) in [
            ("(display: flex)", Some(true)),
            ("(display: inline-grid)", Some(true)),
            ("(display: contents)", Some(true)),
            ("(DISPLAY: FLEX)", Some(true)),
            ("(display: nonsense)", Some(false)),
            ("not (display: nonsense)", Some(true)),
            ("(display: flex !important)", Some(true)),
            ("(position: sticky)", Some(true)),
            ("(gap: 1rem)", Some(true)),
            ("(aspect-ratio: 16 / 9)", Some(true)),
            ("(width: 1cqi)", Some(true)),
            ("(width:)", Some(false)),
            ("(color: color-mix(in lab, red, red))", Some(true)),
            ("(color: rgb(from red r g b))", Some(true)),
            ("(background-color: oklch(0.5 0.1 120))", Some(true)),
            ("(color: nonsense)", Some(false)),
            ("(color: var(--ink))", Some(true)),
            ("(color: inherit)", Some(true)),
            ("(--custom: anything)", Some(true)),
            ("(margin-trim: inline)", Some(false)),
            ("(-webkit-hyphens: none)", Some(false)),
            ("(display: flex) and (gap: 1px)", Some(true)),
            ("(display: flex) and (display: nonsense)", Some(false)),
            ("(display: nonsense) or (display: grid)", Some(true)),
            ("not ((display: nonsense) or (display: block))", Some(false)),
            (
                "((display: flex) and (not (display: nonsense)))",
                Some(true),
            ),
            ("selector(a > b)", Some(true)),
            ("not selector(a > b)", Some(false)),
            ("font-tech(color-colrv1)", Some(false)),
            ("font-tech(variations)", Some(true)),
            ("not font-format(woff2)", Some(false)),
            ("font-format(opentype)", Some(true)),
            ("font-format(embedded-opentype)", Some(false)),
            ("(width: bogus)", Some(false)),
            ("(width: calc(100% - 2px))", Some(true)),
            ("(filter: blur(2px))", Some(true)),
            ("(backdrop-filter: blur(1px) saturate(2))", Some(true)),
            ("(-webkit-backdrop-filter: none)", Some(true)),
            ("(mask-image: linear-gradient(red, blue))", Some(true)),
            ("(clip-path: circle(50%))", Some(true)),
            ("(scrollbar-width: thin)", Some(true)),
            ("(scrollbar-color: red blue)", Some(true)),
            ("(mix-blend-mode: multiply)", Some(true)),
            ("(transform: translateX(1px) rotate(2deg))", Some(true)),
            ("(transform: bogus)", Some(false)),
            ("(margin: 1px auto)", Some(true)),
            ("(margin: red)", Some(false)),
            ("(opacity: 50%)", Some(true)),
            ("(z-index: 1.5)", Some(false)),
            ("(text-wrap: balance)", Some(true)),
            ("(word-break: break-all)", Some(true)),
            ("(word-break: keep-all)", Some(false)),
            (
                "((-webkit-hyphens: none) and (not (margin-trim: inline))) or ((-moz-orient: inline) and (not (color:rgb(from red r g b))))",
                Some(false),
            ),
            ("(foo bar)", Some(false)),
            ("not (foo bar)", Some(true)),
            ("display: flex", None),
            ("(display: flex) and", None),
            ("(a: b) and (c: d) or (e: f)", None),
            ("not (a: b) and (c: d)", None),
        ] {
            assert_eq!(supported(condition), expected, "{condition}");
        }
    }
}
