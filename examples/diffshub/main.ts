import { app, BrowserWindow, ipcMain, shell } from '@pierre/apps'

function createWindow() {
  const window = new BrowserWindow({ width: 1280, height: 820, title: 'DiffsHub' })
  window.loadFile(import.meta.resolve('./index.html'))
}

ipcMain.on('open-external', (_event, url: string) => {
  if (/^https?:\/\//.test(url)) shell.openExternal(url)
})

app.whenReady().then(() => {
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})
