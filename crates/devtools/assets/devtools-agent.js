(() => {
  // Runs in the page for the developer tools. Nodes go to the panel by their id, and stay known by it.
  const known = new Map();
  let limit = 2000;
  const id = node => { if (known.get(node._id)?.deref() !== node) known.set(node._id, new WeakRef(node)); return node._id; };
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
  const observer = globalThis.document && new MutationObserver(() => { state ||= 1; });
  observer?.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  // Console values. An object goes to the panel as a short text and a handle, which gives its properties.
  const held = new Map(), logged = new Map();
  let handles = 0, logs = 0;
  const trim = (map, size) => { for (const key of map.keys()) { if (map.size <= size) break; map.delete(key); } };
  const indexed = value => Array.isArray(value) || ArrayBuffer.isView(value) && !(value instanceof DataView);
  const first = (iterable, count) => { const list = []; for (const item of iterable) { if (list.length === count) break; list.push(item); } return list; };
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
      const list = indexed(value), name = value.constructor?.name ?? 'Object', size = list ? value.length : value instanceof Map || value instanceof Set ? value.size : null;
      if (inner) return size != null ? `${name}(${size})` : name === 'Object' ? '{…}' : name;
      if (size != null && !list) return `${name}(${size})`;
      // Only the first keys of a big array or object are read.
      const keys = list ? Array.from({ length: Math.min(size, 6) }, (_, at) => String(at)) : Object.keys(value);
      const parts = keys.slice(0, 5).map(key => (list ? '' : key + ': ') + (Object.getOwnPropertyDescriptor(value, key)?.get ? '(…)' : short(value[key], true)));
      if (keys.length > 5) parts.push('…');
      return list ? `${Array.isArray(value) ? '' : name}(${size}) [${parts.join(', ')}]` : `${name === 'Object' ? '' : name + ' '}{${parts.join(', ')}}`;
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
      if (object instanceof Map) first(object, 200).forEach(([key, value], at) => add(at, () => ({ key, value })));
      if (object instanceof Set) first(object, 200).forEach((value, at) => add(at, () => value));
      const keys = indexed(object) ? Array.from({ length: Math.min(object.length, 500) }, (_, at) => String(at)).concat(Array.isArray(object) ? ['length'] : []) : Reflect.ownKeys(object).slice(0, 500);
      for (const key of keys) { const slot = Object.getOwnPropertyDescriptor(object, key); if (slot) add(typeof key === 'symbol' ? key.toString() : key, () => slot.get ?? slot.set ?? slot.value); }
      const parent = Object.getPrototypeOf(object);
      if (parent) add('[[Prototype]]', () => parent);
      return list;
    },
    clear() { held.clear(); logged.clear(); },
    // The tools closed: the page stops paying for them.
    close() { observer?.disconnect(); api.clear(); delete globalThis.__appsLogged; },
    poll() {
      if (known.size > limit) { for (const [key, ref] of known) if (!ref.deref()) known.delete(key); limit = Math.max(2000, known.size * 2); }
      const value = state; state = 0; return value;
    },
    // The children of each node. A null id gives the root element.
    tree: ({ ids }) => Object.fromEntries(ids.map(id => [id ?? 'root', id == null ? [entry(document.documentElement)] : get(id)?.isConnected ? kids(get(id)).map(entry) : []])),
    path({ id: from }) { const path = []; for (let node = get(from); node && node !== document; node = up(node)) path.unshift(id(node)); return path; },
    pick({ x, y }) { let node = document.elementFromPoint(x, y); for (let inner; (inner = node?.shadowRoot?.elementFromPoint(x, y)) && inner !== node;) node = inner; return node && id(node); },
  };
  return (method, params) => api[method](params ?? {});
})()
