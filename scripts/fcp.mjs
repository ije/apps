#!/usr/bin/env node
// Measures first contentful paint: the time from process start to the first frame with text or images.
//
//   apps build examples/react
//   node scripts/fcp.mjs examples/react/dist/linux-arm64/react 10
//
// Each run starts the app with APPS_TRACE_FCP=1, waits for the runtime's line and quits the app.
// The first run fills the file cache, so it is printed but not in the median.
import { spawn } from 'node:child_process'

const [exe, count = '10'] = process.argv.slice(2)
if (!exe) {
  console.error('Usage: node scripts/fcp.mjs <app executable> [runs]')
  process.exit(1)
}

const run = () => new Promise((resolve, reject) => {
  const start = performance.timeOrigin + performance.now()
  const child = spawn(exe, [], { env: { ...process.env, APPS_TRACE_FCP: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
  const timer = setTimeout(() => child.kill('SIGKILL'), 10000)
  let log = '', match
  child.stderr.on('data', data => {
    log += data
    match ??= /first contentful paint at ([\d.]+)/.exec(log)
    if (match) child.kill()
  })
  child.once('exit', () => {
    clearTimeout(timer)
    if (match) resolve(Number(match[1]) - start)
    else reject(new Error(`No first contentful paint:\n${log}`))
  })
})

const times = []
for (let index = 0; index <= Number(count); index++) {
  const time = await run()
  console.log(`run ${index}${index ? '' : ' (cold)'}: ${time.toFixed(1)} ms`)
  if (index) times.push(time)
}
times.sort((a, b) => a - b)
console.log(`median ${times[times.length >> 1].toFixed(1)} ms, min ${times[0].toFixed(1)} ms, max ${times.at(-1).toFixed(1)} ms`)
