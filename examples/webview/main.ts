import { BrowserWindow, WebContentsView, type AppModule } from '@pierre/apps'

export default {
  async ready() {
    const window = new BrowserWindow({ width: 1000, height: 720, title: 'WebKit browser' })
    window.loadFile(import.meta.resolve('./index.html'))

    const view = new WebContentsView({ webPreferences: { devTools: true } })
    window.contentView.addChildView(view)
    view.setBounds({ x: 16, y: 90, width: 968, height: 590 })
    view.webContents.on('did-finish-load', () => console.log('Loaded:', view.webContents.getURL()))
    window.on('closed', () => view.webContents.close())
    await view.webContents.loadURL('https://webkit.org')
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit()
  },
} satisfies AppModule
