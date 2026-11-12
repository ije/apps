use crate::jsc::payload::Payload;
use anyhow::{Context, Result, bail};
use rustls::{
    CertificateError, ClientConfig,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};
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
    Tls(Box<(Arc<ClientConfig>, ServerName<'static>, Arc<Checked>)>),
}
#[derive(Debug)]
struct Checked(
    Arc<dyn ServerCertVerifier>,
    bool,
    Mutex<Option<(&'static str, String)>>,
);
impl ServerCertVerifier for Checked {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let error = match self.0.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Err(rustls::Error::InvalidCertificate(error)) => error,
            result => return result,
        };
        let own = |cert| {
            webpki::EndEntityCert::try_from(cert).is_ok_and(|cert| cert.issuer() == cert.subject())
        };
        let (code, message) = match &error {
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                ("CERT_HAS_EXPIRED", "certificate has expired".into())
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                ("CERT_NOT_YET_VALID", "certificate is not yet valid".into())
            }
            CertificateError::Revoked => ("CERT_REVOKED", "certificate revoked".into()),
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                let presented = match &error {
                    CertificateError::NotValidForNameContext { presented, .. } => presented
                        .iter()
                        .map(|name| {
                            name.replace("DnsName(\"", "DNS:")
                                .replace("IpAddress(", "IP Address:")
                                .trim_end_matches(['"', ')'])
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => String::new(),
                };
                let target = match server_name {
                    ServerName::IpAddress(_) => {
                        format!("IP: {} is not in the cert's list: ", server_name.to_str())
                    }
                    _ => format!(
                        "Host: {}. is not in the cert's altnames: ",
                        server_name.to_str()
                    ),
                };
                (
                    "ERR_TLS_CERT_ALTNAME_INVALID",
                    format!(
                        "Hostname/IP does not match certificate's altnames: {target}{presented}"
                    ),
                )
            }
            CertificateError::UnknownIssuer if intermediates.is_empty() && own(end_entity) => (
                "DEPTH_ZERO_SELF_SIGNED_CERT",
                "self-signed certificate".into(),
            ),
            CertificateError::UnknownIssuer if intermediates.last().is_some_and(own) => (
                "SELF_SIGNED_CERT_IN_CHAIN",
                "self-signed certificate in certificate chain".into(),
            ),
            CertificateError::UnknownIssuer if intermediates.is_empty() => (
                "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
                "unable to verify the first certificate".into(),
            ),
            CertificateError::UnknownIssuer => (
                "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
                "unable to get local issuer certificate".into(),
            ),
            CertificateError::BadSignature => (
                "CERT_SIGNATURE_FAILURE",
                "certificate signature failure".into(),
            ),
            _ => ("CERT_UNTRUSTED", "certificate not trusted".into()),
        };
        *self.2.lock().unwrap() = Some((code, message));
        if self.1 {
            Err(rustls::Error::InvalidCertificate(error))
        } else {
            Ok(ServerCertVerified::assertion())
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_verify_schemes()
    }
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
impl Transport for tokio_rustls::client::TlsStream<Accepted> {
    fn control(&self) -> std::io::Result<socket2::Socket> {
        self.get_ref().0.control()
    }
    fn address(&self) -> Value {
        self.get_ref().0.address()
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
    details["kind"] = "error".into();
    if syscall == "tls" {
        return details;
    }
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
    details["code"] = code.into();
    details["errno"] = errno.into();
    details["syscall"] = syscall.into();
    details["description"] = text.into();
    details
}

async fn run(
    mut stream: Accepted,
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
    loop {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let upgrading = tokio::sync::Notify::new();
        // Reads go on while a write waits for the peer, or two sockets piped into each other deadlock.
        // A read stops only while it waits, so no plain byte that arrives before a TLS upgrade is lost.
        let read = async {
            let mut bytes = [0; 16384];
            let reading = async {
                loop {
                    let _ = paused.wait_for(|paused| !paused).await;
                    let Ok(permit) = output.reserve().await else {
                        return Ok(());
                    };
                    let size = match reader.read(&mut bytes).await {
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => 0,
                        result => result.map_err(io("read"))?,
                    };
                    permit.send(if size == 0 {
                        json!({"kind":"end"}).into()
                    } else {
                        Payload::with_bytes(json!({"kind":"data"}), "bytes", bytes[..size].to_vec())
                    });
                    wake.wake();
                    if size == 0 {
                        return Ok(());
                    }
                }
            };
            tokio::select! {
                result = reading => result,
                _ = upgrading.notified() => Ok(()),
            }
        };
        let write = async {
            while let Some(command) = commands.recv().await {
                match command {
                    Command::Write(data) => {
                        writer.write_all(&data).await.map_err(io("write"))?;
                        writer.flush().await.map_err(io("write"))?;
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
                                &socket2::TcpKeepalive::new()
                                    .with_time(Duration::from_millis(delay)),
                            );
                        }
                    }
                    Command::Tls(upgrade) => {
                        upgrading.notify_one();
                        return Ok(Some(upgrade));
                    }
                }
            }
            Ok(None)
        };
        let Some(upgrade) = tokio::try_join!(read, write)?.1 else {
            return Ok(());
        };
        let (config, name, checked) = *upgrade;
        let plain = reader.unsplit(writer);
        if !events(json!({"kind":"tls","address":plain.address()}).into()).await {
            return Ok(());
        }
        let secure = tokio_rustls::TlsConnector::from(config)
            .connect(name, plain)
            .await
            .map_err(|error| {
                let found = checked.2.lock().unwrap().take();
                let inner = error.get_ref().and_then(|inner| inner.downcast_ref());
                let details = match (found, inner) {
                    (Some((code, message)), _) => json!({"code":code,"message":message}),
                    // JavaScript names alerts and cipher suites: the rustls texts cost 33 KB.
                    (None, Some(rustls::Error::AlertReceived(alert))) => {
                        json!({"alert":u8::from(*alert)})
                    }
                    (None, Some(rustls::Error::InvalidMessage(_))) => {
                        json!({"code":"ERR_SSL_WRONG_VERSION_NUMBER","message":"wrong version number"})
                    }
                    (None, Some(_)) => {
                        json!({"code":"ERR_SSL_HANDSHAKE_FAILURE","message":"handshake failure"})
                    }
                    (None, None) if error.kind() == std::io::ErrorKind::UnexpectedEof => json!({
                        "code": "ECONNRESET",
                        "message": concat!(
                            "Client network socket disconnected ",
                            "before secure TLS connection was established"
                        ),
                    }),
                    (None, None) => return (error, "read", json!({})),
                };
                (error, "tls", details)
            })?;
        let (_, session) = secure.get_ref();
        let modern = session.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3);
        let event = json!({
            "kind": "secure",
            "protocol": if modern { "TLSv1.3" } else { "TLSv1.2" },
            "cipher": session.negotiated_cipher_suite().map(|suite| u16::from(suite.suite())),
            "alpn": session.alpn_protocol().map(String::from_utf8_lossy),
            "error": checked.2.lock().unwrap().take().map(|(code, _)| code),
        });
        let cert = session
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map_or(Vec::new(), |cert| cert.to_vec());
        if !events(Payload::with_bytes(event, "bytes", cert)).await {
            return Ok(());
        }
        stream = Box::new(secure);
    }
}

