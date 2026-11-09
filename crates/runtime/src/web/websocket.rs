use crate::web::bytes;
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

pub(crate) struct Socket {
    commands: mpsc::UnboundedSender<Message>,
    events: mpsc::Receiver<crate::jsc::payload::Payload>,
    written: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
type Stream = WebSocketStream<Box<dyn Io>>;

async fn run(
    socket: Stream,
    mut commands: mpsc::UnboundedReceiver<Message>,
    events: mpsc::Sender<crate::jsc::payload::Payload>,
    written: Arc<AtomicUsize>,
    wake: crate::host::Waker,
) -> Result<()> {
    let (writer, mut reader) = socket.split();
    let writer = tokio::sync::Mutex::new(writer);
    // WARNING: reads must go on while a send waits for the peer, or two peers that both stream can deadlock.
    let read = async {
        while let Some(message) = reader.next().await {
            let event = match message? {
                Message::Text(text) => json!({"kind":"message","text":text.as_str()}).into(),
                Message::Binary(bytes) => crate::jsc::payload::Payload::with_bytes(
                    json!({"kind":"message"}),
                    "bytes",
                    bytes,
                ),
                Message::Close(frame) => {
                    let (code, reason) = frame
                        .map(|f| (u16::from(f.code), f.reason.to_string()))
                        .unwrap_or((1005, String::new()));
                    let _ = writer.lock().await.close().await;
                    let _ = events
                        .send(
                            json!({"kind":"close","code":code,"reason":reason,"clean":true}).into(),
                        )
                        .await;
                    wake.wake();
                    return Ok(());
                }
                _ => continue,
            };
            if events.send(event).await.is_err() {
                return Ok(());
            }
            wake.wake();
        }
        let _ = events
            .send(json!({"kind":"close","code":1006,"reason":"","clean":false}).into())
            .await;
        wake.wake();
        Ok(())
    };
    let write = async {
        while let Some(command) = commands.recv().await {
            let (size, closing) = (command.len(), command.is_close());
            writer.lock().await.send(command).await?;
            written.fetch_add(size, Ordering::Relaxed);
            if commands.is_empty() {
                wake.wake();
            }
            if closing {
                // WARNING: a peer that never answers the close frame would keep the socket CLOSING forever.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let _ = events
                    .send(json!({"kind":"close","code":1006,"reason":"","clean":false}).into())
                    .await;
                wake.wake();
                break;
            }
        }
        anyhow::Ok(())
    };
    tokio::select! {
        result = read => result,
        result = write => result,
    }
}

fn limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(16 << 20))
        .max_frame_size(Some(16 << 20))
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) sockets: HashMap<u64, Socket>,
    next_id: u64,
}

impl State {
    fn start(
        &mut self,
        open: impl Future<Output = Result<(Stream, String)>> + Send + 'static,
    ) -> Result<u64> {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel(64);
        let written = Arc::new(AtomicUsize::new(0));
        let counter = written.clone();
        let wake = crate::host::Waker::current();
        let task = crate::web::network()?.spawn(async move {
            let result = async {
                let (socket, protocol) = open.await?;
                let _ = event_tx
                    .send(json!({"kind":"open","protocol":protocol}).into())
                    .await;
                wake.wake();
                run(socket, command_rx, event_tx.clone(), counter, wake.clone()).await
            }
            .await;
            if let Err(error) = result {
                let _ = event_tx
                    .send(json!({"kind":"error","message":format!("{error:#}")}).into())
                    .await;
                wake.wake();
            }
        });
        self.next_id += 1;
        self.sockets.insert(
            self.next_id,
            Socket {
                commands,
                events,
                written,
                task,
            },
        );
        Ok(self.next_id)
    }

    pub(crate) fn accept(
        &mut self,
        upgrade: hyper::upgrade::OnUpgrade,
        protocol: String,
    ) -> Result<u64> {
        ensure!(self.sockets.len() < 1024, "Too many WebSockets");
        self.start(async move {
            let transport: Box<dyn Io> = Box::new(hyper_util::rt::TokioIo::new(upgrade.await?));
            let socket = WebSocketStream::from_raw_socket(
                transport,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                Some(limits()),
            )
            .await;
            Ok((socket, protocol))
        })
    }

    pub(crate) fn call(&mut self, op: &str, args: &Value, renderer: bool) -> Result<Value> {
        match op {
            "connect" => {
                ensure!(self.sockets.len() < 256, "Too many WebSockets");
                let url = url::Url::parse(args[0].as_str().context("Expected WebSocket URL")?)?;
                ensure!(
                    matches!(url.scheme(), "ws" | "wss") && url.fragment().is_none(),
                    "Invalid WebSocket URL"
                );
                let mut request = url.as_str().into_client_request()?;
                let protocols: Vec<String> = serde_json::from_value(args[1].clone())?;
                if !protocols.is_empty() {
                    request
                        .headers_mut()
                        .insert("sec-websocket-protocol", protocols.join(", ").parse()?);
                }
                let id = self.start(async move {
                    let host = url.host_str().context("Missing WebSocket host")?;
                    let port = url
                        .port_or_known_default()
                        .context("Missing WebSocket port")?;
                    let stream = crate::web::connect(host, port, renderer).await?;
                    let stream: Box<dyn Io> = if url.scheme() == "wss" {
                        let name = ServerName::try_from(host.trim_matches(['[', ']']).to_owned())?;
                        Box::new(
                            TlsConnector::from(crate::web::tls())
                                .connect(name, stream)
                                .await?,
                        )
                    } else {
                        Box::new(stream)
                    };
                    let (socket, response) = tokio_tungstenite::client_async_with_config(
                        request,
                        stream,
                        Some(limits()),
                    )
                    .await?;
                    let protocol = response
                        .headers()
                        .get("sec-websocket-protocol")
                        .map(|v| v.to_str())
                        .transpose()?
                        .unwrap_or("");
                    ensure!(
                        protocol.is_empty() || protocols.iter().any(|p| p == protocol),
                        "Server selected an unrequested protocol"
                    );
                    Ok((socket, protocol.to_owned()))
                })?;
                Ok(json!(id))
            }
            "send" | "close" => {
                let id = args[0].as_u64().context("Expected socket ID")?;
                let socket = self.sockets.get(&id).context("WebSocket is closed")?;
                let message = if op == "close" {
                    let code = args[1].as_u64().unwrap_or(1000) as u16;
                    Message::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: args[2].as_str().unwrap_or("").to_owned().into(),
                    }))
                } else if args[2] == true {
                    Message::Binary(bytes(&args[1])?.into())
                } else {
                    Message::Text(args[1].as_str().context("Expected text")?.to_owned().into())
                };
                let size = message.len();
                let _ = socket.commands.send(message);
                Ok(json!(size))
            }
            "abort" => {
                self.sockets
                    .remove(&args[0].as_u64().context("Expected socket ID")?);
                Ok(Value::Null)
            }
            "poll" => {
                let mut events = Vec::new();
                self.sockets.retain(|id, socket| {
                    let written = socket.written.swap(0, Ordering::Relaxed);
                    if written > 0 {
                        events.push(json!({"id":id,"kind":"written","bytes":written}));
                    }
                    for _ in 0..64 {
                        match socket.events.try_recv() {
                            Ok(event) => {
                                let mut event = event.into_value();
                                let done =
                                    matches!(event["kind"].as_str(), Some("close" | "error"));
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
            _ => bail!("Unknown WebSocket operation: {op}"),
        }
    }
}
