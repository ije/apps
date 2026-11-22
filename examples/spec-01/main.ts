import { writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'
import { app, BrowserWindow, ipcMain, shell, type AppModule } from '@pierre/apps'

ipcMain.on('save', (_event, name: string, png: string) => {
  writeFile(join(app.getPath('downloads'), basename(name)), Buffer.from(png.slice(png.indexOf(',') + 1), 'base64'))
})

function createWindow() {
  const window = new BrowserWindow({
    width: 1280,
    height: 860,
    minWidth: 420,
    minHeight: 320,
    backgroundColor: '#eeeeec',
    titleBarStyle: app.platform === 'darwin' ? 'hidden' : 'default',
    trafficLightPosition: { x: 34, y: 31 },
  })
  window.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url)
    return { action: 'deny' }
  })
  window.loadFile(import.meta.resolve('./index.html'))
}

export default {
  ready: createWindow,
  activate() {
    if (!app.hasWindows) createWindow()
  },
  windowClose() {
    if (!app.hasWindows && app.platform !== 'darwin') app.quit()
  },
} satisfies AppModule
