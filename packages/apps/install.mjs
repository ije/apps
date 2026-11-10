// Links bin/apps.mjs to the native CLI, so `apps` runs without a Node process.
import { realpathSync, renameSync, symlinkSync } from 'node:fs'
import { dirname, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

if (process.platform !== 'win32') {
  try {
    const binary = realpathSync((await import(`@pierre/apps-${process.platform}-${process.arch}`)).default)
    const shim = fileURLToPath(new URL('bin/apps.mjs', import.meta.url))
    symlinkSync(relative(dirname(shim), binary), shim + '.tmp')
    renameSync(shim + '.tmp', shim)
  } catch {}
}
