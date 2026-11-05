import { contextBridge, ipcRenderer } from '@pierre/apps'
import { GET as diff } from './api/diff'
import { GET as diffFile } from './api/github-diff-file'

const routes: Record<string, (request: Request) => Promise<Response>> = {
  '/api/diff': diff,
  '/api/github-diff-file': diffFile,
}

type Write = (chunk: string | null, error?: string) => Promise<boolean>

// The diffshub.com API routes run here: pages cannot read GitHub responses across origins.
contextBridge.exposeInMainWorld('diffshub', {
  openExternal: (url: string) => ipcRenderer.send('open-external', url),
  async request(path: string, headers: Record<string, string>, write: Write) {
    const url = new URL(path, 'https://diffshub.com')
    const response = await routes[url.pathname](new Request(url, { headers }))
    const reader = response.body!.getReader()
    const decoder = new TextDecoder()
    ;(async () => {
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) return await write(decoder.decode() || null) && write(null)
          if (!(await write(decoder.decode(value, { stream: true })))) return reader.cancel()
        }
      } catch (error) {
        write(null, String(error))
      }
    })()
    return { status: response.status, headers: [...response.headers] }
  },
})
