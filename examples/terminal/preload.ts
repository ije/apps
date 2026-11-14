import { terminal } from '@pierre/apps'
import { homedir } from 'node:os'

// Only a preload script can start programs. Without a command, a session runs the login shell.
export const createTerminalSession = (cwd = homedir()) => new terminal.Session({ cwd })
