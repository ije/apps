// A small client of the D-Bus session bus: method calls and signals, without file descriptors.
use anyhow::{Context as _, Result, bail, ensure};
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Byte(u8),
    Bool(bool),
    U32(u32),
    I32(i32),
    U64(u64),
    F64(f64),
    Str(String),
    Path(String),
    Sig(String),
    /// The signature of the elements and the elements.
    Array(String, Vec<Value>),
    Struct(Vec<Value>),
    Entry(Box<Value>, Box<Value>),
    Variant(Box<Value>),
}

impl Value {
    pub(crate) fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    /// An `a{sv}` dictionary.
    pub(crate) fn dict(entries: Vec<(&str, Value)>) -> Self {
        let entries = entries.into_iter().map(|(key, value)| {
            Self::Entry(
                Box::new(Self::str(key)),
                Box::new(Self::Variant(Box::new(value))),
            )
        });
        Self::Array("{sv}".into(), entries.collect())
    }

    fn signature(&self) -> String {
        match self {
            Self::Byte(_) => "y".into(),
            Self::Bool(_) => "b".into(),
            Self::U32(_) => "u".into(),
            Self::I32(_) => "i".into(),
            Self::U64(_) => "t".into(),
            Self::F64(_) => "d".into(),
            Self::Str(_) => "s".into(),
            Self::Path(_) => "o".into(),
            Self::Sig(_) => "g".into(),
            Self::Array(element, _) => format!("a{element}"),
            Self::Struct(fields) => format!(
                "({})",
                fields.iter().map(Self::signature).collect::<String>()
            ),
            Self::Entry(key, value) => format!("{{{}{}}}", key.signature(), value.signature()),
            Self::Variant(_) => "v".into(),
        }
    }

    /// The value inside variants.
    pub(crate) fn plain(&self) -> &Value {
        match self {
            Self::Variant(inner) => inner.plain(),
            value => value,
        }
    }

