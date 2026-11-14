import { BrowserWindow, ipcMain, shell, type AppModule } from '@pierre/apps'

function createWindow() {
  const window = new BrowserWindow({ width: 1280, height: 820, title: 'DiffsHub' })
  window.loadFile(import.meta.resolve('./index.html'))
}

ipcMain.on('open-external', (_event, url: string) => {
  if (/^https?:\/\//.test(url)) shell.openExternal(url)
})

export default {
  ready: createWindow,
  activate(app) {
    if (!app.hasWindows) createWindow()
  },
  windowClose(app) {
    if (!app.hasWindows && app.platform !== 'darwin') app.quit()
  },
} satisfies AppModule
