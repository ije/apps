pub(crate) fn counter_text(kind: &str, number: i64) -> String {
  match kind {
    "none" => String::new(),
    "disc" => "•".into(),
    "circle" => "◦".into(),
    "square" => "▪".into(),
    "disclosure-closed" => "▸".into(),
    "disclosure-open" => "▾".into(),
    "decimal-leading-zero" => format!("{number:02}"),
    "lower-alpha" | "lower-latin" | "upper-alpha" | "upper-latin" if number > 0 => {
      let mut letters = Vec::new();
      let mut rest = number;
      while rest > 0 {
        rest -= 1;
        letters.push((b'a' + (rest % 26) as u8) as char);
        rest /= 26;
      }
      let letters: String = letters.into_iter().rev().collect();
      if kind.starts_with("upper") {
        letters.to_ascii_uppercase()
      } else {
        letters
      }
    }
    "lower-roman" | "upper-roman" if (1..4000).contains(&number) => {
      let mut rest = number;
      let mut roman = String::new();
      for (value, digits) in [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
      ] {
        while rest >= value {
          roman.push_str(digits);
          rest -= value;
        }
      }
      if kind == "upper-roman" {
        roman.to_ascii_uppercase()
      } else {
        roman
      }
    }
    "lower-greek" | "hiragana" | "katakana" if number > 0 => {
      let digits: Vec<char> = match kind {
                "lower-greek" => "αβγδεζηθικλμνξοπρστυφχψω",
                "hiragana" => "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわゐゑをん",
                _ => "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヰヱヲン",
            }
            .chars()
            .collect();
      let mut letters = Vec::new();
      let mut rest = number;
      while rest > 0 {
        rest -= 1;
        letters.push(digits[(rest % digits.len() as i64) as usize]);
        rest /= digits.len() as i64;
      }
      letters.into_iter().rev().collect()
    }
    "cjk-decimal" if number >= 0 => number
      .to_string()
      .chars()
      .map(|digit| {
        "〇一二三四五六七八九"
          .chars()
          .nth(digit as usize - '0' as usize)
          .unwrap_or(digit)
      })
      .collect(),
    "cjk-ideographic" | "trad-chinese-informal" | "simp-chinese-informal"
      if (0..10000).contains(&number) =>
    {
      let digits: Vec<char> = "零一二三四五六七八九".chars().collect();
      let mut text = String::new();
      let mut zero = false;
      for (unit, name) in [(1000, "千"), (100, "百"), (10, "十"), (1, "")] {
        let digit = (number / unit % 10) as usize;
        if digit == 0 {
          zero = !text.is_empty();
          continue;
        }
        if std::mem::take(&mut zero) {
          text.push('零');
        }
        if !(unit == 10 && digit == 1 && number < 20) {
          text.push(digits[digit]);
        }
        text.push_str(name);
      }
      if text.is_empty() {
        text.push('零');
      }
      text
    }
    "armenian" | "upper-armenian" | "lower-armenian" | "georgian" | "hebrew"
      if (1..10000).contains(&number) =>
    {
      let symbols: &[(i64, &str)] = match kind {
        "georgian" => &[
          (9000, "ჰ"),
          (8000, "ჯ"),
          (7000, "ჴ"),
          (6000, "ხ"),
          (5000, "ჭ"),
          (4000, "წ"),
          (3000, "ძ"),
          (2000, "ც"),
          (1000, "ჩ"),
          (900, "შ"),
          (800, "ყ"),
          (700, "ღ"),
          (600, "ქ"),
          (500, "ფ"),
          (400, "ჳ"),
          (300, "ტ"),
          (200, "ს"),
          (100, "რ"),
          (90, "ჟ"),
          (80, "პ"),
          (70, "ო"),
          (60, "ჲ"),
          (50, "ნ"),
          (40, "მ"),
          (30, "ლ"),
          (20, "კ"),
          (10, "ი"),
          (9, "თ"),
          (8, "ჱ"),
          (7, "ზ"),
          (6, "ვ"),
          (5, "ე"),
          (4, "დ"),
          (3, "გ"),
          (2, "ბ"),
          (1, "ა"),
        ],
        "hebrew" => &[
          (400, "ת"),
          (300, "ש"),
          (200, "ר"),
          (100, "ק"),
          (90, "צ"),
          (80, "פ"),
          (70, "ע"),
          (60, "ס"),
          (50, "נ"),
          (40, "מ"),
          (30, "ל"),
          (20, "כ"),
          (19, "יט"),
          (18, "יח"),
          (17, "יז"),
          (16, "טז"),
          (15, "טו"),
          (10, "י"),
          (9, "ט"),
          (8, "ח"),
          (7, "ז"),
          (6, "ו"),
          (5, "ה"),
          (4, "ד"),
          (3, "ג"),
          (2, "ב"),
          (1, "א"),
        ],
        _ => &[
          (9000, "Ք"),
          (8000, "Փ"),
          (7000, "Ւ"),
          (6000, "Ց"),
          (5000, "Ր"),
          (4000, "Տ"),
          (3000, "Վ"),
          (2000, "Ս"),
          (1000, "Ռ"),
          (900, "Ջ"),
          (800, "Պ"),
          (700, "Չ"),
          (600, "Ո"),
          (500, "Շ"),
          (400, "Ն"),
          (300, "Յ"),
          (200, "Մ"),
          (100, "Ճ"),
          (90, "Ղ"),
          (80, "Ձ"),
          (70, "Հ"),
          (60, "Կ"),
          (50, "Ծ"),
          (40, "Խ"),
          (30, "Լ"),
          (20, "Ի"),
          (10, "Ժ"),
          (9, "Թ"),
          (8, "Ը"),
          (7, "Է"),
          (6, "Զ"),
          (5, "Ե"),
          (4, "Դ"),
          (3, "Գ"),
          (2, "Բ"),
          (1, "Ա"),
        ],
      };
      let mut rest = number;
      let mut text = String::new();
      for &(value, symbol) in symbols {
        while rest >= value {
          text.push_str(symbol);
          rest -= value;
        }
      }
      if kind == "lower-armenian" {
        text.to_lowercase()
      } else {
        text
      }
    }
    _ => number.to_string(),
  }
}

#[test]
fn counters_name_zero_and_disclosure_like_chrome() {
  for (kind, number, text) in [
    ("cjk-ideographic", 0, "零"),
    ("trad-chinese-informal", 0, "零"),
    ("simp-chinese-informal", 0, "零"),
    ("disclosure-closed", 3, "▸"),
    ("disclosure-open", 3, "▾"),
  ] {
    assert_eq!(counter_text(kind, number), text, "{kind} {number}");
  }
}
