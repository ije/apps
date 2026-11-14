import { BrowserWindow, type AppModule } from '@pierre/apps'

export default {
  ready() {
    const window = new BrowserWindow({ width: 1120, height: 760 })
    window.loadFile(import.meta.resolve('./index.html'))
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit()
  },
} satisfies AppModule
