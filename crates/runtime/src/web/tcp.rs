use crate::jsc::payload::Payload;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, watch},
    task::JoinHandle,
};

enum Command {
    Write(Vec<u8>),
    End,
    NoDelay(bool),
    KeepAlive(bool, u64),
}
trait Transport: AsyncRead + AsyncWrite + Send + Unpin + 'static {
    fn control(&self) -> std::io::Result<socket2::Socket>;
    fn address(&self) -> Value;
}
impl Transport for TcpStream {
    fn control(&self) -> std::io::Result<socket2::Socket> {
        socket2::SockRef::from(self).try_clone()
    }
    fn address(&self) -> Value {
        let local = self.local_addr().ok();
        let peer = self.peer_addr().ok();
        json!({"localAddress":local.map(|a|a.ip().to_string()),"localPort":local.map(|a|a.port()),"remoteAddress":peer.map(|a|a.ip().to_string()),"remotePort":peer.map(|a|a.port()),"remoteFamily":peer.map(|a|if a.is_ipv4(){"IPv4"}else{"IPv6"})})
    }
}
#[cfg(unix)]
impl Transport for tokio::net::UnixStream {
    fn control(&self) -> std::io::Result<socket2::Socket> {
        socket2::SockRef::from(self).try_clone()
    }
    fn address(&self) -> Value {
        json!({})
    }
}

type Failure = (std::io::Error, &'static str, Value);

fn failure((error, syscall, mut details): Failure) -> Value {
    let (code, errno) = if syscall == "getaddrinfo" {
        ("ENOTFOUND", Some(-3008))
    } else {
        (
            crate::node::code(&error),
            error.raw_os_error().map(|code| -code.abs()),
        )
    };
    let mut text = error.to_string();
    text.truncate(text.find(" (os error").unwrap_or(text.len()));
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_lowercase();
    }
    details["kind"] = "error".into();
    details["code"] = code.into();
    details["errno"] = errno.into();
    details["syscall"] = syscall.into();
    details["description"] = text.into();
    details
}

async fn run<T: Transport>(
    stream: T,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut paused: watch::Receiver<bool>,
    output: mpsc::Sender<Payload>,
    wake: &crate::host::Waker,
) -> Result<(), Failure> {
    let events = async |event: Payload| {
        let sent = output.send(event).await.is_ok();
        wake.wake();
        sent
    };
    let io = |syscall| move |error| (error, syscall, json!({}));
    if !events(json!({"kind":"connect","address":stream.address()}).into()).await {
        return Ok(());
    }
    let control = stream.control().map_err(io("connect"))?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    // WARNING: reads go on while a write waits for the peer, or two sockets piped into each other deadlock.
    let read = async {
        let mut bytes = [0; 16384];
        loop {
            let _ = paused.wait_for(|paused| !paused).await;
            let size = reader.read(&mut bytes).await.map_err(io("read"))?;
            if size == 0 {
                events(json!({"kind":"end"}).into()).await;
                return Ok(());
            }
            if !events(Payload::with_bytes(
                json!({"kind":"data"}),
                "bytes",
                bytes[..size].to_vec(),
            ))
            .await
            {
                return Ok(());
            }
        }
    };
    let write = async {
        while let Some(command) = commands.recv().await {
            match command {
                Command::Write(data) => {
                    writer.write_all(&data).await.map_err(io("write"))?;
                    if !events(json!({"kind":"written","bytes":data.len()}).into()).await {
                        break;
                    }
                }
                Command::End => {
                    writer.shutdown().await.map_err(io("shutdown"))?;
                    events(json!({"kind":"finish"}).into()).await;
                    break;
                }
                Command::NoDelay(value) => {
                    let _ = control.set_tcp_nodelay(value);
                }
                Command::KeepAlive(value, delay) => {
                    let _ = control.set_keepalive(value);
                    if value && delay > 0 {
                        let _ = control.set_tcp_keepalive(
                            &socket2::TcpKeepalive::new().with_time(Duration::from_millis(delay)),
                        );
                    }
                }
            }
        }
        Ok(())
    };
    tokio::try_join!(read, write).map(|_| ())
}

