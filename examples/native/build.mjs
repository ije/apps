import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

if (!['darwin', 'linux'].includes(process.platform)) throw Error('This example needs macOS or Linux');
const root = dirname(fileURLToPath(import.meta.url));
const output = join(root, 'build/Release');
const headers = process.env.NODE_INCLUDE_DIR ?? join(dirname(dirname(process.execPath)), 'include/node');
const darwin = process.platform === 'darwin';
mkdirSync(output, { recursive: true });
for (const args of [
  [...(darwin ? ['-bundle', '-undefined', 'dynamic_lookup'] : ['-shared', '-fPIC']), '-DNAPI_VERSION=8', '-I', headers, join(root, 'addon.c'), '-o', join(output, 'addon.node')],
  [darwin ? '-dynamiclib' : '-shared', '-fPIC', join(root, 'math.c'), '-o', join(output, `math.${darwin ? 'dylib' : 'so'}`)],
]) {
  const result = spawnSync(process.env.CC ?? 'cc', ['-Wall', '-Wextra', '-Werror', ...args], { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status) process.exit(result.status);
}
