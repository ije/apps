use crate::platform::lock;
use anyhow::{Context as _, Result, bail};
use std::sync::{Arc, Mutex, mpsc};
use std::{cell::RefCell, thread::JoinHandle, time::Duration, time::Instant};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{self, Atom, AtomEnum, ConnectionExt as _, EventMask};
use x11rb::{NONE, rust_connection::RustConnection, wrapper::ConnectionExt as _};

x11rb::atom_manager! {
    Names: NamesCookie {
        CLIPBOARD,
        Manager: b"CLIPBOARD_MANAGER",
        Save: b"SAVE_TARGETS",
        Targets: b"TARGETS",
        AtomList: b"ATOM",
        Parts: b"INCR",
        Utf8: b"UTF8_STRING",
        Latin1: b"STRING",
        Text: b"TEXT",
        PlainUtf8: b"text/plain;charset=utf-8",
        PlainUtf8Upper: b"text/plain;charset=UTF-8",
        Plain: b"text/plain",
        Landing: b"APPS_SELECTION",
    }
}

impl Names {
  // The text targets, best first.
  fn text(&self) -> [Atom; 6] {
    [
      self.Utf8,
      self.PlainUtf8,
      self.PlainUtf8Upper,
      self.Latin1,
      self.Text,
      self.Plain,
    ]
  }
}

// How long a paste waits for the owner, and for each part of a large paste.
const WAIT: Duration = Duration::from_secs(4);

struct Port {
  conn: RustConnection,
  window: u32,
}

impl Port {
  fn open() -> Result<Self> {
    let (conn, screen) = RustConnection::connect(None).context("connecting the clipboard")?;
    let root = conn.setup().roots.get(screen).context("no X screen")?.root;
    let window = conn.generate_id()?;
    let heard = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
    let aux = xproto::CreateWindowAux::new().event_mask(heard);
    let class = xproto::WindowClass::INPUT_ONLY;
    conn.create_window(0, window, root, 0, 0, 1, 1, 0, class, 0, &aux)?;
    conn.flush()?;
    Ok(Self { conn, window })
  }
}

// Steps of a handover to the clipboard manager. It can say that it is done before it reads, so it needs both.
#[derive(Default)]
struct Handover {
  finished: Option<mpsc::SyncSender<()>>,
  read: bool,
  done: bool,
}

// The data of each target, with text under all text targets.
type Items = Vec<(Atom, Arc<[u8]>)>;

struct Owner {
  port: Port,
  names: Names,
  items: Mutex<Option<Items>>,
  handover: Mutex<Handover>,
}

pub(super) struct Selection {
  owner: Arc<Owner>,
  server: Option<JoinHandle<()>>,
  reader: RefCell<Option<Port>>,
}

impl Selection {
  pub fn open() -> Result<Self> {
    let port = Port::open()?;
    let names = Names::new(&port.conn)?.reply()?;
    let (items, handover) = (Mutex::new(None), Mutex::default());
    let owner = Arc::new(Owner {
      port,
      names,
      items,
      handover,
    });
    let serving = owner.clone();
    let server = std::thread::spawn(move || {
      let _ = serving
        .serve()
        .inspect_err(|error| eprintln!("apps: clipboard: {error:#}"));
    });
    Ok(Self {
      owner,
      server: Some(server),
      reader: RefCell::new(None),
    })
  }

  pub fn copy(&self, items: Vec<(String, Vec<u8>)>) {
    if let Err(error) = self.owner.copy(items) {
      eprintln!("apps: clipboard: {error:#}");
    }
  }

  pub fn types(&self) -> Vec<String> {
    self
      .kinds()
      .inspect_err(|error| eprintln!("apps: clipboard: {error:#}"))
      .unwrap_or_default()
  }

  fn kinds(&self) -> Result<Vec<String>> {
    let names = &self.owner.names;
    let text = names.text();
    let atoms: Vec<Atom> = match self.owner.owns()? {
      true => lock(&self.owner.items)
        .iter()
        .flatten()
        .map(|(atom, _)| *atom)
        .collect(),
      false => {
        let (data, _) = self.fetch(names.Targets)?;
        let words = data.as_chunks::<4>().0.iter();
        words.map(|word| u32::from_ne_bytes(*word)).collect()
      }
    };
    let conn = &self.owner.port.conn;
    let cookies: Vec<_> = atoms
      .iter()
      .filter(|atom| !text.contains(atom))
      .map(|atom| conn.get_atom_name(*atom))
      .collect::<Result<_, _>>()?;
    let mut kinds = Vec::new();
    if atoms.iter().any(|atom| text.contains(atom)) {
      kinds.push("text/plain".to_owned());
    }
    for cookie in cookies {
      let name = String::from_utf8(cookie.reply()?.name)?;
      if name.contains('/') && !kinds.contains(&name) {
        kinds.push(name);
      }
    }
    Ok(kinds)
  }

