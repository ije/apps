pub(crate) mod crypto;
pub(crate) mod gpu;
pub(crate) mod platform;
pub(crate) mod serve;
pub(crate) mod storage;
pub(crate) mod tcp;
pub(crate) mod websocket;
pub(crate) mod worker;

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::{collections::HashMap, sync::OnceLock, time::Duration};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};

static NETWORK: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
use url::{Url, quirks};

pub(crate) fn network() -> Result<&'static tokio::runtime::Runtime> {
    if NETWORK.get().is_none() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let _ = NETWORK.set(runtime);
    }
    Ok(NETWORK.get().unwrap())
}

pub(crate) fn bytes(value: &Value) -> Result<Vec<u8>> {
    if let Some(bytes) = crate::jsc::payload::read(value) {
        return bytes;
    }
    Ok(match value.as_str() {
        Some(text) => STANDARD.decode(text)?,
        None => Vec::deserialize(value)?,
    })
}

pub(crate) fn base64(bytes: &[u8]) -> Value {
    crate::jsc::payload::write(bytes).unwrap_or_else(|| Value::String(STANDARD.encode(bytes)))
}

pub(crate) fn private_address(ip: std::net::IpAddr) -> bool {
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
        std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or_else(
            || {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || (ip.segments()[0] & 0xfe00) == 0xfc00
                    || (ip.segments()[0] & 0xffc0) == 0xfe80
            },
            |ip| private_address(ip.into()),
        ),
    }
}

struct PublicDns;

impl reqwest::dns::Resolve for PublicDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| private_address(address.ip()))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Pages cannot access private networks",
                )
                .into());
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
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
    agent: Option<reqwest::Client>,
}

