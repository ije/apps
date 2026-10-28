import { app, BrowserWindow } from '@pierre/apps'

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 900, height: 720, title: 'Native editor' })
  window.loadFile(import.meta.resolve('./index.html'))
})
app.on('window-all-closed', () => app.quit())
