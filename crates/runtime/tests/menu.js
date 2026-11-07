(() => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  for (const packaged of [false, true]) {
    for (const mode of ['default', 'custom', 'removed']) {
      const calls = [];
      let events = [];
      const shell = __appsNativeShell((...args) => { calls.push(args); return args[0] === 'native.poll' ? events.splice(0) : null; }, __apps.events.EventEmitter);
      const app = new __apps.events.EventEmitter();
      Object.assign(app, { getName: () => 'Menu Test', isReady: () => false, isPackaged: packaged });
      shell.bindApp(app, 'menu-test');
      const custom = shell.Menu.buildFromTemplate([{ label: 'Custom', submenu: [{ label: 'Copy', role: 'copy' }] }]);
      if (mode !== 'default') shell.Menu.setApplicationMenu(mode === 'custom' ? custom : null);
      assert(calls.length === 0, 'Menus set before startup must wait for the native app');
      __appsInitMenu();
      assert(calls.length === 1 && calls[0][0] === 'native.menu.set', 'Install the menu once at startup');
      const menu = shell.Menu.getApplicationMenu();
      if (mode === 'removed') assert(menu === null && calls[0][1] === null, 'Respect explicit menu removal');
      else if (mode === 'custom') assert(menu === custom && JSON.stringify(calls[0][1]) === JSON.stringify([{ label: 'Custom', type: 'submenu', enabled: true, visible: true, checked: false, click: calls[0][1][0].click, submenu: [{ label: 'Copy', type: 'normal', role: 'copy', accelerator: 'CommandOrControl+C', enabled: true, visible: true, checked: false, click: calls[0][1][0].submenu[0].click }] }]), 'Preserve a menu set before startup');
      else {
        assert(menu.items.map(item => item.label).join(',') === 'Menu Test,File,Edit,View,Window,Help', 'Default menu sections');
        const quit = menu.items[0].submenu.items.find(item => item.role === 'quit');
        assert(quit.label === 'Quit Menu Test' && quit.accelerator === 'Command+Q', 'Use the app name and standard quit shortcut');
        assert(menu.items[3].submenu.items.some(item => item.role === 'toggleDevTools') === !packaged, 'Developer Tools are only available in development');
        if (!packaged) globalThis.menuTemplate = calls[0][1];
      }
      __appsInitMenu();
      shell.poll();
      assert(calls.length === 1 && !shell.pending(), 'Native menu roles do not need polling or repeated setup');
      shell.Menu.setApplicationMenu(custom);
      assert(calls.length === 2 && calls[1][1][0].label === 'Custom', 'Replace the menu after startup');
      shell.Menu.setApplicationMenu(null);
      assert(calls.length === 3 && calls[2][1] === null && shell.Menu.getApplicationMenu() === null, 'Remove the menu after startup');
      let clicked = false;
      shell.Menu.setApplicationMenu(shell.Menu.buildFromTemplate([{ label: 'Run', click: () => { clicked = true; } }]));
      assert(shell.pending(), 'Custom menu callbacks keep polling active');
      events.push({ type: 'menu', id: calls.at(-1)[1][0].click });
      shell.poll();
      assert(clicked, 'Custom menu callbacks still run');
    }
  }
  const calls = [], events = [];
  const shell = __appsNativeShell((...args) => { calls.push(args); return args[0] === 'native.poll' ? events.splice(0) : null; }, __apps.events.EventEmitter);
  const app = Object.assign(new __apps.events.EventEmitter(), { getName: () => 'Roles', isReady: () => true, isPackaged: false });
  const reloads = [], window = { webContents: { reload: () => reloads.push('reload') } };
  shell.bindApp(app, 'roles', () => window);
  const { Menu, MenuItem } = shell;
  const roles = ['undo', 'redo', 'cut', 'copy', 'paste', 'pasteAndMatchStyle', 'delete', 'selectAll', 'reload', 'forceReload', 'toggleDevTools', 'resetZoom', 'zoomIn', 'zoomOut', 'toggleSpellChecker', 'togglefullscreen', 'window', 'minimize', 'close', 'help', 'about', 'services', 'hide', 'hideOthers', 'unhide', 'quit', 'showSubstitutions', 'toggleSmartQuotes', 'toggleSmartDashes', 'toggleTextReplacement', 'startSpeaking', 'stopSpeaking', 'zoom', 'front', 'appMenu', 'fileMenu', 'editMenu', 'viewMenu', 'shareMenu', 'recentDocuments', 'toggleTabBar', 'selectNextTab', 'selectPreviousTab', 'showAllTabs', 'mergeAllWindows', 'clearRecentDocuments', 'moveTabToNewWindow', 'windowMenu'];
  const all = Menu.buildFromTemplate(roles.map(role => ({ role })));
  assert(all.items.every(item => item.label), 'Every Electron role has a default label');
  assert(['Reload', 'Actual Size', 'Zoom In', 'Roles', 'Edit', 'Window'].every(label => all.items.some(item => item.label === label)), 'Electron role labels');
  assert(all.items.find(item => item.role === 'viewMenu').submenu.items.map(item => item.role).join() === 'reload,forceReload,toggleDevTools,,resetZoom,zoomIn,zoomOut,,togglefullscreen', 'viewMenu default submenu');
  const sub = Menu.buildFromTemplate([{ label: 'New' }]);
  let clickArgs;
  const menu = Menu.buildFromTemplate([{ id: 'file', label: 'File', submenu: sub }, { id: 'run', label: 'Run', click: (...args) => { clickArgs = args; } }, { id: 'check', type: 'checkbox' }, { type: 'separator' }, { id: 'a', type: 'radio', checked: true }, { id: 'b', type: 'radio' }, { role: 'reload' }]);
  assert(menu.items[0].submenu === sub && menu.items[0].type === 'submenu' && typeof menu.items[1].click === 'function' && menu.getMenuItemById('New') === null && menu.getMenuItemById('run') === menu.items[1], 'Menu instances, MenuItem and getMenuItemById');
  menu.append(new MenuItem({ id: 'last', label: 'Last' }));
  menu.insert(0, new MenuItem({ id: 'first', label: 'First' }));
  assert(menu.items[0].id === 'first' && menu.items.at(-1).id === 'last' && menu.items.at(-1).menu === menu, 'append and insert');
  Menu.setApplicationMenu(menu);
  const template = calls.at(-1)[1], id = name => template.find(item => item.label === name || item.type === name)?.click;
  events.push({ type: 'menu', id: template[2].click, shift: true }, { type: 'menu', id: template[3].click }, { type: 'menu', id: template[6].click }, { type: 'menu', id: template[7].click });
  shell.poll();
  assert(clickArgs[0] === menu.getMenuItemById('run') && clickArgs[1] === window && clickArgs[2].shiftKey === true && clickArgs[2].triggeredByAccelerator === false, 'click(menuItem, browserWindow, event)');
  assert(menu.getMenuItemById('check').checked && !menu.getMenuItemById('a').checked && menu.getMenuItemById('b').checked && reloads.length === 1, 'Checkbox, radio and reload roles');
  assert(id('normal') !== undefined && template[1].submenu[0].label === 'New', 'Submenus serialize');
  window.webContents.zoomLevel = 0;
  const zooms = Menu.buildFromTemplate([{ role: 'zoomIn' }, { role: 'zoomOut' }, { role: 'resetZoom' }]).items, levels = [];
  for (const item of [zooms[0], zooms[0], zooms[1], zooms[2]]) { item.click(); levels.push(window.webContents.zoomLevel); }
  assert(levels.join() === '0.5,1,0.5,0', 'Zoom roles step the focused page by half a level');
  const accelerated = Menu.buildFromTemplate(['Meta+Home', 'F24', 'MediaPlayPause', 'CmdOrCtrl+Plus'].map(accelerator => ({ label: accelerator, accelerator })));
  Menu.setApplicationMenu(accelerated);
  assert(calls.at(-1)[1].map(item => item.accelerator).join() === 'Meta+Home,F24,MediaPlayPause,CmdOrCtrl+Plus', 'Accelerators pass to the native menu');

  app.isReady = () => false;
  let early;
  try { new shell.Tray('serverTemplate.png'); } catch (error) { early = error; }
  assert(early?.message.includes('whenReady'), 'Tray needs a ready app');
  app.isReady = () => true;
  const first = new shell.Tray('serverTemplate.png'), firstId = calls.at(-1)[1].id;
  const second = new shell.Tray('other.png'), secondId = calls.at(-1)[1].id;
  const actions = [];
  first.setContextMenu(Menu.buildFromTemplate([{ label: 'Server', icon: 'server.png', sublabel: 'Running', submenu: [{ label: 'Settings', click: () => actions.push('first') }] }]));
  const header = calls.at(-1)[1].template[0], settings = header.submenu[0].click;
  assert(header.icon === 'server.png' && header.sublabel === 'Running', 'Tray menu icon and subtitle');
  second.setContextMenu(Menu.buildFromTemplate([{ label: 'Other', click: () => actions.push('second') }]));
  const other = calls.at(-1)[1].template[0].click;
  first.popUpContextMenu(Menu.buildFromTemplate([{ label: 'Temporary', click: () => actions.push('popup') }]));
  const popup = calls.at(-1)[1];
  first.once('click', (event, bounds, point) => { assert(event.altKey && bounds.width === 24 && point.x === 10, 'Tray click arguments'); actions.push('click'); });
  events.push({ type: 'tray', id: firstId, event: 'click', alt: true, bounds: { width: 24 }, position: { x: 10 } },
    ...[settings, other, popup.template[0].click].map(id => ({ type: 'menu', id })),
    { type: 'tray', id: firstId, request: popup.request }, { type: 'menu', id: popup.template[0].click });
  shell.poll();
  first.setContextMenu(null);
  events.push({ type: 'menu', id: settings }, { type: 'menu', id: other });
  shell.poll();
  assert(actions.join() === 'click,first,second,popup,second', 'Tray menu callbacks stay with their owner and expire on removal');
  first.destroy(); first.destroy();
  let destroyed;
  try { first.getBounds(); } catch (error) { destroyed = error; }
  assert(first.isDestroyed() && destroyed && !second.isDestroyed(), 'Tray destruction is independent and idempotent');
  second.on('right-click', () => actions.push('stale'));
  second.destroy();
  events.push({ type: 'tray', id: secondId, event: 'right-click' });
  shell.poll();
  assert(!actions.includes('stale'), 'Discard events for destroyed trays');

  const seen = [], context = Menu.buildFromTemplate([{ label: 'Pick', click: () => seen.push('pick') }]);
  context.on('menu-will-show', () => seen.push('show')).on('menu-will-close', () => seen.push('close'));
  context.popup({ x: 1, y: 2, callback: () => seen.push('callback') });
  const request = calls.at(-1)[1];
  assert(calls.at(-1)[0] === 'native.menu.popup' && request.request > 0 && request.x === 1 && seen.join() === 'show' && shell.pending(), 'Menu.popup returns before the menu closes');
  events.push({ type: 'menu', id: request.template[0].click }, { type: 'popup', id: request.request }, { type: 'menu', id: request.template[0].click });
  shell.poll();
  assert(seen.join() === 'show,pick,close,callback', 'Popup clicks, close and callback reach their menu once');
})();
