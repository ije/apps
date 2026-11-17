#!/usr/bin/env node
import { spawnSync } from 'node:child_process'

const name = `@pierre/apps-${process.platform}-${process.arch}`
let binary
try {
  binary = (await import(name)).default
} catch {
  const musl = process.platform === 'linux' && !process.report.getReport().header.glibcVersionRuntime
  console.error(
    musl
      ? 'apps: Linux with musl libc is not supported. Use a glibc Linux.'
      : `apps: ${name} is missing. Reinstall @pierre/apps with optional dependencies.`,
  )
  process.exit(1)
}
// apps stops the app on Ctrl+C, so Node must outlive it.
process.on('SIGINT', () => {})
const { status, signal, error } = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' })
if (error) throw error
if (signal) {
  process.removeAllListeners('SIGINT')
  process.kill(process.pid, signal)
}
process.exit(status ?? 1)