impl State {
    pub fn configure_storage(&mut self, packaged: bool, namespace: &str, window: Option<u64>) {
        self.storage.configure(namespace, window);
        self.workers.configure(namespace, packaged);
    }
    pub fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "url" => {
                let input = args[0].as_str().context("Expected URL")?;
                let base = args[1].as_str().map(Url::parse).transpose()?;
                let mut url = Url::options().base_url(base.as_ref()).parse(input)?;
                if let Some(key) = args[2].as_str() {
                    let value = args[3].as_str().context("Expected URL component")?;
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
                Ok(json!({
                    "href": url.as_str(), "origin": quirks::origin(&url),
                    "protocol": quirks::protocol(&url), "username": url.username(),
                    "password": quirks::password(&url), "host": quirks::host(&url),
                    "hostname": quirks::hostname(&url), "port": quirks::port(&url),
                    "pathname": url.path(), "search": quirks::search(&url), "hash": quirks::hash(&url)
                }))
            }
            "params.parse" => Ok(json!(
                url::form_urlencoded::parse(args[0].as_str().context("Expected query")?.as_bytes())
                    .collect::<Vec<_>>()
            )),
            "params.stringify" => {
                let pairs: Vec<(String, String)> = serde_json::from_value(args[0].clone())?;
                Ok(json!(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(pairs)
                        .finish()
                ))
            }
            "random" => {
                let length = args[0].as_u64().context("Expected byte length")?;
                ensure!(length <= 65536, "Random byte quota exceeded");
                let mut bytes = vec![0; length as usize];
                getrandom::fill(&mut bytes)
                    .map_err(|e| anyhow::anyhow!("Random source failed: {e}"))?;
                Ok(base64(&bytes))
            }
            "digest" => {
                let bytes = bytes(&args[1])?;
                let digest = match args[0].as_str().context("Expected algorithm")? {
                    "SHA-1" => sha1_smol::Sha1::from(&bytes).digest().bytes().to_vec(),
                    "SHA-256" => Sha256::digest(&bytes).to_vec(),
                    "SHA-384" => Sha384::digest(&bytes).to_vec(),
                    "SHA-512" => Sha512::digest(&bytes).to_vec(),
                    _ => bail!("Unsupported digest algorithm"),
                };
                Ok(base64(&digest))
            }
            "decode" => {
                let bytes = bytes(&args[0])?;
                let mut offset = 0;
                let mut end = bytes.len();
                while let Err(error) = std::str::from_utf8(&bytes[offset..]) {
                    offset += error.valid_up_to();
                    if args[1] == true && error.error_len().is_none() {
                        end = offset;
                        break;
                    }
                    ensure!(args[2] != true, "Invalid UTF-8 data");
                    offset += error.error_len().unwrap_or(bytes.len() - offset);
                }
                Ok(
                    json!({"text": String::from_utf8_lossy(&bytes[..end]), "pending": base64(&bytes[end..])}),
                )
            }
            "fetch.start" => {
                ensure!(self.fetches.len() < 64, "Too many active fetch requests");
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
                let mut body = bytes::Bytes::from(bytes(&args[0]["body"])?);
                let redirect = args[0]["redirect"].as_str().unwrap_or("follow").to_owned();
                let network = network()?;
                if self.agent.is_none() {
                    let mut builder = reqwest::Client::builder()
                        .redirect_policy(reqwest::redirect::Policy::none())
                        .connect_timeout(Duration::from_secs(30));
                    if self.renderer {
                        builder = builder
                            .no_proxy()
                            .dns_resolver(std::sync::Arc::new(PublicDns));
                    }
                    self.agent = Some(builder.build()?);
                }
                let agent = self.agent.as_ref().unwrap().clone();
                let renderer = self.renderer;
                let (events_tx, events) = mpsc::channel(64);
                let (demand, mut demand_rx) = mpsc::channel(1);
                let (upload, upload_rx) = mpsc::channel::<Option<bytes::Bytes>>(16);
                let mut upload_rx = (args[0]["stream"] == true).then_some(upload_rx);
                let task = network.spawn(async move {
                    let result: Result<()> = async {
                        if upload_rx.is_some() { for _ in 0..16 { if events_tx.send(json!({"kind":"upload"}).into()).await.is_err() { return Ok(()); } } }
                        let mut redirects = 0;
                        let mut response = loop {
                            if renderer {
                                let ip = match url.host() {
                                    Some(url::Host::Ipv4(ip)) => Some(std::net::IpAddr::V4(ip)),
                                    Some(url::Host::Ipv6(ip)) => Some(std::net::IpAddr::V6(ip)),
                                    _ => None,
                                };
                                ensure!(!ip.is_some_and(private_address), "Pages cannot access private networks");
                            }
                            let mut request = agent.request(reqwest::Method::from_bytes(method.as_bytes())?, url.clone());
                            for (name, value) in &headers { request = request.header(name, value); }
                            let streaming = upload_rx.is_some();
                            let request_body = if let Some(receiver) = upload_rx.take() {
                                let sender = events_tx.clone();
                                reqwest::Body::wrap_stream(futures_util::stream::unfold((receiver, sender), |(mut receiver, sender)| async move {
                                    let chunk = receiver.recv().await.flatten()?;
                                    sender.send(json!({"kind": "upload"}).into()).await.ok()?;
                                    Some((Ok::<_, std::io::Error>(chunk), (receiver, sender)))
                                }))
                            } else { reqwest::Body::from(body.clone()) };
                            let response = request.body(request_body).send().await?;
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
                        let headers: Vec<_> = response.headers().iter().map(|(name, value)| {
                            (name.as_str(), value.as_bytes().iter().map(|&b| char::from(b)).collect::<String>())
                        }).collect();
                        let metadata = json!({"kind": "headers", "status": response.status().as_u16(),
                            "statusText": response.status().canonical_reason().unwrap_or(""), "headers": headers,
                            "url": response.url().to_string(), "redirected": redirects > 0});
                        if events_tx.send(metadata.into()).await.is_err() { return Ok(()); }
                        let (mut buffer, mut wanted, mut ended) = (Vec::new(), false, false);
                        loop {
                            tokio::select! {
                                demand = demand_rx.recv() => { if demand.is_none() { break; } wanted = true; }
                                chunk = response.chunk(), if !ended && buffer.len() < 1 << 20 => match chunk? { Some(bytes) => buffer.extend_from_slice(&bytes), None => ended = true },
                            }
                            if wanted && (ended || !buffer.is_empty()) {
                                wanted = false;
                                let done = buffer.is_empty();
                                let event = if done { json!({"kind": "done"}).into() } else { crate::jsc::payload::Payload::with_bytes(json!({"kind":"chunk"}), "bytes", std::mem::take(&mut buffer)) };
                                if events_tx.send(event).await.is_err() || done { break; }
                            }
                        }
                        Ok(())
                    }.await;
                    if let Err(error) = result { let _ = events_tx.send(json!({"kind": "error", "message": format!("{error:#}")}).into()).await; }
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
            _ => bail!("Unknown web operation: {op}"),
        }
    }
}
