import { app, BrowserWindow } from '@pierre/apps'

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 800, height: 600, title: 'Three.js · WebGPU' })
  window.loadFile(import.meta.resolve('./index.html'))
})
app.on('window-all-closed', () => app.quit())
