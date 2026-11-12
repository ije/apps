(() => {
  // Runs in the page for the developer tools. Nodes go to the panel by their id, and stay known by it.
  const known = new Map();
  const id = node => { known.set(node._id, new WeakRef(node)); return node._id; };
  const get = id => known.get(id)?.deref() ?? null;
  // The layers of a node: its elements and texts, without the ones that draw nothing.
  const kids = node => [...(node.shadowRoot?.childNodes ?? []), ...node.childNodes].filter(kid => kid.nodeType === 1 ? !/^(head|script|style|link|meta|title|template|noscript)$/.test(kid.localName) : kid.nodeType === 3 && kid.data.trim());
  const up = node => node.parentNode?.host ?? node.parentNode;
  const entry = node => {
    if (node.nodeType === 3) return { id: id(node), text: node.data.trim().slice(0, 120) };
    const list = kids(node), text = list.length === 1 && list[0].nodeType === 3 ? list[0].data.trim().slice(0, 120) : undefined;
    return { id: id(node), tag: node.localName, key: node.id || undefined, cls: node.getAttribute('class') || undefined, text, kids: text === undefined ? list.length : 0 };
  };
  let state = 2;
  // The main process has no document.
  if (globalThis.document) new MutationObserver(() => { state ||= 1; }).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  // Console values. An object goes to the panel as a short text and a handle, which gives its properties.
  const held = new Map(), logged = new Map();
  let handles = 0, logs = 0;
  const trim = (map, size) => { for (const key of map.keys()) { if (map.size <= size) break; map.delete(key); } };
  const short = (value, inner) => {
    try {
      const type = typeof value;
      if (type === 'string') return inner ? JSON.stringify(value.length > 60 ? value.slice(0, 60) + '…' : value) : value;
      if (type === 'bigint') return value + 'n';
      if (type === 'function') return `ƒ ${value.name}()`;
      if (value === null || type !== 'object') return String(value);
      if (value instanceof Error || value instanceof RegExp) return String(value);
      if (value instanceof Date) return Number.isNaN(+value) ? 'Invalid Date' : value.toISOString();
      if (value.nodeType === 1) return `<${value.localName}${value.id ? '#' + value.id : ''}>`;
      const name = value.constructor?.name ?? 'Object', size = Array.isArray(value) ? value.length : value instanceof Map || value instanceof Set ? value.size : null;
      if (inner) return size != null ? `${name}(${size})` : name === 'Object' ? '{…}' : name;
      if (size != null && !Array.isArray(value)) return `${name}(${size})`;
      const keys = Object.keys(value), parts = keys.slice(0, 5).map(key => (Array.isArray(value) ? '' : key + ': ') + (Object.getOwnPropertyDescriptor(value, key).get ? '(…)' : short(value[key], true)));
      if (keys.length > 5) parts.push('…');
      return Array.isArray(value) ? `(${size}) [${parts.join(', ')}]` : `${name === 'Object' ? '' : name + ' '}{${parts.join(', ')}}`;
    } catch { return Object.prototype.toString.call(value); }
  };
  const inspect = value => {
    const type = value === null ? 'null' : typeof value;
    if (type !== 'object' && type !== 'function') return { type, text: short(value) };
    held.set(++handles, value);
    trim(held, 4000);
    const error = value instanceof Error;
    return { type: error ? 'error' : type, text: short(value), id: handles, stack: error ? String(value.stack ?? '') : undefined };
  };
  globalThis.__appsLogged = args => {
    if (!args.some(value => value !== null && /object|function/.test(typeof value))) return;
    logged.set(++logs, args);
    trim(logged, 300);
    return logs;
  };
  const api = {
    inspect: ({ value = null }) => inspect(value),
    args: ({ ref }) => (logged.get(ref) ?? []).map(inspect),
    props({ id }) {
      const object = held.get(id), list = [];
      if (object == null) return list;
      const add = (name, read) => { let item; try { item = inspect(read()); } catch (error) { item = { type: 'error', text: String(error) }; } list.push({ name: String(name), ...item }); };
      if (object instanceof Map) [...object].slice(0, 200).forEach(([key, value], at) => add(at, () => ({ key, value })));
      if (object instanceof Set) [...object].slice(0, 200).forEach((value, at) => add(at, () => value));
      for (const key of Reflect.ownKeys(object).slice(0, 500)) { const slot = Object.getOwnPropertyDescriptor(object, key); add(typeof key === 'symbol' ? key.toString() : key, () => slot.get ?? slot.set ?? slot.value); }
      const parent = Object.getPrototypeOf(object);
      if (parent) add('[[Prototype]]', () => parent);
      return list;
    },
    clear() { held.clear(); logged.clear(); },
    poll() { const value = state; state = 0; return value; },
    // The children of each node. A null id gives the root element.
    tree: ({ ids }) => Object.fromEntries(ids.map(id => [id ?? 'root', id == null ? [entry(document.documentElement)] : get(id)?.isConnected ? kids(get(id)).map(entry) : []])),
    path({ id: from }) { const path = []; for (let node = get(from); node && node !== document; node = up(node)) path.unshift(id(node)); return path; },
    pick({ x, y }) { const node = document.elementFromPoint(x, y); return node && id(node); },
  };
  return (method, params) => api[method](params ?? {});
})()
