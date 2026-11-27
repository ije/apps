// The fields of date and time inputs, formatted like the system does.
use crate::css;
use crate::dom::Node;
use crate::style;
use renderer::px;

/// A field or the text between fields, and its widest text.
pub(crate) struct Part {
  pub text: String,
  widest: [&'static str; 2],
  names: &'static [String],
  pub field: bool,
}

// The date, time and month patterns, like `dd/MM/yyyy`, and the names of the months and the halves of the day.
struct Patterns {
  date: String,
  time: String,
  month: String,
  months: Vec<String>,
  halves: [String; 2],
}

fn patterns() -> &'static Patterns {
  static PATTERNS: std::sync::OnceLock<Patterns> = std::sync::OnceLock::new();
  PATTERNS.get_or_init(|| {
    let lines = crate::native::date_patterns();
    let mut lines = lines.lines().map(str::to_owned);
    let mut line = || lines.next().unwrap_or_default();
    let [date, time, month, am, pm] = [line(), line(), line(), line(), line()];
    Patterns {
      date,
      time,
      month,
      halves: [am, pm],
      months: lines.collect(),
    }
  })
}

/// Whether the field shows its value as date or time fields.
pub(crate) fn temporal(node: &Node) -> bool {
  node.tag == "input"
    && ["date", "time", "datetime-local", "month", "week"]
      .iter()
      .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
}

fn timed(node: &Node) -> bool {
  node.input_type().eq_ignore_ascii_case("time")
}

