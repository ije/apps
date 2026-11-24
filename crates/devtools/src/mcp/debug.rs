use super::{source_map::SourceMap, *};

pub(super) fn debugger(args: &Value, reply: &Reply) -> Result<Option<Value>> {
    let action = args["action"].as_str().context("Expected an action")?;
    let paused = STATE.with_borrow(|state| state.paused.as_ref().map(|(ctx, _)| *ctx));
    let text = match action {
        "state" => return state(args).map(Some),
        "resume" | "step_over" | "step_into" | "step_out" => {
            let ctx = paused.context("The app is not paused")?;
            let method = match action {
                "resume" => "Debugger.resume",
                "step_over" => "Debugger.stepOver",
                "step_into" => "Debugger.stepInto",
                _ => "Debugger.stepOut",
            };
            if action != "resume" {
                STATE.with_borrow_mut(|state| state.stepping = Some(reply.clone()));
            }
            request(ctx, method, json!({}))?;
            if action != "resume" {
                return Ok(None);
            }
            "Resumed.".into()
        }
        "pause" => {
            let ctx = target(args)?;
            debug(ctx)?;
            request(ctx, "Debugger.pause", json!({}))?;
            "The debugger pauses at the next statement that runs. Make the app run code, then use action state.".into()
        }
        "exceptions" => {
            let ctx = target(args)?;
            debug(ctx)?;
            let mode = args["state"].as_str().unwrap_or("uncaught");
            request(ctx, "Debugger.setPauseOnExceptions", json!({"state": mode}))?;
            format!("Pause on exceptions: {mode}.")
        }
        "break" => {
            let ctx = target(args)?;
            debug(ctx)?;
            breakpoint(ctx, args)?
        }
        "clear" => {
            let ctx = target(args)?;
            debug(ctx)?;
            let id = args["id"].as_str();
            let ids: Vec<String> = STATE
                .with_borrow_mut(|state| {
                    let session = state.sessions.get_mut(&ctx)?;
                    let (removed, kept) = std::mem::take(&mut session.breakpoints)
                        .into_iter()
                        .partition(|(breakpoint, _)| id.is_none_or(|id| id == breakpoint));
                    session.breakpoints = kept;
                    Some(removed.into_iter().map(|(id, _)| id).collect::<Vec<_>>())
                })
                .unwrap_or_default();
            for id in &ids {
                request(
                    ctx,
                    "Debugger.removeBreakpoint",
                    json!({"breakpointId": id}),
                )?;
            }
            format!("Removed {} breakpoint(s).", ids.len())
        }
        _ => bail!("Unknown debugger action: {action}"),
    };
    Ok(Some(text.into()))
}

fn breakpoint(ctx: usize, args: &Value) -> Result<String> {
    let url = args["url"].as_str().context("Expected a url")?;
    let line = args["line"].as_u64().context("Expected a line")?.max(1) as u32 - 1;
    let column = args["column"].as_u64().unwrap_or(1).max(1) as u32 - 1;
    let options = json!({"condition": args["condition"].as_str().unwrap_or("")});
    let wanted = url
        .strip_prefix("file://")
        .unwrap_or(url)
        .trim_start_matches("./")
        .replace('\\', "/");
    let absolute = std::path::absolute(&wanted)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let matches = |source: &str| {
        let source = source
            .strip_prefix("file://")
            .unwrap_or(source)
            .replace('\\', "/");
        source == absolute || source == wanted || source.ends_with(&format!("/{wanted}"))
    };
    let locations: Vec<(String, u32, u32)> = STATE.with_borrow(|state| {
        let Some(session) = state.sessions.get(&ctx) else {
            return Vec::new();
        };
        session
            .scripts
            .iter()
            .filter_map(|(id, script)| {
                let (line, column) = source_map(script)?.generated(matches, line, column)?;
                Some((id.clone(), line, column))
            })
            .collect()
    });
    let mut placed = Vec::new();
    if locations.is_empty() {
        let pattern: String = wanted
            .chars()
            .flat_map(|c| {
                if c.is_ascii_alphanumeric() || c == '/' {
                    vec![c]
                } else {
                    vec!['\\', c]
                }
            })
            .collect();
        let response = request(
            ctx,
            "Debugger.setBreakpointByUrl",
            json!({"lineNumber": line, "columnNumber": column, "urlRegex": format!("{pattern}$"), "options": options}),
        )?;
        let id = response["breakpointId"].as_str().unwrap_or("").to_owned();
        let found = response["locations"].as_array().map_or(0, Vec::len);
        placed.push((
            id,
            format!(
                "{url}:{} ({found} loaded script(s) match; scripts loaded later match by URL)",
                line + 1
            ),
        ));
    }
    for (script, generated, generated_column) in locations {
        let response = request(
            ctx,
            "Debugger.setBreakpoint",
            json!({"location": {"scriptId": script, "lineNumber": generated, "columnNumber": generated_column}, "options": options}),
        )?;
        let at = STATE.with_borrow(|state| location(state, &response["actualLocation"]));
        placed.push((
            response["breakpointId"].as_str().unwrap_or("").to_owned(),
            at,
        ));
    }
    let text = placed
        .iter()
        .map(|(id, at)| format!("Breakpoint {id} at {at}"))
        .collect::<Vec<_>>()
        .join("\n");
    STATE.with_borrow_mut(|state| {
        if let Some(session) = state.sessions.get_mut(&ctx) {
            session.breakpoints.extend(placed);
        }
    });
    Ok(text)
}

