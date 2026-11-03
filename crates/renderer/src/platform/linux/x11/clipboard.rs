//! The X11 clipboard, on its own connection and served by a thread. Adapted from the x11 module of
//! arboard (Apache-2.0 OR MIT), through Zed.
//!
//! See <https://tronche.com/gui/x/icccm/sec-2.html#s-2.6> and
//! <https://freedesktop.org/wiki/ClipboardManager/>.

use anyhow::{Context as _, Result, bail};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, NONE,
    connection::Connection as _,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
            SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Time, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

x11rb::atom_manager! {
    Atoms: AtomCookies {
        CLIPBOARD,
        CLIPBOARD_MANAGER,
        SAVE_TARGETS,
        TARGETS,
        ATOM,
        INCR,
        UTF8_STRING,
        UTF8_MIME_0: b"text/plain;charset=utf-8",
        UTF8_MIME_1: b"text/plain;charset=UTF-8",
        // ISO Latin-1.
        STRING,
        TEXT,
        TEXT_MIME_UNKNOWN: b"text/plain",
        // The property on our window that selection owners write into.
        APPS_CLIPBOARD,
    }
}

/// Some owners take seconds to answer.
const TIMEOUT: Duration = Duration::from_millis(4000);
/// The longest wait between the segments of an incremental transfer.
const SEGMENT_TIMEOUT: Duration = Duration::from_millis(10);

struct Connection {
    conn: RustConnection,
    window: u32,
}

impl Connection {
    fn new() -> Result<Self> {
        let (conn, screen) =
            RustConnection::connect(None).context("connecting the clipboard to the X server")?;
        let root = conn.setup().roots.get(screen).context("no X screen")?.root;
        let window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            COPY_FROM_PARENT,
            // Property changes carry incremental transfers, and the destroy ends the server thread.
            &CreateWindowAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        conn.flush()?;
        Ok(Self { conn, window })
    }
}

#[derive(PartialEq, Eq)]
enum Handover {
    Idle,
    InProgress { written: bool, notified: bool },
    Finished,
}

struct Inner {
    server: Connection,
    atoms: Atoms,
    /// The text while this app owns the clipboard.
    text: Mutex<Option<Vec<u8>>>,
    handover: Mutex<Handover>,
    handover_done: Condvar,
}

pub(crate) struct Clipboard {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

impl Clipboard {
    pub fn new() -> Result<Self> {
        let server = Connection::new()?;
        let atoms = Atoms::new(&server.conn)?.reply()?;
        let inner = Arc::new(Inner {
            server,
            atoms,
            text: Mutex::new(None),
            handover: Mutex::new(Handover::Idle),
            handover_done: Condvar::new(),
        });
        let thread = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn({
                let inner = inner.clone();
                move || {
                    if let Err(error) = inner.serve() {
                        eprintln!("apps: clipboard: {error:#}");
                    }
                }
            })?;
        Ok(Self {
            inner,
            thread: Some(thread),
        })
    }

    pub fn write(&self, text: &str) -> Result<()> {
        let inner = &self.inner;
        // ICCCM 2.6.1.3: ownership is asserted again whenever the data changes.
        inner.server.conn.set_selection_owner(
            inner.server.window,
            inner.atoms.CLIPBOARD,
            Time::CURRENT_TIME,
        )?;
        inner.server.conn.flush()?;
        *inner.text.lock().unwrap() = Some(text.as_bytes().to_vec());
        Ok(())
    }

