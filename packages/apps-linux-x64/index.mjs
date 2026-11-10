import { fileURLToPath } from 'node:url'

export default fileURLToPath(new URL('bin/apps', import.meta.url))
