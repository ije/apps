import { fileURLToPath } from 'node:url'

export default fileURLToPath(new URL('bin/apps.exe', import.meta.url))
