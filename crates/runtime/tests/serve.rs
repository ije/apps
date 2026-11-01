use apps_runtime::{
    host::Role,
    jsc::{Api, Engine},
};
use std::{
    net::{TcpListener, TcpStream},
    rc::Rc,
    time::{Duration, Instant},
};

#[test]
fn serve_requests_and_lifecycle() {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine
        .eval(include_str!("serve.js"), "serve-tests")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("testResult", "serve-result").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(Instant::now() < deadline, "Server tests did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn serve_closes_sockets_with_context() {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    let port: u16 = engine.eval("__apps.serve({ hostname: '127.0.0.1', port: 0, fetch: () => new Response('hello') }).port", "serve-test").unwrap().parse().unwrap();
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    drop(engine);
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Context did not release the listener"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn serve_is_main_only() {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Preload(1)).unwrap();
    assert!(
        engine
            .eval(
                "__apps.serve({ port: 0, fetch: () => new Response() })",
                "serve-test"
            )
            .unwrap_err()
            .to_string()
            .contains("only available in the main context")
    );
}

#[test]
fn native_events_wake_the_loop_without_polling() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    apps_runtime::host::Waker::install(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval("globalThis.done='pending';const server=__apps.serve({hostname:'127.0.0.1',port:0,fetch:async request=>new Response(await request.text()+'!')});fetch(`http://127.0.0.1:${server.port}`,{method:'POST',body:'woken'}).then(r=>r.text()).then(text=>{done=text;server.stop()},error=>done=String(error))", "wake").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = wakes.load(Ordering::Relaxed);
        assert_eq!(engine.tick().unwrap(), -1, "the loop must not poll");
        if engine.eval("done", "wake").unwrap() != "pending" {
            break;
        }
        while wakes.load(Ordering::Relaxed) == seen {
            assert!(
                Instant::now() < deadline,
                "a native event did not wake the loop"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(engine.eval("done", "wake").unwrap(), "woken!");
}

#[test]
fn stalled_stream_responses_wait_for_the_client_without_polling() {
    use std::io::{Read, Write};
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
    let port: u16 = engine.eval("const chunk=new Uint8Array(1<<16);globalThis.sent=0;__apps.serve({hostname:'127.0.0.1',port:0,fetch:()=>new Response(new ReadableStream({pull(c){sent++;c.enqueue(chunk);}}))}).port", "stall").unwrap().parse().unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: test\r\n\r\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut last, mut quiet) = (String::new(), 0);
    while quiet < 20 {
        let delay = engine.tick().unwrap();
        let sent = engine.eval("sent", "stall").unwrap();
        quiet = if delay == -1 && sent == last {
            quiet + 1
        } else {
            0
        };
        last = sent;
        assert!(
            Instant::now() < deadline,
            "a stalled response kept the loop polling"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    stream
        .set_read_timeout(Some(Duration::from_millis(1)))
        .unwrap();
    let (mut bytes, mut total) = (vec![0; 1 << 16], 0);
    while engine.eval("sent", "stall").unwrap() == last {
        if let Ok(size) = stream.read(&mut bytes) {
            total += size;
        }
        engine.tick().unwrap();
        assert!(
            Instant::now() < deadline,
            "reading did not resume the response after {total} bytes"
        );
    }
}
