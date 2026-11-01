import { app, BrowserWindow } from '@pierre/apps'

function createWindow() {
  const window = new BrowserWindow({ width: 860, height: 640 })
  window.loadFile(import.meta.resolve('./index.html'))
}

app.whenReady().then(() => {
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})
