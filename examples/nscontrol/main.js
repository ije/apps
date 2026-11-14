import { BrowserWindow } from '@pierre/apps';

export default {
  ready() {
    const window = new BrowserWindow({ width: 560, height: 600, title: 'NSControl' });
    window.loadFile('index.html');
  },
  windowClose(app) {
    if (!app.hasWindows) app.quit();
  },
};
