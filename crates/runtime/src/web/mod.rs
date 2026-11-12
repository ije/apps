pub(crate) mod crypto;
pub mod gpu;
pub mod media;
pub(crate) mod platform;
pub(crate) mod serve;
pub(crate) mod storage;
pub(crate) mod tcp;
pub(crate) mod websocket;
pub(crate) mod worker;

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};

static NETWORK: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static TLS: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
static ROOTS: OnceLock<Arc<NativeRoots>> = OnceLock::new();
use url::{Url, quirks};

pub(crate) fn network() -> Result<&'static tokio::runtime::Runtime> {
    if NETWORK.get().is_none() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // A current-thread runtime runs its tasks and IO only while a thread waits in its `block_on`.
        if NETWORK.set(runtime).is_ok() {
            std::thread::Builder::new()
                .name("apps-network".into())
                .spawn(|| {
                    NETWORK
                        .get()
                        .unwrap()
                        .block_on(std::future::pending::<()>())
                })?;
        }
    }
    Ok(NETWORK.get().unwrap())
}

#[derive(Debug)]
pub(crate) struct NativeRoots(
    OnceLock<Option<Arc<rustls::client::WebPkiServerVerifier>>>,
    rustls::crypto::WebPkiSupportedAlgorithms,
);

impl rustls::client::danger::ServerCertVerifier for NativeRoots {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // Loading the OS root certificates takes about 100 ms, so wait for the first TLS handshake.
        self.0
            .get_or_init(|| {
                let mut roots = rustls::RootCertStore::empty();
                roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
                rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
                    .build()
                    .ok()
            })
            .as_ref()
            .ok_or_else(|| rustls::Error::General("No root certificates".into()))?
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.1)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.1)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.1.supported_schemes()
    }
}

pub(crate) fn roots() -> Arc<NativeRoots> {
    ROOTS
        .get_or_init(|| {
            Arc::new(NativeRoots(
                OnceLock::new(),
                rustls::crypto::ring::default_provider().signature_verification_algorithms,
            ))
        })
        .clone()
}

pub(crate) fn tls() -> Arc<rustls::ClientConfig> {
    TLS.get_or_init(|| {
        Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(roots())
                .with_no_client_auth(),
        )
    })
    .clone()
}

pub fn download(url: &str) -> Result<Vec<u8>> {
    let client: Client<_, http_body_util::Empty<bytes::Bytes>> = Client::builder(
        hyper_util::rt::TokioExecutor::new(),
    )
    .build(hyper_rustls::HttpsConnector::new(
        Connector(false),
        tls(),
        false,
        Arc::new(hyper_rustls::DefaultServerNameResolver::default()),
    ));
    network()?.block_on(async {
        let mut url = Url::parse(url)?;
        for _ in 0..20 {
            ensure!(
                matches!(url.scheme(), "http" | "https"),
                "Cannot download {url}"
            );
            let response = client.get(url.as_str().parse()?).await?;
            let status = response.status();
            if status.is_redirection()
                && let Some(location) = response.headers().get("location")
            {
                url = url.join(location.to_str()?)?;
                continue;
            }
            ensure!(status.is_success(), "Cannot download {url}: HTTP {status}");
            return Ok(response.into_body().collect().await?.to_bytes().to_vec());
        }
        bail!("Too many redirects: {url}")
    })
}

pub(crate) fn bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(bytes) = crate::jsc::payload::read(value) {
        return bytes.map(Vec::from);
    }
    Ok(match value.as_str() {
        Some(text) => STANDARD.decode(text)?,
        None => Vec::deserialize(value)?,
    })
}

pub(crate) fn base64(bytes: &[u8]) -> Value {
    crate::jsc::payload::write(bytes::Bytes::copy_from_slice(bytes))
}

static LISTENING: std::sync::Mutex<Vec<std::net::SocketAddr>> = std::sync::Mutex::new(Vec::new());

pub(crate) struct Listening(std::net::SocketAddr);

impl Listening {
    pub(crate) fn new(address: std::net::SocketAddr) -> Self {
        LISTENING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(address);
        Self(address)
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        let mut addresses = LISTENING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = addresses.iter().position(|&address| address == self.0) {
            addresses.swap_remove(index);
        }
    }
}

pub(crate) static ASSETS: std::sync::OnceLock<HashMap<String, std::path::PathBuf>> =
    std::sync::OnceLock::new();

