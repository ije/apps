import { BrowserWindow, type AppModule } from '@pierre/apps'

export default {
  ready() {
    const window = new BrowserWindow({ width: 800, height: 600, title: 'HTML in Canvas' })
    window.loadFile(import.meta.resolve('./index.html'))
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit()
  },
} satisfies AppModule
