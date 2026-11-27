use super::*;

fn cpu_time() -> f64 {
  let usage = desktop_now(std::ptr::null_mut(), "cpu", &Value::Null).unwrap_or_default();
  (usage["user"].as_f64().unwrap_or(0.) + usage["system"].as_f64().unwrap_or(0.)) / 1000.
}

pub(super) fn profile(args: &Value) -> Result<Answer> {
  if args["action"].is_null()
    && let Some(duration) = args["duration"].as_u64()
  {
    profile(&json!({"action": "start"}))?;
    std::thread::sleep(Duration::from_millis(duration.min(60_000)));
    return profile(&json!({"action": "stop"}));
  }
  let action = args["action"].as_str();
  ensure!(
    matches!(action, Some("start" | "stop")),
    "Pass action start or stop, or a duration"
  );
  let mut running = lock(&PROFILE);
  if action == Some("start") {
    ensure!(running.is_none(), "A profile is already running");
    let mut keys: Vec<_> = lock(&PAGES).keys().map(|key| Some(*key)).collect();
    if !ONE_THREAD.load(Ordering::Acquire) {
      keys.insert(0, None);
    }
    let (cpu, mut threads) = (cpu_time(), Vec::new());
    for key in keys {
      if dispatch("profile", json!({"action": "start"}), key).is_ok() {
        threads.push(key);
      }
    }
    *running = Some((Instant::now(), cpu, threads));
    return Ok(Value::from("Profiling. Call profile with action stop to get the report.").into());
  }
  let (started, cpu, threads) = running.take().context("No profile is running")?;
  drop(running);
  let elapsed = started.elapsed().as_secs_f64() * 1000.;
  let (mut frames, mut functions, mut samples) = (Vec::new(), HashMap::new(), 0);
  let mut text = String::new();
  for key in threads {
    let report = match dispatch("profile", json!({"action": "stop"}), key) {
      Ok(report) => report.value,
      Err(error) => {
        if let Some(key) = key.filter(|key| *key != 0) {
          text += &format!("Window {key}: {error:#}\n");
        }
        continue;
      }
    };
    samples += report["samples"].as_u64().unwrap_or(0);
    for frame in report["frames"].as_array().into_iter().flatten() {
      frames.push((
        frame[0].as_u64().unwrap_or(0),
        frame[1].as_f64().unwrap_or(0.),
      ));
    }
    for function in report["functions"].as_array().into_iter().flatten() {
      let entry: &mut (u64, u64) = functions
        .entry(function[0].as_str().unwrap_or("").to_owned())
        .or_default();
      entry.0 += function[1].as_u64().unwrap_or(0);
      entry.1 += function[2].as_u64().unwrap_or(0);
    }
  }
  let cpu = cpu_time() - cpu;
  text.insert_str(
    0,
    &format!(
      "Profiled {elapsed:.0} ms. Process CPU {cpu:.0} ms ({:.1}% of one core).\n",
      cpu / elapsed * 100.
    ),
  );
  if frames.is_empty() {
    text += "No window drew a frame.\n";
  }
  frames.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
  for times in frames.chunk_by(|a, b| a.0 == b.0) {
    text += &format!(
      "Window {}: {} frames ({:.1} fps). Frame CPU time from render to paint: avg {:.2} ms, p95 {:.2} ms, max {:.2} ms.\n",
      times[0].0,
      times.len(),
      times.len() as f64 / elapsed * 1000.,
      times.iter().map(|(_, time)| time).sum::<f64>() / times.len() as f64,
      times[(times.len() * 95 / 100).min(times.len() - 1)].1,
      times[times.len() - 1].1,
    );
  }
  if samples == 0 {
    text += "No JavaScript ran.";
    return Ok(Value::from(text).into());
  }
  let mut functions: Vec<_> = functions.into_iter().collect();
  functions.sort_unstable_by_key(|(_, counts)| std::cmp::Reverse(*counts));
  text += &format!(
    "JavaScript: {samples} samples, about 1 ms each.\n   self   total  context  function  location\n"
  );
  for (name, (own, total)) in functions.into_iter().take(30) {
    text += &format!("{own:7} {total:7}  {name}\n");
  }
  Ok(Value::from(text).into())
}

