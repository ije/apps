use serde_json::{Value, json};

// Keeps only the sources and mappings of an inline source map, without its source texts.
pub(super) fn compact(url: &str) -> Option<String> {
  let mut map: Value = serde_json::from_slice(&decode(url)?).ok()?;
  Some(json!({"sources": map["sources"].take(), "mappings": map["mappings"].take()}).to_string())
}

fn decode(url: &str) -> Option<Vec<u8>> {
  let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
  if head.ends_with(";base64") {
    return base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok();
  }
  let mut bytes = Vec::with_capacity(data.len());
  let mut input = data.bytes();
  while let Some(byte) = input.next() {
    if byte != b'%' {
      bytes.push(byte);
      continue;
    }
    let hex = [input.next()?, input.next()?];
    bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
  }
  Some(bytes)
}

pub(super) struct SourceMap {
  sources: Vec<String>,
  mappings: Vec<[u32; 5]>,
}

impl SourceMap {
  pub(super) fn parse(raw: &str) -> Option<Self> {
    let map: Value = match raw.starts_with('{') {
      true => serde_json::from_str(raw).ok()?,
      false => serde_json::from_slice(&decode(raw)?).ok()?,
    };
    let sources: Vec<String> = map["sources"]
      .as_array()?
      .iter()
      .map(|source| source.as_str().unwrap_or("").to_owned())
      .collect();
    let mut mappings = Vec::new();
    let (mut source, mut line, mut column) = (0i64, 0i64, 0i64);
    for (generated, group) in map["mappings"].as_str()?.split(';').enumerate() {
      let mut generated_column = 0i64;
      for segment in group.split(',').filter(|segment| !segment.is_empty()) {
        let (mut values, mut count, mut value, mut shift) = ([0i64; 5], 0, 0i64, 0);
        for byte in segment.bytes() {
          let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            _ => 63,
          } as i64;
          if shift > 55 {
            return None;
          }
          value += (digit & 31) << shift;
          if digit & 32 != 0 {
            shift += 5;
            continue;
          }
          values[count.min(4)] = if value & 1 == 1 {
            -(value >> 1)
          } else {
            value >> 1
          };
          (count, value, shift) = (count + 1, 0, 0);
        }
        generated_column += values[0];
        if count >= 4 {
          (source, line, column) = (source + values[1], line + values[2], column + values[3]);
          if !(0..sources.len() as i64).contains(&source) || line < 0 || column < 0 {
            return None;
          }
          mappings.push([
            generated as u32,
            generated_column as u32,
            source as u32,
            line as u32,
            column as u32,
          ]);
        }
      }
    }
    Some(Self { sources, mappings })
  }

  // The source, line and column of a place in the generated code.
  pub(super) fn original(&self, line: u32, column: u32) -> Option<(&str, u32, u32)> {
    let index = self
      .mappings
      .partition_point(|mapping| (mapping[0], mapping[1]) <= (line, column));
    let mapping = self.mappings[..index]
      .last()
      .filter(|mapping| mapping[0] == line)
      .or_else(|| {
        self
          .mappings
          .get(index)
          .filter(|mapping| mapping[0] == line)
      })?;
    Some((&self.sources[mapping[2] as usize], mapping[3], mapping[4]))
  }

  // The place in the generated code of a line and column in the first source that `source` accepts.
  pub(super) fn generated(
    &self,
    source: impl Fn(&str) -> bool,
    line: u32,
    column: u32,
  ) -> Option<(u32, u32)> {
    let source = self.sources.iter().position(|name| source(name))? as u32;
    self
      .mappings
      .iter()
      .filter(|mapping| mapping[2] == source && mapping[3] >= line)
      .min_by_key(|mapping| (mapping[3], mapping[4] < column, mapping[0], mapping[1]))
      .map(|mapping| (mapping[0], mapping[1]))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn source_maps_map_both_ways() {
    let map = SourceMap::parse("data:application/json;charset=utf-8,%7B%22sources%22%3A%5B%22%2Fapp%2Fit%27s.ts%22%5D%2C%22mappings%22%3A%22%3BAAAA%2CIAAI%3BAACA%22%7D").unwrap();
    assert_eq!(map.sources, ["/app/it's.ts"]);
    assert_eq!(
      map.mappings,
      [[1, 0, 0, 0, 0], [1, 4, 0, 0, 4], [2, 0, 0, 1, 4]]
    );
    assert_eq!(map.original(1, 5), Some(("/app/it's.ts", 0, 4)));
    assert_eq!(map.original(2, 9), Some(("/app/it's.ts", 1, 4)));
    assert_eq!(map.original(0, 0), None);
    assert_eq!(map.generated(|_| true, 0, 3), Some((1, 4)));
    assert_eq!(map.generated(|_| true, 1, 0), Some((2, 0)));
    assert_eq!(map.generated(|_| true, 2, 0), None);
    assert!(SourceMap::parse(r#"data:,{"sources":["a"],"mappings":"ACAA"}"#).is_none());
    let compacted =
      compact(r#"data:,{"sources":["a"],"sourcesContent":["x"],"mappings":"AAAA"}"#).unwrap();
    assert_eq!(compacted, r#"{"sources":["a"],"mappings":"AAAA"}"#);
    assert_eq!(
      SourceMap::parse(&compacted).unwrap().mappings,
      [[0, 0, 0, 0, 0]]
    );
  }
}
