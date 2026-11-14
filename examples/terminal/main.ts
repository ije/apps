import { app, BrowserWindow } from '@pierre/apps'

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 760, height: 520, title: 'Terminal' })
  window.loadFile(import.meta.resolve('./index.html'))
})

app.on('window-all-closed', () => app.quit())
