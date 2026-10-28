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
})();
