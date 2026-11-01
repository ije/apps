use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    collections::HashMap,
    convert::Infallible,
    net::TcpListener,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

type Body = UnsyncBoxBody<Bytes, std::io::Error>;
type Reply = (
    oneshot::Sender<Response<Body>>,
    Option<hyper::upgrade::OnUpgrade>,
);
type RequestEvent = (
    crate::jsc::payload::Payload,
    Option<(Reply, mpsc::Sender<()>)>,
);

fn full(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

struct Server {
    events: mpsc::Receiver<RequestEvent>,
    replies: HashMap<u64, Reply>,
    task: JoinHandle<()>,
    listener: Arc<Mutex<Option<tokio::net::TcpListener>>>,
    stop: watch::Sender<bool>,
    streams: HashMap<u64, (mpsc::Sender<Option<Bytes>>, Arc<AtomicBool>)>,
    sockets: Vec<u64>,
    uploads: HashMap<u64, mpsc::Sender<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut listener) = self.listener.lock() {
            listener.take();
        }
        self.task.abort();
    }
}

#[derive(Default)]
pub(crate) struct State {
    servers: HashMap<u64, Server>,
    next_id: u64,
}

impl State {
    pub(crate) fn call(
        &mut self,
        op: &str,
        args: &Value,
        sockets: &mut crate::web::websocket::State,
    ) -> Result<Value> {
        match op {
            "start" => {
                let hostname = args[0]["hostname"].as_str().context("Expected hostname")?;
                let port = u16::try_from(args[0]["port"].as_u64().context("Expected port")?)?;
                let listener = TcpListener::bind((hostname, port))?;
                let address = listener.local_addr()?;
                listener.set_nonblocking(true)?;
                let network = crate::web::network()?;
                let _guard = network.enter();
                let listener = tokio::net::TcpListener::from_std(listener)?;
                let (sender, events) = mpsc::channel(64);
                let wake = crate::host::Waker::current();
                // WARNING: `stop` drops the listener on the JavaScript thread, so the port is free before it returns.
                let listener = Arc::new(Mutex::new(Some(listener)));
                let (stop, stopping) = watch::channel(false);
                let accepting = listener.clone();
                let task = network.spawn(async move {
                    let _closed = crate::host::WakeOnDrop(wake.clone());
                    let next_id = Arc::new(AtomicU64::new(0));
                    let mut connections = JoinSet::new();
                    let mut stopped = stopping.clone();
                    loop {
                        let accepted = tokio::select! {
                            accepted = std::future::poll_fn(|cx| match accepting.lock().as_deref() {
                                Ok(Some(listener)) => listener.poll_accept(cx).map(Some),
                                _ => Poll::Ready(None),
                            }) => accepted,
                            _ = stopped.wait_for(|stop| *stop) => None,
                        };
                        while connections.try_join_next().is_some() {}
                        let (stream, peer) = match accepted {
                            Some(Ok(accepted)) => accepted,
                            Some(Err(_)) => {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            None => break,
                        };
                        let _ = stream.set_nodelay(true);
                        let sender = sender.clone();
                        let next_id = next_id.clone();
                        let wake = wake.clone();
                        let mut stopped = stopping.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |mut request: Request<Incoming>| {
                                let sender = sender.clone();
                                let wake = wake.clone();
                                let id = next_id.fetch_add(1, Ordering::Relaxed);
                                async move {
                                    let upgrade = request.headers().get("upgrade").is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket")).then(|| hyper::upgrade::on(&mut request));
                                    let (parts, mut body) = request.into_parts();
                                    let has_body = !hyper::body::Body::is_end_stream(&body);
                                    let authority = parts.uri.authority().map(|value| value.as_str())
                                        .or_else(|| parts.headers.get("host").and_then(|value| value.to_str().ok()))
                                        .map(str::to_owned).unwrap_or_else(|| address.to_string());
                                    let path = parts.uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
                                    let headers: Vec<_> = parts.headers.iter().map(|(name, value)| {
                                        (name.as_str(), value.as_bytes().iter().map(|&byte| char::from(byte)).collect::<String>())
                                    }).collect();
                                    let (reply, result) = oneshot::channel();
                                    let (demand, mut requests) = mpsc::channel(1);
                                    let event = json!({"id": id, "url": format!("http://{authority}{path}"), "method": parts.method.as_str(), "headers": headers, "body": has_body, "peer": {"address":peer.ip().to_string(),"port":peer.port(),"family":if peer.is_ipv4(){"IPv4"}else{"IPv6"}}});
                                    if sender.send((event.into(), Some(((reply, upgrade), demand)))).await.is_err() {
                                        return Ok::<_, Infallible>(Response::builder().status(503).body(full(Bytes::new())).unwrap());
                                    }
                                    wake.wake();
                                    // WARNING: `poll` only sees a dropped reply, so wake it when the client goes away.
                                    let _closed = crate::host::WakeOnDrop(wake.clone());
                                    let (failed, failure) = oneshot::channel();
                                    tokio::spawn(async move {
                                      let (mut buffer, mut wanted, mut ended) = (Vec::new(), false, false);
                                      loop {
                                        tokio::select! {
                                            demand = requests.recv() => { if demand.is_some() { wanted=true; } else { break; } },
                                            frame = tokio::time::timeout(Duration::from_secs(30),body.frame()), if !ended && buffer.len() < 1 << 20 => match frame {
                                                Ok(Some(Ok(frame))) => { if let Ok(bytes)=frame.into_data() { buffer.extend_from_slice(&bytes); } },
                                                Ok(None) => ended=true,
                                                error => {
                                                    let (status,message)=match error { Err(_)=>(408,"Request body timed out".to_owned()),Ok(Some(Err(error)))=>(400,error.to_string()),_=>unreachable!() };
                                                    let _=sender.send((json!({"kind":"body","id":id,"error":message}).into(),None)).await;
                                                    wake.wake();
                                                    let _=failed.send(status); return;
                                                }
                                            },
                                        }
                                        if wanted && (ended || !buffer.is_empty()) {
                                            wanted=false;
                                            let done=buffer.is_empty();
                                            let event=if done { json!({"kind":"body","id":id,"done":true}).into() } else { crate::jsc::payload::Payload::with_bytes(json!({"kind":"body","id":id}), "bytes", std::mem::take(&mut buffer)) };
                                            if sender.send((event,None)).await.is_err() { break; }
                                            wake.wake();
                                            if done { break; }
                                        }
                                      }
                                    });
                                    tokio::pin!(result);
                                    let response=tokio::select! {
                                        response=&mut result=>response,
                                        status=failure=>match status {Ok(status)=>return Ok(Response::builder().status(status).header("connection","close").body(full(Bytes::new())).unwrap()),Err(_)=>result.await},
                                    };
                                    Ok(response.unwrap_or_else(|_|Response::builder().status(503).body(full(Bytes::new())).unwrap()))
                                }
                            });
                            let connection = http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(Duration::from_secs(5))
                                .serve_connection(TokioIo::new(stream), service).with_upgrades();
                            tokio::pin!(connection);
                            // WARNING: a stopping server closes idle connections and lets requests in flight finish, like Node's `server.close()`.
                            let stop = tokio::select! {
                                _ = connection.as_mut() => false,
                                _ = stopped.wait_for(|stop| *stop) => true,
                            };
                            if stop {
                                connection.as_mut().graceful_shutdown();
                                let _ = connection.await;
                            }
                        });
                    }
                    while connections.join_next().await.is_some() {}
                });
                self.next_id += 1;
                self.servers.insert(
                    self.next_id,
                    Server {
                        events,
                        replies: HashMap::new(),
                        streams: HashMap::new(),
                        sockets: Vec::new(),
                        uploads: HashMap::new(),
                        task,
                        listener,
                        stop,
                    },
                );
                Ok(json!({"id": self.next_id, "port": address.port()}))
            }
            "poll" => {
                let (mut events, mut closed) = (Vec::new(), Vec::new());
                for (&id, server) in &mut self.servers {
                    server.replies.retain(|request, reply| {
                        if reply.0.is_closed() {
                            events.push(json!({"server":id,"id":request,"kind":"cancel"}));
                            false
                        } else {
                            true
                        }
                    });
                    server.uploads.retain(|_, upload| !upload.is_closed());
                    server.streams.retain(|request, (stream, _)| {
                        let closed = stream.is_closed();
                        if closed {
                            events.push(json!({"server":id,"id":request,"kind":"cancel"}));
                        }
                        !closed
                    });
                    for _ in 0..64 {
                        let Ok((event, reply)) = server.events.try_recv() else {
                            break;
                        };
                        let mut event = event.into_value();
                        if let Some((reply, demand)) = reply {
                            if reply.0.is_closed() {
                                continue;
                            }
                            let request = event["id"].as_u64().unwrap();
                            if event["body"] == true {
                                server.uploads.insert(request, demand);
                            }
                            server.replies.insert(request, reply);
                        }
                        if event["kind"] == "body"
                            && (event["done"] == true || event.get("error").is_some())
                        {
                            server.uploads.remove(&event["id"].as_u64().unwrap());
                        }
                        event["server"] = json!(id);
                        events.push(event);
                    }
                    if server.task.is_finished() && server.events.is_empty() {
                        events.push(json!({"server":id,"kind":"closed"}));
                        closed.push(id);
                    }
                }
                for id in closed {
                    self.servers.remove(&id);
                }
                Ok(json!(events))
            }
            "pull" | "cancel" => {
                let server = self
                    .servers
                    .get_mut(&args[0].as_u64().context("Expected server ID")?)
                    .context("Server is closed")?;
                let id = args[1].as_u64().context("Expected request ID")?;
                if op == "cancel" {
                    server.uploads.remove(&id);
                } else if let Some(upload) = server.uploads.get(&id) {
                    match upload.try_send(()) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(_) => bail!("Request body is closed"),
                    }
                } else {
                    bail!("Request body is closed");
                }
                Ok(Value::Null)
            }
            "respond" => {
                let server = args[0].as_u64().context("Expected server ID")?;
                let id = args[1].as_u64().context("Expected request ID")?;
                let Some(server) = self.servers.get_mut(&server) else {
                    return Ok(Value::Null);
                };
                if !server.replies.contains_key(&id) {
                    return Ok(Value::Null);
                }
                let value = &args[2];
                let status = value["status"].as_u64().context("Expected status")?;
                ensure!((200..=599).contains(&status), "Invalid response status");
                let body = crate::web::bytes(&value["body"])?;
                let headers: Vec<(String, String)> =
                    serde_json::from_value(value["headers"].clone())?;
                let mut response = Response::builder().status(status as u16);
                for (name, value) in headers {
                    if name.eq_ignore_ascii_case("transfer-encoding") {
                        continue;
                    }
                    let value = value
                        .chars()
                        .map(|c| u8::try_from(c as u32))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    response = response.header(name, value);
                }
                if let Some(reason) = value["statusText"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                {
                    let bytes = reason
                        .chars()
                        .map(|c| u8::try_from(c as u32))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    response = response.extension(hyper::ext::ReasonPhrase::try_from(bytes)?);
                }
                let body = if value["stream"] == true {
                    let (sender, receiver) = mpsc::channel::<Option<Bytes>>(16);
                    let full = Arc::new(AtomicBool::new(false));
                    server.streams.insert(id, (sender, full.clone()));
                    let closed = crate::host::WakeOnDrop(crate::host::Waker::current());
                    StreamBody::new(futures_util::stream::unfold(
                        (receiver, closed, full),
                        |(mut receiver, closed, full)| async move {
                            let bytes = receiver.recv().await.flatten()?;
                            // WARNING: a writer that found the queue full waits for this wake.
                            if full.swap(false, Ordering::AcqRel) {
                                closed.0.wake();
                            }
                            Some((
                                Ok::<_, std::io::Error>(hyper::body::Frame::data(bytes)),
                                (receiver, closed, full),
                            ))
                        },
                    ))
                    .boxed_unsync()
                } else {
                    full(Bytes::from(body))
                };
                let response = response.body(body)?;
                let _ = server.replies.remove(&id).unwrap().0.send(response);
                Ok(Value::Null)
            }
            "write" => {
                let server = self
                    .servers
                    .get_mut(&args[0].as_u64().context("Expected server ID")?)
                    .context("Server is closed")?;
                let id = args[1].as_u64().context("Expected request ID")?;
                let (stream, full) = server
                    .streams
                    .get(&id)
                    .context("Response stream is closed")?;
                let chunk = if args[2].is_null() {
                    None
                } else {
                    Some(Bytes::from(crate::web::bytes(&args[2])?))
                };
                full.store(true, Ordering::Release);
                match stream.try_send(chunk) {
                    Ok(()) => {
                        full.store(false, Ordering::Release);
                        if args[2].is_null() {
                            server.streams.remove(&id);
                        }
                        Ok(json!(true))
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => Ok(json!(false)),
                    Err(_) => bail!("Response stream is closed"),
                }
            }
            "upgrade" => {
                let server = self
                    .servers
                    .get_mut(&args[0].as_u64().context("Expected server ID")?)
                    .context("Server is closed")?;
                let id = args[1].as_u64().context("Expected request ID")?;
                let key = args[2].as_str().context("Expected WebSocket key")?;
                ensure!(
                    base64::engine::general_purpose::STANDARD.decode(key)?.len() == 16,
                    "Invalid WebSocket key"
                );
                let protocol = args[3].as_str().unwrap_or("");
                let mut response = Response::builder()
                    .status(101)
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade")
                    .header(
                        "sec-websocket-accept",
                        tokio_tungstenite::tungstenite::handshake::derive_accept_key(
                            key.as_bytes(),
                        ),
                    );
                if !protocol.is_empty() {
                    response = response.header("sec-websocket-protocol", protocol);
                }
                let response = response.body(full(Bytes::new()))?;
                let (reply, upgrade) = server
                    .replies
                    .remove(&id)
                    .context("Request is no longer pending")?;
                server.uploads.remove(&id);
                let socket = sockets.accept(
                    upgrade.context("Request is not a WebSocket upgrade")?,
                    protocol.to_owned(),
                )?;
                server.sockets.retain(|id| sockets.sockets.contains_key(id));
                server.sockets.push(socket);
                let _ = reply.send(response);
                Ok(json!(socket))
            }
            "stop" => {
                let id = args[0].as_u64().context("Expected server ID")?;
                if args[1] != true {
                    if let Some(server) = self.servers.get(&id) {
                        if let Ok(mut listener) = server.listener.lock() {
                            listener.take();
                        }
                        server.stop.send_replace(true);
                    }
                } else if let Some(server) = self.servers.remove(&id) {
                    for id in &server.sockets {
                        sockets.call("abort", &json!([id]), false)?;
                    }
                }
                Ok(Value::Null)
            }
            _ => bail!("Unknown server operation: {op}"),
        }
    }
}
