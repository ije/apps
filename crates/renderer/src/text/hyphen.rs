use super::{Fonts, Line, TextRun, TextSystem};
use crate::Pixels;
use rustc_hash::FxHashMap;
use std::rc::Rc;

pub const SOFT_HYPHEN: char = '\u{ad}';

/// Other values than `hyphens: manual` with the hyphen of the font.
#[derive(Debug, PartialEq)]
pub struct Hyphenation {
  /// Whether soft hyphens break.
  pub manual: bool,
  /// The language whose words break.
  pub auto: Option<Rc<str>>,
  pub character: Option<Rc<str>>,
}

/// A place where a row can end with a hyphen.
#[derive(Clone)]
pub struct Hyphen {
  /// The byte offset of the character that starts the next row.
  pub at: u32,
  pub line: Rc<Line>,
  /// In the last word, which keeps whole when it fits a row of its own.
  pub last: bool,
}

/// Byte offsets by word, for each language.
pub(super) type Syllables = Vec<(Rc<str>, FxHashMap<Rc<str>, Rc<[u32]>>)>;

impl Fonts {
  fn syllables(&self, word: &str, language: &Rc<str>) -> Rc<[u32]> {
    let mut all = self.syllables.borrow_mut();
    let mut at = all.iter().position(|(known, _)| known == language);
    if let Some(known) = at.and_then(|at| all[at].1.get(word)) {
      return known.clone();
    }
    let offsets = self.backend.hyphenate(word, language).into_iter();
    let found: Rc<[u32]> = offsets.map(|offset| offset as u32).collect();
    if all.iter().map(|(_, words)| words.len()).sum::<usize>() >= 8192 {
      all.clear();
      at = None;
    }
    let at = at.unwrap_or_else(|| {
      all.push((language.clone(), FxHashMap::default()));
      all.len() - 1
    });
    all[at].1.insert(word.into(), found.clone());
    found
  }
}

pub(super) fn find(hyphens: &[Hyphen], at: usize) -> Option<&Hyphen> {
  let found = hyphens.binary_search_by_key(&(at as u32), |hyphen| hyphen.at);
  Some(&hyphens[found.ok()?])
}

impl TextSystem {
  pub(super) fn hyphens(
    &self,
    text: &str,
    pieces: &[(usize, &TextRun)],
    font_size: Pixels,
  ) -> Option<Rc<[Hyphen]>> {
    let auto = |run: &TextRun| {
      run
        .hyphenation
        .as_ref()
        .is_some_and(|kind| kind.auto.is_some())
    };
    if !text.contains(SOFT_HYPHEN) && !pieces.iter().any(|(_, run)| auto(run)) {
      return None;
    }
    // The last word starts here or later.
    let tail = text.trim_end().rfind(char::is_whitespace).unwrap_or(0);
    let mut hyphens = Vec::new();
    let mut start = 0;
    for &(len, run) in pieces {
      let piece = start..start + len;
      start += len;
      let kind = run.hyphenation.as_deref();
      let mut places: Vec<usize> = Vec::new();
      let mut last = usize::MAX;
      if kind.is_none_or(|kind| kind.manual) {
        let soft = text[piece.clone()].match_indices(SOFT_HYPHEN);
        places.extend(soft.map(|(at, mark)| piece.start + at + mark.len()));
      }
      if let Some(language) = kind.and_then(|kind| kind.auto.as_ref()) {
        let mut at = piece.start;
        let english = language
          .get(..2)
          .is_some_and(|name| name.eq_ignore_ascii_case("en"));
        for word in text[piece]
          .split(|ch: char| !ch.is_alphanumeric() && !matches!(ch, SOFT_HYPHEN | '\'' | '\u{2019}'))
        {
          // Like Chrome, short words and capitalized English words keep whole.
          if word.chars().nth(4).is_some()
            && !word.contains(SOFT_HYPHEN)
            && !(english && word.starts_with(char::is_uppercase))
          {
            if at >= tail {
              last = last.min(at);
            }
            let offsets = self.syllables(word, language);
            places.extend(offsets.iter().map(|&offset| at + offset as usize));
          }
          at += word.len();
          at += text[at..].chars().next().map_or(0, char::len_utf8);
        }
        places.sort_unstable();
      }
      places.retain(|&at| at < text.len());
      if places.is_empty() {
        continue;
      }
      let size = run.font_size.unwrap_or(font_size);
      let mark = match kind.and_then(|kind| kind.character.as_deref()) {
        Some(mark) => mark,
        None if self.covers(self.face(&run.font), '\u{2010}') => "\u{2010}",
        None => "-",
      };
      let run = run.part(mark.len(), Default::default());
      let line = self.shape_line(mark, size, &run).line;
      hyphens.extend(places.into_iter().map(|at| Hyphen {
        at: at as u32,
        line: line.clone(),
        last: at > last,
      }));
    }
    (!hyphens.is_empty()).then(|| hyphens.into())
  }
}

#[cfg(test)]
mod tests {
  use crate::{Font, Hyphenation, TextRun, TextSystem, Wrapping, px};
  use std::rc::Rc;

  #[test]
  fn rows_end_with_hyphens_at_soft_hyphens_and_syllables() {
    let system = TextSystem::new();
    let rows = |text: &str, hyphenation: Option<Hyphenation>, width| {
      let run = TextRun {
        len: text.len(),
        font: Font::new("Helvetica"),
        hyphenation: hyphenation.map(Rc::new),
        ..TextRun::default()
      };
      let wrapping = Wrapping {
        width: Some(px(width)),
        ..Wrapping::default()
      };
      let lines = system.shape_text(text, px(18.), &[run], &wrapping);
      let line = &lines[0];
      let rows = (0..line.rows()).map(|row| {
        let text = &line.text[line.row_index(row)..line.row_index(row + 1)];
        let hyphen = if line.hyphen(row).is_some() { "-" } else { "" };
        text.replace('\u{ad}', "") + hyphen
      });
      rows.collect::<Vec<_>>()
    };
    let text = "extra\u{ad}ordi\u{ad}nary";
    let kind = |manual, auto: Option<&str>| {
      Some(Hyphenation {
        manual,
        auto: auto.map(Into::into),
        character: None,
      })
    };
    assert_eq!(rows(text, None, 100.), ["extraordi-", "nary"]);
    assert_eq!(rows(text, None, 200.), ["extraordinary"]);
    assert_eq!(rows(text, None, 1.), ["extra-", "ordi-", "nary"]);
    assert_eq!(rows(text, kind(false, None), 100.), ["extraordinary"]);
    // The hyphen must fit, or the row breaks earlier.
    let exact = {
      let run = TextRun {
        len: 9,
        font: Font::new("Helvetica"),
        ..TextRun::default()
      };
      system.shape_line("extraordi", px(18.), &run).line.width.0
    };
    assert_eq!(rows(text, None, exact + 1.), ["extra-", "ordinary"]);
    if cfg!(target_os = "macos") {
      let auto = || kind(true, Some("en"));
      assert_eq!(
        rows("An incomprehensible thing", auto(), 80.),
        ["An in-", "compre-", "hensible ", "thing"]
      );
      // Capitalized words, and the last word when it fits a row.
      assert_eq!(
        rows("x Hyphenation is", auto(), 60.),
        ["x ", "Hyphenation ", "is"]
      );
      assert_eq!(rows("the automobile", auto(), 100.), ["the ", "automobile"]);
      assert_eq!(
        rows("extraordinary x", kind(true, Some("zz")), 60.),
        ["extraordinary ", "x"]
      );
    }
  }
}
