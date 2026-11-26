use crate::css::value::{font::ordered, grammar::grammar};
use crate::css::{
    value::anchor, value::color::serialize as serialize_color, value::shape::Position,
    value::shape::Shaped, value::shorthand::expand, value::tokens::components,
    value::tokens::number, value::tokens::write_number, value::tokens::write_printed,
    value::units::Kind, value::units::Lengths,
};
use std::borrow::Cow;

// Chrome's text for a declared value: lowercase keywords, units and functions, short numbers, `0px` for
// lengths, colors as `rgb()`, strings in double quotes and `, ` between items. Values with `var()` stay as
// written, like in Chrome.
pub(crate) fn specified(name: &str, value: &str) -> String {
    let value = value.trim();
    if name.starts_with("--")
        || ["var(", "env(", "attr("]
            .iter()
            .any(|function| value.contains(function))
    {
        return value.to_owned();
    }
    if let Some(text) = anchor::specified(name, value) {
        return text;
    }
    let functions = anchor::has_function(value)
        .then(|| anchor::canonical(name, value))
        .flatten();
    let value = functions.as_deref().unwrap_or(value);
    let ordered = ordered(name, value)
        .flatten()
        .filter(|_| !value.contains(['"', '\'']));
    let value = ordered.as_deref().unwrap_or(value);
    let grammar = grammar(name).unwrap_or("*");
    let length =
        grammar.contains("length") && !grammar.contains("number>") && !grammar.contains("integer>")
            || name == "translate"
            || name.ends_with("-shadow")
            || name.ends_with("-origin")
            || name.ends_with("-position");
    // Names that authors choose keep their case.
    let named = matches!(
        name,
        "font" | "font-family" | "animation" | "animation-name" | "list-style" | "list-style-type"
    ) || name.starts_with("grid")
        || name.starts_with("counter-")
        || name.starts_with("container")
        || name.ends_with("-name");
    let mut out = String::with_capacity(value.len());
    canonical(value, length, named, &mut out);
    let numeric =
        |part: &str| part.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'));
    let layers = |out: &str, layer: &dyn Fn(Vec<&str>) -> String| {
        components(out, ',')
            .into_iter()
            .map(|item| layer(components(item.trim(), ' ')))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match name {
        "clip-path" | "offset-path" if let Some(shaped) = Shaped::parse(value) => shaped.text(None),
        "overflow-clip-margin" => match components(&out, ' ')[..] {
            ["padding-box"] => "0px".into(),
            [edge, length] | [length, edge] if edge.ends_with("-box") => match (edge, length) {
                ("padding-box", length) => length.into(),
                (edge, "0px") => edge.into(),
                _ => format!("{edge} {length}"),
            },
            _ => out,
        },
        "offset-path" if out.starts_with("url(") => out.trim_end_matches(" border-box").to_owned(),
        "offset-rotate" => match components(&out, ' ')[..] {
            [angle, auto @ ("auto" | "reverse")] => format!("{auto} {angle}"),
            _ => out,
        },
        "offset-anchor" | "offset-position" => match Position::parse(&components(value, ' ')) {
            Some(position) => position.text(None),
            None => out,
        },
        "offset" => {
            let mut parts = Vec::new();
            let wide = matches!(
                out.as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            );
            if wide || !expand(name, value, &mut parts) || parts.len() != 5 {
                return out;
            }
            let [position, path, distance, rotate, anchor] =
                std::array::from_fn(|index| specified(&parts[index].0, &parts[index].1));
            let mut text = Vec::new();
            text.extend(Some(position).filter(|position| position != "normal"));
            if path != "none" {
                text.push(path);
                text.extend(Some(distance).filter(|distance| distance != "0px"));
                text.extend(Some(rotate).filter(|rotate| rotate != "auto"));
            }
            if text.is_empty() {
                text.push("normal".into());
            }
            if anchor != "auto" {
                text.push(format!("/ {anchor}"));
            }
            text.join(" ")
        }
        "mask-repeat" | "background-repeat" => layers(&out, &|parts| match parts[..] {
            ["repeat", "no-repeat"] => "repeat-x".into(),
            ["no-repeat", "repeat"] => "repeat-y".into(),
            [a, b] if a == b => a.into(),
            _ => parts.join(" "),
        }),
        "font-family" => layers(&out, &|parts| match parts[..] {
            // Chrome quotes names of several words and unquotes names that are one identifier.
            [quoted]
                if quoted.len() > 2
                    && quoted.starts_with('"')
                    && quoted[1..quoted.len() - 1].bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
                    })
                    && !quoted[1..].starts_with(|c: char| c.is_ascii_digit() || c == '-')
                    && !matches!(
                        &*quoted[1..quoted.len() - 1].to_ascii_lowercase(),
                        "serif"
                            | "sans-serif"
                            | "monospace"
                            | "cursive"
                            | "fantasy"
                            | "system-ui"
                            | "math"
                            | "emoji"
                            | "fangsong"
                            | "ui-serif"
                            | "ui-sans-serif"
                            | "ui-monospace"
                            | "ui-rounded"
                            | "inherit"
                            | "initial"
                            | "unset"
                            | "default"
                            | "revert"
                    ) =>
            {
                quoted[1..quoted.len() - 1].to_owned()
            }
            [first, ..] if parts.len() > 1 && !first.starts_with('"') => {
                format!("\"{}\"", parts.join(" "))
            }
            _ => parts.join(" "),
        }),
        "align-items" | "align-self" | "align-content" | "justify-items" | "justify-self"
            if out == "first baseline" =>
        {
            "baseline".into()
        }
        "shape-outside" => crate::css::value::outside_text(&out, None).unwrap_or(out),
        "grid-auto-flow" if out.contains("dense") => if out.contains("column") {
            "column dense"
        } else {
            "dense"
        }
        .to_owned(),
        // A row flow with auto rows is the same as no rows, and Chrome writes it so.
        "grid" => match out.split_once('/') {
            Some((rows, columns)) if rows.trim() == "auto-flow" => {
                format!("none /{columns}")
            }
            _ => out,
        },
        "aspect-ratio" => match components(&out, ' ')[..] {
            [ratio] if numeric(ratio) || ratio.starts_with("calc(") => format!("{ratio} / 1"),
            _ => out,
        },
        "transition" => layers(&out, &|mut parts| {
            let times: Vec<String> = parts
                .iter()
                .filter(|part| numeric(part))
                .map(|part| part.to_string())
                .collect();
            if times.iter().all(|time| time == "0s") {
                parts.retain(|part| !numeric(part));
            } else if times.get(1).is_some_and(|delay| delay == "0s") {
                let delay = parts.iter().rposition(|part| *part == "0s");
                parts.remove(delay.unwrap_or_default());
            }
            parts.retain(|part| !matches!(*part, "ease" | "normal"));
            if parts.len() > 1 {
                parts.retain(|part| *part != "all");
            }
            if parts.is_empty() {
                "all".into()
            } else {
                parts.join(" ")
            }
        }),
        // Chrome lists every part of each animation, in this order.
        "animation" if out != "none" => layers(&out, &|parts| {
            let mut slots: [Option<&str>; 8] = [None; 8];
            let mut times = 0;
            for part in parts {
                let slot = match part {
                    _ if numeric(part) && part.ends_with('s') => {
                        times += 1;
                        if times == 1 { 0 } else { 2 }
                    }
                    "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out" | "step-start"
                    | "step-end" => 1,
                    _ if part.starts_with("cubic-bezier(")
                        || part.starts_with("steps(")
                        || part.starts_with("linear(") =>
                    {
                        1
                    }
                    "infinite" => 3,
                    _ if numeric(part) => 3,
                    "normal" | "reverse" | "alternate" | "alternate-reverse"
                        if slots[4].is_none() =>
                    {
                        4
                    }
                    "forwards" | "backwards" | "both" | "none" if slots[5].is_none() => 5,
                    "running" | "paused" if slots[6].is_none() => 6,
                    _ => 7,
                };
                slots[slot] = Some(part);
            }
            slots
                .iter()
                .zip(["0s", "ease", "0s", "1", "normal", "none", "running", "none"])
                .map(|(value, initial)| value.unwrap_or(initial))
                .collect::<Vec<_>>()
                .join(" ")
        }),
        "columns" => match components(&out, ' ')[..] {
            [count, width] if count.parse::<u32>().is_ok() => format!("{width} {count}"),
            _ => out,
        },
        "translate" | "scale" => {
            let mut parts = components(&out, ' ');
            let zero = if name == "scale" { "1" } else { "0px" };
            if parts.len() == 3 && parts[2] == zero {
                parts.pop();
            }
            if parts.len() == 2
                && (if name == "scale" {
                    parts[1] == parts[0]
                } else {
                    parts[1] == zero
                })
            {
                parts.pop();
            }
            parts.join(" ")
        }
        "background-position"
        | "object-position"
        | "mask-position"
        | "transform-origin"
        | "perspective-origin" => layers(&out, &|parts| match parts[..] {
            [part @ ("top" | "bottom")] => format!("center {part}"),
            [part] => format!("{part} center"),
            _ => parts.join(" "),
        }),
        "counter-reset" | "counter-set" | "counter-increment" if out != "none" => {
            let parts = components(&out, ' ');
            let mut text = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                text.push(part.to_string());
                if !numeric(part) && parts.get(index + 1).is_none_or(|next| !numeric(next)) {
                    text.push(
                        if name == "counter-increment" {
                            "1"
                        } else {
                            "0"
                        }
                        .into(),
                    );
                }
            }
            text.join(" ")
        }
        "border-image-slice"
        | "border-image-width"
        | "border-image-outset"
        | "border-image-repeat" => {
            let mut parts = components(&out, ' ');
            let fill = parts
                .iter()
                .position(|part| *part == "fill")
                .map(|at| parts.remove(at));
            let sides = match parts.len() {
                0 => return out,
                1 => [0; 4],
                2 => [0, 1, 0, 1],
                3 => [0, 1, 2, 1],
                _ => [0, 1, 2, 3],
            };
            let mut text = shortest(sides.map(|side| parts[side]));
            if let Some(fill) = fill {
                text.push(' ');
                text.push_str(fill);
            }
            text
        }
        "opacity" | "fill-opacity" | "stroke-opacity" | "flood-opacity" | "stop-opacity" => {
            match out
                .strip_suffix('%')
                .and_then(|percent| percent.parse::<f64>().ok())
            {
                Some(percent) => number(percent / 100.),
                None => out,
            }
        }
        "box-shadow" | "text-shadow" if out != "none" => layers(&out, &|mut parts| {
            if let Some(color) = parts.iter().position(|part| {
                !numeric(part)
                    && *part != "inset"
                    && !["calc(", "min(", "max(", "clamp("]
                        .iter()
                        .any(|function| part.starts_with(function))
            }) {
                let color = parts.remove(color);
                parts.insert(0, color);
            }
            parts.join(" ")
        }),
        _ => out,
    }
}