pub(crate) static LOCAL_NETWORK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// Check every resolved address, including redirects. Permit private networks only on loopback ports bound by
// this app.
pub(crate) async fn connect(
    host: &str,
    port: u16,
    restricted: bool,
) -> std::io::Result<tokio::net::TcpStream> {
    let mut addresses: Vec<_> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
        .await?
        .collect();
    if restricted && !LOCAL_NETWORK.load(std::sync::atomic::Ordering::Relaxed) {
        let listening = LISTENING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        addresses.retain(|address| {
            !private_address(address.ip())
                || address.ip().is_loopback()
                    && listening.iter().any(|bound| {
                        bound.port() == address.port()
                            && (bound.ip() == address.ip()
                                || bound.ip().is_unspecified()
                                    && bound.is_ipv4() == address.is_ipv4())
                    })
        });
        if addresses.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Pages cannot access private networks",
            ));
        }
    }
    use futures_util::future::{Either, select};
    let ipv6 = addresses
        .first()
        .ok_or(std::io::ErrorKind::NotFound)?
        .is_ipv6();
    let (preferred, other): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .partition(|address| address.is_ipv6() == ipv6);
    let mut first = std::pin::pin!(tokio::net::TcpStream::connect(preferred.as_slice()));
    let stream = match tokio::time::timeout(Duration::from_millis(300), &mut first).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if other.is_empty() => return Err(error),
        Ok(Err(_)) => tokio::net::TcpStream::connect(other.as_slice()).await?,
        Err(_) if other.is_empty() => first.await?,
        Err(_) => {
            let second = std::pin::pin!(tokio::net::TcpStream::connect(other.as_slice()));
            match select(first, second).await {
                Either::Left((Ok(stream), _)) | Either::Right((Ok(stream), _)) => stream,
                Either::Left((Err(_), rest)) | Either::Right((Err(_), rest)) => rest.await?,
            }
        }
    };
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

fn private_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
        }
        std::net::IpAddr::V6(ip) => {
            let s = ip.segments();
            // NAT64 prefixes reach the embedded IPv4 address, so check that address instead.
            if s[0] == 0x64 && s[1] == 0xff9b && (s[2..6] == [0; 4] || s[2] == 1) {
                let [a, b, c, d] = [(s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8];
                return private_address(std::net::Ipv4Addr::new(a, b, c, d).into());
            }
            ip.to_ipv4_mapped().map_or_else(
                || {
                    ip.is_loopback()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                        || s[..6] == [0; 6]
                        || (s[0] & 0xfe00) == 0xfc00
                        || (s[0] & 0xffc0) == 0xfe80
                        || (s[0] & 0xffc0) == 0xfec0
                },
                |ip| private_address(ip.into()),
            )
        }
    }
}

type Agent = Client<
    hyper_rustls::HttpsConnector<Connector>,
    http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error>,
>;

#[derive(Clone)]
struct Connector(bool);

impl tower_service::Service<hyper::Uri> for Connector {
    type Response = hyper_util::rt::TokioIo<tokio::net::TcpStream>;
    type Error = std::io::Error;
    type Future = std::pin::Pin<Box<dyn Future<Output = std::io::Result<Self::Response>> + Send>>;

    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: hyper::Uri) -> Self::Future {
        let restricted = self.0;
        Box::pin(async move {
            let host = uri.host().ok_or(std::io::ErrorKind::InvalidInput)?;
            let port = uri
                .port_u16()
                .unwrap_or(if uri.scheme_str() == Some("https") {
                    443
                } else {
                    80
                });
            let stream =
                tokio::time::timeout(Duration::from_secs(30), connect(host, port, restricted))
                    .await
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
            Ok(hyper_util::rt::TokioIo::new(stream))
        })
    }
}

