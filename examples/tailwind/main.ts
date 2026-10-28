import { app, BrowserWindow } from '@pierre/apps'

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 860, height: 640 })
  window.loadFile(import.meta.resolve('./index.html'))
})

app.on('window-all-closed', () => app.quit())