pub(super) fn state(args: &Value) -> Result<Value> {
    pump();
    let Some((ctx, paused)) = STATE.with_borrow(|state| state.paused.clone()) else {
        let breakpoints = STATE.with_borrow(|state| {
            state
                .sessions
                .values()
                .flat_map(|session| &session.breakpoints)
                .map(|(id, at)| format!("\n  {id} at {at}"))
                .collect::<String>()
        });
        return Ok(format!(
            "Running. Breakpoints:{}",
            if breakpoints.is_empty() {
                " none"
            } else {
                &breakpoints
            }
        )
        .into());
    };
    let frames = paused["callFrames"].as_array().cloned().unwrap_or_default();
    let mut text = format!("Paused ({})", paused["reason"].as_str().unwrap_or("pause"));
    if let Some(description) = paused["data"]["description"].as_str() {
        text += &format!(": {description}");
    }
    STATE.with_borrow(|state| {
        for (index, frame) in frames.iter().enumerate().take(30) {
            text += &format!(
                "\n#{index} {}  {}",
                name(frame),
                location(state, &frame["location"])
            );
        }
    });
    let frame = frames
        .get(args["frame"].as_u64().unwrap_or(0) as usize)
        .context("No such call frame")?;
    for scope in frame["scopeChain"].as_array().into_iter().flatten() {
        let kind = scope["type"].as_str().unwrap_or("scope");
        let Some(id) = scope["object"]["objectId"]
            .as_str()
            .filter(|_| !matches!(kind, "global" | "globalLexicalEnvironment"))
        else {
            continue;
        };
        let properties = request(
            ctx,
            "Runtime.getProperties",
            json!({"objectId": id, "ownProperties": true, "generatePreview": true}),
        )?;
        let variables: String = properties["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|property| {
                property["name"]
                    .as_str()
                    .is_some_and(|name| !name.starts_with("$apps") && name != "$out")
            })
            .take(50)
            .map(|property| {
                format!(
                    "\n  {} = {}",
                    property["name"].as_str().unwrap_or("?"),
                    short(&property["value"])
                )
            })
            .collect();
        if !variables.is_empty() {
            text += &format!(
                "\n{kind}{}:{variables}",
                scope["name"]
                    .as_str()
                    .map(|name| format!(" {name}"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(text.into())
}

pub(super) fn name(frame: &Value) -> &str {
    frame["functionName"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or("(anonymous)")
}

fn short(object: &Value) -> String {
    let text = match (object["type"].as_str(), object.get("value")) {
        (Some("undefined"), _) => "undefined".into(),
        (_, Some(value)) => value.to_string(),
        _ => match object["preview"]["properties"].as_array() {
            Some(properties) => {
                let array = object["subtype"] == "array";
                let items = properties.iter().map(|property| {
                    let value = property["value"].as_str().unwrap_or("");
                    let value = if property["type"] == "string" {
                        json!(value).to_string()
                    } else {
                        value.to_owned()
                    };
                    if array {
                        value
                    } else {
                        format!("{}: {value}", property["name"].as_str().unwrap_or(""))
                    }
                });
                let mut items: Vec<_> = items.collect();
                if object["preview"]["overflow"] == true {
                    items.push("…".into());
                }
                let (open, close) = if array { ("[", "]") } else { ("{", "}") };
                let class = object["className"]
                    .as_str()
                    .filter(|class| *class != "Object" && !array);
                format!(
                    "{}{open}{}{close}",
                    class.map(|class| format!("{class} ")).unwrap_or_default(),
                    items.join(", ")
                )
            }
            None => object["description"]
                .as_str()
                .unwrap_or("?")
                .lines()
                .next()
                .unwrap_or("")
                .into(),
        },
    };
    if text.len() > 300 {
        format!("{}…", &text[..text.floor_char_boundary(300)])
    } else {
        text
    }
}

pub(super) fn location(state: &State, location: &Value) -> String {
    let id = location["scriptId"].as_str().unwrap_or("");
    let line = location["lineNumber"].as_i64().unwrap_or(0).max(0) as u32;
    let column = location["columnNumber"].as_i64().unwrap_or(0).max(0) as u32;
    let Some(script) = state
        .sessions
        .values()
        .find_map(|session| session.scripts.get(id))
    else {
        return if id == "4294967295" {
            "[native code]".into()
        } else {
            format!("script {id}:{}:{}", line + 1, column + 1)
        };
    };
    if let Some(map) = source_map(script)
        && let Some((source, line, column)) = map.original(line, column)
    {
        return format!(
            "{}:{}:{}",
            source.strip_prefix("file://").unwrap_or(source),
            line + 1,
            column + 1
        );
    }
    format!("{}:{}:{}", script.url, line + 1, column + 1)
}

pub(super) fn source_map(script: &Script) -> Option<Rc<SourceMap>> {
    script
        .map
        .get_or_init(|| SourceMap::parse(&script.raw.take()).map(Rc::new))
        .clone()
}
