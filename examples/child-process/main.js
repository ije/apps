import { spawn } from 'node:child_process';
import { once } from 'node:events';
const ls = spawn('ls', ['-lh', '/usr']);

ls.stdout.on('data', (data) => {
  console.log(`stdout: ${data}`);
});

ls.stderr.on('data', (data) => {
  console.error(`stderr: ${data}`);
});

const [code] = await once(ls, 'close');
console.log(`child process exited with code ${code}`);
