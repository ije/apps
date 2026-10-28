use apps_runtime::{
    host::Role,
    html::Document,
    jsc::{Api, Engine},
};
use std::time::{Duration, Instant};

#[test]
#[ignore = "release performance measurement"]
fn binary_io() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("data.bin");
    let data: Vec<u8> = (0..8 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    std::fs::write(&path, data).unwrap();
    let engine = Engine::new(Api::load().unwrap(), Default::default(), Role::Main).unwrap();
    engine
        .eval(
            &format!("globalThis.path={}", serde_json::json!(path)),
            "bench",
        )
        .unwrap();
    engine
        .eval("__apps.fs.readFileSync(path)", "bench")
        .unwrap();
    let mut reads = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        assert_eq!(
            engine
                .eval("__apps.fs.readFileSync(path).length", "bench")
                .unwrap(),
            "8388608"
        );
        reads.push(start.elapsed().as_secs_f64() * 1000.);
    }
    reads.sort_by(f64::total_cmp);
    println!("readFileSync_8MiB_median_ms={:.3}", reads[4]);
    engine
        .eval("globalThis.data=__apps.fs.readFileSync(path)", "bench")
        .unwrap();
    let mut rounds = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        engine.eval(r#"globalThis.done=false;globalThis.failure='';(async()=>{const compressed=await new Response(new Blob([data]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer();const decoded=new Uint8Array(await new Response(new Blob([compressed]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());if(decoded.length!==data.length||decoded[1000]!==data[1000])throw Error('bad round trip');done=true})().catch(e=>failure=String(e))"#, "bench").unwrap();
        while engine.eval("done||failure", "bench").unwrap() == "" {
            assert!(start.elapsed() < Duration::from_secs(30));
            engine.tick().unwrap();
        }
        assert_eq!(engine.eval("failure", "bench").unwrap(), "");
        rounds.push(start.elapsed().as_secs_f64() * 1000.);
    }
    rounds.sort_by(f64::total_cmp);
    println!("gzip_round_trip_8MiB_median_ms={:.3}", rounds[1]);
}

#[test]
#[ignore = "release performance measurement; the baseline may time out"]
fn descendant_selector() {
    let doc = Document::parse(&format!(
        "{}<span></span>{}",
        "<div class='a'>".repeat(1000),
        "</div>".repeat(1000)
    ));
    let selector = format!(".missing {}span", ".a ".repeat(29));
    let leaf = doc
        .nodes
        .iter()
        .position(|node| node.tag == "span")
        .unwrap();
    let started = Instant::now();
    assert!(!doc.matches(leaf, &selector));
    println!(
        "descendant_30_parts_1000_nodes_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
}