  pub fn data(&self, kind: &str) -> Option<Vec<u8>> {
    if kind == "text/plain" {
      return self.paste().map(String::into_bytes);
    }
    let read = || -> Result<Vec<u8>> {
      let atom = self.owner.port.conn.intern_atom(false, kind.as_bytes())?;
      let atom = atom.reply()?.atom;
      if self.owner.owns()? {
        let items = lock(&self.owner.items);
        let item = items.iter().flatten().find(|(own, _)| *own == atom);
        return Ok(item.context("the clipboard has no such type")?.1.to_vec());
      }
      Ok(self.fetch(atom)?.0)
    };
    read()
      .inspect_err(|error| eprintln!("apps: clipboard: {error:#}"))
      .ok()
  }

  pub fn paste(&self) -> Option<String> {
    self
      .text()
      .inspect_err(|error| eprintln!("apps: clipboard: {error:#}"))
      .ok()
  }

  fn text(&self) -> Result<String> {
    let names = &self.owner.names;
    if self.owner.owns()? {
      let items = lock(&self.owner.items);
      let text = items.iter().flatten().find(|(atom, _)| *atom == names.Utf8);
      return Ok(String::from_utf8(
        text.context("the clipboard holds no text")?.1.to_vec(),
      )?);
    }
    // Opens the reader once, so a failed connection is not tried again for each target.
    if self.reader.borrow().is_none() {
      *self.reader.borrow_mut() = Some(Port::open()?);
    }
    let kinds = names.text();
    let listed = self
      .fetch(names.Targets)
      .ok()
      .filter(|(_, kind)| *kind == names.AtomList);
    let offered: Option<Vec<Atom>> = listed.map(|(data, _)| {
      data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_ne_bytes(*word))
        .collect()
    });
    let shared = |kind: &Atom| {
      offered
        .as_ref()
        .is_none_or(|offered| offered.contains(kind))
    };
    // With a target list, only the best target is asked for.
    let tries = if offered.is_some() { 1 } else { kinds.len() };
    for kind in kinds.into_iter().filter(shared).take(tries) {
      let Ok((data, got)) = self.fetch(kind) else {
        continue;
      };
      match got {
        _ if got == names.Latin1 => return Ok(data.into_iter().map(char::from).collect()),
        _ if kinds.contains(&got) => return Ok(String::from_utf8(data)?),
        _ => {}
      }
    }
    bail!("the clipboard holds no text")
  }

  fn fetch(&self, target: Atom) -> Result<(Vec<u8>, Atom)> {
    let names = &self.owner.names;
    let mut reader = self.reader.borrow_mut();
    let port = match &mut *reader {
      Some(port) => port,
      empty => empty.insert(Port::open()?),
    };
    let (conn, window) = (&port.conn, port.window);
    // A fresh property shows which change answers this request.
    conn.delete_property(window, names.Landing)?;
    conn.convert_selection(
      window,
      names.CLIPBOARD,
      target,
      names.Landing,
      x11rb::CURRENT_TIME,
    )?;
    conn.sync()?;
    let mut parts: Option<Vec<u8>> = None;
    let mut deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
      // The owner answers on its own time, so this looks for events every millisecond.
      let Some(event) = conn.poll_for_event()? else {
        std::thread::sleep(Duration::from_micros(1000));
        continue;
      };
      match event {
        Event::SelectionNotify(answer) if parts.is_none() => {
          if answer.property == NONE || answer.target != target {
            bail!("the clipboard can't give {target}");
          }
          let whole = conn.get_property(
            true,
            window,
            answer.property,
            AtomEnum::ANY,
            0,
            u32::MAX / 4,
          )?;
          let whole = whole.reply()?;
          if whole.type_ != names.Parts {
            return Ok((whole.value, whole.type_));
          }
          // Deleting the INCR property, which only a read with its own type does, starts the parts.
          let start = conn
            .get_property(true, window, answer.property, names.Parts, 0, 1)?
            .reply()?;
          let size = start
            .value32()
            .and_then(|mut words| words.next())
            .unwrap_or(0);
          parts = Some(Vec::with_capacity(size as usize));
          deadline = Instant::now() + WAIT;
        }
        Event::PropertyNotify(change)
          if change.atom == names.Landing && change.state == xproto::Property::NEW_VALUE =>
        {
          let Some(data) = &mut parts else {
            continue;
          };
          let kind = if target == names.Targets {
            names.AtomList
          } else {
            target
          };
          let part = conn
            .get_property(true, window, change.atom, kind, 0, u32::MAX / 4)?
            .reply()?;
          // An empty part ends the transfer.
          if part.value.is_empty() {
            return Ok((parts.unwrap_or_default(), target));
          }
          data.extend_from_slice(&part.value);
          deadline = Instant::now() + WAIT;
        }
        _ => {}
      }
    }
    bail!("the clipboard did not answer in time")
  }
}

