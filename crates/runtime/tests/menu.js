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
      else if (mode === 'custom') assert(menu === custom && calls[0][1] === custom.items, 'Preserve a menu set before startup');
      else {
        assert(menu.items.map(item => item.label).join(',') === 'Menu Test,File,Edit,View,Window,Help', 'Default menu sections');
        const quit = menu.items[0].submenu.find(item => item.role === 'quit');
        assert(quit.label === 'Quit Menu Test' && quit.accelerator === 'Command+Q', 'Use the app name and standard quit shortcut');
        assert(menu.items[3].submenu.some(item => item.role === 'toggleDevTools') === !packaged, 'Developer Tools are only available in development');
        if (!packaged) globalThis.menuTemplate = menu.items;
      }
      __appsInitMenu();
      shell.poll();
      assert(calls.length === 1 && !shell.pending(), 'Native menu roles do not need polling or repeated setup');
      shell.Menu.setApplicationMenu(custom);
      assert(calls.length === 2 && calls[1][1] === custom.items, 'Replace the menu after startup');
      shell.Menu.setApplicationMenu(null);
      assert(calls.length === 3 && calls[2][1] === null && shell.Menu.getApplicationMenu() === null, 'Remove the menu after startup');
      let clicked = false;
      shell.Menu.setApplicationMenu(shell.Menu.buildFromTemplate([{ label: 'Run', click: () => { clicked = true; } }]));
      assert(shell.pending(), 'Custom menu callbacks keep polling active');
      events.push({ type: 'menu', id: shell.Menu.getApplicationMenu().items[0].click });
      shell.poll();
      assert(clicked, 'Custom menu callbacks still run');
    }
  }
})();
