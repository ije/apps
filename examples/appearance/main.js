import { BrowserWindow } from '@pierre/apps';

export default {
  ready() {
    const window = new BrowserWindow({ width: 640, height: 640, title: 'Appearance' });
    window.loadFile('index.html');
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit();
  },
};
