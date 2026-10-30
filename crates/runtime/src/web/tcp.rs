use crate::jsc::payload::Payload;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

enum Command {
    Write(Vec<u8>),
    End,
    Pause(bool),
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

async fn run<T: Transport>(
    stream: T,
    mut commands: mpsc::UnboundedReceiver<Command>,
    output: mpsc::Sender<Payload>,
    wake: &crate::host::Waker,
) -> Result<()> {
    let events = async |event: Payload| {
        output.send(event).await?;
        wake.wake();
        Ok::<_, anyhow::Error>(())
    };
    events(json!({"kind":"connect","address":stream.address()}).into()).await?;
    let mut paused = false;
    let mut reading = true;
    let mut ended = false;
    let mut bytes = [0; 16384];
    let control = stream.control()?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(Command::Write(data)) => { writer.write_all(&data).await?; events(json!({"kind":"written","bytes":data.len()}).into()).await?; }
                    Some(Command::End) => { writer.shutdown().await?; ended=true; events(json!({"kind":"finish"}).into()).await?; if !reading {break;} }
                    Some(Command::Pause(value)) => paused=value,
                    Some(Command::NoDelay(value)) => { let _ = control.set_tcp_nodelay(value); }
                    Some(Command::KeepAlive(value, delay)) => {
                        let _ = control.set_keepalive(value);
                        if value && delay > 0 { let _ = control.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(Duration::from_millis(delay))); }
                    }
                    None => break,
                }
            }
            result = reader.read(&mut bytes), if !paused && reading => {
                let size = result?;
                if size == 0 { reading=false; events(json!({"kind":"end"}).into()).await?; if ended {break;} }
                else { events(Payload::with_bytes(json!({"kind":"data"}), "bytes", bytes[..size].to_vec())).await?; }
            }
        }
    }
    Ok(())
}

enum Accepted {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}
struct Connection {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Payload>,
    task: JoinHandle<()>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Server {
    events: mpsc::Receiver<Accepted>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
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
        future: impl std::future::Future<Output = Result<Accepted>> + Send + 'static,
    ) -> Result<u64> {
        let (commands, incoming) = mpsc::unbounded_channel();
        let (output, events) = mpsc::channel(64);
        let wake = crate::host::Waker::current();
        let task = crate::web::network()?.spawn(async move {
            let result = async {
                match future.await? {
                    Accepted::Tcp(stream) => run(stream, incoming, output.clone(), &wake).await,
                    #[cfg(unix)]
                    Accepted::Unix(stream) => run(stream, incoming, output.clone(), &wake).await,
                }
            }
            .await;
            if let Err(error) = &result {
                let _ = output
                    .send(json!({"kind":"error","message":format!("{error:#}")}).into())
                    .await;
            }
            let _ = output
                .send(json!({"kind":"close","hadError":result.is_err()}).into())
                .await;
            wake.wake();
        });
        self.next_id += 1;
        self.connections.insert(
            self.next_id,
            Connection {
                commands,
                events,
                task,
            },
        );
        Ok(self.next_id)
    }
    pub(crate) fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "ip" => Ok(json!(
                args[0]
                    .as_str()
                    .and_then(|value| value.parse::<std::net::IpAddr>().ok())
                    .map_or(0, |ip| if ip.is_ipv4() { 4 } else { 6 })
            )),
            "connect" => {
                let options = args[0].clone();
                let id = self.start(async move {
                    if let Some(path) = options["path"].as_str() {
                        #[cfg(unix)]
                        {
                            return Ok(Accepted::Unix(
                                tokio::net::UnixStream::connect(path).await?,
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = path;
                            bail!("Unix sockets are unavailable");
                        }
                    }
                    let host = options["host"].as_str().unwrap_or("127.0.0.1");
                    let port = u16::try_from(options["port"].as_u64().context("Expected port")?)?;
                    Ok(Accepted::Tcp(TcpStream::connect((host, port)).await?))
                })?;
                Ok(json!(id))
            }
            "listen" => {
                let options = &args[0];
                let runtime = crate::web::network()?;
                let _guard = runtime.enter();
                let (sender, events) = mpsc::channel(64);
                let wake = crate::host::Waker::current();
                let (address, task) = if let Some(path) = options["path"].as_str() {
                    #[cfg(unix)]
                    {
                        let listener = tokio::net::UnixListener::bind(path)?;
                        let path = path.to_owned();
                        (
                            json!({"path":path}),
                            runtime.spawn(async move {
                                loop {
                                    match listener.accept().await {
                                        Ok((stream, _)) => {
                                            if sender.send(Accepted::Unix(stream)).await.is_err() {
                                                break;
                                            }
                                            wake.wake();
                                        }
                                        Err(_) => {
                                            tokio::time::sleep(Duration::from_millis(100)).await
                                        }
                                    }
                                }
                            }),
                        )
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        bail!("Unix sockets are unavailable");
                    }
                } else {
                    let host = options["host"].as_str().unwrap_or("0.0.0.0");
                    let port = u16::try_from(options["port"].as_u64().unwrap_or(0))?;
                    let listener = std::net::TcpListener::bind((host, port))?;
                    let address = listener.local_addr()?;
                    listener.set_nonblocking(true)?;
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    (
                        json!({"address":address.ip().to_string(),"port":address.port(),"family":if address.is_ipv4(){"IPv4"}else{"IPv6"}}),
                        runtime.spawn(async move {
                            loop {
                                match listener.accept().await {
                                    Ok((stream, _)) => {
                                        if sender.send(Accepted::Tcp(stream)).await.is_err() {
                                            break;
                                        }
                                        wake.wake();
                                    }
                                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                                }
                            }
                        }),
                    )
                };
                self.next_id += 1;
                self.servers.insert(self.next_id, Server { events, task });
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
                    "pause" => Command::Pause(args[1] == true),
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
                    let socket = self.start(async { Ok(stream) })?;
                    events.push(json!({"server":server,"id":socket,"kind":"connection"}));
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