enum Accepted {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}
struct Connection {
    commands: mpsc::UnboundedSender<Command>,
    paused: watch::Sender<bool>,
    events: mpsc::Receiver<Payload>,
    task: JoinHandle<()>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
enum Listener {
    Tcp(tokio::net::TcpListener),
    #[cfg(unix)]
    Unix(tokio::net::UnixListener, String),
}
struct Server {
    events: mpsc::Receiver<Accepted>,
    listener: Arc<Mutex<Option<Listener>>>,
    task: JoinHandle<()>,
    _port: Option<crate::web::Listening>,
}
impl Drop for Server {
    fn drop(&mut self) {
        // WARNING: the listener closes here, on the JavaScript thread, so `close()` frees the address before `listen()` runs again.
        if let Ok(mut listener) = self.listener.lock() {
            #[cfg(unix)]
            if let Some(Listener::Unix(_, path)) = listener.as_ref() {
                let _ = std::fs::remove_file(path);
            }
            listener.take();
        }
        self.task.abort();
    }
}
#[derive(Default)]
pub(crate) struct State {
    connections: HashMap<u64, Connection>,
    servers: HashMap<u64, Server>,
    next_id: u64,
}

impl State {
    fn start(
        &mut self,
        future: impl std::future::Future<Output = Result<Accepted, Failure>> + Send + 'static,
    ) -> Result<u64> {
        let (commands, incoming) = mpsc::unbounded_channel();
        let (paused, pausing) = watch::channel(false);
        let (output, events) = mpsc::channel(64);
        let wake = crate::host::Waker::current();
        let task = crate::web::network()?.spawn(async move {
            let result = async {
                match future.await? {
                    Accepted::Tcp(stream) => {
                        run(stream, incoming, pausing, output.clone(), &wake).await
                    }
                    #[cfg(unix)]
                    Accepted::Unix(stream) => {
                        run(stream, incoming, pausing, output.clone(), &wake).await
                    }
                }
            }
            .await;
            let failed = result.is_err();
            if let Err(error) = result {
                let _ = output.send(failure(error).into()).await;
            }
            let _ = output
                .send(json!({"kind":"close","hadError":failed}).into())
                .await;
            wake.wake();
        });
        self.next_id += 1;
        self.connections.insert(
            self.next_id,
            Connection {
                commands,
                paused,
                events,
                task,
            },
        );
        Ok(self.next_id)
    }
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "ip" => Ok(json!(args[0].as_str().map_or(0, |value| {
                match value.split_once('%') {
                    Some((ip, zone)) => {
                        if !zone.is_empty() && ip.parse::<std::net::Ipv6Addr>().is_ok() {
                            6
                        } else {
                            0
                        }
                    }
                    None => value
                        .parse::<std::net::IpAddr>()
                        .map_or(0, |ip| if ip.is_ipv4() { 4 } else { 6 }),
                }
            }))),
            "connect" => {
                let options = args[0].clone();
                let id = self.start(async move {
                    if let Some(path) = options["path"].as_str() {
                        #[cfg(unix)]
                        return tokio::net::UnixStream::connect(path)
                            .await
                            .map(Accepted::Unix)
                            .map_err(|error| (error, "connect", json!({"address":path})));
                        #[cfg(not(unix))]
                        return Err((
                            std::io::Error::from(std::io::ErrorKind::Unsupported),
                            "connect",
                            json!({"address":path}),
                        ));
                    }
                    let host = options["host"].as_str().unwrap_or("localhost");
                    let port = options["port"]
                        .as_u64()
                        .and_then(|port| u16::try_from(port).ok())
                        .unwrap_or(0);
                    let lookup = || {
                        (
                            std::io::Error::from(std::io::ErrorKind::NotFound),
                            "getaddrinfo",
                            json!({"hostname":host}),
                        )
                    };
                    let mut last = lookup();
                    for address in tokio::net::lookup_host((host, port))
                        .await
                        .map_err(|_| lookup())?
                    {
                        match TcpStream::connect(address).await {
                            Ok(stream) => return Ok(Accepted::Tcp(stream)),
                            Err(error) => {
                                last = (
                                    error,
                                    "connect",
                                    json!({"address":address.ip().to_string(),"port":port}),
                                )
                            }
                        }
                    }
                    Err(last)
                })?;
                Ok(json!(id))
            }
            "listen" => {
                let options = &args[0];
                let runtime = crate::web::network()?;
                let _guard = runtime.enter();
                let (listener, address) = if let Some(path) = options["path"].as_str() {
                    #[cfg(unix)]
                    {
                        match tokio::net::UnixListener::bind(path) {
                            Ok(listener) => {
                                (Listener::Unix(listener, path.into()), json!({"path":path}))
                            }
                            Err(error) => {
                                return Ok(
                                    json!({"error":failure((error, "listen", json!({"address":path})))}),
                                );
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        bail!("Unix sockets are unavailable");
                    }
                } else {
                    let host = options["host"].as_str().unwrap_or("0.0.0.0");
                    let port = u16::try_from(options["port"].as_u64().unwrap_or(0))?;
                    let listener = match std::net::TcpListener::bind((host, port)) {
                        Ok(listener) => listener,
                        Err(error) => {
                            return Ok(
                                json!({"error":failure((error, "listen", json!({"address":host,"port":port})))}),
                            );
                        }
                    };
                    let address = listener.local_addr()?;
                    listener.set_nonblocking(true)?;
                    (
                        Listener::Tcp(tokio::net::TcpListener::from_std(listener)?),
                        json!({"address":address.ip().to_string(),"family":if address.is_ipv4(){"IPv4"}else{"IPv6"},"port":address.port()}),
                    )
                };
                let listener = Arc::new(Mutex::new(Some(listener)));
                let accepting = listener.clone();
                let (sender, events) = mpsc::channel(64);
                let wake = crate::host::Waker::current();
                let task = runtime.spawn(async move {
                    loop {
                        let accepted =
                            std::future::poll_fn(|cx| match accepting.lock().as_deref() {
                                Ok(Some(Listener::Tcp(listener))) => listener
                                    .poll_accept(cx)
                                    .map_ok(|(stream, _)| Accepted::Tcp(stream)),
                                #[cfg(unix)]
                                Ok(Some(Listener::Unix(listener, _))) => listener
                                    .poll_accept(cx)
                                    .map_ok(|(stream, _)| Accepted::Unix(stream)),
                                _ => Poll::Pending,
                            })
                            .await;
                        match accepted {
                            Ok(stream) => {
                                if sender.send(stream).await.is_err() {
                                    break;
                                }
                                wake.wake();
                            }
                            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                        }
                    }
                });
                self.next_id += 1;
                self.servers.insert(
                    self.next_id,
                    Server {
                        events,
                        listener,
                        task,
                        _port: address["port"]
                            .as_u64()
                            .map(|port| crate::web::Listening::new(port as u16)),
                    },
                );
                Ok(json!({"id":self.next_id,"address":address}))
            }
            "write" | "end" | "pause" | "noDelay" | "keepAlive" => {
                let socket = self
                    .connections
                    .get(&args[0].as_u64().context("Expected socket ID")?)
                    .context("Socket is closed")?;
                let command = match op {
                    "write" => Command::Write(crate::web::bytes(&args[1])?),
                    "end" => Command::End,
                    "pause" => {
                        socket.paused.send_replace(args[1] == true);
                        return Ok(Value::Null);
                    }
                    "noDelay" => Command::NoDelay(args[1] == true),
                    _ => Command::KeepAlive(args[1] == true, args[2].as_u64().unwrap_or(0)),
                };
                socket
                    .commands
                    .send(command)
                    .map_err(|_| anyhow::anyhow!("Socket is closed"))?;
                Ok(Value::Null)
            }
            "destroy" => {
                self.connections
                    .remove(&args[0].as_u64().context("Expected socket ID")?);
                Ok(Value::Null)
            }
            "stop" => {
                self.servers
                    .remove(&args[0].as_u64().context("Expected server ID")?);
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                let mut accepted = Vec::new();
                for (&id, server) in &mut self.servers {
                    for _ in 0..64 {
                        let Ok(stream) = server.events.try_recv() else {
                            break;
                        };
                        accepted.push((id, stream));
                    }
                }
                for (server, stream) in accepted {
                    let address = match &stream {
                        Accepted::Tcp(stream) => stream.address(),
                        #[cfg(unix)]
                        Accepted::Unix(stream) => stream.address(),
                    };
                    let socket = self.start(async { Ok(stream) })?;
                    events.push(
                        json!({"server":server,"id":socket,"kind":"connection","address":address}),
                    );
                }
                self.connections.retain(|id, socket| {
                    for _ in 0..64 {
                        match socket.events.try_recv() {
                            Ok(event) => {
                                let mut event = event.into_value();
                                let done = event["kind"] == "close";
                                event["id"] = json!(id);
                                events.push(event);
                                if done {
                                    return false;
                                }
                            }
                            Err(mpsc::error::TryRecvError::Empty) => break,
                            Err(mpsc::error::TryRecvError::Disconnected) => return false,
                        }
                    }
                    true
                });
                Ok(json!(events))
            }
            _ => bail!("Unsupported TCP operation: {op}"),
        }
    }
}
