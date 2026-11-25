//! The operations of terminal sessions for page and preload scripts.

use crate::render::media::terminal::{KILL, Terminal};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

const SIGNALS: [(&str, i32); 5] = [
    ("SIGHUP", 1),
    ("SIGINT", 2),
    ("SIGQUIT", 3),
    ("SIGKILL", 9),
    ("SIGTERM", 15),
];

/// Runs an operation of a terminal session for the page, or for its preload script.
pub(crate) fn call(
    store: &mut crate::render::Store,
    id: u64,
    preload: bool,
    op: &str,
    args: &Value,
) -> Result<Value> {
    ensure!(
        store.app.features.has(crate::bundle::Feature::Terminal),
        "The terminal feature is disabled. Add \"terminal\" to \"features\" in package.json"
    );
    // Only a preload script starts programs, and only the page has elements.
    ensure!(
        if preload {
            op != "attach"
        } else {
            !matches!(op, "create" | "spawn")
        },
        "terminal.{op} is unavailable in this context"
    );
    let page = store.pages.get_mut(&id).context("Window is closed")?;
    let state = &mut page.state;
    let terminals = state.media.terminals.clone();
    let mut terminals = terminals.borrow_mut();
    let terminals = &mut *terminals;
    let world = usize::from(preload);
    if op == "poll" {
        let (changed, events) = terminals.poll(page.doc.nodes.latest(), world, |node| {
            state.tree.reaches(&page.doc, node, 0)
        })?;
        page.dirty |= changed;
        return Ok(json!(events));
    }
    let number = |index: usize| args[index].as_f64().unwrap_or(0.);
    // A string is a command line for the login shell of the user.
    let program = || -> Result<_> {
        let argv = match &args[1] {
            Value::String(command) => vec![command.as_str()],
            list => list
                .as_array()
                .into_iter()
                .flatten()
                .map(|arg| arg.as_str().context("Expected argument strings"))
                .collect::<Result<Vec<_>>>()?,
        };
        let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
        let env = args[2]["env"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| {
                let value = value.as_str().context("Expected environment strings")?;
                Ok(format!("{key}={value}"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((
            argv,
            args[1].is_string(),
            env,
            args[2]["cwd"].as_str().unwrap_or(""),
        ))
    };
    match op {
        "create" => {
            let mut terminal = Terminal::new()?;
            if args[0] != true {
                let (argv, shell, env, cwd) = program()?;
                terminal.spawn(&argv, shell, &env, cwd)?;
            }
            return Ok(json!(terminals.create(terminal)));
        }
        "attach" => {
            let node = args[0].as_u64().context("Invalid node")? as usize;
            ensure!(
                page.doc
                    .nodes
                    .get(node)
                    .is_some_and(|node| node.tag == "terminal"),
                "Expected a terminal element"
            );
            terminals.attach(node, args[1].as_u64().map(|session| session as u32))?;
            page.dirty = true;
            return Ok(Value::Null);
        }
        _ => {}
    }
    let session = args[0].as_u64().unwrap_or(0) as u32;
    if op == "close" {
        terminals.close(session);
        page.dirty = true;
        return Ok(Value::Null);
    }
    let terminal = terminals.get(session);
    if op == "open" {
        return Ok(json!(
            terminal.map(|terminal| terminal.open_in(world)).is_some()
        ));
    }
    let terminal = terminal.context("The terminal session is closed")?;
    // Reads draw no frame, and neither does a pointer that only moves above the terminal.
    page.dirty |=
        !matches!(op, "info" | "text") && !(op == "mouse" && args[1] == 2 && args[6] != true);
    match op {
        "spawn" => {
            let (argv, shell, env, cwd) = program()?;
            return Ok(json!(terminal.spawn(&argv, shell, &env, cwd)?));
        }
        "write" => terminal.write(number(1) as u32, crate::node::bytes(&args[2])?),
        "key" => {
            let name = args[2].as_str().unwrap_or("");
            let text = args[3].as_str().unwrap_or("");
            return Ok(json!(terminal.key(
                number(1) as u32,
                name,
                number(4) as u32,
                text
            )));
        }
        "mouse" => {
            return Ok(json!(terminal.mouse(
                number(1) as u32,
                number(2) as u32,
                number(3) as u32,
                number(4),
                number(5),
                args[6].as_bool().unwrap_or(false),
            )));
        }
        "control" => {
            let op = number(1) as u32;
            let value = match args[2].as_str() {
                Some(signal) if op == KILL => {
                    SIGNALS
                        .iter()
                        .find(|(name, _)| *name == signal)
                        .with_context(|| format!("Unsupported signal {signal}"))?
                        .1 as f64
                }
                _ => number(2),
            };
            terminal.control(op, value);
        }
        "info" => return terminal.info(args[1] == true),
        "text" => return Ok(json!(terminal.text(args[1].as_bool().unwrap_or(false)))),
        "preedit" => terminal.set_preedit(args[1].as_str().unwrap_or("")),
        _ => bail!("Unknown terminal operation: {op}"),
    }
    Ok(Value::Null)
}
