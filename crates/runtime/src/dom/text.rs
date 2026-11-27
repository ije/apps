pub(crate) fn bullets(value: &str, offset: usize) -> usize {
  value[..utf16_index(value, offset)].chars().count()
}

pub(crate) fn unbullet(value: &str, bullets: usize) -> usize {
  value.chars().take(bullets).map(char::len_utf16).sum()
}

pub(crate) fn shown_index(source: &str, shown: &str, offset: usize) -> usize {
  if source.is_empty() || source == shown {
    return utf16_index(shown, offset);
  }
  let (mut at, mut units) = (0, 0);
  for c in source.chars() {
    if units >= offset {
      break;
    }
    units += c.len_utf16();
    at += shown_len(c, &shown[at..]);
  }
  at
}

pub(crate) fn source_offset(source: &str, shown: &str, byte: usize) -> usize {
  if source.is_empty() || source == shown {
    return shown[..shown.floor_char_boundary(byte)]
      .encode_utf16()
      .count();
  }
  let (mut at, mut units) = (0, 0);
  for c in source.chars() {
    if at >= byte {
      break;
    }
    at += shown_len(c, &shown[at..]);
    // Like Chrome, a place inside the letters of one case mapping, like the `SS` of `ß`, is before them.
    if at > byte {
      break;
    }
    units += c.len_utf16();
  }
  units
}

/// The bytes at the start of `shown` that the source char `c` became: none for collapsed white space, one char, or
/// the chars of a case mapping like `ß` to `SS`.
fn shown_len(c: char, shown: &str) -> usize {
  let Some(next) = shown.chars().next() else {
    return 0;
  };
  if c.is_ascii_whitespace() && next != ' ' && next != c {
    return 0;
  }
  let full = |mapped: &mut dyn ExactSizeIterator<Item = char>| {
    if mapped.len() < 2 {
      return None;
    }
    let (mut rest, mut len) = (shown.chars(), 0);
    for m in mapped {
      if rest.next() != Some(m) {
        return None;
      }
      len += m.len_utf8();
    }
    Some(len)
  };
  if next != c
    && let Some(len) = full(&mut c.to_uppercase()).or_else(|| full(&mut c.to_lowercase()))
  {
    return len;
  }
  next.len_utf8()
}

pub(crate) fn utf16_index(text: &str, index: usize) -> usize {
  let mut offset = 0;
  for (byte, c) in text.char_indices() {
    if offset >= index {
      return byte;
    }
    offset += c.len_utf16();
  }
  text.len()
}

#[cfg(test)]
mod tests {

  #[test]
  fn offsets_map_through_collapsed_white_space() {
    let (source, shown) = ("  a  b\n c ", "a b c");
    for (offset, byte) in [
      (0, 0),
      (2, 0),
      (3, 1),
      (5, 2),
      (6, 3),
      (8, 4),
      (9, 5),
      (10, 5),
    ] {
      assert_eq!(super::shown_index(source, shown, offset), byte, "{offset}");
    }
    assert_eq!(super::source_offset(source, shown, 0), 0);
    assert_eq!(super::source_offset(source, shown, 1), 3);
    assert_eq!(super::source_offset(source, shown, 2), 4);
    assert_eq!(super::source_offset(source, shown, 3), 6);
    assert_eq!(super::shown_index("héllo", "héllo", 2), 3);
    assert_eq!(super::shown_index("", "abc", 2), 2);
    // `text-transform: uppercase` makes one `ß` two letters.
    let (source, shown) = ("groß ist", "GROSS IST");
    assert_eq!(super::shown_index(source, shown, 4), 5);
    assert_eq!(super::shown_index(source, shown, 5), 6);
    assert_eq!(super::source_offset(source, shown, 5), 4);
    assert_eq!(super::source_offset(source, shown, 4), 3);
    assert_eq!(super::source_offset(source, shown, 7), 6);
  }
}
