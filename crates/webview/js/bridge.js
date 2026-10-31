(() => {
  const send = data => window.ipc.postMessage(JSON.stringify({ message: data, url: location.href, mainFrame: window === window.top }));
  Object.defineProperty(window, 'apps', { value: Object.freeze({ postMessage: data => send({ kind: 'message', data }) }) });
  if (window === window.top) addEventListener('DOMContentLoaded', () => send({ kind: 'dom-ready' }), { once: true });
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[level];
    console[level] = (...args) => {
      original.apply(console, args);
      send({ kind: 'console', level, args: args.map(value => { try { return typeof value === 'string' ? value : JSON.stringify(value); } catch { return String(value); } }) });
    };
  }
  addEventListener('error', event => send({ kind: 'error', message: event.message, source: event.filename, line: event.lineno }));
  addEventListener('unhandledrejection', event => send({ kind: 'error', message: String(event.reason) }));
})();
