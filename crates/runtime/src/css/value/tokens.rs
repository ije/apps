pub(crate) fn components(value: &str, separator: char) -> Vec<&str> {
  let bytes = value.as_bytes();
  let mut parts = Vec::new();
  let (mut open, mut depth, mut start, mut closed) = (0u64, 0u32, 0, false);
  let mut quote_end = 0;
  let simple = 'scan: {
    for (index, &byte) in bytes.iter().enumerate() {
      if index < quote_end {
        continue;
      }
      // The tokenizer ends a value after a block, as in `rotate(1deg)scale(2)`, so this fast scan must
      // give up there.
      if std::mem::take(&mut closed)
        && separator == ' '
        && !byte.is_ascii_whitespace()
        && byte != b','
      {
        break 'scan false;
      }
      match byte {
        // A string without escapes or newlines is one token, so the scan can skip it.
        b'"' | b'\'' => {
          match bytes[index + 1..]
            .iter()
            .position(|&c| c == byte || matches!(c, b'\\' | b'\n' | b'\r' | b'\x0c'))
          {
            Some(end) if bytes[index + 1 + end] == byte => quote_end = index + end + 2,
            _ => break 'scan false,
          }
        }
        b'\\' => break 'scan false,
        b'/' if bytes.get(index + 1) == Some(&b'*') => break 'scan false,
        b'(' | b'[' | b'{' => {
          // `url(` without a quote is an unquoted URL token, which needs the tokenizer.
          if depth == 32
            || byte == b'('
              && index >= 3
              && bytes[index - 3..index].eq_ignore_ascii_case(b"url")
              && !matches!(
                bytes[index + 1..].iter().find(|c| !c.is_ascii_whitespace()),
                Some(b'"' | b'\'')
              )
          {
            break 'scan false;
          }
          open = open << 2 | u64::from(byte / 32 - 1);
          depth += 1;
        }
        b')' | b']' | b'}' => {
          if depth == 0 || open & 3 != u64::from(byte / 32 - 1) {
            break 'scan false;
          }
          open >>= 2;
          depth -= 1;
          closed = depth == 0;
        }
        byte if depth == 0 && separator != ' ' && byte == separator as u8 => {
          parts.extend(Some(value[start..index].trim()).filter(|part| !part.is_empty()));
          start = index + 1;
        }
        byte if depth == 0 && separator == ' ' && byte.is_ascii_whitespace() => {
          parts.extend(Some(value[start..index].trim()).filter(|part| !part.is_empty()));
          start = index + 1;
        }
        _ => {}
      }
    }
    depth == 0
  };
  if simple {
    parts.extend(Some(value[start..].trim()).filter(|part| !part.is_empty()));
    return parts;
  }
  tokenized(value, separator)
}

pub(crate) fn tokenized(value: &str, separator: char) -> Vec<&str> {
  let mut parts = Vec::new();
  let mut input = cssparser::Parser::new(value);
  let mut start = input.position();
  let mut block = false;
  loop {
    let position = input.position();
    let Ok(token) = input.next_including_whitespace().cloned() else {
      break;
    };
    if separator == ' '
      && std::mem::take(&mut block)
      && !matches!(
        token,
        cssparser::Token::WhiteSpace(_) | cssparser::Token::Comma | cssparser::Token::Delim(_)
      )
    {
      let part = input.slice(start..position).trim();
      if !part.is_empty() {
        parts.push(part);
      }
      start = position;
    }
    if matches!(token, cssparser::Token::Comma) && separator == ','
      || matches!(token, cssparser::Token::WhiteSpace(_)) && separator == ' '
      || matches!(token, cssparser::Token::Delim(delim) if delim == separator)
    {
      let part = input.slice(start..position).trim();
      if !part.is_empty() {
        parts.push(part);
      }
      start = input.position();
    } else if matches!(
      token,
      cssparser::Token::Function(_)
        | cssparser::Token::ParenthesisBlock
        | cssparser::Token::SquareBracketBlock
        | cssparser::Token::CurlyBracketBlock
    ) {
      if input
        .parse_nested_block(|input| {
          input
            .expect_no_error_token()
            .map_err(cssparser::ParseError::<()>::from)
        })
        .is_err()
      {
        return Vec::new();
      }
      block = true;
    }
  }
  let part = input.slice_from(start).trim();
  if !part.is_empty() {
    parts.push(part);
  }
  parts
}

#[test]
fn numbers_print_like_the_exact_formatter() {
  let exact = |value: f64| {
    let value = if value.abs() < 1e-9 { 0. } else { value };
    let digits = if value == 0. || !value.is_finite() {
      0
    } else {
      (5 - value.abs().log10().floor() as i32).max(0) as usize
    };
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') {
      text.trim_end_matches('0').trim_end_matches('.')
    } else {
      &text
    };
    if text == "-0" { "0" } else { text }.to_owned()
  };
  let mut values = vec![0., -0., 1., 2.5, 0.3, 1e-9, -1e-9, 2e-9, 9.999995];
  values.extend([9.9999949, 0.0000125, 123456.5, 99999.95, 1e300]);
  values.extend([12.000004, 0.00999999, f64::NAN]);
  values.extend([0.1 + 0.2, 1. / 3., -2. / 3., 1. / 0.]);
  values.extend((1..60).flat_map(|power| [2f64.powi(-power), -3. * 2f64.powi(-power)]));
  let mut seed = 7u64;
  for _ in 0..200_000 {
    seed = seed
      .wrapping_mul(6364136223846793005)
      .wrapping_add(1442695040888963407);
    let mantissa = (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
    values.push(mantissa * 10f64.powi((seed % 19) as i32 - 10));
    values.push(((seed >> 20) % 2_000_000) as f64 / 10f64.powi((seed % 9) as i32));
  }
  for value in values {
    assert_eq!(number(value), exact(value), "{value:e}");
  }
}