pub(super) fn profile_thread(args: &Value) -> Result<Value> {
  if args["action"] == "start" {
    if STATE.with_borrow(|state| state.profile.is_some()) {
      return Ok(Value::Null);
    }
    let contexts = contexts();
    let one = ONE_THREAD.load(Ordering::Acquire);
    // Sampling covers a whole VM: main has one, and the pages of a thread share one.
    let mut tracked: Vec<(usize, String)> = Vec::new();
    for (ctx, role) in &contexts {
      let label = match role {
        Role::Main => "main".into(),
        _ if one => "pages".into(),
        Role::Preload(id) | Role::Renderer(id) => format!("window {id}"),
      };
      if !tracked.iter().any(|(_, found)| *found == label) {
        tracked.push((*ctx, label));
      }
    }
    let attached = contexts
      .iter()
      .map(|(ctx, _)| *ctx)
      .filter(|ctx| STATE.with_borrow(|state| !state.sessions.contains_key(ctx)))
      .collect();
    for (ctx, _) in &contexts {
      attach(*ctx)?;
    }
    for (ctx, _) in &tracked {
      request(
        *ctx,
        "ScriptProfiler.startTracking",
        json!({"includeSamples": true}),
      )?;
    }
    STATE.with_borrow_mut(|state| {
      state.samples.clear();
      state.profile = Some(Profile { tracked, attached });
    });
    FRAMES.set(Some(Vec::new()));
    return Ok(Value::Null);
  }
  let profile = STATE
    .with_borrow_mut(|state| state.profile.take())
    .context("No profile is running")?;
  let frames: Vec<_> = FRAMES
    .take()
    .unwrap_or_default()
    .into_iter()
    .map(|(window, time)| json!([window, time.as_secs_f64() * 1000.]))
    .collect();
  for (ctx, _) in &profile.tracked {
    let _ = request(*ctx, "ScriptProfiler.stopTracking", json!({}));
  }
  let traces = STATE.with_borrow_mut(|state| std::mem::take(&mut state.samples));
  let mut functions: HashMap<String, (u64, u64)> = HashMap::new();
  let mut samples = 0;
  STATE.with_borrow(|state| {
    for (ctx, data) in &traces {
      let label = profile
        .tracked
        .iter()
        .find_map(|(found, label)| (found == ctx).then_some(label.as_str()))
        .unwrap_or("");
      for trace in data["stackTraces"].as_array().into_iter().flatten() {
        samples += 1;
        let mut seen = Vec::new();
        for (index, frame) in trace["stackFrames"]
          .as_array()
          .into_iter()
          .flatten()
          .enumerate()
        {
          let key = format!(
            "{label}  {}  {}",
            frame["name"]
              .as_str()
              .filter(|name| !name.is_empty())
              .unwrap_or("(anonymous)"),
            location(
              state,
              &json!({
                  "scriptId": frame["sourceID"],
                  "lineNumber": frame["line"].as_i64().unwrap_or(0) - 1,
                  "columnNumber": frame["column"].as_i64().unwrap_or(0) - 1,
              })
            )
          );
          if seen.contains(&key) {
            continue;
          }
          let entry = functions.entry(key.clone()).or_default();
          entry.0 += u64::from(index == 0);
          entry.1 += 1;
          seen.push(key);
        }
      }
    }
  });
  // Only the debugger tool keeps the debugger on.
  for ctx in profile.attached {
    if STATE.with_borrow(|state| {
      state
        .sessions
        .get(&ctx)
        .is_some_and(|session| !session.debugging)
    }) {
      let _ = request(ctx, "Debugger.disable", json!({}));
      STATE.with_borrow_mut(|state| state.sessions.remove(&ctx));
    }
  }
  Ok(json!({
      "frames": frames,
      "samples": samples,
      "functions": functions.into_iter().map(|(name, (own, total))| json!([name, own, total])).collect::<Vec<_>>(),
  }))
}
