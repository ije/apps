import { BrowserWindow, type AppModule } from '@pierre/apps'

export default {
  ready() {
    const window = new BrowserWindow({ width: 760, height: 520, title: 'Terminal' })
    window.loadFile(import.meta.resolve('./index.html'))
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit()
  },
} satisfies AppModule
