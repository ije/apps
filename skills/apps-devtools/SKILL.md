---
name: apps-devtools
description: Debug a running @pierre/apps app over local HTTP with curl. Screenshots, DOM, JavaScript evaluation, console, mouse and keyboard input, memory, CPU and frame profiles, and a breakpoint debugger. Use it when you run, test, or debug an app with `apps run`.
---

# apps devtools

`apps run` can serve devtools on a local port. Call the tools with `curl`. No MCP setup is necessary.

## Start the app

Start the app as a background command. Port `0` picks a free port, so each session has its own app and port:

```sh
apps run --enable-devtools-mcp --devtools-mcp-port 0
```

Read the port from the output line `apps: devtools MCP server at http://127.0.0.1:<port>/mcp`.
Add `--watch` to restart on file changes. The port stays the same after a restart.
Stop the app when you are done.

## Call a tool

Send the JSON arguments as the POST body. The answer is text, JSON, or PNG. An error answers HTTP 400 with a message.

```sh
D=http://127.0.0.1:<port>
curl -s $D/                         # all tools and their arguments
curl -s $D/windows -d '{}'
curl -s $D/evaluate -d '{"expression":"document.title","window":1}'
curl -s $D/screenshot -d '{"window":1}' -o /tmp/shot.png    # then read the PNG
```

Omit `window` to use the main process. Window ids come from `windows`. Lines and columns are 1-based and refer to your source files.

| Tool | Use |
|---|---|
| `windows` | Open windows: id, title, URL, bounds |
| `evaluate` | Run JavaScript in the main process or a page. Promises are awaited |
| `screenshot` | PNG of a window, or of one element with `selector` |
| `dom` | HTML of the page or of an element, its box, and computed `styles` |
| `input` | Real input: `click`, `move`, `press`, `pressed_move` and `release` for a paced drag, `drag` (from `x`, `y` to `to_x`, `to_y`), `drop` (`paths` dragged in from another app, with `hold` or `leave`), `scroll` (trackpad pixels, or mouse wheel lines with `"lines":true`, and a gesture `"phase"`: `began`, `changed`, `ended`, `momentum`, `momentum_ended`), `key` (`enter`, `cmd-a`), `type` |
| `console` | Console messages and uncaught errors since the last call |
| `memory` | Process memory, JavaScript heap, DOM nodes. `gc`, `snapshot` |
| `profile` | JavaScript samples, frame times, CPU. `{"duration":3000}`, or `action` `start` and `stop` |
| `debugger` | Breakpoints, pause, step, call stack and variables |

## Debug with breakpoints

1. Set a breakpoint: `curl -s $D/debugger -d '{"action":"break","url":"src/app.ts","line":12}'`. Add `"window":1` for page code.
2. Make the code run, for example `curl -s $D/input -d '{"action":"click","selector":"#save","window":1}'`. A call that hits the breakpoint answers `The app paused in the debugger at ...`.
3. Read the call stack and variables: `curl -s $D/debugger -d '{"action":"state"}'`. `evaluate` runs in the paused frame.
4. Continue with `step_over`, `step_into`, `step_out` or `resume`.

While the app is paused, its windows do not draw, and `windows`, `screenshot`, `input` and `memory` fail. Resume before you use them. Remove breakpoints with `{"action":"clear"}`.
