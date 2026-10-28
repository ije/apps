import { app, BrowserWindow } from '@pierre/apps'

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 1120, height: 760 })
  window.loadFile(import.meta.resolve('./index.html'))
})

app.on('window-all-closed', () => app.quit())