type Accepted = Box<dyn Transport>;
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
        // The listener closes here, on the JavaScript thread, so `close()` frees the address before `listen()` runs again.
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
            let result =
                async { run(future.await?, incoming, pausing, output.clone(), &wake).await }.await;
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
            #[cfg(unix)]
            "open" => {
                use std::os::fd::FromRawFd;
                // A copy, so destroying the socket leaves the inherited descriptor to its owner.
                let fd = unsafe {
                    libc::fcntl(
                        args[0].as_i64().context("Expected descriptor")? as i32,
                        libc::F_DUPFD_CLOEXEC,
                        64,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
                stream.set_nonblocking(true)?;
                let id = self.start(async move {
                    tokio::net::UnixStream::from_std(stream)
                        .map(|stream| Box::new(stream) as Accepted)
                        .map_err(|error| (error, "connect", json!({})))
                })?;
                Ok(json!(id))
            }
            "connect" => {
                let options = args[0].clone();
                let id = self.start(async move {
                    if let Some(path) = options["path"].as_str() {
                        #[cfg(unix)]
                        return tokio::net::UnixStream::connect(path)
                            .await
                            .map(|stream| Box::new(stream) as Accepted)
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
                            Ok(stream) => return Ok(Box::new(stream) as Accepted),
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
                let (listener, address, bound) = if let Some(path) = options["path"].as_str() {
                    #[cfg(unix)]
                    {
                        match tokio::net::UnixListener::bind(path) {
                            Ok(listener) => (
                                Listener::Unix(listener, path.into()),
                                json!({"path":path}),
                                None,
                            ),
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
                        Some(crate::web::Listening::new(address)),
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
                                    .map_ok(|(stream, _)| Box::new(stream) as Accepted),
                                #[cfg(unix)]
                                Ok(Some(Listener::Unix(listener, _))) => listener
                                    .poll_accept(cx)
                                    .map_ok(|(stream, _)| Box::new(stream) as Accepted),
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
                        _port: bound,
                    },
                );
                Ok(json!({"id":self.next_id,"address":address}))
            }
            "write" | "end" | "pause" | "noDelay" | "keepAlive" | "tls" => {
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
                    "tls" => {
                        let options = &args[1];
                        let verifier: Arc<dyn ServerCertVerifier> = match options["ca"].as_str() {
                            Some(ca) => {
                                let mut roots = rustls::RootCertStore::empty();
                                for cert in CertificateDer::pem_slice_iter(ca.as_bytes()) {
                                    roots.add(cert?)?;
                                }
                                rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
                                    .build()?
                            }
                            None => crate::web::roots(),
                        };
                        let checked = Arc::new(Checked(
                            verifier,
                            options["reject"] != false,
                            Mutex::default(),
                        ));
                        let mut config = ClientConfig::builder()
                            .dangerous()
                            .with_custom_certificate_verifier(checked.clone())
                            .with_no_client_auth();
                        config.alpn_protocols =
                            serde_json::from_value::<Vec<String>>(options["alpn"].clone())?
                                .into_iter()
                                .map(String::into_bytes)
                                .collect();
                        let name = ServerName::try_from(
                            options["servername"]
                                .as_str()
                                .context("Expected server name")?
                                .to_owned(),
                        )?;
                        Command::Tls(Box::new((Arc::new(config), name, checked)))
                    }
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
                    let address = stream.address();
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