impl Drop for Selection {
  fn drop(&mut self) {
    let handed = self.owner.hand_over();
    let _ = handed.inspect_err(|error| eprintln!("apps: clipboard manager: {error:#}"));
    let port = &self.owner.port;
    let _ = port.conn.destroy_window(port.window);
    let _ = port.conn.flush();
    if let Some(server) = self.server.take() {
      let _ = server.join();
    }
  }
}

impl Owner {
  fn copy(&self, items: Vec<(String, Vec<u8>)>) -> Result<()> {
    let (conn, names) = (&self.port.conn, &self.names);
    let mut owned: Items = Vec::new();
    for (kind, data) in items {
      let data: Arc<[u8]> = data.into();
      if kind == "text/plain" {
        let text = [names.Utf8, names.PlainUtf8, names.PlainUtf8Upper];
        owned.extend(text.map(|atom| (atom, data.clone())));
        continue;
      }
      let atom = conn.intern_atom(false, kind.as_bytes())?.reply()?.atom;
      owned.push((atom, data));
    }
    *lock(&self.items) = Some(owned);
    conn.set_selection_owner(self.port.window, names.CLIPBOARD, x11rb::CURRENT_TIME)?;
    conn.flush()?;
    Ok(())
  }

  fn owns(&self) -> Result<bool> {
    let owner = self
      .port
      .conn
      .get_selection_owner(self.names.CLIPBOARD)?
      .reply()?
      .owner;
    Ok(owner == self.port.window)
  }

  fn serve(&self) -> Result<()> {
    let names = &self.names;
    loop {
      match self.port.conn.wait_for_event()? {
        Event::DestroyNotify(gone) if gone.window == self.port.window => return Ok(()),
        Event::SelectionClear(lost) if lost.selection == names.CLIPBOARD => {
          *lock(&self.items) = None;
        }
        Event::SelectionRequest(request) => {
          // One refused answer, like a text past the request size limit, must not end the server.
          // A lost connection ends it at the next wait.
          if let Err(error) = self.answer(&request) {
            eprintln!("apps: clipboard: {error:#}");
            continue;
          }
          if request.target != names.Targets {
            self.progress(|handover| handover.read = true);
          }
        }
        Event::SelectionNotify(note) if note.selection == names.Manager => {
          self.progress(|handover| handover.done = true);
        }
        _ => {}
      }
    }
  }

  fn progress(&self, step: impl FnOnce(&mut Handover)) {
    let mut handover = lock(&self.handover);
    if handover.finished.is_some() {
      step(&mut handover);
      if handover.read && handover.done {
        let _ = handover
          .finished
          .take()
          .map(|finished| finished.try_send(()));
      }
    }
  }

  fn answer(&self, request: &xproto::SelectionRequestEvent) -> Result<()> {
    let names = &self.names;
    let conn = &self.port.conn;
    // Obsolete clients name no property, and then the target is the property.
    let property = if request.property == NONE {
      request.target
    } else {
      request.property
    };
    let items = lock(&self.items).clone();
    let (to, ours) = (request.requestor, request.selection == names.CLIPBOARD);
    let given = match items.filter(|_| ours) {
      Some(items) if request.target == names.Targets => {
        let mut targets = vec![names.Targets, names.Save];
        targets.extend(items.iter().map(|(atom, _)| *atom));
        conn.change_property32(
          xproto::PropMode::REPLACE,
          to,
          property,
          names.AtomList,
          &targets,
        )?;
        true
      }
      Some(items) => match items.iter().find(|(atom, _)| *atom == request.target) {
        Some((_, data)) => {
          conn.change_property8(
            xproto::PropMode::REPLACE,
            to,
            property,
            request.target,
            data,
          )?;
          true
        }
        None => false,
      },
      None => false,
    };
    let note = xproto::SelectionNotifyEvent {
      response_type: xproto::SELECTION_NOTIFY_EVENT,
      property: if given { property } else { NONE },
      sequence: request.sequence,
      time: request.time,
      requestor: to,
      selection: request.selection,
      target: request.target,
    };
    conn.send_event(false, to, EventMask::NO_EVENT, note)?;
    conn.flush()?;
    Ok(())
  }

  // Waits for the clipboard manager at most 100 ms.
  fn hand_over(&self) -> Result<()> {
    let empty = lock(&self.items).is_none();
    if empty || !self.owns()? {
      return Ok(());
    }
    let (finished, wait) = mpsc::sync_channel(1);
    *lock(&self.handover) = Handover {
      finished: Some(finished),
      ..Handover::default()
    };
    let (conn, window, names) = (&self.port.conn, self.port.window, &self.names);
    conn.convert_selection(
      window,
      names.Manager,
      names.Save,
      names.Landing,
      x11rb::CURRENT_TIME,
    )?;
    conn.flush()?;
    if wait.recv_timeout(Duration::from_millis(100)).is_err() {
      bail!("the clipboard manager did not take the text in time");
    }
    Ok(())
  }
}
