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