struct Fetch {
    events: Receiver<crate::jsc::payload::Payload>,
    demand: Sender<()>,
    upload: Sender<Option<bytes::Bytes>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct State {
    pub(crate) renderer: bool,
    platform: crate::web::platform::State,
    servers: crate::web::serve::State,
    sockets: crate::web::websocket::State,
    workers: crate::web::worker::State,
    tcp: crate::web::tcp::State,
    storage: crate::web::storage::State,
    next_id: u64,
    fetches: HashMap<u64, Fetch>,
    agent: Option<Agent>,
}

pub(crate) fn url(input: &str, base: Option<&str>, set: Option<(&str, &str)>) -> Result<Url> {
    let base = base.map(Url::parse).transpose()?;
    let mut url = Url::options().base_url(base.as_ref()).parse(input)?;
    if let Some((key, value)) = set {
        match key {
            "href" => quirks::set_href(&mut url, value)?,
            "protocol" => {
                let _ = quirks::set_protocol(&mut url, value);
            }
            "username" => {
                let _ = quirks::set_username(&mut url, value);
            }
            "password" => {
                let _ = quirks::set_password(&mut url, value);
            }
            "host" => {
                let _ = quirks::set_host(&mut url, value);
            }
            "hostname" => {
                let _ = quirks::set_hostname(&mut url, value);
            }
            "port" => {
                let _ = quirks::set_port(&mut url, value);
            }
            "pathname" => quirks::set_pathname(&mut url, value),
            "search" => quirks::set_search(&mut url, value),
            "hash" => quirks::set_hash(&mut url, value),
            _ => bail!("Invalid URL component"),
        }
    }
    Ok(url)
}

impl State {
    pub fn configure_storage(&mut self, host: &crate::host::Host, window: Option<u64>) {
        let namespace = host.app_name.as_deref().unwrap_or("apps");
        self.storage.configure(namespace, window);
        self.workers.configure(
            namespace,
            host.packaged,
            host.disabled,
            host.scripts.clone(),
        );
    }
    pub fn call(&mut self, op: &str, args: &mut Value) -> Result<Value> {
        match op {
            "params.parse" => Ok(json!(
                url::form_urlencoded::parse(args[0].as_str().context("Expected query")?.as_bytes())
                    .collect::<Vec<_>>()
            )),
            "digest" => {
                let bytes = crate::node::bytes(&args[1])?;
                let algorithm = match args[0].as_str().context("Expected algorithm")? {
                    "SHA-1" => &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
                    "SHA-256" => &ring::digest::SHA256,
                    "SHA-384" => &ring::digest::SHA384,
                    "SHA-512" => &ring::digest::SHA512,
                    _ => bail!("Unsupported digest algorithm"),
                };
                Ok(crate::jsc::payload::write(
                    ring::digest::digest(algorithm, bytes).as_ref().to_vec(),
                ))
            }
            "asset" => {
                let url = Url::parse(args[0].as_str().context("Expected URL")?)?;
                let name = url.path().trim_start_matches('/');
                let path = ASSETS.get().and_then(|assets| assets.get(name));
                Ok(match path.map(std::fs::read) {
                    Some(Ok(data)) => crate::jsc::payload::write(data),
                    _ => Value::Null,
                })
            }
            "fetch.start" => {
                ensure!(self.fetches.len() < 256, "Too many active fetch requests");
                let mut url = Url::parse(args[0]["url"].as_str().context("Expected URL")?)?;
                ensure!(
                    matches!(url.scheme(), "http" | "https"),
                    "Fetch supports HTTP and HTTPS URLs"
                );
                ensure!(
                    url.username().is_empty() && url.password().is_none(),
                    "URL credentials are not supported"
                );
                url.set_fragment(None);
                let mut method = args[0]["method"]
                    .as_str()
                    .context("Expected method")?
                    .to_owned();
                let mut headers: Vec<(String, String)> =
                    serde_json::from_value(args[0]["headers"].clone())?;
                headers.retain(|(name, _)| {
                    let name = name.to_ascii_lowercase();
                    !matches!(
                        name.as_str(),
                        "host"
                            | "content-length"
                            | "transfer-encoding"
                            | "connection"
                            | "upgrade"
                            | "keep-alive"
                            | "te"
                            | "trailer"
                            | "expect"
                    ) && !name.starts_with("proxy-")
                });
                let mut body = bytes::Bytes::from(bytes(&args[0]["body"])?);
                let redirect = args[0]["redirect"].as_str().unwrap_or("follow").to_owned();
                let network = network()?;
                let renderer = self.renderer;
                let agent = self
                    .agent
                    .get_or_insert_with(|| {
                        Client::builder(hyper_util::rt::TokioExecutor::new())
                            .pool_timer(hyper_util::rt::TokioTimer::new())
                            .build(hyper_rustls::HttpsConnector::new(
                                Connector(renderer),
                                tls(),
                                false,
                                Arc::new(hyper_rustls::DefaultServerNameResolver::default()),
                            ))
                    })
                    .clone();
                let (events_tx, events) = mpsc::channel(64);
                let (demand, mut demand_rx) = mpsc::channel(1);
                let (upload, upload_rx) = mpsc::channel::<Option<bytes::Bytes>>(16);
                let mut upload_rx = (args[0]["stream"] == true).then_some(upload_rx);
                let wake = crate::host::Waker::current();
                let task = network.spawn(async move {
                    let result: Result<()> = async {
                        if upload_rx.is_some() { for _ in 0..16 { if events_tx.send(json!({"kind":"upload"}).into()).await.is_err() { return Ok(()); } } wake.wake(); }
                        let mut redirects = 0;
                        let response = loop {
                            let mut request = hyper::Request::builder().method(method.as_bytes()).uri(url.as_str());
                            for (name, value) in &headers { request = request.header(name, value); }
                            let has = |name: &str| headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(name));
                            if !has("accept") { request = request.header("accept", "*/*"); }
                            if !has("accept-encoding") && !has("range") { request = request.header("accept-encoding", "gzip"); }
                            let streaming = upload_rx.is_some();
                            let request_body = if let Some(receiver) = upload_rx.take() {
                                let sender = (events_tx.clone(), wake.clone());
                                http_body_util::StreamBody::new(futures_util::stream::unfold((receiver, sender), |(mut receiver, sender)| async move {
                                    let chunk = receiver.recv().await.flatten()?;
                                    sender.0.send(json!({"kind": "upload"}).into()).await.ok()?;
                                    sender.1.wake();
                                    Some((Ok::<_, std::io::Error>(hyper::body::Frame::data(chunk)), (receiver, sender)))
                                })).boxed_unsync()
                            } else { http_body_util::Full::new(body.clone()).map_err(|never| match never {}).boxed_unsync() };
                            let response = agent.request(request.body(request_body)?).await?;
                            let status = response.status().as_u16();
                            if !matches!(status, 301 | 302 | 303 | 307 | 308) { break response; }
                            ensure!(redirect != "error", "Redirect is not allowed");
                            if redirect == "manual" { break response; }
                            let Some(location) = response.headers().get("location") else { break response; };
                            ensure!(redirects < 20, "Too many redirects");
                            let mut next = url.join(location.to_str()?)?;
                            ensure!(matches!(next.scheme(), "http" | "https"), "Invalid redirect scheme");
                            ensure!(next.username().is_empty() && next.password().is_none(), "Redirect credentials are not supported");
                            next.set_fragment(None);
                            if url.origin() != next.origin() {
                                headers.retain(|(name, _)| !matches!(name.as_str(), "authorization" | "proxy-authorization" | "cookie"));
                            }
                            ensure!(!streaming || status == 303, "Cannot replay a stream body after a redirect");
                            if (matches!(status, 301 | 302) && method == "POST") || (status == 303 && method != "GET" && method != "HEAD") {
                                method = "GET".into();
                                body = bytes::Bytes::new();
                                headers.retain(|(name, _)| !matches!(name.as_str(), "content-type" | "content-length" | "content-encoding" | "content-language" | "content-location"));
                            }
                            url = next;
                            redirects += 1;
                        };
                        let (parts, mut response) = response.into_parts();
                        let mut gzip = parts.headers.get_all("content-encoding").iter().any(|value| value == "gzip").then(|| flate2::write::GzDecoder::new(Vec::new()));
                        let headers: Vec<_> = parts.headers.iter().filter(|(name, _)| gzip.is_none() || !matches!(name.as_str(), "content-encoding" | "content-length")).map(|(name, value)| {
                            (name.as_str(), value.as_bytes().iter().map(|&b| char::from(b)).collect::<String>())
                        }).collect();
                        let metadata = json!({"kind": "headers", "status": parts.status.as_u16(),
                            "statusText": parts.status.canonical_reason().unwrap_or(""), "headers": headers,
                            "url": url.as_str(), "redirected": redirects > 0});
                        if events_tx.send(metadata.into()).await.is_err() { return Ok(()); }
                        wake.wake();
                        let (mut buffer, mut input, mut wanted, mut ended, mut pulled) = (Vec::new(), bytes::Bytes::new(), false, false, false);
                        loop {
                            // Limit read-ahead before the first pull: unread bodies retain buffers until GC.
                            let limit = if pulled { 1 << 20 } else { 64 << 10 };
                            // Gzip data can inflate 1000 times, so inflate it in small steps.
                            while !input.is_empty() && buffer.len() < limit {
                                match &mut gzip {
                                    Some(decoder) => { decoder.write_all(&input.split_to(input.len().min(4096)))?; buffer.append(decoder.get_mut()); }
                                    None => buffer.extend_from_slice(&std::mem::take(&mut input)),
                                }
                            }
                            if wanted && (ended || !buffer.is_empty()) {
                                wanted = false;
                                let done = buffer.is_empty();
                                let event = if done { json!({"kind": "done"}).into() } else { crate::jsc::payload::Payload::with_bytes(json!({"kind":"chunk"}), "bytes", std::mem::take(&mut buffer)) };
                                if events_tx.send(event).await.is_err() { break; }
                                wake.wake();
                                if done { break; }
                                continue;
                            }
                            tokio::select! {
                                demand = demand_rx.recv() => { if demand.is_none() { break; } wanted = true; pulled = true; }
                                frame = response.frame(), if !ended && input.is_empty() && buffer.len() < limit => match frame.transpose()? {
                                    Some(frame) => input = frame.into_data().unwrap_or_default(),
                                    None => {
                                        if let Some(decoder) = gzip.as_mut().filter(|decoder| decoder.header().is_some()) { decoder.try_finish()?; buffer.append(decoder.get_mut()); }
                                        ended = true;
                                    }
                                },
                            }
                        }
                        Ok(())
                    }.await;
                    if let Err(error) = result { let _ = events_tx.send(json!({"kind": "error", "message": format!("{error:#}")}).into()).await; wake.wake(); }
                });
                self.next_id += 1;
                self.fetches.insert(
                    self.next_id,
                    Fetch {
                        events,
                        demand,
                        upload,
                        task,
                    },
                );
                Ok(json!(self.next_id))
            }
            "fetch.upload" => {
                if let Some(fetch) = self
                    .fetches
                    .get(&args[0].as_u64().context("Expected fetch ID")?)
                {
                    let chunk = if args[1].is_null() {
                        None
                    } else {
                        Some(bytes::Bytes::from(bytes(&args[1])?))
                    };
                    if let Err(mpsc::error::TrySendError::Full(_)) = fetch.upload.try_send(chunk) {
                        bail!("Upload is not ready");
                    }
                }
                Ok(Value::Null)
            }
            "fetch.pull" => {
                if let Some(fetch) = self
                    .fetches
                    .get(&args[0].as_u64().context("Expected fetch ID")?)
                {
                    let _ = fetch.demand.try_send(());
                }
                Ok(Value::Null)
            }
            "fetch.cancel" => {
                self.fetches
                    .remove(&args[0].as_u64().context("Expected fetch ID")?);
                Ok(Value::Null)
            }
            "fetch.poll" => {
                let mut events = Vec::new();
                self.fetches.retain(|id, fetch| {
                    for _ in 0..64 {
                        match fetch.events.try_recv() {
                            Ok(event) => { let mut event = event.into_value(); let done = event["kind"] == "done" || event["kind"] == "error"; event["id"] = json!(id); events.push(event); if done { return false; } }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => { events.push(json!({"id": id, "kind": "error", "message": "Fetch worker closed"})); return false; }
                        }
                    }
                    true
                });
                Ok(json!(events))
            }
            _ if op.starts_with("platform.") => self.platform.call(&op[9..], args),
            _ if op.starts_with("serve.") => self.servers.call(&op[6..], args, &mut self.sockets),
            _ if op.starts_with("socket.") => self.sockets.call(&op[7..], args, self.renderer),
            _ if op.starts_with("crypto.") => crate::web::crypto::call(&op[7..], args),
            _ if op.starts_with("worker.") => self.workers.call(&op[7..], args),
            _ if op.starts_with("tcp.") => self.tcp.call(&op[4..], args),
            _ if op.starts_with("storage.") => self.storage.call(&op[8..], args),
            "cpus" => Ok(std::thread::available_parallelism()
                .map_or(1, |count| count.get())
                .into()),
            _ => bail!("Unknown web operation: {op}"),
        }
    }
}
