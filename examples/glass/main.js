import { app, BrowserWindow, ipcMain, screen } from '@pierre/apps';

app.whenReady().then(() => {
  const window = new BrowserWindow({ width: 760, height: 520 });
  const float = new BrowserWindow({
    title: 'Magic Mouse',
    width: 260,
    height: 76,
    frame: false,
    transparent: true,
    backgroundColor: '#00000000',
    show: false,
    opacity: 0,
    alwaysOnTop: true,
    focusable: false,
    visualEffectState: 'active',
    resizable: false,
    movable: false,
    hasShadow: false,
  });
  let enabled = false;
  let ready = false;
  let pending = false;
  let hovered = false;
  let closing = false;
  let remaining = 3000;
  let deadline = 0;
  let dismissal;
  let animation;

  function fadeTo(target) {
    clearInterval(animation);
    const from = float.getOpacity();
    const start = Date.now();
    const duration = 220 * Math.abs(target - from);
    animation = setInterval(() => {
      const progress = duration ? Math.min((Date.now() - start) / duration, 1) : 1;
      const eased = progress * progress * (3 - 2 * progress);
      float.setOpacity(from + (target - from) * eased);
      if (progress === 1) {
        clearInterval(animation);
        if (target === 0) {
          float.hide();
          hovered = false;
        }
      }
    }, 8);
  }

  function dismissLater() {
    clearTimeout(dismissal);
    if (hovered || closing) return;
    deadline = Date.now() + remaining;
    dismissal = setTimeout(() => {
      remaining = 0;
      fadeTo(0);
    }, remaining);
  }

  function showFloat() {
    if (!ready) {
      pending = true;
      return;
    }
    pending = false;
    closing = false;
    remaining = 3000;
    const { x, y, width } = screen.getDisplayMatching(window.getBounds()).workArea;
    const left = Math.round(x + (width - 260) / 2);
    const top = y + 16;
    float.setPosition(left, top);
    const cursor = screen.getCursorScreenPoint();
    hovered = cursor.x >= left && cursor.x < left + 260 && cursor.y >= top && cursor.y < top + 76;
    float.webContents.send('mouse-state', enabled);
    float.showInactive();
    fadeTo(1);
    dismissLater();
  }

  ipcMain.on('mouse-ready', (event) => event.reply('mouse-state', enabled));
  ipcMain.on('mouse-toggle', () => {
    enabled = !enabled;
    window.webContents.send('mouse-state', enabled);
    showFloat();
  });
  ipcMain.on('float-ready', () => {
    ready = true;
    if (pending) showFloat();
  });
  ipcMain.on('float-hover', (_event, value) => {
    if (!float.isVisible() || closing || hovered === value) return;
    hovered = value;
    if (hovered) {
      remaining = Math.max(0, deadline - Date.now());
      clearTimeout(dismissal);
      fadeTo(1);
    } else {
      dismissLater();
    }
  });
  ipcMain.on('float-close', () => {
    closing = true;
    clearTimeout(dismissal);
    fadeTo(0);
  });
  window.on('closed', () => {
    for (const channel of ['mouse-ready', 'mouse-toggle', 'float-ready', 'float-hover', 'float-close']) {
      ipcMain.removeAllListeners(channel);
    }
    clearTimeout(dismissal);
    clearInterval(animation);
    float.destroy();
  });

  window.loadFile(import.meta.resolve('./index.html'));
  float.loadFile(import.meta.resolve('./float.html'));
});

app.on('window-all-closed', () => app.quit());
