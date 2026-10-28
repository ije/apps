import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

if (!['darwin', 'linux'].includes(process.platform)) throw Error('This example needs macOS or Linux');
const root = dirname(fileURLToPath(import.meta.url));
const output = join(root, 'build/Release');
const headers = process.env.NODE_INCLUDE_DIR ?? join(dirname(dirname(process.execPath)), 'include/node');
mkdirSync(output, { recursive: true });
const flags = process.platform === 'darwin' ? ['-bundle', '-undefined', 'dynamic_lookup'] : ['-shared', '-fPIC'];
const result = spawnSync(process.env.CC ?? 'cc', [
  ...flags, '-DNAPI_VERSION=8', '-Wall', '-Wextra', '-Werror',
  '-I', headers, join(root, 'addon.c'), '-o', join(output, 'addon.node'),
], { stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
