use apps_runtime::{
    host::{Host, Page, Role},
    jsc::{Api, Engine},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

#[test]
fn web_apis_in_every_context() {
    let api = Api::load().unwrap();
    for role in [Role::Main, Role::Preload(1), Role::Renderer(1)] {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: apps_runtime::dom::Document::parse("<html><body></body></html>"),
                dirty: false,
            },
        );
        let engine = Engine::new(api.clone(), host, role).unwrap();
        for script in [
            include_str!("web.js"),
            include_str!("web_standards.js"),
            include_str!("wasm.js"),
        ] {
            engine.eval(script, "web-tests").unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                engine.tick().unwrap();
                let result = engine.eval("testResult", "test-result").unwrap();
                if result != "pending" {
                    assert_eq!(result, "ok", "{role:?}: {result}");
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{role:?}: async test did not settle: {}",
                    engine.eval("testStep", "test-step").unwrap()
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
fn fetch_streams_redirects_and_abort() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let shutdown = stopped.clone();
    let beacons = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let server = std::thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            let (mut socket, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            let beacons = beacons.clone();
            std::thread::spawn(move || {
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let end = loop {
                    let mut byte = [0];
                    if socket.read_exact(&mut byte).is_err() {
                        return;
                    }
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break request.len();
                    }
                };
                let header = String::from_utf8_lossy(&request).to_string();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|n| n.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                request.resize(end + length, 0);
                if socket.read_exact(&mut request[end..]).is_err() {
                    return;
                }
                if header.to_lowercase().contains("transfer-encoding: chunked") {
                    loop {
                        let mut line = Vec::new();
                        while !line.ends_with(b"\r\n") {
                            let mut byte = [0];
                            if socket.read_exact(&mut byte).is_err() {
                                return;
                            }
                            line.push(byte[0]);
                        }
                        let count =
                            usize::from_str_radix(String::from_utf8_lossy(&line).trim(), 16)
                                .unwrap();
                        if count == 0 {
                            if socket.read_exact(&mut [0; 2]).is_err() {
                                return;
                            }
                            break;
                        }
                        let offset = request.len();
                        request.resize(offset + count, 0);
                        if socket.read_exact(&mut request[offset..]).is_err() {
                            return;
                        }
                        if socket.read_exact(&mut [0; 2]).is_err() {
                            return;
                        }
                        if header.split_whitespace().nth(1) == Some("/early") {
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                            return;
                        }
                    }
                }
                let path = header.split_whitespace().nth(1).unwrap();
                match path {
                    "/redirect" => {
                        let _ = socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    "/post-redirect" => {
                        let _ = socket.write_all(b"HTTP/1.1 303 See Other\r\nLocation: /method\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    "/stream" | "/abort" => {
                        if socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\none\r\n").is_err() { return; }
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = socket.write_all(b"3\r\ntwo\r\n0\r\n\r\n");
                    }
                    "/slow" => {
                        std::thread::sleep(Duration::from_millis(200));
                        let _ = socket.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    "/empty" => {
                        let _ = socket
                            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                    }
                    _ => {
                        let body = if path == "/beacon" {
                            beacons.lock().unwrap().push(
                                serde_json::json!({ "header": header, "body": request[end..] }),
                            );
                            Vec::new()
                        } else if path == "/beacons" {
                            serde_json::to_vec(&*beacons.lock().unwrap()).unwrap()
                        } else if matches!(path, "/echo" | "/wasm") {
                            request[end..].to_vec()
                        } else if path == "/method" {
                            header
                                .split_whitespace()
                                .next()
                                .unwrap()
                                .as_bytes()
                                .to_vec()
                        } else {
                            br#"{"answer":42}"#.to_vec()
                        };
                        let status = if path == "/missing" {
                            "404 Not Found"
                        } else {
                            "200 OK"
                        };
                        let content_type = if path == "/wasm" {
                            "application/wasm"
                        } else {
                            "application/json"
                        };
                        let _ = write!(
                            socket,
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        if !header.starts_with("HEAD ") {
                            let _ = socket.write_all(&body);
                        }
                    }
                }
            });
        }
    });
    let engine = Engine::new(
        Api::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    engine
        .eval(
            &format!("globalThis.base = 'http://{address}';"),
            "test-url",
        )
        .unwrap();
    engine
        .eval(include_str!("fetch.js"), "fetch-tests")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = loop {
        engine.tick().unwrap();
        let result = engine.eval("testResult", "test-result").unwrap();
        if result != "pending" {
            break result;
        }
        if Instant::now() > deadline {
            break format!("timeout: {}", engine.eval("testStep", "test-step").unwrap());
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    stopped.store(true, Ordering::Relaxed);
    server.join().unwrap();
    assert_eq!(result, "ok");
}

#[test]
#[ignore = "Manual stream throughput measurement"]
fn stream_throughput() {
    let engine = Engine::new(
        Api::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    engine.eval(r#"
        globalThis.benchmark = 'pending';
        (async () => {
            const results = {};
            for (const [name, count, chunk] of [['values', 100000, 1], ['bytes', 4096, new Uint8Array(65536)]]) {
                let sent = 0, received = 0;
                const start = Date.now();
                await new ReadableStream({ pull(c) { if (sent++ < count) c.enqueue(chunk); else c.close(); } })
                    .pipeTo(new WritableStream({ write() { received++; } }));
                if (received !== count) throw Error('Lost chunks');
                results[name] = { chunks: received, milliseconds: Date.now() - start };
            }
            benchmark = JSON.stringify(results);
        })().catch(error => { benchmark = String(error); });
    "#, "stream-benchmark").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("benchmark", "benchmark-result").unwrap();
        if result != "pending" {
            assert!(result.starts_with('{'), "{result}");
            println!("{result}");
            break;
        }
        assert!(Instant::now() < deadline, "Benchmark did not settle");
    }
}

#[test]
fn runtime_imports_follow_features() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            let (mut request, mut byte) = (Vec::new(), [0]);
            while !request.ends_with(b"\r\n\r\n") && socket.read_exact(&mut byte).is_ok() {
                request.push(byte[0]);
            }
            let body = if request.starts_with(b"GET /a.js") {
                "import b from './b.js';export default b+1"
            } else {
                "export default 41"
            };
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\ncontent-type: text/javascript\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let api = Api::load().unwrap();
    let run = |disabled: u32, role: Role, code: &str| {
        let host = Rc::new(RefCell::new(Host::default()));
        host.borrow_mut().disabled = disabled;
        host.borrow_mut().pages.insert(
            1,
            Page {
                doc: apps_runtime::dom::Document::parse("<html><body></body></html>"),
                dirty: false,
            },
        );
        let engine = Engine::new(api.clone(), host, role).unwrap();
        engine
            .eval(
                &format!("globalThis.result='pending';Promise.resolve().then(()=>{code}).then(v=>result=String(v),e=>result=e.message)"),
                "test",
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            engine.tick().unwrap();
            let result = engine.eval("result", "test").unwrap();
            if result != "pending" {
                return result;
            }
            assert!(Instant::now() < deadline, "{code} did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let defaults = apps_runtime::bundle::disabled_features(None).unwrap();
    let data = "import('data:text/javascript,export default 6*7').then(m=>m.default)";
    let http = format!("import('http://127.0.0.1:{port}/a.js').then(m=>m.default)");
    let strings = "[eval('1'), new Function('return 2')()].join()";
    assert_eq!(run(0, Role::Renderer(1), strings), "1,2");
    assert!(run(defaults, Role::Renderer(1), strings).contains("disabled in pages"));
    assert_eq!(run(0, Role::Renderer(1), data), "42");
    assert!(run(defaults, Role::Renderer(1), data).contains("jsExecute"));
    assert_eq!(run(0, Role::Main, &http), "42");
    assert!(run(defaults, Role::Main, &http).contains("httpImport"));
    assert!(run(0, Role::Main, "import('file:///x.js')").contains("Cannot import"));
}