    pub fn read(&self) -> Result<String> {
        let inner = &self.inner;
        let atoms = &inner.atoms;
        if inner.is_owner()? {
            let text = inner
                .text
                .lock()
                .unwrap()
                .clone()
                .context("the clipboard is empty")?;
            return Ok(String::from_utf8(text)?);
        }
        let formats = [
            atoms.UTF8_STRING,
            atoms.UTF8_MIME_0,
            atoms.UTF8_MIME_1,
            atoms.STRING,
            atoms.TEXT,
            atoms.TEXT_MIME_UNKNOWN,
        ];
        let reader = Connection::new()?;
        let available = inner
            .read_single(&reader, atoms.TARGETS)
            .ok()
            .filter(|(_, format)| *format == atoms.ATOM)
            .map(|(bytes, _)| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| u32::from_le_bytes(*chunk))
                    .collect::<Vec<_>>()
            });
        let candidates: Vec<Atom> = match available {
            Some(available) => formats
                .into_iter()
                .filter(|format| available.contains(format))
                .take(1)
                .collect(),
            // Without a target list, each format is tried.
            None => formats.to_vec(),
        };
        for format in candidates {
            let Ok((bytes, actual)) = inner.read_single(&reader, format) else {
                continue;
            };
            if actual == atoms.STRING {
                return Ok(bytes.into_iter().map(char::from).collect());
            }
            if formats.contains(&actual) {
                return Ok(String::from_utf8(bytes)?);
            }
        }
        bail!("the clipboard has no text")
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let inner = &self.inner;
        if let Err(error) = inner.hand_over() {
            eprintln!("apps: clipboard manager: {error:#}");
        }
        // The destroy notification stops the server thread.
        let _ = inner.server.conn.destroy_window(inner.server.window);
        let _ = inner.server.conn.flush();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Inner {
    fn is_owner(&self) -> Result<bool> {
        let owner = self
            .server
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        Ok(owner == self.server.window)
    }

    /// Converts the clipboard to `target` and returns the bytes and their format.
    fn read_single(&self, reader: &Connection, target: Atom) -> Result<(Vec<u8>, Atom)> {
        let atoms = &self.atoms;
        // Deleted first, so the property notification shows when the owner got the request.
        reader
            .conn
            .delete_property(reader.window, atoms.APPS_CLIPBOARD)?;
        reader.conn.convert_selection(
            reader.window,
            atoms.CLIPBOARD,
            target,
            atoms.APPS_CLIPBOARD,
            Time::CURRENT_TIME,
        )?;
        reader.conn.sync()?;
        let mut incremental: Option<Vec<u8>> = None;
        let mut deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let Some(event) = reader.conn.poll_for_event()? else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            match event {
                Event::SelectionNotify(event) => {
                    // ICCCM 2.4: the target matches the request, and no property means no data.
                    if event.property == NONE || event.target != target {
                        bail!("the clipboard can't be converted");
                    }
                    if incremental.is_some() {
                        continue;
                    }
                    let reply = reader
                        .conn
                        .get_property(
                            true,
                            event.requestor,
                            event.property,
                            AtomEnum::ANY,
                            0,
                            u32::MAX / 4,
                        )?
                        .reply()?;
                    if reply.type_ != atoms.INCR {
                        return Ok((reply.value, reply.type_));
                    }
                    // The property is deleted only with its own type, which says the segments may come.
                    let reply = reader
                        .conn
                        .get_property(
                            true,
                            event.requestor,
                            event.property,
                            atoms.INCR,
                            0,
                            u32::MAX / 4,
                        )?
                        .reply()?;
                    let size = reply
                        .value32()
                        .and_then(|mut values| values.next())
                        .unwrap_or(0);
                    incremental = Some(Vec::with_capacity(size as usize));
                    deadline += SEGMENT_TIMEOUT;
                }
                // Each segment of an incremental transfer comes as a new property value.
                Event::PropertyNotify(event)
                    if event.atom == atoms.APPS_CLIPBOARD && event.state == Property::NEW_VALUE =>
                {
                    let Some(data) = &mut incremental else {
                        continue;
                    };
                    let kind = if target == atoms.TARGETS {
                        atoms.ATOM
                    } else {
                        target
                    };
                    let reply = reader
                        .conn
                        .get_property(true, event.window, event.atom, kind, 0, u32::MAX / 4)?
                        .reply()?;
                    if reply.value_len == 0 {
                        return Ok((incremental.unwrap_or_default(), target));
                    }
                    data.extend(reply.value);
                    deadline = Instant::now() + SEGMENT_TIMEOUT;
                }
                _ => {}
            }
        }
        bail!("timed out reading the clipboard")
    }

