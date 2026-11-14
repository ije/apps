import { BrowserWindow, type AppModule } from '@pierre/apps'

function createWindow() {
  const window = new BrowserWindow({ width: 560, height: 320 })
  window.loadFile(import.meta.resolve('./index.html'))
}

export default {
  ready: createWindow,
  activate(app) {
    if (!app.hasWindows) createWindow()
  },
  windowClose(app) {
    if (!app.hasWindows && app.platform !== 'darwin') app.quit()
  },
  fetch() {
    return new Response('hello world')
  },
  port: 8089,
} satisfies AppModule
