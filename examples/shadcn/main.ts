import { BrowserWindow, type AppModule } from '@pierre/apps'

function createWindow() {
  const window = new BrowserWindow({ width: 960, height: 760 })
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
} satisfies AppModule
