import { BrowserWindow } from '@pierre/apps';

export default {
  ready() {
    const window = new BrowserWindow({ width: 560, height: 440 });
    window.loadFile(import.meta.resolve('./index.html'));
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit();
  },
};
