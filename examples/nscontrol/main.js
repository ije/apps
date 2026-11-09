import { app, BrowserWindow } from '@pierre/apps';

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 560, height: 600, title: 'NSControl' });
  window.loadFile('index.html');
});

app.on('window-all-closed', () => app.quit());