// A math expression as Chrome's simplified text: like terms added, numbers first, then percentages, then
// other units in alphabetical order, and other functions last. None when it isn't such a sum.
pub(super) fn math_sum(text: &str, units: Option<&Lengths>) -> Option<String> {
    // The terms by unit, and the other functions with their signs and factors.
    type Terms = (Vec<(Cow<'static, str>, f64)>, Vec<(String, f64, f64)>);
    fn add(into: &mut Terms, from: Terms, scale: f64) {
        for (unit, value) in from.0 {
            match into.0.iter_mut().find(|(known, _)| *known == unit) {
                Some(term) => term.1 += value * scale,
                None => into.0.push((unit, value * scale)),
            }
        }
        into.1.extend(
            from.1
                .into_iter()
                .map(|(text, sign, factor)| (text, sign * scale, factor)),
        );
    }
    fn sum(rest: &mut &str, units: Option<&Lengths>) -> Option<Terms> {
        let mut terms = product(rest, units)?;
        loop {
            let trimmed = rest.trim_start();
            let sign = match trimmed.as_bytes() {
                [b'+', next, ..] if next.is_ascii_whitespace() => 1.,
                [b'-', next, ..] if next.is_ascii_whitespace() => -1.,
                _ => return Some(terms),
            };
            *rest = &trimmed[1..];
            let next = product(rest, units)?;
            add(&mut terms, next, sign);
        }
    }
    fn product(rest: &mut &str, units: Option<&Lengths>) -> Option<Terms> {
        let mut terms = atom(rest, units)?;
        loop {
            let trimmed = rest.trim_start();
            let divide = match trimmed.as_bytes().first() {
                Some(b'*') => false,
                Some(b'/') => true,
                _ => return Some(terms),
            };
            *rest = &trimmed[1..];
            let mut other = atom(rest, units)?;
            let number = |terms: &Terms| match &terms.0[..] {
                [(unit, value)] if unit.is_empty() && terms.1.is_empty() => Some(*value),
                _ => None,
            };
            let factor = match number(&other) {
                Some(0.) if divide => return None,
                Some(factor) if divide => 1. / factor,
                Some(factor) => factor,
                None if !divide => {
                    let factor = number(&terms)?;
                    std::mem::swap(&mut terms, &mut other);
                    factor
                }
                None => return None,
            };
            for term in &mut terms.0 {
                term.1 *= factor;
            }
            for term in &mut terms.1 {
                term.2 *= factor;
            }
        }
    }
    fn atom(rest: &mut &str, units: Option<&Lengths>) -> Option<Terms> {
        *rest = rest.trim_start();
        let bytes = rest.as_bytes();
        let name = bytes
            .iter()
            .position(|byte| !byte.is_ascii_alphanumeric() && *byte != b'-')
            .filter(|&end| end > 0 && bytes[end] == b'(' && bytes[0].is_ascii_alphabetic());
        if bytes.first() == Some(&b'(')
            || name.is_some_and(|end| rest[..end].eq_ignore_ascii_case("calc"))
        {
            *rest = &rest[name.unwrap_or(0) + 1..];
            let terms = sum(rest, units)?;
            *rest = rest.trim_start().strip_prefix(')')?;
            return Some(terms);
        }
        if let Some(end) = name {
            let mut depth = 0;
            let close = rest[end..].bytes().position(|byte| {
                depth += i32::from(byte == b'(') - i32::from(byte == b')');
                depth == 0
            })? + end;
            let mut text = String::new();
            canonical(&rest[..=close], false, false, &mut text);
            *rest = &rest[close + 1..];
            // Math of absolute units folds to `calc()`, which is a term.
            // Computed values resolve a function of lengths without percentages.
            if let Some((pixels, Kind::Length)) = units
                .filter(|_| !text.contains('%'))
                .and_then(|units| units.math(&text, f32::NAN, &[]))
            {
                return Some((vec![(Cow::Borrowed("px"), pixels)], Vec::new()));
            }
            if text.starts_with("calc(") {
                return atom(&mut text.as_str(), units);
            }
            return Some((Vec::new(), vec![(text, 1., 1.)]));
        }
        let word = bytes
            .iter()
            .position(|byte| !byte.is_ascii_alphabetic())
            .unwrap_or(bytes.len());
        let constant = match &rest[..word] {
            word if word.eq_ignore_ascii_case("e") => std::f64::consts::E,
            word if word.eq_ignore_ascii_case("pi") => std::f64::consts::PI,
            _ => f64::NAN,
        };
        if !constant.is_nan() {
            *rest = &rest[word..];
            return Some((vec![(Cow::Borrowed(""), constant)], Vec::new()));
        }
        let end = rest
            .char_indices()
            .find(|&(index, c)| {
                !(c.is_ascii_digit()
                    || c == '.'
                    || matches!(c, '+' | '-')
                        && (index == 0 || matches!(bytes[index - 1], b'e' | b'E'))
                    || matches!(c, 'e' | 'E')
                        && rest[index + 1..]
                            .starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-'))
            })
            .map_or(rest.len(), |(index, _)| index);
        let value: f64 = rest[..end].parse().ok()?;
        let unit_end = rest[end..]
            .find(|c: char| !c.is_ascii_alphabetic() && c != '%')
            .map_or(rest.len(), |at| end + at);
        // Known units need no copy.
        const UNITS: [&str; 18] = [
            "", "%", "px", "em", "rem", "ex", "ch", "lh", "rlh", "vw", "vh", "vmin", "vmax", "cqw",
            "cqh", "deg", "s", "fr",
        ];
        let unit = &rest[end..unit_end];
        let unit = UNITS
            .iter()
            .find(|known| known.eq_ignore_ascii_case(unit))
            .map_or_else(
                || Cow::Owned(unit.to_ascii_lowercase()),
                |known| Cow::Borrowed(*known),
            );
        *rest = &rest[unit_end..];
        let (absolute, scale) = match &*unit {
            "in" => (Some("px"), 96.),
            "cm" => (Some("px"), 96. / 2.54),
            "mm" => (Some("px"), 96. / 25.4),
            "q" => (Some("px"), 96. / 101.6),
            "pt" => (Some("px"), 4. / 3.),
            "pc" => (Some("px"), 16.),
            "rad" => (Some("deg"), 180. / std::f64::consts::PI),
            "grad" => (Some("deg"), 0.9),
            "turn" => (Some("deg"), 360.),
            "ms" => (Some("s"), 0.001),
            _ => (None, 1.),
        };
        let unit = absolute.map_or(unit, Cow::Borrowed);
        // Computed values resolve relative lengths to pixels.
        if let Some(pixels) = units
            .filter(|_| !matches!(&*unit, "" | "%" | "px" | "deg" | "s"))
            .and_then(|units| units.resolve(&format!("1{unit}"), f32::NAN))
        {
            return Some((
                vec![(Cow::Borrowed("px"), value * f64::from(pixels))],
                Vec::new(),
            ));
        }
        Some((vec![(unit, value * scale)], Vec::new()))
    }
    let mut rest = text.trim();
    let (mut terms, others) = sum(&mut rest, units)?;
    if !rest.trim().is_empty() || terms.is_empty() {
        return None;
    }
    terms.sort_by(|a, b| {
        let rank = |unit: &str| match unit {
            "" => 0,
            "%" => 1,
            _ => 2,
        };
        (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0))
    });
    let mut out = String::new();
    for (unit, value) in &terms {
        let value = match out.is_empty() {
            true => *value,
            false => {
                out.push_str(if *value < 0. { " - " } else { " + " });
                value.abs()
            }
        };
        write_printed(value, &mut out);
        out.push_str(unit);
    }
    for (other, sign, factor) in &others {
        if sign.abs() != 1. {
            return None;
        }
        out.push_str(if *sign < 0. { " - " } else { " + " });
        // Like Chrome, a scaled function is a product in parentheses.
        if *factor != 1. {
            out.push('(');
            write_printed(*factor, &mut out);
            out.push_str(" * ");
            out.push_str(other);
            out.push(')');
        } else {
            out.push_str(other);
        }
    }
    Some(out)
}

pub(super) fn canonical(value: &str, length: bool, named: bool, out: &mut String) {
    for (index, item) in components(value, ',').into_iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        for (index, part) in components(item.trim(), ' ').into_iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let lower = part.to_ascii_lowercase();
            let first = part.as_bytes()[0];
            if first == b'"' || first == b'\'' {
                let text = cssparser::Parser::new(part)
                    .expect_string()
                    .map_or_else(|_| part.to_owned(), |text| text.to_string());
                out.push('"');
                out.push_str(&text.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            } else if let Some((function, args)) =
                part.strip_suffix(')').and_then(|part| part.split_once('('))
            {
                let function = function.to_ascii_lowercase();
                match function.as_str() {
                    // Like Chrome, a local font name keeps its case, in quotes.
                    "local" => {
                        let name = args.trim();
                        let name = cssparser::Parser::new(name)
                            .expect_string()
                            .map_or_else(|_| name.to_owned(), |text| text.to_string());
                        out.push_str("local(\"");
                        out.push_str(&name.replace('\\', "\\\\").replace('"', "\\\""));
                        out.push_str("\")");
                    }
                    "url" => {
                        let url = args.trim();
                        let url = cssparser::Parser::new(url)
                            .expect_string()
                            .map_or_else(|_| url.to_owned(), |text| text.to_string());
                        out.push_str("url(\"");
                        out.push_str(&url.replace('\\', "\\\\").replace('"', "\\\""));
                        out.push_str("\")");
                    }
                    "rgb" | "rgba" | "hsl" | "hsla" | "hwb" | "lab" | "lch" | "oklab" | "oklch" => {
                        match serialize_color(part) {
                            Some(color) => out.push_str(&color),
                            None => out.push_str(&lower),
                        }
                    }
                    "drop-shadow" => {
                        let mut parts = String::new();
                        canonical(args, true, named, &mut parts);
                        let mut parts = components(&parts, ' ');
                        if let Some(color) = parts.iter().position(|part| {
                            !part.starts_with(|c: char| {
                                c.is_ascii_digit() || matches!(c, '-' | '+' | '.')
                            })
                        }) {
                            let color = parts.remove(color);
                            parts.insert(0, color);
                        }
                        out.push_str("drop-shadow(");
                        out.push_str(&parts.join(" "));
                        out.push(')');
                    }
                    _ => {
                        // Math of absolute units folds into one value, like `round(23px, 10px)` to `calc(20px)`.
                        const WORDS: &str = "calc min max clamp round mod rem abs sign sin cos tan asin acos atan atan2 pow sqrt hypot log exp e pi up down nearest to zero px in cm mm q pt pc deg rad grad turn s ms";
                        let math = WORDS.split(' ').take(21).any(|name| name == function);
                        let folded = (math
                            && lower
                                .split(|c: char| !c.is_ascii_alphanumeric() || c.is_ascii_digit())
                                .all(|word| word.is_empty() || WORDS.split(' ').any(|known| known == word))
                            // `rem` is a function here, not the unit.
                            && !lower.match_indices("rem").any(|(at, _)| {
                                at > 0 && matches!(lower.as_bytes()[at - 1], b'0'..=b'9' | b'.')
                            }))
                        .then(|| Lengths::new(16., 100., 100.).math(&lower, f32::NAN, &[]))
                        .flatten()
                        .filter(|(value, _)| value.is_finite());
                        let function = if folded.is_some() {
                            "calc".to_owned()
                        } else {
                            function
                        };
                        // Transform functions keep their camel case.
                        let camel = [
                            "translateX",
                            "translateY",
                            "translateZ",
                            "translate3d",
                            "scaleX",
                            "scaleY",
                            "scaleZ",
                            "scale3d",
                            "rotateX",
                            "rotateY",
                            "rotateZ",
                            "rotate3d",
                            "skewX",
                            "skewY",
                            "matrix3d",
                        ];
                        out.push_str(
                            camel
                                .iter()
                                .find(|name| name.eq_ignore_ascii_case(&function))
                                .unwrap_or(&&*function),
                        );
                        out.push('(');
                        match folded {
                            Some((value, kind)) => {
                                write_printed(value, out);
                                out.push_str(match kind {
                                    Kind::Length => "px",
                                    Kind::Angle => "deg",
                                    Kind::Time => "s",
                                    _ => "",
                                });
                            }
                            None => {
                                let start = out.len();
                                if math {
                                    // Like Chrome, each argument is simplified to a sum of terms.
                                    for (index, arg) in
                                        components(args, ',').into_iter().enumerate()
                                    {
                                        if index > 0 {
                                            out.push_str(", ");
                                        }
                                        match math_sum(arg, None) {
                                            Some(sum) => out.push_str(&sum),
                                            None => canonical(arg.trim(), false, named, out),
                                        }
                                    }
                                } else {
                                    canonical(args, function.starts_with("translate"), named, out);
                                }
                                // Folded values inside math lose their `calc()`.
                                let mut from = start;
                                while math
                                    && let Some(at) = out[from..].find("calc(").map(|at| from + at)
                                {
                                    match out[at + 5..].find(')').map(|end| at + 5 + end) {
                                        Some(end) if !out[at + 5..end].contains(['(', ' ']) => {
                                            out.remove(end);
                                            out.replace_range(at..at + 5, "");
                                            from = at;
                                        }
                                        _ => from = at + 5,
                                    }
                                }
                                // The default direction of a linear gradient is left out.
                                if function.ends_with("linear-gradient") {
                                    for default in ["to bottom, ", "180deg, "] {
                                        if out[start..].starts_with(default) {
                                            out.replace_range(start..start + default.len(), "");
                                        }
                                    }
                                }
                            }
                        }
                        out.push(')');
                    }
                }
            } else if first == b'#' && !named {
                out.push_str(&serialize_color(part).unwrap_or(lower));
            } else if part.len() > 1 && part.contains('/') && !part.contains(['(', '"', '\'']) {
                for (index, side) in part.split('/').enumerate() {
                    if index > 0 {
                        out.push_str(" / ");
                    }
                    canonical(side.trim(), length, named, out);
                }
            } else if first.is_ascii_digit()
                || matches!(first, b'+' | b'-' | b'.')
                    && part
                        .as_bytes()
                        .get(1)
                        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
            {
                let end = part
                    .char_indices()
                    .find(|&(index, c)| {
                        !(c.is_ascii_digit()
                            || c == '.'
                            || (c == '+' || c == '-')
                                && (index == 0
                                    || matches!(part.as_bytes()[index - 1], b'e' | b'E'))
                            || matches!(c, 'e' | 'E')
                                && part[index + 1..].starts_with(|c: char| {
                                    c.is_ascii_digit() || c == '+' || c == '-'
                                }))
                    })
                    .map_or(part.len(), |(index, _)| index);
                match part[..end].parse::<f64>() {
                    Ok(number) => {
                        write_number(number, out);
                        let unit = &lower[end..];
                        out.push_str(if unit.is_empty() && number == 0. && length {
                            "px"
                        } else {
                            unit
                        });
                    }
                    Err(_) => out.push_str(part),
                }
            } else if named {
                out.push_str(part);
            } else {
                out.push_str(&lower);
            }
        }
    }
}
pub(crate) fn shortest(sides: [&str; 4]) -> String {
    let count = if sides[3] != sides[1] {
        4
    } else if sides[2] != sides[0] {
        3
    } else if sides[1] != sides[0] {
        2
    } else {
        1
    };
    sides[..count].join(" ")
}