// A field shows its `empty` text, the first of the widest ones, without a value.
#[inline(never)]
fn field(value: Option<String>, widest: [&'static str; 2], names: &'static [String]) -> Part {
  Part {
    text: value.unwrap_or_else(|| widest[0].into()),
    widest,
    names,
    field: true,
  }
}

#[inline(never)]
fn number(value: Option<u32>, widest: [&'static str; 2]) -> Part {
  field(value.map(|number| format!("{number:02}")), widest, &[])
}

pub(crate) fn parts(node: &Node) -> Vec<Part> {
  let kind = node.input_type().to_ascii_lowercase();
  let value = node.current_value().unwrap_or("");
  let digits = |text: &str| !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit());
  let (date, time) = match kind.as_str() {
    "time" => ("", value),
    "datetime-local" => value.split_once(['T', ' ']).unwrap_or_default(),
    _ => (value, ""),
  };
  let mut date = date.splitn(3, '-');
  let year = date
    .next()
    .filter(|year| digits(year) && year.len() >= 4 && year.len() <= 6);
  let mut next = |max: u32| {
    let text = date.next()?;
    let number = text.strip_prefix('W').unwrap_or(text);
    (digits(number) && number.len() == 2)
      .then(|| number.parse::<u32>().ok())
      .flatten()
      .filter(|number| (1..=max).contains(number))
  };
  let count = year.and_then(|year| year.parse::<u64>().ok()).unwrap_or(0);
  let weekday = |year: u64| (year + year / 4 - year / 100 + year / 400) % 7;
  let second = next(match kind.as_str() {
    "week" if weekday(count) == 4 || weekday(count.max(1) - 1) == 3 => 53,
    "week" => 52,
    _ => 12,
  });
  let leap = count.is_multiple_of(4) && (!count.is_multiple_of(100) || count.is_multiple_of(400));
  let day = next(match second {
    Some(2) if leap => 29,
    Some(2) => 28,
    Some(4 | 6 | 9 | 11) => 30,
    _ => 31,
  });
  let whole = match kind.as_str() {
    "month" | "week" => second.is_some(),
    _ => day.is_some(),
  };
  let year = year.filter(|_| whole).map(str::to_owned);
  let (second, day) = (
    second.filter(|_| year.is_some()),
    day.filter(|_| year.is_some()),
  );
  let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
  let clock: Vec<u32> = clock
    .split(':')
    .filter(|part| digits(part) && part.len() == 2)
    .filter_map(|part| part.parse().ok())
    .collect();
  let clock = (clock.len() >= 2 && clock[0] < 24 && clock[1] < 60).then_some(clock);
  let seconds = clock.as_ref().and_then(|clock| clock.get(2)).copied();
  let millis = format!("{fraction:0<3}")
    .get(..3)
    .and_then(|text| text.parse::<u32>().ok());
  let step = (node.attrs.get("step"))
    .and_then(|step| step.trim().parse::<f64>().ok())
    .filter(|step| *step > 0.)
    .unwrap_or(60.);
  let show_millis = millis.is_some_and(|millis| millis != 0) || step % 1. != 0.;
  let show_seconds = show_millis || seconds.is_some_and(|seconds| seconds != 0) || step % 60. != 0.;
  let names = patterns();
  let mut out: Vec<Part> = Vec::new();
  let literal = |c: char| Part {
    text: c.into(),
    widest: [""; 2],
    names: &[],
    field: false,
  };
  let mut write = |pattern: &str| {
    let mut rest = pattern.chars().peekable();
    let mut quoted = false;
    while let Some(c) = rest.next() {
      if c == '\'' {
        quoted = !quoted;
        continue;
      }
      if quoted || !c.is_ascii_alphabetic() {
        let c = if c == '\u{202f}' { ' ' } else { c };
        match out.last_mut() {
          Some(part) if !part.field => part.text.push(c),
          _ => out.push(literal(c)),
        }
        continue;
      }
      let mut count = 1;
      while rest.next_if_eq(&c).is_some() {
        count += 1;
      }
      let time = |at: usize| clock.as_ref().map(|clock| clock[at]);
      let part = match c {
        'd' => number(day, ["dd", "31"]),
        'M' | 'L' if count > 2 => {
          let month = second.and_then(|month| names.months.get(month as usize - 1));
          field(month.cloned(), ["---------", ""], &names.months)
        }
        'M' | 'L' => number(second, ["mm", "12"]),
        'y' | 'u' | 'Y' if kind.starts_with("date") => field(year.clone(), ["yyyy", "275760"], &[]),
        'y' | 'u' | 'Y' => field(year.clone(), ["----", "275760"], &[]),
        'w' => number(second, ["--", "53"]),
        'H' | 'k' => number(time(0), ["--", "23"]),
        'h' | 'K' => {
          let hour = time(0).map(|hour| match hour % 12 {
            0 if c == 'h' => 12,
            hour => hour,
          });
          number(hour, ["--", "12"])
        }
        'm' => number(time(1), ["--", "59"]),
        'a' | 'b' | 'B' => {
          let half = time(0).map(|hour| names.halves[usize::from(hour >= 12)].clone());
          field(half, ["--", ""], &names.halves)
        }
        _ => continue,
      };
      out.push(part);
      if c == 'm' && show_seconds {
        out.push(literal(':'));
        let seconds = time(1).map(|_| seconds.unwrap_or(0));
        out.push(number(seconds, ["--", "59"]));
      }
      if c == 'm' && show_millis {
        out.push(literal('.'));
        let millis = time(1).map(|_| format!("{:03}", millis.unwrap_or(0)));
        out.push(field(millis, ["---", "999"], &[]));
      }
    }
  };
  match kind.as_str() {
    "date" => write(&names.date),
    "time" => write(&names.time),
    "month" => write(&names.month),
    "week" => write("'Week 'ww, yyyy"),
    _ => {
      write(&names.date);
      write(", ");
      write(&names.time);
    }
  }
  out
}

/// The size of the box of the picker icon with its margin, and the size and padding of the icon.
pub(crate) fn icon(node: &Node, em: f32) -> ([f32; 2], f32, f32) {
  match timed(node) {
    true => ([1.05 * em + 14.5, 1.05 * em + 6.], 1.05 * em, 3.),
    false => ([em + 4., em + 4.], em, 2.),
  }
}

/// The natural content size of a date or time input.
pub(crate) fn size(
  node: &Node,
  style: &css::properties::Style,
  units: &css::value::units::Lengths,
  text: &renderer::TextSystem,
  line: f32,
) -> [f32; 2] {
  let em = units.font.em;
  let width = |label: &str| {
    let run = style::text_run(style, units, label.len());
    f32::from(text.shape_line(label, px(em), &run).line.width)
  };
  let fields: f32 = parts(node)
    .iter()
    .map(|part| {
      let own = width(&part.text);
      match part.field {
        true => {
          let names = part.names.iter().map(String::as_str);
          names.chain(part.widest).map(width).fold(own, f32::max) + 2.
        }
        false => own,
      }
    })
    .sum();
  let (room, ..) = icon(node, em);
  [fields + room[0], (line + 2.).max(room[1])]
}