    /// The value of `key` in an `a{sv}` dictionary.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        let Self::Array(_, entries) = self.plain() else {
            return None;
        };
        entries.iter().find_map(|entry| match entry {
            Self::Entry(name, value) if matches!(&**name, Self::Str(name) if name == key) => {
                Some(value.plain())
            }
            _ => None,
        })
    }

    pub(crate) fn as_u32(&self) -> Option<u32> {
        match self.plain() {
            Self::U32(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self.plain() {
            Self::Str(text) | Self::Path(text) | Self::Sig(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn items(&self) -> &[Value] {
        match self.plain() {
            Self::Array(_, items) | Self::Struct(items) => items,
            _ => &[],
        }
    }
}

fn alignment(code: u8) -> usize {
    match code {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

fn pad(out: &mut Vec<u8>, to: usize) {
    out.resize(out.len().next_multiple_of(to), 0);
}

fn put(out: &mut Vec<u8>, value: &Value) {
    let word = |out: &mut Vec<u8>, bytes: &[u8]| {
        pad(out, bytes.len());
        out.extend_from_slice(bytes);
    };
    match value {
        Value::Byte(byte) => out.push(*byte),
        Value::Bool(on) => word(out, &u32::from(*on).to_le_bytes()),
        Value::U32(number) => word(out, &number.to_le_bytes()),
        Value::I32(number) => word(out, &number.to_le_bytes()),
        Value::U64(number) => word(out, &number.to_le_bytes()),
        Value::F64(number) => word(out, &number.to_le_bytes()),
        Value::Str(text) | Value::Path(text) => {
            word(out, &(text.len() as u32).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
            out.push(0);
        }
        Value::Sig(text) => {
            out.push(text.len() as u8);
            out.extend_from_slice(text.as_bytes());
            out.push(0);
        }
        Value::Array(element, items) => {
            word(out, &[0; 4]);
            let at = out.len() - 4;
            pad(out, alignment(element.as_bytes()[0]));
            let start = out.len();
            for item in items {
                put(out, item);
            }
            let size = (out.len() - start) as u32;
            out[at..at + 4].copy_from_slice(&size.to_le_bytes());
        }
        Value::Struct(fields) => {
            pad(out, 8);
            fields.iter().for_each(|field| put(out, field));
        }
        Value::Entry(key, value) => {
            pad(out, 8);
            put(out, key);
            put(out, value);
        }
        Value::Variant(inner) => {
            put(out, &Value::Sig(inner.signature()));
            put(out, inner);
        }
    }
}

// The length of the first complete type of a signature.
fn single(sig: &[u8]) -> Result<usize> {
    match sig.first() {
        Some(b'a') => Ok(1 + single(&sig[1..])?),
        Some(b'(' | b'{') => {
            let mut depth = 0;
            for (at, code) in sig.iter().enumerate() {
                match code {
                    b'(' | b'{' => depth += 1,
                    b')' | b'}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    return Ok(at + 1);
                }
            }
            bail!("Invalid D-Bus signature")
        }
        Some(_) => Ok(1),
        None => bail!("Invalid D-Bus signature"),
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    big: bool,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8]> {
        let bytes = self
            .data
            .get(self.at..self.at + count)
            .context("Short D-Bus message")?;
        self.at += count;
        Ok(bytes)
    }

    fn number<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.at = self.at.next_multiple_of(N);
        let mut bytes: [u8; N] = self.take(N)?.try_into()?;
        if self.big {
            bytes.reverse();
        }
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.number()?))
    }

    fn all(&mut self, mut sig: &[u8]) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        while !sig.is_empty() {
            let length = single(sig)?;
            values.push(self.read(&sig[..length])?);
            sig = &sig[length..];
        }
        Ok(values)
    }

    fn read(&mut self, sig: &[u8]) -> Result<Value> {
        Ok(match sig[0] {
            b'y' => Value::Byte(self.take(1)?[0]),
            b'b' => Value::Bool(self.u32()? != 0),
            b'n' => Value::I32(i16::from_le_bytes(self.number()?).into()),
            b'q' => Value::U32(u16::from_le_bytes(self.number()?).into()),
            b'i' => Value::I32(i32::from_le_bytes(self.number()?)),
            b'u' | b'h' => Value::U32(self.u32()?),
            b'x' | b't' => Value::U64(u64::from_le_bytes(self.number()?)),
            b'd' => Value::F64(f64::from_le_bytes(self.number()?)),
            code @ (b's' | b'o') => {
                let length = self.u32()? as usize;
                let text = String::from_utf8(self.take(length + 1)?[..length].to_vec())?;
                if code == b's' {
                    Value::Str(text)
                } else {
                    Value::Path(text)
                }
            }
            b'g' => {
                let length = usize::from(self.take(1)?[0]);
                Value::Sig(String::from_utf8(
                    self.take(length + 1)?[..length].to_vec(),
                )?)
            }
            b'v' => {
                let Value::Sig(inner) = self.read(b"g")? else {
                    unreachable!("a signature reads as a signature")
                };
                ensure!(
                    single(inner.as_bytes())? == inner.len(),
                    "Invalid D-Bus variant"
                );
                Value::Variant(Box::new(self.read(inner.as_bytes())?))
            }
            b'a' => {
                let length = self.u32()? as usize;
                let element = &sig[1..];
                self.at = self.at.next_multiple_of(alignment(element[0]));
                let end = self.at + length;
                ensure!(end <= self.data.len(), "Short D-Bus message");
                let mut items = Vec::new();
                while self.at < end {
                    items.push(self.read(element)?);
                }
                Value::Array(String::from_utf8(element.to_vec())?, items)
            }
            code @ (b'(' | b'{') => {
                self.at = self.at.next_multiple_of(8);
                let mut fields = self.all(&sig[1..sig.len() - 1])?;
                if code == b'(' {
                    Value::Struct(fields)
                } else {
                    ensure!(fields.len() == 2, "Invalid D-Bus dictionary entry");
                    let value = fields
                        .pop()
                        .map(Box::new)
                        .context("Invalid D-Bus dictionary entry")?;
                    Value::Entry(
                        fields
                            .pop()
                            .map(Box::new)
                            .context("Invalid D-Bus dictionary entry")?,
                        value,
                    )
                }
            }
            _ => bail!("Unknown D-Bus type"),
        })
    }
}

pub(crate) struct Message {
    pub(crate) signal: bool,
    reply: Option<u32>,
    error: Option<String>,
    pub(crate) path: String,
    pub(crate) member: String,
    pub(crate) body: Vec<Value>,
}

pub(crate) struct Bus {
    stream: UnixStream,
    serial: u32,
    pub(crate) name: String,
    signals: Vec<Message>,
}

impl Bus {
    pub(crate) fn session() -> Result<Self> {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().or_else(|| {
            let dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
            Some(format!("unix:path={dir}/bus"))
        });
        let address = address.context("No D-Bus session bus")?;
        let stream = address
            .split(';')
            .filter_map(|address| address.strip_prefix("unix:"))
            .find_map(|keys| {
                keys.split(',').find_map(|key| match key.split_once('=')? {
                    ("path", path) => UnixStream::connect(path).ok(),
                    ("abstract", name) => {
                        use std::os::linux::net::SocketAddrExt as _;
                        let address =
                            std::os::unix::net::SocketAddr::from_abstract_name(name).ok()?;
                        UnixStream::connect_addr(&address).ok()
                    }
                    _ => None,
                })
            })
            .context("Cannot connect to the D-Bus session bus")?;
        stream.set_read_timeout(Some(Duration::from_secs(25)))?;
        // SAFETY: getuid has no failure.
        let uid = unsafe { libc::getuid() }.to_string();
        let hex: String = uid.bytes().map(|byte| format!("{byte:02x}")).collect();
        let mut bus = Self {
            stream,
            serial: 0,
            name: String::new(),
            signals: Vec::new(),
        };
        bus.stream
            .write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())?;
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            let mut byte = [0];
            bus.stream.read_exact(&mut byte)?;
            line.push(byte[0]);
            ensure!(line.len() < 512, "D-Bus authentication failed");
        }
        ensure!(line.starts_with(b"OK "), "D-Bus authentication failed");
        bus.stream.write_all(b"BEGIN\r\n")?;
        let hello = bus.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            &[],
        )?;
        bus.name = hello
            .first()
            .and_then(Value::as_str)
            .context("No D-Bus name")?
            .to_owned();
        Ok(bus)
    }

    pub(crate) fn call(
        &mut self,
        to: &str,
        path: &str,
        interface: &str,
        member: &str,
        args: &[Value],
    ) -> Result<Vec<Value>> {
        self.serial += 1;
        let serial = self.serial;
        let mut body = Vec::new();
        args.iter().for_each(|arg| put(&mut body, arg));
        let mut fields = vec![
            (1, Value::Path(path.into())),
            (2, Value::str(interface)),
            (3, Value::str(member)),
            (6, Value::str(to)),
        ];
        if !args.is_empty() {
            fields.push((8, Value::Sig(args.iter().map(Value::signature).collect())));
        }
        let fields = fields.into_iter().map(|(code, value)| {
            Value::Struct(vec![Value::Byte(code), Value::Variant(Box::new(value))])
        });
        let mut out = vec![b'l', 1, 0, 1];
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&serial.to_le_bytes());
        put(&mut out, &Value::Array("(yv)".into(), fields.collect()));
        pad(&mut out, 8);
        out.extend_from_slice(&body);
        self.stream.write_all(&out)?;
        loop {
            let message = self.receive()?;
            if message.signal {
                self.signals.push(message);
            } else if message.reply == Some(serial) {
                if let Some(error) = message.error {
                    let detail = message
                        .body
                        .first()
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    bail!("{error}: {detail}");
                }
                return Ok(message.body);
            }
        }
    }

    /// Waits for the next signal of a match rule that `AddMatch` added.
    pub(crate) fn signal(&mut self) -> Result<Message> {
        if !self.signals.is_empty() {
            return Ok(self.signals.remove(0));
        }
        self.stream.set_read_timeout(None)?;
        loop {
            let message = self.receive()?;
            if message.signal {
                return Ok(message);
            }
        }
    }

    pub(crate) fn watch(&mut self, rule: &str) -> Result<()> {
        self.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
            &[Value::str(rule)],
        )?;
        Ok(())
    }

    fn receive(&mut self) -> Result<Message> {
        let mut head = [0; 16];
        self.stream.read_exact(&mut head)?;
        let big = head[0] == b'B';
        let word = |at: usize| {
            let bytes = head[at..at + 4].try_into().unwrap_or_default();
            if big {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        };
        let (body, fields) = (word(4) as usize, word(12) as usize);
        ensure!(body + fields < 1 << 27, "D-Bus message is too large");
        let mut data = head.to_vec();
        data.resize(16 + fields.next_multiple_of(8) + body, 0);
        self.stream.read_exact(&mut data[16..])?;
        let mut reader = Reader {
            data: &data,
            at: 12,
            big,
        };
        let mut message = Message {
            signal: head[1] == 4,
            reply: None,
            error: None,
            path: String::new(),
            member: String::new(),
            body: Vec::new(),
        };
        let mut sig = String::new();
        for field in reader.read(b"a(yv)")?.items() {
            let [Value::Byte(code), value] = field.items() else {
                continue;
            };
            match (code, value.plain()) {
                (1, Value::Path(path)) => message.path = path.clone(),
                (3, Value::Str(member)) => message.member = member.clone(),
                (4, Value::Str(error)) => message.error = Some(error.clone()),
                (5, Value::U32(serial)) => message.reply = Some(*serial),
                (8, Value::Sig(text)) => sig = text.clone(),
                _ => {}
            }
        }
        reader.at = reader.at.next_multiple_of(8);
        message.body = reader.all(sig.as_bytes())?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_back_as_written() {
        let filter = Value::Struct(vec![
            Value::str("Images"),
            Value::Array(
                "(us)".into(),
                vec![Value::Struct(vec![Value::U32(0), Value::str("*.png")])],
            ),
        ]);
        let values = vec![
            Value::str("title"),
            Value::dict(vec![
                ("multiple", Value::Bool(true)),
                ("filters", Value::Array("(sa(us))".into(), vec![filter])),
                (
                    "folder",
                    Value::Array("y".into(), vec![Value::Byte(47), Value::Byte(0)]),
                ),
            ]),
            Value::U64(7),
        ];
        let mut out = Vec::new();
        values.iter().for_each(|value| put(&mut out, value));
        let sig: String = values.iter().map(Value::signature).collect();
        assert_eq!(sig, "sa{sv}t");
        let read = Reader {
            data: &out,
            at: 0,
            big: false,
        }
        .all(sig.as_bytes())
        .unwrap();
        assert_eq!(read, values);
        assert_eq!(read[1].get("multiple"), Some(&Value::Bool(true)));
    }
}