#[test]
fn split_values_like_the_tokenizer() {
  let values = [
    "0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)",
    "rotate(1deg)scale(2)",
    "f(x)(y)",
    "f(x)[y]",
    "f(a]b) c",
    "calc(1px)/2 3",
    "calc(1px), 2",
    "  a \t b\n c  ",
    "a,b, ,c,",
    "f(x) , g(y)",
    "translate(1px, 2px) rotate(3deg)",
    "repeat(2, minmax(0, 1fr)) [a b] 1fr",
    "f((1)) g",
    "f(",
    "a)",
    "url(a b) c",
    "URL(x) y",
    "\"a b\" c",
    "a /* b */ c",
    "var(--a,1px) 2px",
    "1px-2px",
    "f(1)%",
    "f(1)-1px",
    "{a b} c",
    "ﬁ g(ä) h",
  ];
  let mut seed = 1u64;
  let random: Vec<String> = (0..20000)
    .map(|_| {
      (0..seed % 24)
        .map(|_| {
          seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
          b"ab1-+.%/*(),[]{}  \t#"[(seed >> 33) as usize % 21] as char
        })
        .collect()
    })
    .collect();
  for value in values
    .iter()
    .copied()
    .chain(random.iter().map(String::as_str))
  {
    for separator in [' ', ',', '/'] {
      assert_eq!(
        components(value, separator),
        tokenized(value, separator),
        "{value:?} {separator:?}"
      );
    }
  }
}

pub(crate) fn number(value: f64) -> String {
  let mut out = String::new();
  write_number(value, &mut out);
  out
}

// Chrome's `%.6g` text of a number: 6 significant digits, an exponent below 1e-4 and from 1e6, within the f32
// range.
pub(crate) fn write_printed(value: f64, out: &mut String) {
  use std::fmt::Write;
  let value = value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX));
  if value == 0. || value.is_nan() {
    out.push('0');
    return;
  }
  // Here both print the same digits, and `write_number` needs no allocation.
  if (1e-3..999_999.).contains(&value.abs()) {
    return write_number(value, out);
  }
  let text = format!("{value:.5e}");
  let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
  let exponent: i32 = exponent.parse().unwrap_or(0);
  let trim = |text: &str| match text.contains('.') {
    true => text.trim_end_matches('0').trim_end_matches('.').len(),
    false => text.len(),
  };
  if (-4..6).contains(&exponent) {
    let start = out.len();
    let _ = write!(out, "{value:.*}", (5 - exponent) as usize);
    out.truncate(start + trim(&out[start..]));
  } else {
    let sign = if exponent < 0 { '-' } else { '+' };
    let _ = write!(
      out,
      "{}e{sign}{:02}",
      &mantissa[..trim(mantissa)],
      exponent.abs()
    );
  }
}

pub(crate) fn write_number(value: f64, out: &mut String) {
  use std::fmt::Write;
  let value = if value.abs() < 1e-9 { 0. } else { value };
  let digits = if value == 0. || !value.is_finite() {
    0
  } else {
    (5 - value.abs().log10().floor() as i32).max(0) as usize
  };
  let scaled = value.abs() * 10f64.powi(digits as i32);
  if (1..=15).contains(&digits) && (scaled.fract() - 0.5).abs() > 1e-6 {
    let (whole, power) = (scaled.round() as u64, 10u64.pow(digits as u32));
    if value < 0. {
      out.push('-');
    }
    let _ = write!(out, "{}", whole / power);
    let (mut fraction, mut width) = (whole % power, digits);
    if fraction != 0 {
      while fraction % 10 == 0 {
        fraction /= 10;
        width -= 1;
      }
      let _ = write!(out, ".{fraction:0width$}");
    }
    return;
  }
  let start = out.len();
  let _ = write!(out, "{value:.digits$}");
  if out[start..].contains('.') {
    let end = out.trim_end_matches('0').trim_end_matches('.').len();
    out.truncate(end);
  }
  if &out[start..] == "-0" {
    out.truncate(start);
    out.push('0');
  }
}

#[test]
fn quoted_strings_split_like_the_tokenizer() {
  for (value, separator) in [
    ("url(\"data:a,b c\"), url( 'x)y' ), none", ','),
    ("\"a b\" 'c,d' e", ' '),
    ("url(\"a\")url(b)", ' '),
    ("url(a b)", ' '),
    ("\"a\\\"b\", c", ','),
    ("\"open, x", ','),
  ] {
    assert_eq!(
      components(value, separator),
      tokenized(value, separator),
      "{value}"
    );
  }
}
