(() => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  const state = {
    themeSource: 'system', shouldUseDarkColors: false, shouldUseHighContrastColors: false,
    shouldUseDarkColorsForSystemIntegratedUI: false, shouldUseInvertedColorScheme: false,
    inForcedColorsMode: false, prefersReducedTransparency: false, shouldDifferentiateWithoutColor: false,
  };
  const events = [], calls = [];
  const shell = __appsNativeShell((op, value) => {
    calls.push(op);
    if (op === 'native.theme.get') return { ...state };
    if (op === 'native.theme.set') { state.themeSource = value; return; }
    if (op === 'native.poll') return events.splice(0);
    throw Error(op);
  }, __apps.events.EventEmitter);
  const theme = shell.nativeTheme;
  assert(theme instanceof __apps.events.EventEmitter && theme.themeSource === 'system', 'Default theme and EventEmitter');
  for (const name of Object.keys(state).filter(name => name !== 'themeSource')) {
    assert(theme[name] === false, name);
    state[name] = true;
    assert(theme[name] === true, 'Read current native preference: ' + name);
    assert(!Reflect.set(theme, name, false) && theme[name], 'Readonly preference: ' + name);
  }
  assert(!shell.pending(), 'Reading preferences must not keep the event loop polling');
  for (const value of [null, undefined, '', 'auto', 'Dark', 1, {}, ['dark']]) {
    const before = calls.length;
    let error;
    try { theme.themeSource = value; } catch (caught) { error = caught; }
    assert(error instanceof TypeError && before === calls.length, 'Reject invalid source before native code');
  }
  let updates = 0, once = 0;
  const listener = function (...args) { assert(this === theme && !args.length, 'Updated event arguments'); updates++; };
  theme.addListener('updated', listener);
  theme.prependOnceListener('updated', () => once++);
  assert(!shell.pending(), 'Theme listeners must not keep the event loop polling');
  for (const source of ['dark', 'light', 'system']) {
    theme.themeSource = source;
    assert(theme.themeSource === source && updates === 0, 'Source changes wait for native notifications');
  }
  events.push({ type: 'theme' }); shell.poll();
  events.push({ type: 'theme' }); shell.poll();
  assert(updates === 2 && once === 1, 'Native updates and once listeners');
  theme.removeListener('updated', listener);
  events.push({ type: 'theme' }); shell.poll();
  assert(updates === 2 && !shell.pending(), 'Listener removal');
  const failing = __appsNativeShell(op => { if (op === 'native.poll') return events.splice(0); if (op === 'native.notification.show') throw Error('Notifications require an app bundle identifier'); if (op === 'native.safeStorage.encrypt') return btoa('sealed'); throw Error('This native desktop operation is currently available on macOS'); }, __apps.events.EventEmitter);
  assert(failing.nativeTheme.shouldUseDarkColors === false && failing.nativeTheme.themeSource === 'system', 'Theme state has defaults where the platform has none');
  failing.nativeTheme.themeSource = 'dark';
  failing.nativeTheme.on('updated', () => {});
  assert(failing.nativeTheme.shouldUseDarkColors === true && failing.nativeTheme.themeSource === 'dark', 'Theme source still applies');
  const sealed = failing.safeStorage.encryptString('x');
  assert(Buffer.isBuffer(sealed) && sealed.toString('base64') === btoa('sealed'), 'safeStorage returns a Buffer');
  globalThis.notificationFailures = [];
  const notification = new failing.Notification({ title: 'x' });
  notification.on('failed', (event, error) => notificationFailures.push(typeof event.preventDefault + ':' + error));
  notification.show();
  assert(failing.Notification.isSupported() === (process.platform === 'darwin') && !notificationFailures.length, 'show() never throws');
  for (const end = Date.now() + 100; !notificationFailures.length && Date.now() < end;) __appsTick();
  assert(notificationFailures.join() === 'function:Notifications require an app bundle identifier', 'Failures emit failed later');
  const screen = __appsNativeShell(op => op === 'native.screen.displays' ? [{ id: 1, bounds: { x: 0, y: 0, width: 100, height: 100 } }, { id: 2, bounds: { x: 100, y: 0, width: 100, height: 100 } }] : op === 'native.screen.cursor' ? { x: 5, y: 6 } : [], __apps.events.EventEmitter).screen;
  assert(screen.getPrimaryDisplay().id === 1 && screen.getAllDisplays().length === 2 && screen.getDisplayNearestPoint({ x: 250, y: 10 }).id === 2 && screen.getDisplayMatching({ x: 90, y: 0, width: 50, height: 10 }).id === 2 && screen.getCursorScreenPoint().y === 6, 'screen');
})();