    /// Answers the requests for our clipboard until the window is destroyed.
    fn serve(&self) -> Result<()> {
        loop {
            match self.server.conn.wait_for_event()? {
                Event::DestroyNotify(_) => return Ok(()),
                // Another app owns the clipboard now.
                Event::SelectionClear(event) if event.selection == self.atoms.CLIPBOARD => {
                    *self.text.lock().unwrap() = None;
                }
                Event::SelectionRequest(event) => {
                    self.answer(event)?;
                    let mut handover = self.handover.lock().unwrap();
                    // Only the contents count as written, not the target list.
                    if let Handover::InProgress { written, notified } = &mut *handover
                        && event.target != self.atoms.TARGETS
                    {
                        *written = true;
                        if *notified {
                            *handover = Handover::Finished;
                            self.handover_done.notify_all();
                        }
                    }
                }
                // Some clipboard managers say they are done before they ask for the contents.
                Event::SelectionNotify(event)
                    if event.selection == self.atoms.CLIPBOARD_MANAGER =>
                {
                    let mut handover = self.handover.lock().unwrap();
                    if let Handover::InProgress { written, notified } = &mut *handover {
                        *notified = true;
                        if *written {
                            *handover = Handover::Finished;
                            self.handover_done.notify_all();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn answer(&self, event: SelectionRequestEvent) -> Result<()> {
        let atoms = &self.atoms;
        let conn = &self.server.conn;
        let text = self.text.lock().unwrap().clone();
        let utf8 = [atoms.UTF8_STRING, atoms.UTF8_MIME_0, atoms.UTF8_MIME_1];
        let answered = match &text {
            Some(_) if event.selection == atoms.CLIPBOARD && event.target == atoms.TARGETS => {
                let targets = [
                    atoms.TARGETS,
                    atoms.SAVE_TARGETS,
                    atoms.UTF8_STRING,
                    atoms.UTF8_MIME_0,
                    atoms.UTF8_MIME_1,
                ];
                conn.change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    event.property,
                    atoms.ATOM,
                    &targets,
                )?;
                true
            }
            Some(text) if event.selection == atoms.CLIPBOARD && utf8.contains(&event.target) => {
                conn.change_property8(
                    PropMode::REPLACE,
                    event.requestor,
                    event.property,
                    event.target,
                    text,
                )?;
                true
            }
            // The clipboard was lost, or the format is not text.
            _ => false,
        };
        conn.send_event(
            false,
            event.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: event.sequence,
                time: event.time,
                requestor: event.requestor,
                selection: event.selection,
                target: event.target,
                property: if answered {
                    event.property
                } else {
                    AtomEnum::NONE.into()
                },
            },
        )?;
        conn.flush()?;
        Ok(())
    }

    /// Asks the clipboard manager to keep the text after the app quits.
    fn hand_over(&self) -> Result<()> {
        if self.text.lock().unwrap().is_none() || !self.is_owner()? {
            return Ok(());
        }
        // Locked before the request, so the server thread can't finish before this waits.
        let mut handover = self.handover.lock().unwrap();
        self.server.conn.convert_selection(
            self.server.window,
            self.atoms.CLIPBOARD_MANAGER,
            self.atoms.SAVE_TARGETS,
            self.atoms.APPS_CLIPBOARD,
            Time::CURRENT_TIME,
        )?;
        self.server.conn.flush()?;
        *handover = Handover::InProgress {
            written: false,
            notified: false,
        };
        let (_handover, timeout) = self
            .handover_done
            .wait_timeout_while(handover, Duration::from_millis(100), |handover| {
                *handover != Handover::Finished
            })
            .unwrap();
        if timeout.timed_out() {
            bail!("the clipboard manager did not take the clipboard in time");
        }
        Ok(())
    }
}
