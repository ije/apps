import { spawn } from 'node:child_process';

const child = spawn('/bin/sh', ['-c', 'exit 0']);
const code = await new Promise((resolve, reject) => {
  child.on('error', reject);
  child.on('close', resolve);
});
