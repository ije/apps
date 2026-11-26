(() => {
  const host = globalThis.__appsDOM, eventState = globalThis.__appsEventState, droppedFiles = globalThis.__appsFiles, drainMicrotasks = globalThis.__appsDrain, microtask = queueMicrotask, json = JSON.parse;
  delete globalThis.__appsDOM; delete globalThis.__appsEventState; delete globalThis.__appsFiles; delete globalThis.__appsDrain;
  // Flush dom.batch before any other host call that reads or changes the page. Replace shared noAttrs and noKids.
  // Never mutate them.
  class Attrs {
    constructor(list = []) { this.list = list; }
    get size() { return this.list.length / 2; }
    get(name) { const list = this.list; for (let i = 0; i < list.length; i += 2) if (list[i] === name) return list[i + 1]; }
    has(name) { const list = this.list; for (let i = 0; i < list.length; i += 2) if (list[i] === name) return true; return false; }
    set(name, value) { const list = this.list; for (let i = 0; i < list.length; i += 2) if (list[i] === name) { list[i + 1] = value; return this; } list.push(name, value); return this; }
    delete(name) { const list = this.list; for (let i = 0; i < list.length; i += 2) if (list[i] === name) { list.splice(i, 2); return true; } return false; }
    keys() { return this.list.filter((_, i) => !(i & 1)); }
    *[Symbol.iterator]() { const list = this.list; for (let i = 0; i < list.length; i += 2) yield [list[i], list[i + 1]]; }
  }
  const noAttrs = new Attrs(), noKids = Object.freeze([]);
  class Record { constructor(id, tag, text) { this.id = id; this.tag = tag; this.text = text; this.attrs = tag[0] === '#' ? null : noAttrs; this.up = null; this.kids = tag === '#text' || tag === '#comment' ? noKids : []; this.i = 0; this.shadow = null; this.host = null; this.ref = null; this.watched = false; } }
  const records = [], texts = [];
  let detached = false;
  let scheduled = false, changed = false, pool = [], reserve = 32, codes = new Int32Array(1024), size = 0;
  // The parsed page has no layout yet, so the first geometry read must lay it out.
  // The layout count when the page last changed, or a script scrolled, since a layout. A layout after it, like the
  // one of a frame, has the change.
  let stale = true, staleAt, scrolled = false, scrolledAt, laid = false, version = 0, shape = 0;
  const rare = { base: 0, title: 0 };
  let retrying = false;
  let highlights = new Map(), highlightShape = -1, highlightsQueued = false, looseRanges = 0;
  const flush = () => {
    if (looseRanges && highlightShape !== shape) queueHighlights();
    if (!size) return;
    let errors;
    try { errors = host('dom.batch', texts.join(''), codes, size); }
    catch (error) {
      if (/Host is busy/.test(error?.message)) { if (!retrying) { retrying = true; setTimeout(() => { retrying = false; flush(); }); } return; }
      errors = [error?.message];
    }
    size = 0; texts.length = 0;
    if (!errors) return;
    for (const error of errors) console.error('DOM: ' + error);
    // Reload after failed batches: Rust can skip invalid writes. Preserve record identities because wrappers
    // reference them.
    const lists = [], seen = new Set();
    for (const rec of records) if (rec && !rec.up) try { lists.push(json(host('dom.tree', rec.id))); } catch {}
    if (!lists.length) return;
    version++;
    for (const rec of records) if (rec?.kids.length) rec.kids = [];
    shape++;
    ids.clear();
    for (const list of lists) {
      for (let i = 0; i < list.length;) {
        const id = list[i], tag = list[i + 2], end = i + 5 + 2 * list[i + 4], up = i ? records[list[i + 1]] : null;
        let rec = records[id];
        if (rec?.tag !== tag) { rec = records[id] = new Record(id, tag, list[i + 3]); if (tag.includes('-')) customTags.add(tag); } else rec.text = list[i + 3];
        if (rec.attrs) { rec.attrs = end > i + 5 ? new Attrs(list.slice(i + 5, end)) : noAttrs; const value = rec.attrs.get('id'); if (value) idAdd(value, rec); }
        if (up) { rec.up = up; rec.i = up.kids.push(rec) - 1; seen.add(rec); }
        i = end;
      }
    }
    for (const rec of records) if (rec) { if (rec.up && !seen.has(rec)) rec.up = null; relist(rec); }
    wakeAll();
  };
  const drain = () => { scheduled = false; flush(); };
  // Replace trailing lone high surrogates with U+FFFD before concatenation, or adjacent strings can pair them
  // and change UTF-16 lengths.
  const alone = value => { const last = value.charCodeAt(value.length - 1); return last >= 0xd800 && last < 0xdc00 ? value.slice(0, -1) + '\u{fffd}' : value; };
  const op = (code, a, b = 0, c = 0) => {
    if (size === codes.length) { const grown = new Int32Array(size * 2); grown.set(codes); codes = grown; }
    changed = stale = true; staleAt = geometry[2]; version++; if (code === 2 || code === 3) { shape++; if (readers) { const kid = records[b]; if (code === 2 && kid.up) noteTree(kid, kid.up); noteTree(kid, records[a], code === 3); } } codes[size] = code; codes[size + 1] = a; codes[size + 2] = typeof b === 'string' ? (texts.push(alone(b)), b.length) : b; codes[size + 3] = typeof c === 'string' ? (texts.push(alone(c)), c.length) : c; size += 4;
    if (!scheduled) { scheduled = true; microtask(drain); } else if (size > 1 << 18) flush();
  };
  let webviewScan = true, pickFiles, openSelect;
  const call = (name, ...args) => { flush(); if (name === 'dom.html') { changed = true; shape++; webviewScan ||= /<webview/i.test(args[1]); } if (name === 'dom.scroll') { scrolled = true; scrolledAt = geometry[2]; version++; } else if (name === 'dom.html' || name === 'dom.focus' || name === 'dom.target' || name === 'dom.hover' || name === 'dom.active' || name === 'dom.font' || name === 'dom.animation' || name === 'dom.sheet' || name === 'dom.adopt' || name === 'dom.constructed' && args[0] || name === 'dom.popoverSource') { stale = true; staleAt = geometry[2]; version++; } return host(name, ...args); };
  // Keep mirror arrays private. Retain constructors and avoid prototype getters. A nonzero first float means
  // Rust moved the buffer.
  const F32 = Float32Array, U8 = Uint8Array;
  let geometry = new F32(1), marks, moves = new F32(1), limit = 0, flagged = -1; geometry[0] = 1;
  let observing = false;
  const changes = () => (stale &&= geometry[2] === staleAt);
  const pending = () => { scrolled &&= geometry[2] === scrolledAt; return changes() || scrolled; };
  const layout = () => {
    if ((pending() || geometry[3]) && !observing) { stale = scrolled = false; laid = true; call('dom.reflow'); }
    if (geometry[0]) { const buffer = host('dom.layout', true), capacity = new F32(buffer, 0, 4)[1]; limit = 4 + capacity * 8; geometry = new F32(buffer, 0, limit); marks = new U8(buffer, limit * 4, capacity); moves = new F32(buffer, limit * 4 + capacity, 3); }
    return geometry;
  };
  // Text that the last layout did not place, like text far from the view, gets a layout when a script reads it.
  const textCall = (op, ...args) => { let result = call(op, ...args); if (result === null) { stale = true; staleAt = geometry[2]; layout(); result = call(op, ...args) ?? (op === 'dom.textRect' ? [] : 0); } return result; };
  // As in Chrome, the contents that a box skips get a layout when a script reads their geometry.
  const slot = node => { layout(); const i = 4 + node._id * 8; if (i >= limit || marks[node._id] & 16 && !(!observing && node.isConnected && call('dom.skipped', node._id, true) > 0 && (stale = true, staleAt = geometry[2], layout(), !(marks[node._id] & 16)))) return -1; if (node._placed !== version) { if (!node.isConnected) return -1; node._placed = version; } return i; };
  const metric = (node, k) => { const i = slot(node); return i < 0 ? 0 : geometry[i + k]; };
  // A box that was never scrolled reads 0 without a layout. A scroll offset doesn't need a layout of the boxes that
  // it or a transform moved, so while the page has no other change it comes from the mirror.
  const scrollOf = (node, k) => {
    if (!observing && !geometry[0] && !changes()) {
      host('dom.layout', false);
      const id = rootElement(node) ? 0 : node._id, i = 4 + id * 8;
      if (i < limit && !(marks[id] & 16)) return marks[id] & 2 ? geometry[i + k] : 0;
    }
    if (rootElement(node)) node = document; else if ((pending() || geometry[3]) && !observing && !call('dom.scrolled', node._id)) return 0;
    return metric(node, k);
  };
  // As in Chrome, a scroll lays out a box that was no scroll box in the last layout, so it scrolls only if it can,
  // and its offset stays in its range.
  const scrollTo = (node, left, top, behavior = null, by = null) => {
    const fresh = !rootElement(node) && !(marks?.[node._id] & 2);
    if (fresh) layout();
    if (moves[2]) { const z = zoomOf(node); if (left !== null) left *= z; if (top !== null) top *= z; }
    const offset = call('dom.scroll', node._id, left, top, behavior, by);
    if (fresh) layout();
    return offset;
  };
  // Do not cache client boxes from observer reads: they use the previous frame's styles.
  const client = node => { if (slot(node) < 0) return [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; if (observing) return call('dom.client', node._id, true); const cached = node._client; if (cached?.[14] === geometry[2] && cached[15] === version) return cached; return node._client = [...call('dom.client', node._id), geometry[2], version]; };
  // Only scroll boxes have a scroll range, so other boxes take the border boxes of their children as their
  // scrollable overflow, then their end padding.
  // Layout is in zoomed pixels, and the offset, client and scroll values of a box in CSS pixels of its zoom.
  const zoomOf = node => moves[2] && !rootElement(node) ? client(node)[13] || 1 : 1;
  // Like Chrome, a zoomed scroll offset is whole CSS pixels.
  const unzoom = (node, offset) => { const z = zoomOf(node); return z === 1 ? offset : Math.round(offset / z); };
  const extent = (node, axis) => {
    const box = client(node), i = slot(node), z = zoomOf(node);
    if (i < 0 || marks[node._id] & 2) return Math.round((box[2 + axis] + (i < 0 ? 0 : geometry[i + 6 + axis])) / z);
    // The bounds of the children of a scaled box are scaled too, so its own size is used.
    if (marks[node._id] & 128 && (box[8] !== geometry[i + 2] || box[9] !== geometry[i + 3])) return Math.round(box[2 + axis] / z);
    if (!box[2] && !box[3] && !box[4] && !box[5] && !box[6] && !box[7] && /^(inline|contents)$/.test(getComputedStyle(node).display)) return 0;
    const start = geometry[i + axis] + box[axis], pending = [...(node._m.shadow ?? node._m).kids];
    // A right-to-left box overflows to its left, so its width counts from its right side.
    const right = !axis && getComputedStyle(node).direction === 'rtl' ? start + box[2] : null;
    let end = 0;
    while (pending.length) {
      const kid = pending.pop(), j = 4 + kid.id * 8;
      if (j >= limit || (marks[kid.id] & 49) !== 1) continue;
      if (geometry[j + 2] || geometry[j + 3]) end = Math.max(end, right === null ? geometry[j + axis] + geometry[j + 2 + axis] - start : right - geometry[j]);
      else if (kid.tag[0] !== '#') pending.push(...(kid.shadow ?? kid).kids);
    }
    return Math.round(Math.max(box[2 + axis], end && end + box[5 + axis]) / z);
  };
  const offset = (node, axis) => {
    if (slot(node) < 0 || node === document.body) return 0;
    const start = node.getBoundingClientRect()[axis ? 'top' : 'left'] + (marks[node._id] & 128 ? client(node)[10 + axis] : 0), parent = node.offsetParent;
    // Offsets are layout positions, so the scroll offsets of the boxes up to the offset parent don't count.
    let scrolled = 0;
    for (let p = node.parentElement; p && p !== parent; p = p.parentElement) if (marks[p._id] & 2) scrolled += metric(p, 4 + axis);
    if (!parent || parent.localName === 'body' && !(marks[parent._id] & 8)) return Math.round((start + metric(document, 4 + axis) + scrolled) / zoomOf(node));
    if (marks[parent._id] & 2) scrolled += metric(parent, 4 + axis);
    return Math.round((start - parent.getBoundingClientRect()[axis ? 'top' : 'left'] - client(parent)[axis] + scrolled) / zoomOf(node));
  };
  const owned = (doc, node) => { if (doc !== document) node._owner = doc; return node; };
  const rootElement = node => node._m.tag === 'html' && node._m.up === records[0];
  // The viewport without the scroll bars of the page.
  const viewSize = () => { layout(); const [width, height, , , bars] = host('dom.viewport'); return [width - bars[1], height - bars[0]]; };
  const xmlName = /^[A-Za-z_:\u0080-\uffff][\w:.\u0080-\uffff-]*$/;
  const create = (tag, text = '', from = -1) => {
    if (!names.has(tag)) { if (!xmlName.test(tag) && !types[tag]) throw new DOMException('Invalid tag', 'InvalidCharacterError'); names.set(tag, tag[0] === '#' ? tag === '#fragment' ? '#document-fragment' : tag : tag.toUpperCase()); }
    if (!pool.length) pool = host('dom.reserve', reserve = Math.min(reserve * 2, 1024)).reverse();
    const id = pool.pop();
    records[id] = new Record(id, tag, text); if (tag.includes('-')) customTags.add(tag); if (from < 0) op(0, id, tag, text); else op(10, id, from); if (tag === 'base' || tag === 'title') rare[tag]++; return id;
  };
  let declarative = null;
  // Attach the declarative shadow roots of the last `load`. Only the page parser and `setHTMLUnsafe()` do this.
  const attachDeclarative = () => {
    const list = declarative; declarative = null;
    for (const rec of list ?? []) {
      const template = of(rec), host = template.parentNode, mode = rec.attrs.get('shadowrootmode').toLowerCase();
      if (host?.nodeType !== 1 || host._shadow || mode !== 'open' && mode !== 'closed') continue;
      try { host.attachShadow({ mode, delegatesFocus: rec.attrs.has('shadowrootdelegatesfocus'), clonable: rec.attrs.has('shadowrootclonable'), serializable: rec.attrs.has('shadowrootserializable') }).append(template.content); template.remove(); } catch {}
    }
  };
  // The session that a `<terminal>` element shows, and the element that shows a session. `terminalOf` gives
  // the ID of the session, or nothing without a session that is open.
  const terminalSession = globalThis.__appsTerminal, terminalSessions = new WeakMap(), terminalViews = new WeakMap();
  delete globalThis.__appsTerminal;
  const terminalOf = node => terminalSession?.(terminalSessions.get(node));
  const load = list => {
    version++; shape++; declarative = null;
    let media = false, radios = null, controls = null;
    for (let i = 0; i < list.length;) {
      const rec = records[list[i]] = new Record(list[i], list[i + 2], list[i + 3]), up = records[list[i + 1]], end = i + 5 + 2 * list[i + 4];
      if (end > i + 5) { rec.attrs = new Attrs(list.slice(i + 5, end)); const id = rec.attrs.get('id'); if (id) idAdd(id, rec); }
      const tag = rec.tag; if (tag.includes('-')) customTags.add(tag);
      if (tag === 'base' || tag === 'title') rare[tag]++;
      if (tag === 'template' && end > i + 5 && rec.attrs.has('shadowrootmode')) (declarative ??= []).push(rec);
      media ||= tag === 'img' || tag === 'video' || tag === 'audio';
      if (readers) noteNode(rec, up);
      if (tag === 'input' && end > i + 5 && rec.attrs.has('checked')) (radios ??= []).push(rec);
      if ((tag === 'input' || tag === 'select' || tag === 'textarea') && end > i + 5 && limited(rec.attrs)) (controls ??= []).push(rec);
      i = end;
      if (up) { rec.up = up; rec.i = up.kids.push(rec) - 1; }
    }
    if (radios) for (const rec of radios) { const node = of(rec); if (node.type === 'radio') setChecked(node, true); }
    if (controls) for (const rec of controls) revalidate(of(rec));
    return media;
  };
  const of = rec => rec ? rec.ref?.deref() ?? wrap(rec.id) : null;
  // A cached list holds the wrappers of its nodes, so drop it whenever the children change, or it keeps removed
  // nodes alive.
  // `childNodes` and `children` are live, but plain arrays, so that reads are fast: each change of the children
  // also changes the lists that the page has read. After a change a node holds its lists weakly until the next
  // read, so lists that the page dropped go away.
  const listRef = Symbol();
  const liveList = (node, i) => {
    const key = i ? '_elements' : '_nodes', list = node[key];
    if (!list) return node._lists?.[i]?.deref() ?? null;
    node[key] = null;
    (node._lists ??= [null, null])[i] = list[listRef] ??= new WeakRef(list);
    return list;
  };
  const listInsert = (node, kid, at) => {
    const nodes = liveList(node, 0), elements = kid.tag[0] === '#' ? null : liveList(node, 1), kids = node._m.kids;
    if (nodes) if (at === kids.length - 1) nodes.push(of(kid)); else nodes.splice(at, 0, of(kid));
    if (elements) {
      let next = at + 1;
      while (next < kids.length && kids[next].tag[0] === '#') next++;
      if (next === kids.length) elements.push(of(kid)); else elements.splice(elements.indexOf(of(kids[next])), 0, of(kid));
    }
  };
  const listRemove = (node, kid, at) => {
    const nodes = liveList(node, 0), elements = kid.tag[0] === '#' ? null : liveList(node, 1);
    if (nodes) nodes.splice(at, 1);
    if (elements) elements.splice(elements.indexOf(of(kid)), 1);
  };
  const relist = rec => {
    const node = rec.ref?.deref();
    if (!node || !(node._nodes || node._elements || node._lists)) return;
    const nodes = liveList(node, 0), elements = liveList(node, 1);
    if (nodes) { nodes.length = 0; for (const kid of rec.kids) nodes.push(of(kid)); }
    if (elements) { elements.length = 0; for (const kid of rec.kids) if (kid.tag[0] !== '#') elements.push(of(kid)); }
    if (!nodes && !elements) node._lists = null;
  };
  const ids = new Map();
  const idAdd = (id, rec) => { const at = ids.get(id); if (!at) ids.set(id, rec); else if (Array.isArray(at)) { if (!at.includes(rec)) at.push(rec); } else if (at !== rec) ids.set(id, [at, rec]); };
  const idDrop = (id, rec) => { const at = ids.get(id); if (at === rec) ids.delete(id); else if (Array.isArray(at)) { const i = at.indexOf(rec); if (i >= 0) at.splice(i, 1); if (at.length === 1) ids.set(id, at[0]); } };
  // The elements with this id inside `root`, in no order.
  const idsWithin = (root, id) => {
    const at = id && ids.get(id);
    if (!at) return [];
    const inside = rec => { if (records[rec.id] !== rec || rec.attrs?.get('id') !== id) return false; for (let up = rec.up; up; up = up.up) { if (up === root) return true; if (up.tag === 'template') return false; } return false; };
    return Array.isArray(at) ? at.filter(inside) : inside(at) ? [at] : [];
  };
  const idSelector = /^#(?:--|-?[A-Za-z_\u0080-\uffff])[\w\u0080-\uffff-]*$/;
  const index = rec => { const kids = rec.up.kids; let i = rec.i; if (kids[i] !== rec) rec.i = i = kids[i + 1] === rec ? i + 1 : kids[i - 1] === rec ? i - 1 : kids.indexOf(rec); return i; };
  const own = rec => rec.text ??= host('dom.source', rec.id);
  const text = root => { let value = ''; const stack = [root]; while (stack.length) { const rec = stack.pop(); if (rec !== root && (rec.tag === '#comment' || rec.tag === '#doctype')) continue; value += own(rec); for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); } return value; };
  const find = (root, test) => { const stack = []; for (let i = root.kids.length; i--;) stack.push(root.kids[i]); while (stack.length) { const rec = stack.pop(); if (test(rec)) return of(rec); if (rec.tag !== 'template') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); } return null; };
  const detach = rec => { version++; shape++; if (rec.kids.length) { if (readers) for (const kid of rec.kids) noteTree(kid, rec); for (const kid of rec.kids) kid.up = null; rec.kids = []; detached = true; relist(rec); } rec.text = ''; };
  const unhold = child => { const parent = child._parent; if (!parent) return; const last = parent._held.pop(); if (last !== child) { parent._held[child._slot] = last; last._slot = child._slot; } };
  const hold = (parent, child) => { unhold(child); child._parent = parent; child._slot = (parent._held ??= []).push(child) - 1; };
  const release = node => { if (node._held) for (const child of node._held) child._parent = null; node._held = null; };
  const definitions = new Map(), defined = new WeakMap(), definitionsWaiting = new Map();
  let constructing = null;
  const wrapped = [], names = new Map(), types = { __proto__: null, '#document': 9, '#text': 3, '#comment': 8, '#fragment': 11, '#doctype': 10 }, events = new WeakMap(), registered = [], pointers = new Set(), captures = new Map(), gotten = new Map(), observers = new Set();
  let readyState = 'loading', currentScript = null, active = null, navStart = null, hovered = null, selection = null, composing = null, mutationQueued = false, selectionDrag = null, quiet = 0, shadows = 0, moving = false, wheeling = false, iterating = false, native = 0, ending = false, hoverVersion = 0;
  const scrollEnds = new Map();
  const selectionPending = new Set();
  const topLayer = [], dialogs = [], autos = [], hints = [], fullscreens = [], watchers = [];
  const selectionChanged = (target = document) => {
    if (selectionPending.has(target)) return;
    selectionPending.add(target);
    setTimeout(() => { selectionPending.delete(target); dispatch(target, new Event('selectionchange', { bubbles: target !== document })); }, 0);
  };
  const XHTML = 'http://www.w3.org/1999/xhtml', SVG = 'http://www.w3.org/2000/svg';
  const listOf = (Type, items) => Object.setPrototypeOf(items, Type.prototype);
  const quote = value => '"' + String(value).replace(/["\\]/g, '\\$&') + '"';
  const assignedSlot = (node, parent = node.parentNode) => {
    if (!parent?._shadow) return null;
    const rec = node._m, name = rec.attrs?.get('slot') ?? '';
    return rec.tag[0] !== '#' || rec.tag === '#text' ? find(parent._shadow._m, slot => slot.tag === 'slot' && (slot.attrs.get('name') ?? '') === name) : null;
  };
  const path = node => { const result = []; while (node) { result.push(node); const parent = node.parentNode; node = assignedSlot(node, parent) ?? parent ?? (node instanceof ShadowRoot ? node.host : null); } return result; };
  // Use WeakRefs for observer targets and global sets so forgotten observers cannot retain removed trees.
  // `watched` on a record says that its node has observers, so a walk up the tree reads no other node.
  const watch = (observer, target, value, set) => { observer.targets.set(target._m.ref, value); (target._observers ??= new Set()).add(observer); target._m.watched = true; set.add(observer._ref ??= new WeakRef(observer)); };
  const unwatchNode = (observer, target) => { const set = target?._observers; if (set?.delete(observer) && !set.size) target._m.watched = false; };
  const unwatch = (observer, target, set) => { observer.targets.delete(target._m.ref); unwatchNode(observer, target); if (!observer.targets.size) set.delete(observer._ref); };
  const unwatchAll = (observer, set) => { for (const ref of observer.targets.keys()) unwatchNode(observer, ref.deref()); observer.targets.clear(); set.delete(observer._ref); };
  // Asks only the observers of the target and of its ancestors, so observers of other trees cost a change nothing.
  // `found` holds each observer that takes the record, followed by whether it takes the old value.
  const change = record => {
    if (!observers.size || quiet) return;
    const type = record.type, attributes = type === 'attributes', own = record.target._m;
    let found = null;
    for (let rec = own; rec; rec = rec.up) {
      const set = rec.watched && rec.ref.deref()?._observers;
      if (set) for (const observer of set) {
        const options = observer.targets.get(rec.ref);
        if (!options?.[type] || rec !== own && !options.subtree || attributes && options.attributeFilter && !options.attributeFilter.includes(record.attributeName)) continue;
        const old = !!(attributes ? options.attributeOldValue : type === 'characterData' && options.characterDataOldValue), at = found ? found.indexOf(observer) : -1;
        if (at < 0) (found ??= []).push(observer, old); else if (old) found[at + 1] = true;
      }
    }
    if (!found) return;
    for (let i = 0; i < found.length; i += 2) found[i].records.push({ addedNodes: [], removedNodes: [], previousSibling: null, nextSibling: null, attributeName: null, attributeNamespace: null, ...record, oldValue: found[i + 1] ? record.oldValue ?? null : null });
    if (!mutationQueued) {
      mutationQueued = true;
      queueMicrotask(() => { mutationQueued = false; for (const ref of observers) { const observer = ref.deref(); if (!observer) { observers.delete(ref); continue; } const records = observer.takeRecords(); if (records.length) try { observer.callback(records, observer); } catch (error) { report(error); } } });
    }
  };
  let reporting = false;
  // An exception in an `error` listener must not report itself again.
  const report = error => {
    if (reporting) return console.error(error);
    reporting = true;
    let message; try { message = 'Uncaught ' + error; } catch { message = 'Uncaught exception'; }
    try { const event = new ErrorEvent('error', { cancelable: true, error, message, filename: error?.sourceURL, lineno: error?.line, colno: error?.column }); dispatch(globalThis, event); if (!event.defaultPrevented) { console.error(error); escalate?.(event); } } finally { reporting = false; }
  };
  globalThis.reportError = report;
  // In a page worker, an error that the worker does not handle goes on to its `Worker` object.
  const escalate = globalThis.__appsReport?.(report); delete globalThis.__appsReport;
  // Before the first frame, the page has no boxes until a layout runs.
  const laidOut = () => { layout(); if (!geometry[2] && flagged !== version) { flagged = version; call('dom.layout', false, true); layout(); } };
  const hitAt = (x, y) => { laidOut(); return wrap(call('dom.hitTest', x, y, true)); };
  const rootOf = (root, node) => { for (let up = node.getRootNode?.(); up; up = up.host?.getRootNode()) if (up === root) return true; return false; };
  const hidden = (node, from) => { const root = node.getRootNode?.(); return root instanceof ShadowRoot && !rootOf(root, from) && (root.mode === 'closed' || hidden(root.host, from)); };
  const within = (node, root) => { while (node && node.getRootNode() !== root) node = node.getRootNode().host ?? null; return node ?? null; };
  const retarget = (node, against) => { for (let root = node.getRootNode?.(); root instanceof ShadowRoot && !rootOf(root, against); root = node.getRootNode()) node = root.host; return node; };
  const dispatch = (target, event, ancestors = path(target)) => {
    const state = eventState(event);
    if (state.dispatching) throw new DOMException('Event is already in dispatch', 'InvalidStateError');
    state.trusted ??= true;
    const shadow = shadows > 0 && ancestors.some(node => node instanceof ShadowRoot);
    if (shadow && !event.composed) { const root = target.getRootNode(); if (root instanceof ShadowRoot) ancestors.length = ancestors.indexOf(root) + 1; }
    if (ancestors.at(-1) === document) ancestors.push(window);
    let seen = target;
    const targets = shadow ? ancestors.map(node => { const value = seen; if (node instanceof ShadowRoot && seen.getRootNode() === node) seen = node.host; return value; }) : null;
    const related = shadows && event.relatedTarget instanceof Node ? event.relatedTarget : null, relatedTargets = related && ancestors.map(node => retarget(related, node));
    if (related) {
      if (relatedTargets[0] === target && target !== related) return true;
      const end = ancestors.findIndex((node, index) => index && (targets?.[index] ?? target) === node && relatedTargets[index] === node);
      if (end > 0) ancestors.length = end;
    }
    Object.assign(state, { target, path: ancestors, dispatching: true, hidden: shadow ? hidden : null });
    const invoke = (index, capture) => {
      const node = ancestors[index], listeners = events.get(node), list = listeners?.get(event.type);
      if (!list?.length) return;
      state.currentTarget = node; state.target = targets?.[index] ?? target; state.eventPhase = state.target === node ? 2 : capture ? 1 : 3;
      if (related) event.relatedTarget = relatedTargets[index];
      // Removal replaces the list and adding appends to it, so the first `length` items are the snapshot.
      for (let i = 0, length = list.length; i < length && !state.immediate; i++) {
        const item = list[i], current = listeners.get(event.type);
        if (item.capture !== capture || current !== list && !current.includes(item)) continue;
        if (item.once) node.removeEventListener(event.type, item.fn, item.capture);
        state.passive = item.passive;
        try { if (typeof item.fn === 'function') item.fn.call(node, event); else item.fn.handleEvent(event); } catch (error) { report(error); }
        state.passive = false;
      }
    };
    const outer = globalThis.event; globalThis.event = event;
    try {
      for (let index = ancestors.length; index-- && !state.stopped;) invoke(index, true);
      for (let index = 0; index < ancestors.length && !state.stopped; index++) if (event.bubbles || (targets?.[index] ?? target) === ancestors[index]) invoke(index, false);
    } finally { globalThis.event = outer; Object.assign(state, { target: shadow && target.getRootNode() instanceof ShadowRoot ? null : target, currentTarget: null, eventPhase: 0, dispatching: false, stopped: false, immediate: false, passive: false }); if (related) event.relatedTarget = related.getRootNode() instanceof ShadowRoot ? null : related; }
    return !event.defaultPrevented;
  };
  const trusted = event => { eventState(event).trusted = true; if (event.view === null) event.view = window; return event; };
  const labelable = rec => /^(button|meter|output|progress|select|textarea)$/.test(rec.tag) || rec.tag === 'input' && rec.attrs.get('type')?.toLowerCase() !== 'hidden';
  const labelControl = label => { const id = label.getAttribute('for'), control = id === null ? find(label._m, labelable) : id && label.getRootNode().getElementById?.(id); return control && labelable(control._m) ? control : null; };
  const disabled = (node, top) => {
    if (!/^(button|input|select|textarea|optgroup|option|fieldset)$/.test(node.localName) && !node._face) return false;
    for (let rec = node._m, child = null; rec && rec.tag[0] !== '#'; child = rec, rec = rec === top ? null : rec.up) if (rec.attrs.has('disabled') && (rec === node._m || rec.tag === 'fieldset' && rec.kids.find(kid => kid.tag === 'legend') !== child || rec.tag === 'optgroup' && node.localName === 'option')) return true;
    return false;
  };
  const activation = node => { const tag = node.localName; return tag === 'button' || tag === 'label' || tag === 'input' && /^(checkbox|radio|submit|reset|image|button|file)$/.test(node.type) || (tag === 'a' || tag === 'area') && node._m.attrs.has('href') || tag === 'summary' && node._m.up?.tag === 'details' && node._m.up.kids.find(kid => kid.tag === 'summary') === node._m; };
  // Keep the scope of `root.querySelectorAll('input[name=...]')`: no template contents, no shadow trees.
  const group = radio => {
    const name = radio.name, form = radio.form, root = radio.getRootNode();
    if (!name || root === radio) return [radio];
    const result = [], stack = [];
    for (let i = root._m.kids.length; i--;) stack.push(root._m.kids[i]);
    while (stack.length) {
      const rec = stack.pop();
      if (rec.tag === 'input' && rec.attrs.get('name') === name) { const other = of(rec); if (other.type === 'radio' && other.form === form) result.push(other); }
      if (rec.tag !== 'template') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]);
    }
    return result;
  };
  // Like Chrome, a box that the user can scroll takes the focus from scripts and the keyboard, not from a press.
  const overflows = id => marks[id] & 2 && 4 + id * 8 < limit && (geometry[10 + id * 8] > 0 || geometry[11 + id * 8] > 0);
  const scroller = node => {
    const id = node._id, tag = node._m.tag;
    if (!overflows(id) || tag === 'html' || tag === 'body') return false;
    const style = getComputedStyle(node);
    return geometry[10 + id * 8] > 0 && /auto|scroll/.test(style.overflowX) || geometry[11 + id * 8] > 0 && /auto|scroll/.test(style.overflowY);
  };
  const focusable = (node, press) => {
    if (node?.nodeType !== 1 || !node.isConnected || disabled(node)) return false;
    laidOut();
    // As in Chrome, the contents that a `content-visibility: auto` box skips can take the focus, which shows them.
    if (marks[node._id] & 64) return false;
    const skipped = call('dom.skipped', node._id, false);
    if (skipped === 2 || skipped < 0 || marks[node._id] & 16 && skipped !== 1) return false;
    const rec = node._m, tag = rec.tag;
    return /^\s*[+-]?\d/.test(rec.attrs.get('tabindex') ?? '') || node.isContentEditable || /^(select|textarea|button|iframe|dialog|terminal)$/.test(tag) || tag === 'input' && node.type !== 'hidden' || (tag === 'a' || tag === 'area') && rec.attrs.has('href') || tag === 'summary' && activation(node) || !press && scroller(node);
  };
  // `:user-valid` and `:user-invalid` match with bit 256, which a change by the user and a submit of the form set.
  const interacted = node => formState(node, node._bits | 256);
  const edited = node => { if (!node._edited) { node._edited = true; node._before = node.value; } };
  const commit = node => { if (node._edited) { node._edited = false; interacted(node); if (node.value !== node._before) dispatch(node, trusted(new Event('change', { bubbles: true }))); } };
  const selectors = (name, id, selector, first) => { try { return call(name, id, String(selector), first); } catch (error) { if (/^SyntaxError/.test(error?.message)) throw new DOMException(error.message.replace(/^SyntaxError: /, ''), 'SyntaxError'); throw error; } };
  const toggles = new Map();
  const validated = /^(required|pattern|min|max|step|maxlength|minlength|type|multiple|size|value|name|selected|form)$/;
  const attributeChanged = (element, name, oldValue, value) => {
    if (name === 'class') { if (readers && classReaders.size) noteClasses(element._m, oldValue, value); return; }
    if (readers && readAttrs.test(name)) { if (name === 'id' || name === 'name') nameShape++; wake(attrReaders.get(name === 'id' && element._m.tag === 'form' ? 'form id' : name), element._m); }
    if (faces && name === 'id' && element._m.tag === 'form') for (const other of all(element.getRootNode()._m, rec => { const form = rec.attrs.get('form'); return !!form && (form === oldValue || form === value) && rec.tag.includes('-'); })) if (other._face) faceChanged(other);
    // As in Chrome, a type that gains the selection API starts with the caret at the start.
    if (name === 'type' && element.localName === 'input' && element._files && element.type !== 'file') element._files = null;
    if (name === 'type' && element.localName === 'input' && selectable(element)) { const old = (oldValue ?? '').toLowerCase(); if (!/^(text|search|tel|url|password)$/.test(inputTypes.has(old) ? old : 'text')) { element._start = element._end = 0; element._direction = 'none'; if (active === element) call('dom.selection', element._id, 0, 0); } }
    if (shadows && name === 'slot') { signal(element.parentNode, element, oldValue ?? ''); signal(element.parentNode, element); }
    else if (shadows && name === 'name' && element.localName === 'slot') { const root = element.getRootNode(); if (root instanceof ShadowRoot && (slotted(root.host, oldValue ?? '') || slotted(root.host, value ?? ''))) queueSlot(element); }
    else if (name === 'popover' && element._popover && popoverState(oldValue) !== popoverState(value)) hidePopover(element, true, true);
    else if (name === 'open' && element.localName === 'dialog' && value === null) closeDialog(element);
    else if (name === 'checked' && element.localName === 'input' && !element._dirty && (oldValue === null) !== (value === null)) setChecked(element, value !== null);
    else if (name === 'disabled') { if (faces) if (element._face) faceChanged(element); else if (element.localName === 'fieldset') reactions(element._m, 'formDisabledCallback'); }
    else if (validated.test(name)) {
      if (name === 'form' && element._face) faceChanged(element);
      revalidate(element.localName === 'option' ? element.select : element);
      if ((name === 'name' || name === 'form') && element.localName === 'input' && element.type === 'radio') { const forms = new Set(); for (const other of element.getRootNode().querySelectorAll?.('input[name=' + quote(name === 'name' ? oldValue ?? '' : element.name) + ']') ?? []) if (other !== element && other.type === 'radio' && !forms.has(other.form)) { forms.add(other.form); revalidate(other); } }
    }
    else if (name === 'open' && element.localName === 'details' && (oldValue === null) !== (value === null)) {
      const pending = toggles.get(element), newState = value === null ? 'closed' : 'open';
      if (pending) pending.newState = newState;
      else { toggles.set(element, { oldState: value === null ? 'open' : 'closed', newState }); setTimeout(() => { const state = toggles.get(element); toggles.delete(element); dispatch(element, new ToggleEvent('toggle', state)); }, 0); }
    }
  };
  const fireClick = (target, init = {}, native = false) => {
    const event = new PointerEvent('click', { bubbles: true, cancelable: true, composed: true, view: window, pointerId: native ? 1 : -1, pointerType: native ? 'mouse' : '', ...init });
    eventState(event).trusted = native;
    return activateClick(target, event, init, native);
  };
  // Every `click` MouseEvent runs this, also from `dispatchEvent`, and Base UI's switches and checkboxes rely on that.
  const activateClick = (target, event, init = {}, native = false) => {
    const activator = path(target).find(node => node.nodeType === 1 && activation(node)), kind = activator?.localName === 'input' ? activator.type : '';
    if (activator && disabled(activator)) return dispatch(target, event);
    const checked = node => node._checked ?? node.hasAttribute('checked');
    const previous = kind === 'checkbox' ? checked(activator) : kind === 'radio' ? group(activator).find(checked) ?? null : undefined, indeterminate = activator?.indeterminate;
    if (kind === 'checkbox') { setChecked(activator, !previous); activator.indeterminate = false; } else if (kind === 'radio') setChecked(activator, true);
    if (kind === 'checkbox' || kind === 'radio') activator._dirty = true;
    const allowed = dispatch(target, event);
    if (kind === 'checkbox' || kind === 'radio') {
      if (!allowed) { if (kind === 'checkbox') { setChecked(activator, previous); activator.indeterminate = indeterminate; } else if (previous) setChecked(previous, true); else setChecked(activator, false); }
      else if (kind === 'checkbox' || previous !== activator) { dispatch(activator, new Event('input', { bubbles: true, composed: true })); if (native) interacted(activator); dispatch(activator, new Event('change', { bubbles: true })); }
    } else if (allowed && activator) {
      const tag = activator.localName, type = tag === 'button' ? activator.type : kind;
      if (tag === 'label') { const control = labelControl(activator), list = path(target); let inner = false; for (let i = 0; list[i] !== activator && !inner; i++) { const node = list[i], name = node.localName; inner = /^(button|details|embed|iframe|label|select|textarea)$/.test(name) || name === 'input' && node.type !== 'hidden' || (name === 'audio' || name === 'video') && node.hasAttribute('controls') || name === 'img' && node.hasAttribute('usemap'); } if (control && !inner && !control.contains(target) && !disabled(control)) { if (focusable(control)) { if (native && control !== active) { mouseFocus = true; hadKey = false; } control.focus(); } fireClick(control, init, native); } }
      else if (tag === 'summary') activator.parentElement.open = !activator.parentElement.open;
      else if ((type === 'submit' || type === 'image') && activator.form) activator.form.requestSubmit(activator);
      else if (type === 'file') pickFiles(activator);
      else if (tag === 'button' || tag === 'input') { if (type === 'reset' && activator.form) activator.form.reset(); invoke(activator, target); }
      else if ((tag === 'a' || tag === 'area') && !/^(_self|_top|_parent)?$/i.test(activator.getAttribute('target') ?? '')) openWindow(activator.href, activator.getAttribute('target'));
      // Pages can only go to app pages and local files, so links to other URLs do nothing.
      else if ((tag === 'a' || tag === 'area') && !activator.hasAttribute('download') && /^(apps|file):$/.test(activator.protocol)) location.assign(activator.href);
    }
    return allowed;
  };
  const rawText = /^(style|script|xmp|iframe|noembed|noframes|plaintext|noscript)$/, voids = /^(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)$/, escapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', '\u00a0': '&nbsp;' };
  const textEscape = /[&<>\u00a0]/, attrEscape = /[&"<>\u00a0]/;
  // Tags whose copies need their wrapper: form state, media, frames, templates, slots and attribute side effects.
  const copied = /^(input|select|textarea|option|optgroup|button|fieldset|output|form|label|details|dialog|slot|template|img|video|audio|canvas|iframe|webview|terminal)$/;
  const integration = /^(foreignObject|desc|title)$/, inSvg = rec => !!rec && rec.tag[0] !== '#' && of(rec) instanceof SVGElement && !integration.test(rec.tag);
  // Use a stack, not recursion: deep trees overflow the JavaScript stack.
  const serialize = (root, inner = false) => {
    const stack = inner ? [] : [root, inSvg(root.up), -1];
    if (inner) for (let i = root.kids.length, svg = inSvg(root); i--;) stack.push(root.kids[i], svg, -1);
    let out = '';
    while (stack.length) {
      const start = stack.pop(), svg = stack.pop(), rec = stack.pop(), tag = rec.tag;
      if (start >= 0) { if (out.length === start) out += own(rec); out += '</' + tag + '>'; continue; }
      if (tag === '#text') { const text = own(rec); out += !svg && rec.up && rawText.test(rec.up.tag) || !textEscape.test(text) ? text : text.replace(/[&<>\u00a0]/g, c => escapes[c]); continue; }
      if (tag === '#comment') { out += '<!--' + rec.text + '-->'; continue; }
      if (tag === '#doctype') { out += '<!DOCTYPE ' + own(rec).split('\0')[0] + '>'; continue; }
      if (tag[0] !== '#') {
        let attrs = '';
        for (let i = 0, list = rec.attrs.list; i < list.length; i += 2) { const value = list[i + 1]; attrs += ' ' + list[i] + '="' + (attrEscape.test(value) ? value.replace(/[&"<>\u00a0]/g, c => escapes[c]) : value) + '"'; }
        out += '<' + tag + attrs + '>';
        if (voids.test(tag)) continue;
        if (tag === 'template') { out += of(rec).innerHTML + '</template>'; continue; }
        stack.push(rec, svg, out.length);
      }
      for (let i = rec.kids.length, content = (svg || tag === 'svg') && !integration.test(tag); i--;) stack.push(rec.kids[i], content, -1);
    }
    return out;
  };
  const reactionQueue = [];
  let reactionDepth = 0, reactionStart = 0;
  const enqueue = (node, reaction) => { (node._reactions ??= []).push(reaction); reactionQueue.push(node); if (!reactionDepth) { reactionDepth++; leave(); } };
  const leave = () => {
    if (--reactionDepth || reactionQueue.length <= reactionStart) return;
    const start = reactionStart;
    for (let i = start; i < reactionQueue.length; i++) {
      const node = reactionQueue[i], list = node._reactions;
      while (list.length) {
        const reaction = list.shift();
        reactionStart = reactionQueue.length;
        try { if (typeof reaction === 'function') upgrade(node, reaction); else node[reaction[0]]?.(...reaction.slice(1)); } catch (error) { report(error); }
      }
    }
    reactionQueue.length = reactionStart = start;
  };
  const scoped = fn => function (...args) { reactionDepth++; try { return fn.apply(this, args); } finally { leave(); } };
  const reactions = (root, name, only) => {
    const stack = [root], leaving = name === 'disconnectedCallback';
    while (stack.length) {
      const rec = stack.pop(), Type = (!only || rec.tag === only) && definitions.get(rec.tag), node = Type && (leaving ? rec.ref?.deref() : of(rec));
      if (node && !node._failed) {
        if (!(node instanceof Type)) { if (!leaving) enqueue(node, Type); }
        else if (name === 'formDisabledCallback') { if (node._face) faceChanged(node); }
        else if (name === 'connectedMoveCallback' && typeof node[name] !== 'function') { enqueue(node, ['disconnectedCallback']); enqueue(node, ['connectedCallback']); }
        else {
          if (name && typeof node[name] === 'function') enqueue(node, [name]);
          if (faces && node._face) faceChanged(node, leaving ? root : null);
        }
      }
      if (rec.tag !== 'template') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]);
      if (rec.shadow) stack.push(rec.shadow);
    }
  };
  const upgrade = (node, Type) => {
    if (node instanceof Type || node._failed) return;
    constructing = { node };
    try { new Type(); } catch (error) { node._failed = true; node._reactions.length = 0; throw error; } finally { constructing = null; }
    flag(node, 64, true);
    if (Type.formAssociated) { faces++; node._face = true; flag(node, 512, true); }
    const observed = defined.get(Type)?.observed;
    if (observed?.size) for (let i = 0, list = node._m.attrs.list; i < list.length; i += 2) if (observed.has(list[i])) node._reactions.push(['attributeChangedCallback', list[i], null, list[i + 1]]);
    if (node.isConnected) node._reactions.push(['connectedCallback']);
    if (node._face) faceChanged(node);
  };
  // Form-associated custom elements: tell one when its form or its disabled state changes.
  let faces = 0;
  // A node that leaves its place with `top` has only the ancestors up to `top`.
  const faceChanged = (node, top) => {
    const form = formOf(node, top), off = disabled(node, top);
    if (form !== (node._form ?? null)) { node._form = form; enqueue(node, ['formAssociatedCallback', form]); }
    if (off !== !!node._off) { node._off = off; enqueue(node, ['formDisabledCallback', off]); }
  };
  const slotChanges = new Set();
  const queueSlot = slot => { if (!slotChanges.size) queueMicrotask(() => { const slots = [...slotChanges]; slotChanges.clear(); for (const slot of slots) dispatch(slot, new Event('slotchange', { bubbles: true })); }); slotChanges.add(slot); };
  const slotted = (host, name) => host._m.kids.some(kid => (kid.attrs?.get('slot') ?? '') === name && (kid.tag[0] !== '#' || kid.tag === '#text'));
  const signal = (host, child, name = child._m.attrs?.get('slot') ?? '') => {
    if (!host?._shadow || child._m.tag[0] === '#' && child._m.tag !== '#text') return;
    const slot = find(host._shadow._m, slot => slot.tag === 'slot' && (slot.attrs.get('name') ?? '') === name);
    if (slot) queueSlot(slot);
  };
  const slotsIn = (root, rec, self = false) => {
    if (!(root instanceof ShadowRoot) || !root.host._m.kids.length) return;
    const test = slot => { if (slot.tag === 'slot' && slotted(root.host, slot.attrs.get('name') ?? '')) queueSlot(of(slot)); return false; };
    if (self) test(rec);
    find(rec, test);
  };
  // Visit only ranges at affected nodes: garbage ranges remain until GC.
  const anchors = new Map(), iterators = new Set();
  let rangeSync = false;
  const place = (range, start, node, offset) => {
    const old = start ? range._sc : range._ec;
    if (start) { range._sc = node; range._so = offset; } else { range._ec = node; range._eo = offset; }
    if (range._lit) { for (const highlight of range._lit) highlight._dirty.add(range); queueHighlights(); }
    if (old === node) return;
    const set = old !== range._sc && old !== range._ec && anchors.get(old._m);
    if (set) { set.delete(range._ref); if (!set.size) anchors.delete(old._m); }
    let own = anchors.get(node._m); if (!own) anchors.set(node._m, own = new Set()); own.add(range._ref);
  };
  const each = (rec, fn) => {
    const set = anchors.get(rec);
    if (!set) return;
    for (const ref of set) {
      const range = ref.deref();
      if (!range) { set.delete(ref); continue; }
      if (fn(range) && selection?.ranges[0] === range && !rangeSync) { rangeSync = true; queueMicrotask(() => { rangeSync = false; if (selection.ranges[0]?.startContainer.isConnected) { selection._x = undefined; selection._send(true); selectionChanged(); } }); }
    }
    if (!set.size) anchors.delete(rec);
  };
  const inside = (root, fn) => {
    if (anchors.size <= 32) { for (const rec of anchors.keys()) { let up = rec; while (up && up !== root) up = up.up; if (up) each(rec, range => fn(range, rec)); } return; }
    const stack = [root];
    while (stack.length) { const rec = stack.pop(); if (anchors.has(rec)) each(rec, range => fn(range, rec)); for (const kid of rec.kids) stack.push(kid); }
  };
  const shift = (range, rec, test, node, to) => {
    let moved = false;
    if (range._sc._m === rec && test(range._so)) { place(range, true, node, to(range._so)); moved = true; }
    if (range._ec._m === rec && test(range._eo)) { place(range, false, node, to(range._eo)); moved = true; }
    return moved;
  };
  const always = () => true;
  const rangesRemove = (parent, child, index) => { if (iterators.size) iteratorsRemove(child); if (!anchors.size) return; inside(child._m, (range, rec) => shift(range, rec, always, parent, () => index)); each(parent._m, range => shift(range, parent._m, at => at > index, parent, at => at - 1)); };
  // `parent` held the removed tree, so the blur of a removed focused element still reaches the document, as in browsers.
  const forgetRemoved = (parent, next) => {
    if (topLayer.length || dialogs.length) {
      if (fullscreens.some(node => !node.isConnected)) exitFullscreen(true);
      for (const node of [...topLayer, ...dialogs]) if (!node.isConnected) { if (node._popover) hidePopover(node, false, false); if (dialogs.includes(node)) closeDialog(node); }
    }
    if (captures.size) for (const [id, node] of captures) if (!node.isConnected) captures.delete(id);
    if (gotten.size) for (const [id, node] of gotten) if (!node.isConnected) { gotten.delete(id); dispatch(document, trusted(new PointerEvent('lostpointercapture', { pointerId: id, pointerType: 'mouse', isPrimary: true, bubbles: true, composed: true }))); }
    if (parent && navStart && (!navStart[0].isConnected || navStart[1] && !navStart[1].isConnected)) navStart = [parent, next && of(next)];
    if (!active || active.isConnected) return;
    finishComposition();
    // As in Chrome, Tab goes on from the place of the removed element.
    const previous = active, ancestors = () => parent?.isConnected ? [...path(previous), ...path(parent)] : undefined; active = null; navStart = parent ? [parent, next && of(next)] : null; call('dom.focus', []); if (windowFocused) { dispatch(previous, new FocusEvent('blur', { view: window, composed: true }), ancestors()); dispatch(previous, new FocusEvent('focusout', { view: window, bubbles: true, composed: true }), ancestors()); }
  };
  const watched = node => { if (observers.size && !quiet) for (let own = node._m, rec = own; rec; rec = rec.up) { const set = rec.watched && rec.ref.deref()?._observers; if (set) for (const observer of set) { const options = observer.targets.get(rec.ref); if (options?.childList && (rec === own || options.subtree)) return true; } } return false; };
  const ignore = () => {};
  // Call removed after both trees detach the old children, before reinserted children run connected
  // callbacks.
  const replaceAll = scoped((node, replace) => {
    const rec = node._m;
    const observed = watched(node);
    if (!rec.kids.length && !observed && !shadows) { replace(ignore); if (requiredSelects) optionsChanged(rec); return; }
    // A wrapper keeps its removed subtree in the arena until a GC, so only an observer's record wraps the old children.
    const kids = definitions.size > 0 && node.isConnected ? rec.kids.slice() : [], old = observed ? rec.kids.map(of) : [];
    if (rec.kids.length) { if (node._shadow) for (const kid of rec.kids) signal(node, of(kid)); if (iterators.size) iteratorsRemove(node, true); if (anchors.size) inside(rec, (range, at) => shift(range, at, at === rec ? offset => offset > 0 : always, node, () => 0)); }
    let pending = kids.length > 0;
    const removed = () => { if (pending) { pending = false; for (const kid of kids) reactions(kid, 'disconnectedCallback'); } };
    replace(removed); removed();
    if (requiredSelects) optionsChanged(rec);
    forgetRemoved(node);
    if (shadows) { if (node._shadow) for (const kid of rec.kids) signal(node, of(kid)); slotsIn(node.getRootNode(), rec); }
    if (observed && (old.length || rec.kids.length)) change({ type: 'childList', target: node, removedNodes: listOf(NodeList, old), addedNodes: listOf(NodeList, rec.kids.map(of)) });
  });
  const convert = children => children.map(child => child instanceof Node ? child : document.createTextNode(String(child)));
  const viable = (node, nodes) => { while (node && nodes.includes(node)) node = node.nextSibling; return node; };
  const insertNodes = scoped((parent, nodes, before, replaced = null) => {
    if (parent.nodeType === 9 && replaced && !nodes.includes(replaced)) { parent.removeChild(replaced); replaced = null; }
    if (!observers.size || quiet || nodes.length < 2 && !replaced) {
      for (const node of nodes) parent.insertBefore(node, before);
      if (replaced && !nodes.includes(replaced) && replaced.parentNode === parent) parent.removeChild(replaced);
      return;
    }
    nodes = nodes.flatMap(node => node.nodeType === 11 ? node._m.kids.map(of) : [node]);
    for (const node of nodes) node.parentNode?.removeChild(node);
    const removed = replaced?.parentNode === parent ? [replaced] : [], previousSibling = removed.length ? replaced.previousSibling : before ? before.previousSibling : parent.lastChild;
    quiet++;
    try { for (const node of nodes) parent.insertBefore(node, before); if (removed.length) parent.removeChild(replaced); } finally { quiet--; }
    if (nodes.length || removed.length) change({ type: 'childList', target: parent, addedNodes: listOf(NodeList, nodes), removedNodes: listOf(NodeList, removed), previousSibling, nextSibling: before });
  });
  const contextTag = node => !node ? 'body' : node.namespaceURI === SVG ? integration.test(node.localName) ? 'body' : 'svg' : node.localName === 'svg' ? 'body' : node.localName;
  const parseFragment = scoped((source, context) => {
    const fragment = document.createDocumentFragment(), tag = contextTag(context);
    if (tag === 'svg') fragment._svg = true;
    if (load(json(call('dom.html', fragment._id, String(source), tag)))) __appsImages(fragment);
    if (definitions.size) reactions(fragment._m);
    return fragment;
  });
  const setHTML = (node, value, inert = false) => replaceAll(node, removed => {
    const list = json(node.nodeType === 1 ? call('dom.html', node._id, String(value), contextTag(node)) : call('dom.html', node._id, String(value)));
    release(node); detach(node._m); const media = load(list); relist(node._m); removed();
    if (!inert) { if (media) __appsImages(node); if (definitions.size) for (const kid of node._m.kids) reactions(kid); }
  });
  class List extends Array { static get [Symbol.species]() { return Array; } item(index) { return this[index] ?? null; } toString() { return Object.prototype.toString.call(this); } }
  class NodeList extends List {}
  // Lists give these to the runtime's own code, as a property of the page can hide a method.
  const raw = Symbol(), lookup = Symbol(), owner = Symbol();
  class HTMLCollection extends List { namedItem(name) { if (name === '') return null; for (const element of this[raw] ?? this) if (element._m.attrs.get('id') === name || element.namespaceURI === XHTML && element._m.attrs.get('name') === name) return element; return null; } }
  // Other live lists are proxies that look for their nodes again after a change of what they read: elements with some
  // tags or classes below a root, and some attributes. Only lists that are up to date wait for changes, by what they
  // read and by their root, or under null without one.
  let readers = 0, readersLimit = 1024, nameShape = 0;
  const tagReaders = new Map(), classReaders = new Map(), attrReaders = new Map(), readAttrs = /^(id|name|type|form|for|href)$/;
  // Lists of elements by tag or class only lose `gone`, an element that leaves the tree.
  const wakeSet = (roots, root, gone) => {
    const set = roots.get(root);
    if (!set) return;
    readers -= set.size;
    for (const ref of set) {
      const reader = ref.deref();
      if (reader && gone && reader.reads.drops && !reader.stale) { reader.drop(gone); continue; }
      if (reader) reader.stale = true;
      set.delete(ref);
    }
    if (set.size) readers += set.size; else roots.delete(root);
  };
  const wake = (roots, at, gone) => {
    if (!roots?.size) return;
    wakeSet(roots, null, gone);
    for (let rec = at; rec && roots.size; rec = rec.up) wakeSet(roots, rec, gone);
  };
  const wakeAll = () => { for (const map of [tagReaders, classReaders]) for (const roots of map.values()) for (const root of [...roots.keys()]) wakeSet(roots, root); };
  const noteClasses = (rec, old, value) => {
    const before = old ? old.split(/[\t\n\f\r ]+/) : [], after = value ? value.split(/[\t\n\f\r ]+/) : [];
    for (const name of before) if (classReaders.has(name) && !after.includes(name)) wake(classReaders.get(name), rec);
    for (const name of after) if (classReaders.has(name) && !before.includes(name)) wake(classReaders.get(name), rec);
  };
  const noteNode = (rec, parent, leaves) => {
    const gone = leaves ? rec : null;
    if (!parent || rec.tag[0] === '#') return;
    wake(tagReaders.get(rec.tag), parent, gone); wake(tagReaders.get('*'), parent, gone);
    if (rec.tag.includes('-')) wake(tagReaders.get('-'), parent);
    if (classReaders.size) { const value = rec.attrs.get('class'); if (value) for (const name of value.split(/[\t\n\f\r ]+/)) wake(classReaders.get(name), parent, gone); }
  };
  // A node that joins or leaves `parent`. A big subtree counts as a change of everything.
  const noteTree = (kid, parent, leaves = false) => {
    if (!kid.kids.length) return noteNode(kid, parent, leaves);
    const stack = [kid];
    for (let count = 0; stack.length; count++) {
      if (count === 64) return wakeAll();
      const rec = stack.pop();
      noteNode(rec, parent, leaves);
      for (const kid of rec.kids) stack.push(kid);
    }
  };
  const reserved = new Map();
  class Live {
    constructor(read, reads, named) { this.read = read; this.reads = reads; this.version = reads ? -1 : 0; this.ref = this.list = null; this.named = named; this.stale = true; this.names = null; this.namesAt = 0; }
    drop(rec) { const list = this.list, last = list.length - 1; if (last < 0) return; const at = list[0]._m === rec ? 0 : list[last]._m === rec ? last : list.findIndex(node => node._m === rec); if (at >= 0) { if (at === last) list.length = last; else list.splice(at, 1); this.names = null; } }
    sync(list) {
      if (this.version >= 0 ? this.version !== version || this.stale : this.stale) {
        if (this.version >= 0) this.version = version;
        this.stale = false; this.names = null; this.list = list;
        if (this.reads) waitFor(this, this.reads);
        const found = this.read(); list.length = found.length;
        for (let i = 0; i < found.length; i++) if (list[i] !== found[i]) list[i] = found[i];
      }
      return list;
    }
    // The elements by `id` and `name`. Like the properties of the browser's lists, they hide nothing but array methods.
    byName(list, key, hidden = false) {
      const code = typeof key === 'string' && this.named ? key.charCodeAt(0) : 48;
      if (code >= 48 && code <= 57) return undefined;
      if (!hidden) {
        let own = reserved.get(Object.getPrototypeOf(list));
        if (!own) {
          own = new Set(Object.getOwnPropertyNames(Object.prototype).concat('length'));
          for (let type = Object.getPrototypeOf(list); type !== Array.prototype; type = Object.getPrototypeOf(type)) for (const name of Object.getOwnPropertyNames(type)) own.add(name);
          reserved.set(Object.getPrototypeOf(list), own);
        }
        if (own.has(key)) return undefined;
      }
      if (!this.names || this.namesAt !== nameShape) {
        this.names = new Map(); this.namesAt = nameShape;
        for (const node of list) for (const name of new Set([node._m.attrs.get('id'), node.namespaceURI === XHTML ? node._m.attrs.get('name') : ''])) if (name) { const found = this.names.get(name); if (found) found.push(node); else this.names.set(name, [node]); }
      }
      const found = this.names.get(key);
      return found && (found.length > 1 ? this.named(key, found) : found[0]);
    }
    get(list, key) {
      if (this.version >= 0 || this.stale) this.sync(list);
      if (typeof key === 'string') { const code = key.charCodeAt(0); if (code >= 48 && code <= 57 || key === 'length') return list[key]; return this.byName(list, key) ?? list[key]; }
      if (key === raw) return list; if (key === lookup) return name => this.byName(list, name, true);
      return list[key];
    }
    has(list, key) { this.sync(list); return key in list || this.byName(list, key) !== undefined; }
    set(list, key, value) { list[key] = value; return true; }
    ownKeys(list) { this.byName(this.sync(list), 'x'); return [...new Set(Reflect.ownKeys(list).filter(key => key !== owner).concat(this.names ? [...this.names.keys()].filter(key => this.byName(list, key)) : []))]; }
    getOwnPropertyDescriptor(list, key) { this.sync(list); const own = Reflect.getOwnPropertyDescriptor(list, key); if (own) return own; const value = this.byName(list, key); return value && { value, configurable: true, enumerable: false, writable: false }; }
  }
  const collections = new WeakMap(), first = (_, found) => found[0];
  const waitFor = (reader, { root = null, tags = [], classes = [], attrs = [] }) => {
    const ref = reader.ref ??= new WeakRef(reader);
    for (const [map, keys] of [[tagReaders, tags], [classReaders, classes], [attrReaders, attrs]]) for (const key of keys) {
      let roots = map.get(key); if (!roots) map.set(key, roots = new Map());
      let set = roots.get(root); if (!set) roots.set(root, set = new Set());
      readers -= set.size; set.add(ref); readers += set.size;
    }
    if (readers <= readersLimit) return;
    readers = 0;
    for (const map of [tagReaders, classReaders, attrReaders]) for (const [key, roots] of map) {
      for (const [root, set] of roots) { for (const ref of set) if (!ref.deref()) set.delete(ref); readers += set.size; if (!set.size) roots.delete(root); }
      if (!roots.size) map.delete(key);
    }
    readersLimit = Math.max(1024, readers * 2);
  };
  // The live list `key` of a node, the same one while the page holds it. It reads everything without `reads`.
  const live = (node, key, Type, read, reads, named = first, handler = Live) => {
    let lists = collections.get(node);
    if (!lists) collections.set(node, lists = new Map());
    let list = lists.get(key)?.deref();
    if (!list) {
      if (lists.size >= (lists.limit ?? 32)) { for (const [key, ref] of lists) if (!ref.deref()) lists.delete(key); lists.limit = Math.max(32, lists.size * 2); }
      const items = listOf(Type, []), reader = new handler(read, reads, named);
      items[owner] = node; list = new Proxy(items, reader); lists.set(key, new WeakRef(list));
    }
    return list;
  };
  const all = (root, test) => { const found = []; find(root, rec => { if (rec.tag[0] !== '#' && test(rec)) found.push(of(rec)); return false; }); return found; };
  class NamedNodeMap extends List { getNamedItem(name) { return this.find(attr => attr.name === name) ?? null; } }
  class DOMRectList extends List {}
  const badAttr = name => { if (!name || /[\0\t\n\f\r />=]/.test(name)) throw new DOMException(`'${name}' is not a valid attribute name`, 'InvalidCharacterError'); return name; };
  const attrNode = (element, name) => { const nodes = element._attrs ??= new Map(); let attr = nodes.get(name); if (attr?.ownerElement !== element) nodes.set(name, attr = new Attr(element, name, element._m.attrs.get(name))); return attr; };
  const token = value => { value = String(value); if (!value) throw new DOMException('Token is empty', 'SyntaxError'); if (/[\t\n\f\r ]/.test(value)) throw new DOMException('Token contains whitespace', 'InvalidCharacterError'); return value; };
  const tokens = list => { const value = list.value; if (value !== list._source) { list._source = value; list._tokens = [...new Set(value.split(/[\t\n\f\r ]+/).filter(Boolean))]; } return list._tokens; };
  // Keep the parsed tokens of an own write, so the next read does not split the value again.
  const setTokens = (list, next) => { const value = next.join(' '); list.value = value; list._source = value; list._tokens = next; };
  const tokenIndex = key => { if (typeof key !== 'string') return -1; const i = +key; return i >>> 0 === i && String(i) === key ? i : -1; };
  const indexed = {
    get(list, key, receiver) { const i = tokenIndex(key); return i < 0 ? Reflect.get(list, key, receiver) : tokens(list)[i]; },
    has(list, key) { const i = tokenIndex(key); return i < 0 ? Reflect.has(list, key) : i < tokens(list).length; },
  };
  // Resolve the list once per method to avoid repeated proxy traps.
  const rawLists = new WeakMap(), rawList = list => rawLists.get(list) ?? list;
  class DOMTokenList {
    constructor(element, name) { this._element = element; this._name = name; this._source = null; this._tokens = null; const proxy = new Proxy(this, indexed); rawLists.set(proxy, this); return proxy; }
    get value() { const list = rawList(this); return list._element.getAttribute(list._name) ?? ''; } set value(value) { const list = rawList(this); list._element.setAttribute(list._name, value); }
    get length() { return tokens(rawList(this)).length; }
    item(index) { return tokens(rawList(this))[index] ?? null; }
    contains(value) { return tokens(rawList(this)).includes(String(value)); }
    add(...values) { const list = rawList(this), next = tokens(list).slice(); for (const value of values.map(token)) if (!next.includes(value)) next.push(value); if (next.length || list._element.hasAttribute(list._name)) setTokens(list, next); }
    remove(...values) { const list = rawList(this); values = values.map(token); const next = tokens(list).filter(value => !values.includes(value)); if (next.length || list._element.hasAttribute(list._name)) setTokens(list, next); }
    toggle(value, force) { const list = rawList(this); value = token(value); const add = force === undefined ? !list.contains(value) : !!force; if (add) list.add(value); else list.remove(value); return add; }
    replace(old, value) { const list = rawList(this); old = token(old); value = token(value); const next = tokens(list).slice(), index = next.indexOf(old); if (index < 0) return false; next[index] = value; setTokens(list, [...new Set(next)]); return true; }
    supports() { if (rawList(this)._name !== 'rel') throw new TypeError('DOMTokenList has no supported tokens'); return true; }
    forEach(fn, thisArg) { tokens(rawList(this)).forEach((value, index) => fn.call(thisArg, value, index, this)); }
    keys() { return tokens(rawList(this)).keys(); } values() { return tokens(rawList(this)).values(); } entries() { return tokens(rawList(this)).entries(); } [Symbol.iterator]() { return this.values(); }
    toString() { return rawList(this).value; }
  }
  class Node {
    constructor(id, tag) { if (id === null) { this._m = new Record(null, tag, ''); this.nodeType = 2; return; } const rec = this._m = records[id]; this._id = id; this._nodes = this._elements = this._lists = this._held = null; this._parent = null; this._slot = 0; rec.ref = new WeakRef(this); wrapped.push(rec); registered.push(id); this.nodeType = types[tag] ?? 1; this.nodeName = names.get(tag) ?? (tag[0] === '#' ? tag === '#fragment' ? '#document-fragment' : tag : tag.toUpperCase()); }
    get ownerDocument() { if(this.nodeType===9)return null;const root=this.getRootNode();return root.nodeType===9?root:root._owner??document; }
    get parentNode() { return of(this._m.up); }
    get parentElement() { const up = this._m.up; return up && up.tag[0] !== '#' ? of(up) : null; }
    get assignedSlot() { const slot = assignedSlot(this); return slot?.getRootNode().mode === 'closed' ? null : slot; }
    get childNodes() { return this._nodes ??= liveList(this, 0) ?? listOf(NodeList, this._m.kids.map(of)); }
    get firstChild() { return of(this._m.kids[0]); }
    get lastChild() { const kids = this._m.kids; return of(kids[kids.length - 1]); }
    get nextSibling() { const rec = this._m; if (!rec.up) return null; const next = rec.up.kids[index(rec) + 1]; if (next) next.i = rec.i + 1; return of(next); }
    get previousSibling() { const rec = this._m; if (!rec.up) return null; const previous = rec.up.kids[index(rec) - 1]; if (previous) previous.i = rec.i - 1; return of(previous); }
    get isConnected() { let rec = this._m; while (rec.up ?? rec.host) rec = rec.up ?? rec.host; return rec.tag === '#document'; }
    get textContent() { return text(this._m); }
    set textContent(value) {
      value = String(value ?? '');
      if (this instanceof CharacterData) { this.data = value; return; }
      // Like Chrome, keep a text child with the same text. The standard replaces it, so observers still see that.
      const kids = this._m.kids;
      if (value ? kids.length === 1 && kids[0].tag === '#text' && own(kids[0]) === value && !watched(this) : !kids.length && !own(this._m)) return;
      replaceAll(this, () => { release(this); detach(this._m); op(1, this._id, ''); if (value) { const text = document.createTextNode(value); op(2, this._id, text._id, -1); text._m.up = this._m; text._m.i = 0; this._m.kids.push(text._m); hold(this, text); relist(this._m); } });
    }
    get nodeValue() { return this instanceof CharacterData ? this.textContent : null; }
    set nodeValue(value) { if (this instanceof CharacterData) this.textContent = value; }
    appendChild(child) { return this.insertBefore(child, null); }
    insertBefore(child, before) {
      if (!(child instanceof Node)) throw new TypeError('Expected a Node');
      before ??= null;
      if (child.nodeType === 11) {
        const rec = this._m, frag = child._m, ref = before?._m ?? null;
        if (this.nodeType !== 1 && this.nodeType !== 11 && this.nodeType !== 9) throw new DOMException('Invalid parent', 'HierarchyRequestError');
        if (ref && ref.up !== rec) throw new DOMException('Reference is not a child', 'NotFoundError');
        for (let up = rec; up; up = up.up ?? up.host) if (up === frag) throw new DOMException('Cannot create a DOM cycle', 'HierarchyRequestError');
        if (this.nodeType === 9) { const elements = frag.kids.filter(kid => kid.tag[0] !== '#').length; if (elements > 1 || frag.kids.some(kid => kid.tag === '#text') || elements && rec.kids.some(kid => kid !== ref && kid.tag[0] !== '#')) throw new DOMException('A document has one element and no text', 'HierarchyRequestError'); }
        const kids = frag.kids, observed = observers.size > 0 && !quiet;
        if (!kids.length) return child;
        // Take all children out at once, as the standard does, so each move doesn't shift the rest. Ranges and
        // iterators need each removal.
        const drain = !anchors.size && !iterators.size && !frag.host;
        // Move unwrapped children as records to avoid retaining removed trees. Keep SVG wrapping: wrap infers
        // namespaces from parents.
        if (drain && !observed && !(this._nodes || this._elements || this._lists) && !this._shadow && !child._svg && !this._svg && !(this instanceof SVGElement) && (this.nodeType === 1 || this.nodeType === 11 || this._id === 0) && kids.some(kid => !kid.ref?.deref() && kid.tag !== 'link')) {
          const connected = definitions.size > 0 && this.isConnected, root = shadows ? this.getRootNode() : null;
          frag.kids = []; for (const kid of kids) { kid.up = null; if (readers) noteTree(kid, frag); } relist(frag);
          // Insert each run before `ref` with one copy, not one splice per child.
          let run = [];
          const flush = () => {
            if (ref && run.length) { const at = index(ref), all = rec.kids; rec.kids = all.slice(0, at).concat(run, all.slice(at)); run.forEach((kid, i) => { kid.i = at + i; }); ref.i = at + run.length; }
            for (const kid of run) { if (connected) reactions(kid, 'connectedCallback'); if (root) slotsIn(root, kid, true); }
            run = [];
          };
          for (const kid of kids) {
            if (kid.ref?.deref() || kid.tag === 'link') { flush(); this.insertBefore(of(kid), before); continue; }
            op(2, this._id, kid.id, ref ? ref.id : -1);
            kid.up = rec; run.push(kid);
            if (!ref) kid.i = rec.kids.push(kid) - 1;
          }
          flush();
          if (requiredSelects) optionsChanged(rec);
          forgetRemoved();
          return child;
        }
        const nodes = kids.map(of);
        if (drain) { frag.kids = []; for (const kid of kids) { kid.up = null; if (readers) noteTree(kid, frag); } relist(frag); }
        quiet++;
        try { for (const node of nodes) this.insertBefore(node, before); } finally { quiet--; }
        if (observed) { change({ type: 'childList', target: child, removedNodes: listOf(NodeList, nodes) }); change({ type: 'childList', target: this, addedNodes: listOf(NodeList, nodes), previousSibling: nodes[0].previousSibling, nextSibling: before }); }
        return child;
      }
      const rec = this._m, kid = child._m, ref = before?._m ?? null;
      if (child.nodeType === 9 || child.nodeType === 2 || child.nodeType === 10 && this.nodeType !== 9 || this.nodeType !== 1 && this.nodeType !== 11 && this.nodeType !== 9) throw new DOMException(child.nodeType === 9 || child.nodeType === 10 ? 'Invalid child' : 'Invalid parent', 'HierarchyRequestError');
      if (ref && ref.up !== rec) throw new DOMException('Reference is not a child', 'NotFoundError');
      for (let up = rec; up; up = up.up ?? up.host) if (up === kid) throw new DOMException('Cannot create a DOM cycle', 'HierarchyRequestError');
      if (this.nodeType === 9 && (child.nodeType === 3 || child.nodeType === 1 && rec.kids.some(other => other !== kid && other !== ref && other.tag[0] !== '#'))) throw new DOMException('A document has one element and no text', 'HierarchyRequestError');
      const observed = observers.size > 0 && !quiet, connected = definitions.size > 0 && child.isConnected, parented = !!kid.up;
      let removal = null;
      if (kid.up && (observed || shadows || anchors.size || iterators.size)) {
        const from = of(kid.up);
        if (observed) removal = { type: 'childList', target: from, removedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling };
        if (from._shadow) signal(from, child);
        rangesRemove(from, child, index(kid));
      }
      // Keep the option a select shows by default, so a new option inserted before it doesn't take its place.
      if (ref && (rec.tag === 'select' || rec.tag === 'optgroup' && rec.up?.tag === 'select')) selectedness(rec.tag === 'select' ? this : of(rec.up));
      op(2, this._id, child._id, ref ? ref.id : -1);
      if (ref !== kid) {
        if (kid.up) { const from = kid.up.ref?.deref(), at = index(kid); kid.up.kids.splice(at, 1); if (from && (from._nodes || from._elements || from._lists)) listRemove(from, kid, at); }
        kid.up = rec; if (ref) rec.kids.splice(kid.i = index(ref), 0, kid); else kid.i = rec.kids.push(kid) - 1;
        if (this._nodes || this._elements || this._lists) listInsert(this, kid, kid.i);
      }
      hold(this, child);
      if (rec.tag === 'select' || rec.tag === 'optgroup') revalidate(rec.tag === 'select' ? this : this.parentElement); else if (requiredSelects) optionsChanged(rec);
      if (ref && anchors.size) { const at = index(kid); each(rec, range => shift(range, rec, offset => offset > at, this, offset => offset + 1)); }
      if (connected && moving && child.isConnected) reactions(kid, 'connectedMoveCallback');
      else { if (connected) reactions(kid, 'disconnectedCallback'); if (definitions.size && child.isConnected) reactions(kid, 'connectedCallback'); }
      if (shadows) { if (this._shadow) signal(this, child); slotsIn(this.getRootNode(), kid, true); }
      if (parented) forgetRemoved();
      if (removal) change(removal);
      if (observed) change({ type: 'childList', target: this, addedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling });
      if (kid.tag === 'link' && kid.attrs.has('href') && /(^|\s)(stylesheet|preload|modulepreload)(\s|$)/i.test(kid.attrs.get('rel') ?? '') && child.isConnected) setTimeout(() => dispatch(child, new Event('error')));
      return child;
    }
    removeChild(child) {
      if (!(child instanceof Node) || child._m.up !== this._m) throw new DOMException('Node is not a child', 'NotFoundError');
      const observed = observers.size > 0 && !quiet, connected = definitions.size > 0 && child.isConnected, kid = child._m, i = index(kid);
      const record = observed ? { type: 'childList', target: this, removedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling } : null;
      if (this._shadow) signal(this, child);
      rangesRemove(this, child, i);
      op(3, this._id, child._id); this._m.kids.splice(i, 1); kid.up = null; if (this._nodes || this._elements || this._lists) listRemove(this, kid, i); unhold(child); child._parent = null;
      if (this._m.tag === 'select' || this._m.tag === 'optgroup') revalidate(this._m.tag === 'select' ? this : this.parentElement); else if (requiredSelects) optionsChanged(this._m);
      if (connected) reactions(kid, 'disconnectedCallback');
      forgetRemoved(this, this._m.kids[i] ?? null);
      if (record) change(record);
      return child;
    }
    replaceChild(child, old) {
      if (!(old instanceof Node) || old._m.up !== this._m) throw new DOMException('Node is not a child', 'NotFoundError');
      if (child === old) return old;
      const observed = observers.size > 0 && !quiet, previousSibling = observed ? old.previousSibling : null, nextSibling = observed ? old.nextSibling === child ? child.nextSibling : old.nextSibling : null, addedNodes = observed ? listOf(NodeList, child.nodeType === 11 ? child._m.kids.map(of) : [child]) : null;
      quiet++;
      try { this.insertBefore(child, old); this.removeChild(old); } finally { quiet--; }
      if (observed) change({ type: 'childList', target: this, addedNodes, removedNodes: listOf(NodeList, [old]), previousSibling, nextSibling });
      return old;
    }
    before(...children) { const parent = this.parentNode; if (!parent) return; const nodes = convert(children); let previous = this.previousSibling; while (previous && nodes.includes(previous)) previous = previous.previousSibling; const before = viable(previous ? previous.nextSibling : parent.firstChild, nodes); insertNodes(parent, nodes, before); }
    after(...children) { const parent = this.parentNode; if (!parent) return; const nodes = convert(children), before = viable(this.nextSibling, nodes); insertNodes(parent, nodes, before); }
    replaceWith(...children) { const parent = this.parentNode; if (!parent) return; const nodes = convert(children), before = viable(this.nextSibling, nodes); insertNodes(parent, nodes, before, this); }
    remove() { this.parentNode?.removeChild(this); }
    hasChildNodes() { return this._m.kids.length > 0; }
    contains(node) { for (let rec = node?._m; rec; rec = rec.up) if (rec === this._m) return true; return false; }
    getRootNode(options) { const composed = options?.composed; let rec = this._m; while (rec.up ?? (composed ? rec.host : null)) rec = rec.up ?? rec.host; return of(rec); }
    cloneNode(deep = false) {
      if (this.nodeType === 9) { const copy = this instanceof XMLDocument ? new XMLDocument(this._type) : wrap(create('#document')); copy._declaration = this._declaration; if (deep) for (const kid of this._m.kids.map(of)) copy.append(kid.nodeType === 10 ? document.implementation.createDocumentType(kid.name, kid.publicId, kid.systemId) : kid.cloneNode(true)); return copy; }
      const cloned = this.nodeType === 4 ? document.createTextNode(this.data) : this.nodeType === 7 ? new ProcessingInstruction(create('#comment', this.data), this.target) : this.nodeType === 3 ? document.createTextNode(this.data) : this.nodeType === 8 ? document.createComment(this.data) : this.nodeType === 11 ? document.createDocumentFragment() : this.namespaceURI === XHTML ? wrap(create(this.localName)) : document.createElementNS(this.namespaceURI, this._m.tag);
      if (this.nodeType === 1) { for (let i = 0, list = this._m.attrs.list; i < list.length; i += 2) cloned.setAttribute(list[i], list[i + 1]); if (this._value !== undefined) setValue(cloned, this._value); if (this._files) { cloned._files = fileList([...this._files]); showFiles(cloned); } if (this._checked !== undefined) { setChecked(cloned, this._checked); cloned._dirty = this._dirty; } if (this._selected !== undefined) cloned._selected = this._selected; }
      // Loop, not recursion: deep trees overflow the JavaScript stack. Fill each copy before it joins its parent.
      // Plain elements, texts and comments copy as records (op 10) and get wrappers only when read, so the namespace
      // that `wrap` infers must match. Other nodes clone through their wrappers. Templates clone their own content.
      // Entries: source record, its wrapper, copy record, copy wrapper, whether the copy is an SVG element.
      if (deep) {
        const nodes = [this._m, this, cloned._m, cloned, cloned instanceof SVGElement], starts = [], fast = !this._svg;
        for (let i = 0; i < nodes.length; i += 5) {
          starts.push(nodes.length);
          const rec = nodes[i], svg = nodes[i + 4];
          if (i && nodes[i + 1] instanceof HTMLTemplateElement) continue;
          for (const kid of rec.kids) {
            const tag = kid.tag, source = kid.ref?.deref(), inferred = tag === 'svg' || svg && !integration.test(rec.tag);
            if (fast && (tag === '#text' || tag === '#comment' || tag[0] !== '#' && !copied.test(tag) && !definitions.has(tag) && (!source || source.namespaceURI === (inferred ? SVG : XHTML)))) {
              const copy = records[create(tag, tag[0] === '#' ? own(kid) : '', kid.id)];
              if (kid.attrs?.size) { copy.attrs = new Attrs(kid.attrs.list.slice()); const id = copy.attrs.get('id'); if (id) idAdd(id, copy); }
              nodes.push(kid, null, copy, null, inferred);
            } else { const child = source ?? of(kid), clone = child.cloneNode(child instanceof HTMLTemplateElement); nodes.push(kid, child, clone._m, clone, clone instanceof SVGElement); }
          }
        }
        // A wrapped copy under a record copy links as a record too. Its parent wraps after the whole copy is linked,
        // so `wrap` infers the namespace from the right ancestors, and then holds it.
        const held = [];
        for (let i = nodes.length - 5; i >= 0; i -= 5) {
          const parent = nodes[i + 2], start = starts[i / 5], end = starts[i / 5 + 1] ?? nodes.length;
          if (start === end) continue;
          for (let j = start; j < end; j += 5) {
            const kid = nodes[j + 2];
            if (nodes[i + 3] && (nodes[j + 3] || parent.tag === 'select' || parent.tag === 'optgroup')) nodes[i + 3].appendChild(nodes[j + 3] ?? of(kid));
            else { kid.up = parent; kid.i = parent.kids.push(kid) - 1; op(2, parent.id, kid.id, -1); if (nodes[j + 3]) held.push(parent, nodes[j + 3]); }
          }
          relist(parent);
        }
        for (let i = 0; i < held.length; i += 2) hold(of(held[i]), held[i + 1]);
      }
      const Type = this.nodeType === 1 && definitions.get(cloned.localName); if (Type) enqueue(cloned, Type); return cloned;
    }
    isSameNode(other) { return this === other; }
    lookupNamespaceURI(prefix) { if (prefix === 'xml') return 'http://www.w3.org/XML/1998/namespace'; if (prefix === 'xmlns') return 'http://www.w3.org/2000/xmlns/'; const node = this.nodeType === 9 ? this.documentElement : this.nodeType === 1 ? this : this.parentElement; for (let up = node; up; up = up.parentElement) { if ((up.prefix ?? null) === (prefix || null) && up.namespaceURI) return up.namespaceURI; const value = up._m.attrs.get(prefix ? 'xmlns:' + prefix : 'xmlns'); if (value !== undefined) return value || null; } return null; }
    lookupPrefix(namespace) { if (!namespace) return null; for (let up = this.nodeType === 9 ? this.documentElement : this.nodeType === 1 ? this : this.parentElement; up; up = up.parentElement) { if (up.namespaceURI === namespace && up.prefix) return up.prefix; for (const [key, value] of up._m.attrs) if (value === namespace && key.startsWith('xmlns:')) return key.slice(6); } return null; }
    isDefaultNamespace(namespace) { return this.lookupNamespaceURI(null) === (namespace || null); }
    isEqualNode(other) {
      if (!(other instanceof Node)) return false;
      for (const stack = [this._m, other._m]; stack.length;) {
        const b = stack.pop(), a = stack.pop();
        if (a.tag !== b.tag || a.kids.length !== b.kids.length || (a.attrs ? a.attrs.size !== b.attrs.size || a.attrs.list.some((name, i) => !(i & 1) && b.attrs.get(name) !== a.attrs.list[i + 1]) : own(a) !== own(b))) return false;
        for (let i = 0; i < a.kids.length; i++) stack.push(a.kids[i], b.kids[i]);
      }
      return true;
    }
    compareDocumentPosition(other) { if (this === other) return 0; const [a, b] = [this._m, other._m].map(rec => { const chain = []; for (; rec; rec = rec.up) chain.push(rec); return chain.reverse(); }); if (a[0] !== b[0]) return 1 | 32 | (a[0].id < b[0].id ? 4 : 2); let i = 0; while (a[i] && a[i] === b[i]) i++; if (i === a.length) return 4 | 16; if (i === b.length) return 2 | 8; return index(a[i]) < index(b[i]) ? 4 : 2; }
    normalize() { for (const child of this._m.kids.map(of)) { if (child.nodeType !== 3) { child.normalize(); continue; } if (child.parentNode !== this) continue; if (!child.length) { child.remove(); continue; } for (let next = child.nextSibling; next?.nodeType === 3; next = child.nextSibling) { const length = child.length, at = index(next._m); child.appendData(next.data); if (anchors.size) { each(next._m, range => shift(range, next._m, always, child, o => o + length)); each(this._m, range => shift(range, this._m, o => o === at, child, () => length)); } next.remove(); } } }
    addEventListener(name, fn, options = false) { if (!fn || options?.signal?.aborted) return; name = String(name); const capture = typeof options === 'boolean' ? options : !!options?.capture; let listeners = events.get(this); if (!listeners) events.set(this, listeners = new Map()); const list = listeners.get(name) ?? []; if (list.some(item => item.fn === fn && item.capture === capture)) return; const scroll = name === 'wheel' || name === 'mousewheel' || name === 'touchstart' || name === 'touchmove'; if (name === 'wheel' && !wheeling) { wheeling = true; call('dom.wheel'); } if (name === 'animationiteration' && !iterating) { iterating = true; call('dom.iterations'); } if (name === 'scrollend') ending = true; const item = { fn, capture, once: !!options?.once, passive: !!(options?.passive ?? (scroll && (this === globalThis || this === document || this === document.documentElement || this === document.body))), signal: options?.signal, abort: null }; list.push(item); listeners.set(name, list); item.signal?.addEventListener('abort', item.abort = () => { const now = listeners.get(name); if (now.includes(item)) listeners.set(name, now.filter(other => other !== item)); }, { once: true }); }
    removeEventListener(name, fn, options = false) { name = String(name); const capture = typeof options === 'boolean' ? options : !!options?.capture, listeners = events.get(this), list = listeners?.get(name), item = list?.find(item => item.fn === fn && item.capture === capture); if (!item) return; item.signal?.removeEventListener('abort', item.abort); listeners.set(name, list.filter(other => other !== item)); }
    dispatchEvent(event) { if (!(event instanceof Event)) throw new TypeError('Expected an Event'); const state = eventState(event); if (!state.dispatching) state.trusted = false; return event.type === 'click' && event instanceof MouseEvent ? activateClick(this, event) : dispatch(this, event); }
  }
  Object.setPrototypeOf(Node.prototype, EventTarget.prototype); Object.setPrototypeOf(Node, EventTarget);
  for (const [name, value] of Object.entries({ ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4, PROCESSING_INSTRUCTION_NODE: 7, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11, DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32 })) { Node[name] = value; Node.prototype[name] = value; }
  class Attr extends Node {
    constructor(ownerElement, name, value) { super(null, '#attr'); this.ownerElement = ownerElement; this.name = this.nodeName = name; this.localName = ownerElement && ownerElement.namespaceURI !== XHTML ? name.replace(/^.*:/, '') : name; this.prefix = this.localName === name ? null : name.slice(0, -this.localName.length - 1); this.namespaceURI = ownerElement?._spaces?.get(name) ?? (this.prefix ? ownerElement.lookupNamespaceURI(this.prefix) : null); this.specified = true; this._value = value; }
    get value() { return this.ownerElement?._m.attrs.get(this.name) ?? this._value; } set value(value) { this._value = String(value); this.ownerElement?.setAttribute(this.name, this._value); }
    get nodeValue() { return this.value; } set nodeValue(value) { this.value = value ?? ''; }
    get textContent() { return this.value; } set textContent(value) { this.value = value ?? ''; }
    get ownerDocument() { return this.ownerElement?.ownerDocument ?? document; }
    getRootNode() { return this; }
    cloneNode() { return new Attr(null, this.name, this.value); }
    isEqualNode(other) { return other instanceof Attr && other.name === this.name && other.value === this.value; }
  }
  class CharacterData extends Node {
    get data() { return own(this._m); } set data(value) { this.replaceData(0, this.length, String(value ?? '')); }
    get length() { return this.data.length; }
    substringData(offset, count) { offset >>>= 0; const value = this.data; if (offset > value.length) throw new DOMException('Invalid offset', 'IndexSizeError'); return value.slice(offset, offset + (count >>> 0)); }
    appendData(data) { this.replaceData(this.length, 0, data); }
    deleteData(offset, count) { this.replaceData(offset, count, ''); }
    insertData(offset, data) { this.replaceData(offset, 0, data); }
    replaceData(offset, count, data) {
      const rec = this._m, value = own(rec); offset >>>= 0; data = String(data);
      if (offset > value.length) throw new DOMException('Invalid offset', 'IndexSizeError');
      count = Math.min(count >>> 0, value.length - offset);
      const next = value.slice(0, offset) + data + value.slice(offset + count); if (next !== value) { rec.text = next; if (looseRanges) shape++; op(1, this._id, next); if (requiredSelects && this.nodeType === 3) optionsChanged(rec.up); }
      if (anchors.size) each(rec, range => shift(range, rec, at => at > offset && at <= offset + count, this, () => offset) | shift(range, rec, at => at > offset + count, this, at => at + data.length - count));
      if (observers.size) change({ type: 'characterData', target: this, oldValue: value });
    }
  }
  class Text extends CharacterData {
    constructor(data = '', tag) { super(tag ? data : create('#text', String(data)), '#text'); }
    splitText(offset) {
      offset >>>= 0; const value = this.data;
      if (offset > value.length) throw new DOMException('Invalid offset', 'IndexSizeError');
      const other = document.createTextNode(value.slice(offset)), parent = this.parentNode;
      if (parent) {
        parent.insertBefore(other, this.nextSibling);
        const at = index(this._m);
        if (anchors.size) { each(this._m, range => shift(range, this._m, o => o > offset, other, o => o - offset)); each(parent._m, range => shift(range, parent._m, o => o === at + 1, parent, o => o + 1)); }
      }
      this.replaceData(offset, value.length - offset, '');
      return other;
    }
    get wholeText() { let node = this, text = ''; while (node.previousSibling?.nodeType === 3) node = node.previousSibling; for (; node?.nodeType === 3; node = node.nextSibling) text += node.data; return text; }
  }
  // XML documents hold these as text and comments with another wrapper, which the tree keeps.
  class CDATASection extends Text { constructor(id) { super(id, '#text'); this.nodeType = 4; this.nodeName = '#cdata-section'; } }
  class Comment extends CharacterData { constructor(data = '', tag) { super(tag ? data : create('#comment', String(data)), '#comment'); } }
  class ProcessingInstruction extends CharacterData { constructor(id, target) { super(id, '#comment'); this.nodeType = 7; this.nodeName = this.target = target; } }
  class DocumentType extends Node {
    constructor(id, tag) { super(id, tag); this.nodeName = this.name; }
    get name() { return own(this._m).split('\0')[0]; } get publicId() { return own(this._m).split('\0')[1] ?? ''; } get systemId() { return own(this._m).split('\0')[2] ?? ''; }
    get textContent() { return null; } set textContent(_) {} get nodeValue() { return null; } set nodeValue(_) {}
  }
  const quirks = doctype => !doctype || doctype.name !== 'html' || /^(-\/\/w3o\/\/dtd w3 html strict 3\.0\/\/en\/\/|-\/w3c\/dtd html 4\.0 transitional\/en|html)$/i.test(doctype.publicId) || doctype.systemId.toLowerCase() === 'http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd'
    || /^(\+\/\/silmaril\/\/dtd html pro v0r11 19970101\/\/|-\/\/(as|advasoft ltd)\/\/dtd html 3\.0 aswedit \+ extensions\/\/|-\/\/ietf\/\/dtd html|-\/\/metrius\/\/dtd metrius presentational\/\/|-\/\/microsoft\/\/dtd internet explorer [23]\.0|-\/\/netscape comm\. corp\.\/\/dtd|-\/\/o'reilly and associates\/\/dtd html|-\/\/softquad|-\/\/spyglass\/\/dtd html 2\.0 extended\/\/|-\/\/sq\/\/dtd html 2\.0 hotmetal \+ extensions\/\/|-\/\/sun microsystems corp\.\/\/dtd hotjava|-\/\/w3c\/\/dtd html (3|4\.0 (frameset|transitional)|experimental)|-\/\/w3c\/\/dtd w3 html\/\/|-\/\/w3o\/\/dtd w3 html 3\.0\/\/|-\/\/webtechs\/\/dtd mozilla html)/i.test(doctype.publicId)
    || !doctype.systemId && /^-\/\/w3c\/\/dtd html 4\.01 (frameset|transitional)\/\//i.test(doctype.publicId);
  const cssName = name => String(name).replace(/[A-Z]/g, c => '-' + c.toLowerCase());
  const styleNames = new Map();
  const styleName = name => {
    name = String(name);
    let css = styleNames.get(name);
    if (css !== undefined) return css;
    css = name.startsWith('--') ? name : name === 'cssFloat' ? 'float' : name.replace(/^(webkit|moz|ms)(?=[A-Z])/, '-$1').replace(/^(Webkit|Moz|Ms)(?=[A-Z])/, prefix => '-' + prefix.toLowerCase()).replace(/[A-Z]/g, c => '-' + c.toLowerCase());
    if (styleNames.size < 4096) styleNames.set(name, css);
    return css;
  };
  let cssProperties = null;
  const properties = () => cssProperties ??= new Set(call('dom.properties'));
  let cssAliases = null;
  const cssProperty = name => properties().has(name) || name === 'float';
  const declared = new Map();
  const declare = text => { let list = declared.get(text); if (!list) { if (declared.size > 1023) declared.clear(); const flat = /\S/.test(text) ? host('dom.declare', text) : []; list = []; for (let i = 0; i < flat.length; i += 3) { const at = list.findIndex(entry => entry[0] === flat[i]); if (at >= 0) { if (list[at][2] && !flat[i + 2]) continue; list.splice(at, 1); } list.push([flat[i], flat[i + 1], flat[i + 2]]); } list = [...list.filter(entry => !entry[2]), ...list.filter(entry => entry[2])]; declared.set(text, list); } return list.map(entry => entry.slice()); };
  // Values keep the text as written and get Chrome's form only when read, so writes stay cheap.
  const specifiedText = new Map();
  const specified = (name, value) => { if (name.startsWith('--') || /^[a-z-]*$/.test(value) && !/position|origin|family|counter|offset|overflow-clip/.test(name)) return value; const key = name + ':' + value; let text = specifiedText.get(key); if (text === undefined) { if (specifiedText.size > 1023) specifiedText.clear(); specifiedText.set(key, text = host('dom.specified', name, value)); } return text; };
  const specifiedList = list => list.map(([name, value, priority]) => [name, specified(name, value), priority]);
  let shorthands = null, shorthandsOf = null;
  const loadShorthands = () => {
    const names = [...properties()], flat = host('dom.declare', names.map((name, i) => `--i:${i};${name}:initial`).join(';'));
    shorthands = new Map(); shorthandsOf = new Map();
    for (let i = 0, parts; i < flat.length; i += 3) if (flat[i] === '--i') shorthands.set(names[flat[i + 1]], parts = []); else parts.push(flat[i]);
    for (const [name, parts] of shorthands) if (parts.length === 1 && parts[0] === name) shorthands.delete(name);
    // Never fold physical sides into a logical shorthand like `margin-block`: Rust maps it to physical sides.
    const folded = [...shorthands].filter(([name, parts]) => { const side = /-(block|inline)/.exec(name)?.[0]; return parts.length > 1 && (!side || parts[0].includes(side)); }).sort((a, b) => b[1].length - a[1].length || a[0].length - b[0].length);
    for (const entry of folded) for (const part of entry[1]) { const list = shorthandsOf.get(part); if (list) list.push(entry); else shorthandsOf.set(part, [entry]); }
    return shorthands;
  };
  const layers = (value, separator = ',') => { const out = []; let depth = 0, start = 0; for (let i = 0; i < value.length; i++) { const c = value[i]; if (c === '(') depth++; else if (c === ')') depth--; else if (c === separator && !depth) { out.push(value.slice(start, i).trim()); start = i + 1; } } out.push(value.slice(start).trim()); return out; };
  const shorthandValue = (name, values, parts) => {
    if (name === 'position-try') return values[0] === 'normal' || values[0] === values[1] ? values[1] : values.join(' ');
    if (name === 'border-image') return values.every(value => value === values[0]) && /^(inherit|initial|unset|revert|revert-layer)$/.test(values[0]) ? values[0] : values.join() === 'none,100%,1,0,stretch' ? 'none' : `${values[0]} ${values[1]} / ${values[2]} / ${values[3]} ${values[4]}`;
    // `border` resets `border-image`, so it serializes only when that has its initial value.
    if (name === 'border' && parts.length === 17) { const image = values.splice(12); if (image.join() !== 'none,100%,1,0,stretch' && image.some(value => value !== values[0])) return ''; parts = parts.slice(0, 12); }
    if (name === 'border-radius') { const axes = [0, 1].map(axis => shorthandValue('margin', values.map(value => { const both = layers(value, ' '); return both[axis] ?? both[0]; }), ['margin-top'])); return axes[0] === axes[1] ? axes[0] : axes.join(' / '); }
    if (parts.length > 3 && parts.length % 3 === 0 && /-width$/.test(parts[0]) && /-style$/.test(parts[1])) { const first = values.slice(0, 3).join(); for (let i = 3; i < values.length; i += 3) if (values.slice(i, i + 3).join() !== first) return ''; return shorthandValue(name, values.slice(0, 3), parts.slice(0, 3)); }
    if (parts.length === 3 && /-width$/.test(parts[0]) && /-style$/.test(parts[1])) { const kept = values.filter((value, index) => value !== ['3px', 'none', 'currentcolor'][index]); return (name === 'outline' ? kept.reverse() : kept).join(' ') || 'none'; }
    if (values.length === 4 && /(^|-)top(-|$)/.test(parts[0])) { const [top, right, bottom, left] = values; return left !== right ? values.join(' ') : top !== bottom ? `${top} ${right} ${bottom}` : top !== right ? `${top} ${right}` : top; }
    if (name === 'font') { const [style, variant, weight, stretch, size, line, family] = values; return /^(normal|small-caps)$/.test(variant) && !/\d/.test(stretch) ? [style, variant, weight, stretch].filter(value => value !== 'normal').concat(size + (line === 'normal' ? '' : ' / ' + line), family).join(' ') : ''; }
    if (name === 'background') {
      const lists = values.slice(1).map(value => layers(value)), count = lists[0].length;
      if (values.length !== 8 || lists.some(list => list.length !== count)) return '';
      return lists[0].map((_, i) => { const { image, position, size, repeat, clip, origin, attachment } = Object.fromEntries(parts.slice(1).map((part, j) => [part.slice(11), lists[j][i]])), out = image === 'none' ? [] : [image]; if (position !== '0% 0%' || size !== 'auto') out.push(size === 'auto' ? position : position + ' / ' + size); if (repeat !== 'repeat') out.push(repeat); if (attachment !== 'scroll') out.push(attachment); if (origin !== 'padding-box' || clip !== 'border-box') out.push(origin === clip ? origin : origin + ' ' + clip); if (i === count - 1 && values[0] !== 'transparent') out.push(values[0]); return out.join(' ') || 'none'; }).join(', ');
    }
    if (name === 'container') return values[1] === 'normal' ? values[0] : values.join(' / ');
    if (name === 'flex') return values.join(' ');
    if (name === 'flex-flow') return values.filter((value, index) => value !== ['row', 'nowrap'][index]).join(' ') || 'row';
    // Like Chrome: position, image, type, without their initial values.
    if (name === 'list-style') { const [type, position, image] = values, kept = [position, image, type].filter((value, index) => value !== ['outside', 'none', 'disc'][index]); return kept.join(' ') || (image === 'none' && type === 'none' ? 'none' : type); }
    if (name === 'font-variant') return values.filter(value => value !== 'normal').join(' ') || 'normal';
    if (name === 'text-emphasis' && values[0] !== values[1]) return values.filter(value => value !== 'initial').join(' ');
    if (name === 'text-decoration') { const kept = values.filter((value, index) => index === 0 || !['solid', 'currentcolor', 'auto'].includes(value)); return (kept.length ? kept : values.slice(0, 1)).join(' '); }
    const grid = parts.every(part => /^grid-.*-(start|end)$/.test(part));
    if (/^grid(-template)?$/.test(name)) return '';
    if (values.length === 2 && !grid) return values[0] === values[1] ? values[0] : values.join(' ');
    const lists = values.map(value => layers(value)), count = lists[0].length;
    if (lists.some(list => list.length !== count)) return '';
    return lists[0].map((_, i) => lists.map(list => list[i]).reduce((text, value, j) => text + (grid ? ' / ' : /-size$/.test(parts[j]) && /-position$/.test(parts[j - 1]) ? ' / ' : ' ') + value)).join(', ');
  };
  // Only the `-webkit-` name takes these.
  const written = (name, value) => name === 'mask-composite' && /clear|copy|source-|destination-|xor/.test(value) ? '-webkit-' + name : name;
  // `keys` gets the name of each piece, or '' for a folded shorthand, then the piece.
  const declarationText = (list, keys) => {
    const out = [], done = new Set();
    if (list.length > 1 && !shorthandsOf) loadShorthands();
    for (const [name, , priority] of list) {
      if (done.has(name)) continue;
      let text = '';
      for (const [shorthand, parts] of shorthandsOf?.get(name) ?? []) {
        if (parts.some(part => done.has(part))) continue;
        const entries = parts.map(part => list.findLast(entry => entry[0] === part));
        if (entries.some(entry => !entry || entry[2] !== priority)) continue;
        const merged = shorthandValue(shorthand, entries.map(entry => entry[1]), parts);
        if (!merged) continue;
        for (const part of parts) done.add(part);
        text = shorthand + ': ' + merged;
        break;
      }
      const key = text ? '' : name;
      if (!text) { done.add(name); const value = list.findLast(entry => entry[0] === name)[1]; text = written(name, value) + ': ' + value; }
      const piece = text + (priority ? ' !important' : '') + ';';
      out.push(piece);
      if (keys) keys.push(key, piece);
    }
    return out.join(' ');
  };
  // The logical property group of a name and whether the name is logical, or null, as Chromium's groups.
  const logicalGroups = new Map();
  const logicalGroup = name => {
    let group = logicalGroups.get(name);
    if (group !== undefined) return group;
    group = /^(margin|padding|scroll-margin|scroll-padding|inset|border|overflow|overscroll-behavior|corner|min|max|contain-intrinsic)-|^(top|right|bottom|left|width|height|inline-size|block-size)$/.test(name) ? [name.replace(/^(min-|max-|contain-intrinsic-)?(width|height)$/, '$1inline-size').replace(/^(top|right|bottom|left)$/, 'inset-$1').replace(/(^|-)(top|right|bottom|left|inline|block|start|end|x|y)(?=-|$)/g, ''), /(^|-)(inline|block|start|end)(-|$)/.test(name)] : null;
    if (logicalGroups.size < 4096) logicalGroups.set(name, group);
    return group;
  };
  const readOnly = () => { throw new DOMException('Computed styles are read-only', 'NoModificationAllowedError'); };
  // As in Chrome, script focus shows the ring unless the last focus came from a press and no key went down since a press
  // moved the focus.
  let mouseFocus = false, hadKey = false, windowFocused = true, ring = false;
  const showsRing = node => !mouseFocus || hadKey || node.isContentEditable || node.localName === 'textarea' || node.localName === 'input' && !/^(button|submit|reset|checkbox|radio|range|color|file|image)$/i.test(node.getAttribute('type') ?? '');
  class CSSStyleDeclaration {
    constructor(owner) { this._owner = owner; }
    get _list() { return this._owner.read(); }
    get length() { return this._owner.computed ? this._owner.names().length : this._list.length; }
    item(index) { return (this._owner.computed ? this._owner.names()[index] : this._list[index]?.[0]) ?? ''; }
    get parentRule() { return this._owner.rule ?? null; }
    getPropertyValue(name) {
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      if (name.startsWith('-webkit-') && (cssAliases ??= new Set(call('dom.aliases'))).has(name)) name = name.slice(8);
      if (this._owner.computed) return this._owner.computed(name);
      const list = this._list, found = list.findLast(entry => entry[0] === name);
      if (found) return specified(name, found[1]);
      const parts = (shorthands ?? loadShorthands()).get(name), entries = parts?.map(part => list.findLast(entry => entry[0] === part));
      return entries && entries.every(entry => entry && entry[2] === entries[0][2]) ? shorthandValue(name, entries.map(entry => specified(entry[0], entry[1])), parts) : '';
    }
    getPropertyPriority(name) {
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      if (this._owner.computed) return '';
      const list = this._list, parts = (shorthands ?? loadShorthands()).get(name) ?? [name];
      return parts.every(part => list.findLast(entry => entry[0] === part)?.[2] === 'important') ? 'important' : '';
    }
    setProperty(name, value, priority = '') {
      const owner = this._owner;
      if (owner.computed) readOnly();
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      value = value == null ? '' : String(value).trim(); priority = String(priority ?? '').toLowerCase();
      if (value === '') { this.removeProperty(name); return; }
      if (priority && priority !== 'important') return;
      const added = owner.raw || name.startsWith('--') ? [[name, value, priority]] : declare(name + ':' + value + (priority ? ' !important' : ''));
      if (!added.length) return;
      // As in Chrome, a set name keeps its place, unless a later name of its logical group maps the other way.
      const whole = added.find(entry => entry[0] === 'mask' || entry[0] === 'offset')?.[0];
      const next = owner.read().filter(entry => !whole || !entry[0].startsWith(whole + '-') || entry[0] === 'mask-type'), end = [];
      for (const entry of added) {
        const i = next.findLastIndex(old => old[0] === entry[0]), group = i < 0 || entry[0].startsWith('--') ? null : logicalGroup(entry[0]);
        let moved = false;
        if (group) for (let j = i + 1; j < next.length && !moved; j++) { const other = next[j][0].startsWith('--') ? null : logicalGroup(next[j][0]); moved = !!other && other[0] === group[0] && other[1] !== group[1]; }
        if (i >= 0 && !moved) next[i] = entry;
        else { if (i >= 0) next.splice(i, 1); end.push(entry); }
        // A shorthand and its later longhands can leave one name twice. The last one is the declaration.
        for (let j = i - 1; j >= 0; j--) if (next[j][0] === entry[0]) next.splice(j, 1);
      }
      // An unchanged value writes nothing, so it makes no mutation record.
      const list = owner.read();
      if (!end.length && next.length === list.length && next.every((entry, i) => entry === list[i] || entry.every((part, j) => part === list[i][j]))) return;
      owner.write(end.length ? next.concat(end) : next);
    }
    removeProperty(name) {
      if (this._owner.computed) readOnly();
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      const old = this.getPropertyValue(name), parts = new Set((shorthands ?? loadShorthands()).get(name) ?? [name]), list = this._list, kept = list.filter(entry => !parts.has(entry[0]));
      if (kept.length !== list.length) this._owner.write(kept);
      return old;
    }
    *[Symbol.iterator]() { for (let i = 0, length = this.length; i < length; i++) yield this.item(i); }
    get cssText() { return this._owner.computed ? '' : declarationText(specifiedList(this._list)); }
    set cssText(value) { if (this._owner.computed) readOnly(); this._owner.write(this._owner.raw ? String(value).split(';').flatMap(part => { const colon = part.indexOf(':'); return colon > 0 ? [[part.slice(0, colon).trim(), part.slice(colon + 1).trim(), '']] : []; }) : declare(String(value))); }
  }
  const styleKey = key => typeof key === 'string' && !(key.charCodeAt(0) <= 57 && /^\d+$/.test(key));
  const declaration = (owner, Type = CSSStyleDeclaration) => new Proxy(new Type(owner), {
    get: (target, key) => typeof key !== 'string' || key in target ? Reflect.get(target, key) : /^\d+$/.test(key) ? target.item(Number(key)) : target.getPropertyValue(styleName(key)),
    set: (target, key, value) => { if (typeof key !== 'string' || key in target) return Reflect.set(target, key, value); if (styleKey(key)) target.setProperty(styleName(key), value); return true; },
    has: (target, key) => key in target || typeof key === 'string' && (styleKey(key) ? cssProperty(styleName(key)) : Number(key) < target.length),
    ownKeys: target => { const names = [...properties()]; return [...new Set([...Reflect.ownKeys(target), ...Array.from({ length: target.length }, (_, index) => String(index)), ...names, ...names.map(name => name.replace(/^-(webkit|moz|ms)-/, '$1-').replace(/-([a-z])/g, (_, c) => c.toUpperCase()))])]; },
    getOwnPropertyDescriptor: (target, key) => Reflect.getOwnPropertyDescriptor(target, key) ?? (typeof key === 'string' && (styleKey(key) ? cssProperty(styleName(key)) : Number(key) < target.length) ? { value: styleKey(key) ? target.getPropertyValue(styleName(key)) : target.item(Number(key)), writable: true, enumerable: true, configurable: true } : undefined),
  });
  const attrName = (element, name) => element.namespaceURI === XHTML ? String(name).toLowerCase() : String(name);
  // The qualified name of an attribute in a namespace.
  const nsAttr = (element, namespace, name) => namespace && element.namespaceURI !== XHTML ? element._m.attrs.keys().find(key => (key === name || key.endsWith(':' + name)) && (element._spaces?.get(key) ?? (key === name ? null : element.lookupNamespaceURI(key.slice(0, -name.length - 1)))) === namespace) ?? name : name;
  const datasetOwner = Symbol(), datasetHandler = { get: (target, name) => target[datasetOwner].getAttribute('data-' + cssName(name)) ?? undefined, set: (target, name, value) => { if (/-[a-z]/.test(name)) throw new DOMException(`'${name}' is not a valid dataset name`, 'SyntaxError'); target[datasetOwner].setAttribute('data-' + cssName(name), value); return true; }, has: (target, name) => typeof name === 'string' && target[datasetOwner].hasAttribute('data-' + cssName(name)), deleteProperty: (target, name) => { target[datasetOwner].removeAttribute('data-' + cssName(name)); return true; }, ownKeys: target => target[datasetOwner].getAttributeNames().filter(name => name.startsWith('data-')).map(name => name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())), getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }) };
  class Element extends Node {
    constructor(id, tag) { const custom = id === undefined; if(custom){if(constructing?.node){const node=constructing.node;constructing=null;Object.setPrototypeOf(node,new.target.prototype);return node;}tag=constructing?.tag??defined.get(new.target)?.name;if(!tag)throw new TypeError('Illegal element constructor');id=create(tag);constructing=null;} super(id, tag);  this.tagName = this.nodeName; this.localName = tag; this.namespaceURI = XHTML; if (custom) { flag(this, 64, true); if (new.target.formAssociated) { faces++; this._face = true; flag(this, 512, true); } } }
    get attributes() { const attrs = listOf(NamedNodeMap, this._m.attrs.keys().map(name => attrNode(this, name))); for (const attr of attrs) if (!(attr.name in attrs)) attrs[attr.name] = attr; return attrs; }
    getAttributeNames() { return [...this._m.attrs.keys()]; }
    hasAttributes() { return this._m.attrs.size > 0; }
    get nextElementSibling() { let n = this.nextSibling; while (n && n.nodeType !== 1) n = n.nextSibling; return n; }
    get previousElementSibling() { let n = this.previousSibling; while (n && n.nodeType !== 1) n = n.previousSibling; return n; }
    get id() { return this.getAttribute('id') ?? ''; } set id(value) { this.setAttribute('id', value); }
    get className() { return this.getAttribute('class') ?? ''; } set className(value) { this.setAttribute('class', value); }
    get classList() { return this._classList ??= new DOMTokenList(this, 'class'); } set classList(value) { this.classList.value = value; }
    get part() { return this._part ??= new DOMTokenList(this, 'part'); } set part(value) { this.part.value = value; }
    get dataset() { return this._dataset ??= new Proxy({ [datasetOwner]: this }, datasetHandler); }
    getAttribute(name) { return this._m.attrs.get(attrName(this, name)) ?? null; }
    getAttributeNS(namespace, name) { return this.getAttribute(nsAttr(this, namespace, name)); }
    hasAttribute(name) { return this._m.attrs.has(attrName(this, name)); }
    hasAttributeNS(namespace, name) { return this.hasAttribute(nsAttr(this, namespace, name)); }
    setAttribute(name, value) { name = attrName(this, name); value = String(value); const attrs = this._m.attrs, oldValue = attrs.get(name) ?? null; if (oldValue === null) badAttr(name); if (value !== oldValue) op(4, this._id, name, value); (attrs === noAttrs ? this._m.attrs = new Attrs() : attrs).set(name, value); if (name === 'id' && value !== oldValue) { if (oldValue !== null) idDrop(oldValue, this._m); idAdd(value, this._m); } if (observers.size) change({ type: 'attributes', target: this, attributeName: name, oldValue }); if (definitions.size) attributeReaction(this, name, oldValue, value); if (value !== oldValue) attributeChanged(this, name, oldValue, value); }
    setAttributeNS(namespace, name, value) { if (namespace && this.namespaceURI !== XHTML) (this._spaces ??= new Map()).set(String(name), String(namespace)); this.setAttribute(name, value); }
    removeAttribute(name) { name = attrName(this, name); const attrs = this._m.attrs, oldValue = attrs.get(name) ?? null; if (oldValue === null) return; attrs.delete(name); op(5, this._id, name); const attr = this._attrs?.get(name); if (attr) { attr._value = oldValue; attr.ownerElement = null; this._attrs.delete(name); } if (name === 'id') idDrop(oldValue, this._m); if (observers.size) change({ type: 'attributes', target: this, attributeName: name, oldValue }); if (definitions.size) attributeReaction(this, name, oldValue, null); attributeChanged(this, name, oldValue, null); }
    removeAttributeNS(namespace, name) { this.removeAttribute(nsAttr(this, namespace, name)); }
    toggleAttribute(name, force) { const present = this.hasAttribute(name); if (!present) badAttr(attrName(this, name)); const wanted = force === undefined ? !present : !!force; if (wanted !== present) { if (wanted) this.setAttribute(name, ''); else this.removeAttribute(name); } return wanted; }
    getAttributeNode(name) { name = attrName(this, name); return this._m.attrs.has(name) ? attrNode(this, name) : null; }
    setAttributeNode(attr) { if (!(attr instanceof Attr)) throw new TypeError('Expected an Attr'); if (attr.ownerElement === this) return attr; if (attr.ownerElement) throw new DOMException('The attribute is on another element', 'InUseAttributeError'); const old = this.getAttributeNode(attr.name); if (old) this.removeAttributeNode(old); this.setAttribute(attr.name, attr.value); attr.ownerElement = this; (this._attrs ??= new Map()).set(attr.name, attr); return old; }
    setAttributeNodeNS(attr) { return this.setAttributeNode(attr); }
    removeAttributeNode(attr) { if (!(attr instanceof Attr) || attr.ownerElement !== this || !this._m.attrs.has(attr.name)) throw new DOMException('The attribute is not on this element', 'NotFoundError'); this.removeAttribute(attr.name); return attr; }
    get style() {
      if (this._style) return this._style;
      // The parsed list is shared, not copied, so declaration code must never change its entries.
      // A write that only replaces or adds one name in no shorthand, as `setProperty` does, changes one piece of the last text.
      let text = null, list = [], keys = null;
      const write = next => {
        let at = -1, same = !!keys && !!shorthandsOf && (next.length === list.length || next.length === list.length + 1);
        for (let i = 0; same && i < list.length; i++) if (next[i] !== list[i]) { same = at < 0 && next.length === list.length && next[i][0] === list[i][0]; at = i; }
        if (same && next.length > list.length) at = list.length;
        if (same && at >= 0 && !shorthandsOf.has(next[at][0])) {
          const [name, value, priority] = next[at], piece = written(name, value) + ': ' + value + (priority ? ' !important' : '') + ';', j = at < list.length ? keys.indexOf(name) : -1;
          if (j >= 0) keys[j + 1] = piece; else keys.push(name, piece);
          text = '';
          for (let k = 1; k < keys.length; k += 2) text += (k > 1 ? ' ' : '') + keys[k];
        } else if (!same || at >= 0) text = declarationText(next, keys = []);
        list = next; this.setAttribute('style', text);
      };
      return this._style = declaration({ read: () => { const now = this.getAttribute('style') ?? ''; if (now !== text) { text = now; list = declare(now); keys = null; } return list; }, write });
    }
    set style(value) { this.style.cssText = value; }
    get innerHTML() { return inXML(this) ? [...this.childNodes].map(kid => new XMLSerializer().serializeToString(kid)).join('') : serialize(this._m, true) || own(this._m); }
    set innerHTML(value) { setHTML(this, value); }
    get outerHTML() { return inXML(this) ? new XMLSerializer().serializeToString(this) : serialize(this._m); }
    set outerHTML(value) { const parent = this.parentNode; if (!parent) return; if (parent.nodeType === 9) throw new DOMException('Cannot replace the document element', 'NoModificationAllowedError'); reactionDepth++; try { this.replaceWith(parseFragment(value, parent.nodeType === 1 ? parent : null)); } finally { leave(); } }
    get innerText() { return call('dom.skipped', this._id, false) > 0 ? '' : slot(this) < 0 ? this.textContent : call('dom.innerText', this._id); }
    set innerText(value) { const parts = String(value ?? '').split(/\r\n|\r|\n/); replaceAll(this, () => { release(this); detach(this._m); op(1, this._id, ''); }); quiet++; try { parts.forEach((part, index) => { if (index) this.append(document.createElement('br')); if (part) this.append(part); }); } finally { quiet--; } if (observers.size && this._m.kids.length) change({ type: 'childList', target: this, addedNodes: listOf(NodeList, this._m.kids.map(of)) }); }
    matches(selector) { return selectors('dom.matches', this._id, selector); }
    closest(selector) { return wrap(selectors('dom.closest', this._id, selector)); }
    checkVisibility(options = {}) {
      // Like Chrome, the contents that a `content-visibility: auto` box skips count as visible unless the options ask.
      const skipped = call('dom.skipped', this._id, false);
      if (skipped > 0 ? skipped === 2 || options?.contentVisibilityAuto : slot(this) < 0) return false;
      if ((options?.checkVisibilityCSS || options?.visibilityProperty) && getComputedStyle(this).visibility !== 'visible') return false;
      if (options?.checkOpacity || options?.opacityProperty) for (let node = this; node; node = node.parentElement) if (getComputedStyle(node).opacity === '0') return false;
      return true;
    }
    getBoundingClientRect() { const i = slot(this), m = geometry; return i < 0 ? new DOMRect() : new DOMRect(m[i], m[i + 1], m[i + 2], m[i + 3]); }
    getClientRects() { if (slot(this) < 0) return listOf(DOMRectList, []); const parts = call('dom.fragments', this._id); return listOf(DOMRectList, parts ? parts.map(part => new DOMRect(...part)) : [this.getBoundingClientRect()]); }
    get clientWidth() { return rootElement(this) ? Math.round(viewSize()[0]) : Math.round(client(this)[2] / zoomOf(this)); } get clientHeight() { return rootElement(this) ? Math.round(viewSize()[1]) : Math.round(client(this)[3] / zoomOf(this)); }
    get clientLeft() { return Math.round(client(this)[0] / zoomOf(this)); } get clientTop() { return Math.round(client(this)[1] / zoomOf(this)); }
    get scrollTop() { return unzoom(this, scrollOf(this, 5)); } set scrollTop(value) { scrollTo(this, null, Number(value)); }
    get scrollLeft() { return unzoom(this, scrollOf(this, 4)); } set scrollLeft(value) { scrollTo(this, Number(value), null); }
    get scrollWidth() { return rootElement(this) ? Math.round(viewSize()[0] + metric(document, 6)) : extent(this, 0); } get scrollHeight() { return rootElement(this) ? Math.round(viewSize()[1] + metric(document, 7)) : extent(this, 1); }
    scrollTo(left, top) { let behavior = null; if (typeof left === 'object') ({ left, top, behavior = null } = left ?? {}); scrollTo(this, left ?? null, top ?? null, behavior); }
    scroll(left, top) { this.scrollTo(left, top); }
    scrollBy(left, top) { let behavior = null; if (typeof left === 'object') ({ left, top, behavior = null } = left ?? {}); scrollTo(this, this.scrollLeft + (Number(left) || 0), this.scrollTop + (Number(top) || 0), behavior, 'by'); }
    scrollIntoView(options = {}) {
      if (typeof options === 'boolean') options = { block: options ? 'start' : 'end' };
      const target = this.getBoundingClientRect(), own = getComputedStyle(this), zoom = zoomOf(this), margin = side => (parseFloat(own['scrollMargin' + side]) || 0) * zoom;
      let left = target.left - margin('Left'), top = target.top - margin('Top');
      const width = target.width + margin('Left') + margin('Right'), height = target.height + margin('Top') + margin('Bottom');
      for (let p = this.assignedSlot ?? this.parentElement ?? this.getRootNode().host; p; p = p.assignedSlot ?? p.parentElement ?? p.getRootNode().host) {
        const style = getComputedStyle(p), root = p === document.documentElement, z = zoomOf(p), bounds = root ? new DOMRect() : p.getBoundingClientRect(), [viewWidth, viewHeight] = root ? viewSize() : [], to = [null, null]; let scrollable = false;
        const padding = (side, extent) => { const value = style['scrollPadding' + side]; return value.endsWith('%') ? parseFloat(value) / 100 * extent : (parseFloat(value) || 0) * z; };
        for (const [axis, align, start, size, edge, client, near, far] of [
          ['Y', options.block ?? 'start', top, height, root ? 0 : bounds.top + p.clientTop * z, root ? viewHeight : p.clientHeight * z, 'Top', 'Bottom'],
          // `start` and `end` are logical, so in a right-to-left box `end` is its left side.
          ['X', style.direction === 'rtl' ? { start: 'end', end: 'start' }[options.inline] ?? options.inline ?? 'nearest' : options.inline ?? 'nearest', left, width, root ? 0 : bounds.left + p.clientLeft * z, root ? viewWidth : p.clientWidth * z, 'Left', 'Right'],
        ]) {
          // The root always scrolls the page, as the viewport.
          if (!root && !['auto','scroll','hidden'].includes(style['overflow' + axis] || style.overflow)) continue;
          scrollable = true;
          const inset = padding(near, client), extent = client - inset - padding(far, client);
          const before = start - edge - inset, after = start + size - edge - inset - extent;
          let delta = align === 'center' ? (before + after) / 2 : align === 'end' ? after : before;
          // `focus()` centers a box that is out of view and moves a box that is partly in view to the nearest edge, as Chrome does.
          if (align === 'if-needed' && (before + size <= 0 || after - size >= 0)) delta = (before + after) / 2;
          else if (align === 'nearest' || align === 'if-needed') delta = before < 0 && after > 0 ? 0 : before < 0 ? (size <= extent ? before : after) : after > 0 ? (size <= extent ? after : before) : 0;
          if (delta) to[axis === 'Y' ? 1 : 0] = (axis === 'Y' ? p.scrollTop : p.scrollLeft) * z + delta;
        }
        // Like Chrome, a box that does not move can still snap to another area, like one with the focus.
        if (scrollable) {
          const previous = [p.scrollLeft * z, p.scrollTop * z], next = call('dom.scroll', p._id, to[0], to[1], options.behavior ?? null, 'into');
          if (next) { left -= next[0] - previous[0]; top -= next[1] - previous[1]; }
        }
        if (scrollable && options.container === 'nearest') break;
      }
    }
    focus(options) {
      // Like Chrome, a label that can't take the focus gives it to its control.
      if (this.localName === 'label' && !focusable(this)) return labelControl(this)?.focus(options);
      if (active === this || !focusable(this)) return;
      finishComposition();
      const previous = active;
      // As in Chrome, no element has focus during `blur` and `focusout`, and a listener that moves the focus wins.
      if (previous && windowFocused) {
        active = null; call('dom.focus', []); commit(previous);
        dispatch(previous, new FocusEvent('blur', { view: window, composed: true, relatedTarget: this })); dispatch(previous, new FocusEvent('focusout', { view: window, bubbles: true, composed: true, relatedTarget: this }));
        if (active || !this.isConnected) return;
      }
      active = this; navStart = [this]; call('dom.focus', path(this).map(node => node._id), ring = options?.focusVisible ?? showsRing(this), !windowFocused);
      if (editable(this)) call('dom.selection', this._id, this._start ?? 0, this._end ?? 0);
      if (!options?.preventScroll) this.scrollIntoView({ block: 'if-needed', inline: 'if-needed' });
      // Like Chrome, an editing host that gets focus without the selection in it gets a caret at its start.
      if (this.isContentEditable && !this.contains(selection.anchorNode)) selection.collapse(document.createTreeWalker(this, 4).nextNode() ?? this, 0);
      else if (this.isContentEditable && selection.isCollapsed) selection._send();
      // Defer focus events while the window lacks focus. On window focus, emit them for the current element.
      if (!windowFocused) return;
      if (active === this) { dispatch(this, new FocusEvent('focus', { view: window, composed: true, relatedTarget: previous })); dispatch(this, new FocusEvent('focusin', { view: window, bubbles: true, composed: true, relatedTarget: previous })); }
    }
    blur() { if (active !== this) return; finishComposition(); active = null; call('dom.focus', []); if (!windowFocused) return; commit(this); dispatch(this, new FocusEvent('blur', { view: window, composed: true })); dispatch(this, new FocusEvent('focusout', { view: window, bubbles: true, composed: true })); }
    click() { if (disabled(this) || this._clicking) return; this._clicking = true; try { fireClick(this); } finally { this._clicking = false; } }
    get contentEditable() { const value = this.getAttribute('contenteditable')?.toLowerCase(); return value === '' ? 'true' : value === 'true' || value === 'false' || value === 'plaintext-only' ? value : 'inherit'; }
    set contentEditable(value) { value = String(value).toLowerCase(); if (value === 'inherit') this.removeAttribute('contenteditable'); else if (value === 'true' || value === 'false' || value === 'plaintext-only') this.setAttribute('contenteditable', value); else throw new DOMException(`'${value}' is not a valid contentEditable value`, 'SyntaxError'); }
    get isContentEditable() { const value = this.contentEditable; return value === 'true' || value === 'plaintext-only' || value === 'inherit' && !!this.parentElement?.isContentEditable; }
    get tabIndex() { const value = parseInt(this.getAttribute('tabindex'), 10), tag = this.localName; return Number.isNaN(value) ? /^(input|textarea|button|select|iframe|object|video|audio|terminal)$/.test(tag) || (tag === 'a' || tag === 'area') && (this.namespaceURI === XHTML || this.hasAttribute('href')) || tag === 'summary' && activation(this) ? 0 : -1 : value; } set tabIndex(value) { this.setAttribute('tabindex', Number(value) | 0); }
    attachShadow(options) { if (this._shadow) throw new DOMException('Shadow root already exists', 'NotSupportedError'); if(!['open','closed'].includes(options.mode))throw new TypeError('Invalid shadow mode');if(this.namespaceURI!==XHTML||!/^(article|aside|blockquote|body|div|footer|h[1-6]|header|main|nav|p|section|span|[a-z][a-z0-9._]*-[a-z0-9._-]*)$/.test(this.localName))throw new DOMException(`<${this.localName}> cannot have a shadow root`, 'NotSupportedError');const root = document.createDocumentFragment(); shadows++; Object.setPrototypeOf(root,ShadowRoot.prototype);root.host=this;root.mode=options.mode;root.delegatesFocus=!!options.delegatesFocus;root.slotAssignment=options.slotAssignment==='manual'?'manual':'named';root.clonable=!!options.clonable;root.serializable=!!options.serializable;this._shadow=root;root._m.host=this._m;this._m.shadow=root._m;op(6,this._id,root._id);return root; }
    get shadowRoot() { return this._shadow?.mode === 'open' ? this._shadow : null; }
    setPointerCapture(id) { if (!pointers.has(id)) throw new DOMException('Pointer is not active', 'NotFoundError'); captures.set(id, this); }
    hasPointerCapture(id) { return captures.get(id) === this; }
    releasePointerCapture(id) { if (captures.get(id) === this) captures.delete(id); }
    insertAdjacentElement(position, node) { if (!(node instanceof Element)) throw new TypeError('Expected an Element'); return adjacent(this, position, node); }
    insertAdjacentText(position, text) { adjacent(this, position, document.createTextNode(String(text))); }
    webkitMatchesSelector(selector) { return this.matches(selector); }
    attachInternals() { if (this._internals || !definitions.has(this.localName)) throw new DOMException('Only custom elements can attach internals once', 'NotSupportedError'); return this._internals = new ElementInternals(this); }
    getHTML() { return this.innerHTML; }
    setHTMLUnsafe(html) { this.innerHTML = html; attachDeclarative(); }
    get outerText() { return this.innerText; } set outerText(value) { if (!this.parentNode) throw new DOMException('The element has no parent', 'NoModificationAllowedError'); this.replaceWith(...String(value ?? '').split(/\r\n|\r|\n/).flatMap((part, index) => index ? [document.createElement('br'), part] : [part]).filter(part => part !== '')); }
    insertAdjacentHTML(position, source) { const outside = /^(beforebegin|afterend)$/i.test(position), context = outside ? this.parentNode : this; if (outside && (!context || context.nodeType === 9)) throw new DOMException('The element has no parent', 'NoModificationAllowedError'); adjacent(this, position, parseFragment(source, context.nodeType === 1 && !(context.localName === 'html' && context.namespaceURI === XHTML) ? context : null)); }
  }
  Element.prototype.prefix = null;
  for (const [Type, names] of [[Node, ['insertBefore', 'removeChild', 'replaceChild', 'cloneNode']], [Element, ['setAttribute', 'removeAttribute', 'insertAdjacentHTML']]]) for (const name of names) { const fn = Type.prototype[name]; Type.prototype[name] = { [name](a, b) { reactionDepth++; try { return fn.call(this, a, b); } finally { leave(); } } }[name]; }
  const adjacent = (element, position, node) => { position = String(position).toLowerCase(); if (position === 'beforebegin' || position === 'afterend') { if (!element.parentNode) return null; if (position === 'beforebegin') element.before(node); else element.after(node); } else if (position === 'afterbegin') element.prepend(node); else if (position === 'beforeend') element.append(node); else throw new DOMException('Invalid insertion position', 'SyntaxError'); return node; };
  const attributeReaction = (node, name, oldValue, value) => { const Type = definitions.get(node._m.tag); if (Type && node instanceof Type && !node._failed && defined.get(Type).observed.has(name)) enqueue(node, ['attributeChangedCallback', name, oldValue, value]); };
  class HTMLElement extends Element {
    get offsetWidth() { const i = slot(this); return i < 0 ? 0 : Math.round((marks[this._id] & 128 ? client(this)[8] : geometry[i + 2]) / zoomOf(this)); } get offsetHeight() { const i = slot(this); return i < 0 ? 0 : Math.round((marks[this._id] & 128 ? client(this)[9] : geometry[i + 3]) / zoomOf(this)); }
    get offsetParent() {
      if (this.localName === 'body' || rootElement(this) || slot(this) < 0) return null;
      call('dom.layout', false, true); layout();
      if (marks[this._id] & 32) return null;
      const positioned = marks[this._id] & 8;
      for (let node = this.parentElement; node; node = node.parentElement) if (marks[node._id] & 8 || node.localName === 'body' || !positioned && /^(td|th|table)$/.test(node.localName)) return node;
      return null;
    }
    get offsetLeft() { return offset(this, 0); }
    get offsetTop() { return offset(this, 1); }
  }
  class SVGAnimatedString { constructor(element) { this._element = element; } get baseVal() { return this._element.getAttribute('class') ?? ''; } set baseVal(value) { this._element.setAttribute('class', value); } get animVal() { return this.baseVal; } }
  class SVGElement extends Element {constructor(id,tag){super(id,tag);this.namespaceURI=SVG;this.nodeName=this.tagName=tag;}get className(){return this._className??=new SVGAnimatedString(this);}getBBox(){layout();const [x,y,width,height]=call('dom.svgBox',this._id);return new DOMRect(x,y,width,height);}get ownerSVGElement(){for(let node=this.parentElement;node;node=node.parentElement)if(node instanceof SVGSVGElement)return node;return null;}get viewportElement(){return this.ownerSVGElement;}}
  class SVGSVGElement extends SVGElement {
    get viewBox() { const box = viewBoxOf(this) ?? [0, 0, 0, 0]; return { baseVal: new DOMRect(...box), animVal: new DOMRect(...box) }; }
    createSVGPoint() { return new DOMPoint(); } createSVGMatrix() { return new DOMMatrix(); } createSVGRect() { return new DOMRect(); }
  }
  const svgTransform = text => {
    const m = new DOMMatrix();
    for (const [, fn, args] of String(text ?? '').matchAll(/([a-zA-Z]+)\s*\(([^)]*)\)/g)) {
      const [a = 0, b, c = 0] = args.split(/[\s,]+/).filter(Boolean).map(Number);
      if (fn === 'matrix') m.multiplySelf(new DOMMatrix(args.split(/[\s,]+/).filter(Boolean).map(Number))); else if (fn === 'translate') m.translateSelf(a, b ?? 0); else if (fn === 'scale') m.scaleSelf(a, b ?? a); else if (fn === 'rotate') m.translateSelf(b ?? 0, c).rotateSelf(a).translateSelf(-(b ?? 0), -c); else if (fn === 'skewX') m.skewXSelf(a); else if (fn === 'skewY') m.skewYSelf(a);
    }
    return m;
  };
  const viewBoxOf = svg => { const box = (svg.getAttribute('viewBox') ?? '').trim().split(/[\s,]+/).map(Number); return box.length === 4 && box.every(Number.isFinite) && box[2] > 0 && box[3] > 0 ? box : null; };
  // The user space of an <svg>: its viewBox fit into its box by preserveAspectRatio.
  const viewMatrix = svg => {
    const box = viewBoxOf(svg), m = new DOMMatrix();
    if (!box) return m;
    const { width, height } = svg.getBoundingClientRect(), [align = 'xMidYMid', slice] = (svg.getAttribute('preserveAspectRatio') ?? '').trim().split(/\s+/);
    let sx = width / box[2], sy = height / box[3];
    if (align !== 'none') sx = sy = slice === 'slice' ? Math.max(sx, sy) : Math.min(sx, sy);
    const place = (key, free) => align.includes(key + 'Mid') ? free / 2 : align.includes(key + 'Max') ? free : 0;
    return m.translateSelf(place('x', width - box[2] * sx) - box[0] * sx, place('Y', height - box[3] * sy) - box[1] * sy).scaleSelf(sx, sy);
  };
  Object.assign(SVGElement.prototype, {
    getCTM() { const svg = this instanceof SVGSVGElement ? this : this.ownerSVGElement; if (!svg) return null; let m = new DOMMatrix(); for (let node = this; node && node !== svg; node = node.parentElement) m = svgTransform(node.getAttribute('transform')).multiply(m); return viewMatrix(svg).multiply(m); },
    getScreenCTM() { const m = this.getCTM(), svg = this instanceof SVGSVGElement ? this : this.ownerSVGElement; if (!m) return null; const { left, top } = svg.getBoundingClientRect(); return new DOMMatrix([1, 0, 0, 1, left, top]).multiply(m); },
  });
  for (const name of ['x', 'y', 'width', 'height', 'cx', 'cy', 'r', 'rx', 'ry', 'x1', 'y1', 'x2', 'y2']) Object.defineProperty(SVGElement.prototype, name, { configurable: true, get() { const read = () => { const text = this.getAttribute(name) ?? '0'; return { value: parseFloat(text) || 0, valueAsString: text, valueInSpecifiedUnits: parseFloat(text) || 0 }; }; return { get baseVal() { return read(); }, get animVal() { return read(); } }; } });
  for (const attr of ['title','name','type','placeholder','dir','lang','slot']) Object.defineProperty(Element.prototype, attr, { get() { return this.getAttribute(attr) ?? ''; }, set(value) { this.setAttribute(attr, value); } });
  for (const name of ['role','ariaAtomic','ariaAutoComplete','ariaBrailleLabel','ariaBrailleRoleDescription','ariaBusy','ariaChecked','ariaColCount','ariaColIndex','ariaColIndexText','ariaColSpan','ariaCurrent','ariaDescription','ariaDisabled','ariaExpanded','ariaHasPopup','ariaHidden','ariaInvalid','ariaKeyShortcuts','ariaLabel','ariaLevel','ariaLive','ariaModal','ariaMultiLine','ariaMultiSelectable','ariaOrientation','ariaPlaceholder','ariaPosInSet','ariaPressed','ariaReadOnly','ariaRelevant','ariaRequired','ariaRoleDescription','ariaRowCount','ariaRowIndex','ariaRowIndexText','ariaRowSpan','ariaSelected','ariaSetSize','ariaSort','ariaValueMax','ariaValueMin','ariaValueNow','ariaValueText']) {
    const attr = name.replace('aria', 'aria-').toLowerCase();
    Object.defineProperty(Element.prototype, name, { get() { return this.getAttribute(attr); }, set(value) { if (value === null) this.removeAttribute(attr); else this.setAttribute(attr, value); } });
  }
  for (const attr of ['hidden','disabled','readOnly','multiple','autofocus','required','noValidate','formNoValidate','inert']) Object.defineProperty(Element.prototype, attr, { get() { return this.hasAttribute(attr.toLowerCase()); }, set(value) { this.toggleAttribute(attr.toLowerCase(), !!value); } });
  class HTMLSlotElement extends HTMLElement {
    assignedNodes(options = {}) { return call('dom.assignedNodes', this._id, !!options.flatten).map(wrap); }
    assignedElements(options = {}) { return this.assignedNodes(options).filter(node => node.nodeType === 1); }
  }
  const inputTypes = new Set(['hidden','text','search','tel','url','email','password','date','month','week','time','datetime-local','number','range','color','checkbox','radio','file','submit','image','reset','button']);
  const selectable = node => node instanceof HTMLTextAreaElement || /^(text|search|tel|url|password)$/.test(node.type);
  const editable = node => node instanceof HTMLTextAreaElement || node instanceof HTMLInputElement && /^(text|search|tel|url|email|password|number)$/.test(node.type);
  const sanitize = (type, value) => type === 'number' ? /^-?(\d+(\.\d+)?|\.\d+)([eE][-+]?\d+)?$/.test(value) ? value : '' : type === 'color' ? /^#[\da-f]{6}$/i.test(value) ? value.toLowerCase() : '#000000' : /^(text|search|tel|password|url|email)$/.test(type) ? (value = value.replace(/[\r\n]/g, ''), type === 'url' || type === 'email' ? value.trim() : value) : /^(date|time|datetime-local|month|week)$/.test(type) && Number.isNaN(stepKinds[type][3](value)) ? '' : type === 'datetime-local' ? value.replace(' ', 'T') : value;
  const assign = (node, value) => { const old = node.value; setValue(node, value); if (value !== old) { node._start = node._end = value.length; node._direction = 'none'; if (active === node) call('dom.selection', node._id, value.length, value.length); } };
  // The field shows the first file, which the value names, but the value is not stored.
  const showFiles = node => { op(7, node._id, node.value, 0); revalidate(node); };
  const setValue = (node, value) => { node._value = value; op(7, node._id, value, 0); const length = value.length; node._start = Math.min(node._start ?? 0, length); node._end = Math.min(node._end ?? 0, length); revalidate(node); };
  const rangeBounds = node => { const number = name => { const value = parseFloat(node.getAttribute(name)); return Number.isFinite(value) ? value : null; }, min = number('min') ?? 0, step = number('step'); return [min, Math.max(min, number('max') ?? 100), node.getAttribute('step')?.trim().toLowerCase() === 'any' ? 0 : step > 0 ? step : 1]; };
  const rangeValue = (node, raw) => {
    const [min, max, step] = rangeBounds(node), text = String(raw ?? ''); let value = floatText.test(text) ? Number(text) : NaN;
    if (!Number.isFinite(value)) value = (min + max) / 2;
    else if (!step && value >= min && value <= max) return decimalText(text);
    value = Math.min(max, Math.max(min, value));
    if (step) { let n = Math.round((value - min) / step); if (min + n * step > max) n--; value = +(min + n * step).toPrecision(15); }
    return decimalText(String(value));
  };
  // Like Chrome's Decimal: keeps 18 digits of a valid float, and writes fractions with 15 digits at most.
  const floatText = /^-?(\d+(\.\d+)?|\.\d+)([eE][-+]?\d+)?$/;
  const decimalText = text => {
    const [, sign, whole, fraction = '', e = 0] = /^(-?)(\d*)(?:\.(\d*))?(?:e([-+]?\d+))?$/i.exec(text), int = whole.replace(/^0+/, '');
    let exp = +e, digits = int.length >= 18 ? int.slice(0, 18) : int + fraction.slice(0, 18 - int.length);
    exp += int.length >= 18 ? int.length - 18 : int.length - digits.length;
    digits = digits.replace(/^0+/, '');
    if (exp < 0) {
      if (digits.length > 15) { const up = digits[15] >= '5'; exp += digits.length - 15; digits = digits.slice(0, 15); if (up) digits = String(BigInt(digits) + 1n); }
      while (exp < 0 && digits.length > 1 && digits.endsWith('0')) { digits = digits.slice(0, -1); exp++; }
    }
    if (!digits) return '0';
    const adjusted = exp + digits.length - 1;
    if (exp <= 0 && adjusted >= -6) return sign + (!exp ? digits : adjusted >= 0 ? digits.slice(0, adjusted + 1) + '.' + digits.slice(adjusted + 1) : '0.' + '0'.repeat(-adjusted - 1) + digits);
    const mantissa = digits.replace(/0+$/, '');
    return sign + digits[0] + (mantissa.length > 1 ? '.' + mantissa.slice(1) : '') + (adjusted ? (adjusted < 0 ? 'e' : 'e+') + adjusted : '');
  };
  // [scale of `step`, default step, default step base, parse, format, is a date]. Dates count ms since the epoch, months count months.
  const pad = (n, size = 2) => String(n).padStart(size, '0'), dayOf = text => /^(\d{4,})-(\d\d)-(\d\d)$/.exec(text), timeOf = text => /^(\d\d):(\d\d)(?::(\d\d)(?:\.(\d{1,3}))?)?$/.exec(text);
  // `Date.UTC()` takes the years 0 to 99 as 1900 to 1999.
  const utc = (year, month, day) => new Date(0).setUTCFullYear(year, month, day);
  const day = text => { const m = dayOf(text); if (!m || !+m[1]) return NaN; const value = utc(+m[1], m[2] - 1, +m[3]); return new Date(value).getUTCDate() === +m[3] && m[2] > 0 && m[2] <= 12 ? value : NaN; };
  const clock = text => { const m = timeOf(text); return m && m[1] < 24 && m[2] < 60 && (m[3] ?? 0) < 60 ? ((m[1] * 60 + +m[2]) * 60 + +(m[3] ?? 0)) * 1000 + +(m[4] ?? '0').padEnd(3, '0') : NaN; };
  const dayText = value => { const d = new Date(value); return pad(d.getUTCFullYear(), 4) + '-' + pad(d.getUTCMonth() + 1) + '-' + pad(d.getUTCDate()); };
  const clockText = value => { const d = new Date(value), s = d.getUTCSeconds(), ms = d.getUTCMilliseconds(); return pad(d.getUTCHours()) + ':' + pad(d.getUTCMinutes()) + (s || ms ? ':' + pad(s) + (ms ? '.' + pad(ms, 3) : '') : ''); };
  const weekStart = year => { const jan4 = utc(year, 0, 4); return jan4 - (new Date(jan4).getUTCDay() + 6) % 7 * 864e5; };
  const numberOf = text => text === '' ? NaN : Number(text);
  const stepKinds = {
    number: [1, 1, 0, numberOf, String], range: [1, 1, 0, numberOf, String],
    date: [864e5, 1, 0, day, dayText, true], time: [1000, 60, 0, clock, clockText, true],
    'datetime-local': [1000, 60, 0, text => { const [date, time] = text.split(/[T ]/); return time === undefined ? NaN : day(date) + clock(time); }, value => dayText(value) + 'T' + clockText(value)],
    month: [1, 1, 0, text => { const m = /^(\d{4,})-(\d\d)$/.exec(text); return m && +m[1] && m[2] > 0 && m[2] <= 12 ? (m[1] - 1970) * 12 + (m[2] - 1) : NaN; }, value => pad(1970 + Math.floor(value / 12), 4) + '-' + pad(value % 12 + 1), true],
    week: [6048e5, 1, -2592e5, text => { const m = /^(\d{4,})-W(\d\d)$/.exec(text); if (!m || !+m[1] || !+m[2]) return NaN; const value = weekStart(+m[1]) + (m[2] - 1) * 6048e5; return value < weekStart(+m[1] + 1) ? value : NaN; }, value => { const year = new Date(value + 3 * 864e5).getUTCFullYear(); return pad(year, 4) + '-W' + pad((value - weekStart(year)) / 6048e5 + 1); }, true],
  };
  const stepBy = (node, n) => {
    const kind = stepKinds[node.type], attr = node.getAttribute('step');
    if (!kind || attr?.trim().toLowerCase() === 'any') throw new DOMException('This input cannot step', 'InvalidStateError');
    const [scale, fallback, origin, parse] = kind, step = (parseFloat(attr) > 0 ? parseFloat(attr) : fallback) * scale, min = parse(node.getAttribute('min') ?? ''), max = parse(node.getAttribute('max') ?? ''), start = parse(node.getAttribute('value') ?? '');
    const base = Number.isNaN(min) ? Number.isNaN(start) ? origin : start : min, value = Number.isNaN(node.valueAsNumber) ? 0 : node.valueAsNumber, at = (value - base) / step;
    let next = base + (Number.isInteger(at) ? at + n : n > 0 ? Math.ceil(at) + n - 1 : Math.floor(at) + n + 1) * step;
    if (next < min) next = base + Math.ceil((min - base) / step) * step; if (next > max) next = base + Math.floor((max - base) / step) * step;
    node.valueAsNumber = +next.toPrecision(15);
  };
  const setRange = (node, raw) => { const value = rangeValue(node, raw); if (value !== node.value) { setValue(node, value); dispatch(node, trusted(new Event('input', { bubbles: true, composed: true }))); } };
  const formState = (node, bits) => { if (node._bits !== bits) { node._bits = bits; op(8, node._id, bits); } };
  // User edits must change internal state, not value or checked properties: frameworks intercept those
  // properties to detect script writes.
  const setChecked = (node, value) => { node._checked = value; formState(node, node._bits & ~3 | (value ? 3 : 2)); const members = node.type === 'radio' ? group(node) : undefined; if (value && members) for (const other of members) if (other !== node && (other._checked ?? other.hasAttribute('checked'))) { other._checked = false; formState(other, other._bits & ~3 | 2); } revalidate(node, members); };
  // Text and bulk changes below a required select can change its validity.
  let requiredSelects = false;
  const optionsChanged = rec => { while (rec && rec.tag !== 'select') rec = rec.up; if (rec) revalidate(of(rec)); };
  const listed = /^(button|fieldset|input|object|output|select|textarea)$/;
  const candidate = node => !disabled(node) && !node.hasAttribute('readonly') && !node._m.up?.tag?.startsWith('datalist') && (node.localName === 'input' ? !/^(hidden|reset|button|image)$/.test(node.type) : node.localName === 'button' ? node.type === 'submit' : /^(select|textarea)$/.test(node.localName));
  const emailPattern = /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;
  // Rust matches `:valid` and `:invalid` with bit 128, so only controls that can fail a constraint are validated.
  const limited = (attrs, node) => node?._bits & 128 || node?._custom || attrs.list.some((name, i) => !(i & 1) && (/^(required|pattern|min|max|step|maxlength|minlength)$/.test(name) || name === 'type' && /^(email|url|number)$/i.test(attrs.list[i + 1])));
  const revalidate = (node, members) => {
    if (!node || !/^(input|select|textarea|button)$/.test(node.localName)) return;
    const radio = node.type === 'radio'; members = radio ? members ?? group(node) : [node];
    requiredSelects ||= node.localName === 'select' && node._m.attrs.has('required');
    if (members.some(other => limited(other._m.attrs, other))) for (const other of members) formState(other, validity(other, radio ? members : undefined).valid ? other._bits & ~128 : other._bits | 128);
  };
  class ValidityState {}
  for (const name of ['valueMissing','typeMismatch','patternMismatch','tooLong','tooShort','rangeUnderflow','rangeOverflow','stepMismatch','badInput','customError','valid']) Object.defineProperty(ValidityState.prototype, name, { get() { return this['_' + name]; }, enumerable: true });
  const messages = { valueMissing: 'Please fill out this field.', typeMismatch: 'Please enter a valid value.', patternMismatch: 'Please match the requested format.', tooLong: 'Please shorten this text.', tooShort: 'Please lengthen this text.', rangeUnderflow: 'Value is too low.', rangeOverflow: 'Value is too high.', stepMismatch: 'Please enter a valid value.', badInput: 'Please enter a number.' };
  const validity = (node, members) => {
    const flags = { valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: !!node._custom };
    const tag = node.localName, type = node.type, required = node.hasAttribute('required');
    if (tag === 'select') flags.valueMissing = required && !selectedness(node, true).some(option => option._current && ( option.value !== '' || option.index !== 0 || option._m.up.tag !== 'select' || node.multiple || node.size > 1));
    else if (type === 'checkbox') flags.valueMissing = required && !node.checked;
    else if (type === 'radio') { members ??= group(node); flags.valueMissing = members.some(radio => radio.hasAttribute('required')) && !members.some(radio => radio.checked); }
    else if (tag === 'textarea' || tag === 'input' && !/^(hidden|submit|image|reset|button|file)$/.test(type)) {
      const value = node.value, length = value.length, attr = name => node.getAttribute(name);
      flags.valueMissing = required && !value;
      if (value) {
        if (type === 'email') flags.typeMismatch = !(node.hasAttribute('multiple') ? value.split(',') : [value]).every(part => emailPattern.test(part.trim()));
        if (type === 'url') flags.typeMismatch = !URL.canParse(value);
        if (attr('pattern') !== null && tag === 'input') try { flags.patternMismatch = !(node.hasAttribute('multiple') && type === 'email' ? value.split(',') : [value]).every(part => new RegExp('^(?:' + attr('pattern') + ')$', 'v').test(part)); } catch {}
        if (node._edited !== undefined && attr('maxlength') !== null) flags.tooLong = length > Number(attr('maxlength'));
        if (node._edited !== undefined && attr('minlength') !== null) flags.tooShort = length < Number(attr('minlength'));
        if (type === 'number' || type === 'range') {
          const number = Number(value), min = parseFloat(attr('min')), max = parseFloat(attr('max')), step = attr('step') === 'any' ? NaN : parseFloat(attr('step') ?? '1');
          flags.badInput = Number.isNaN(number);
          if (!flags.badInput) { flags.rangeUnderflow = number < min; flags.rangeOverflow = number > max; flags.stepMismatch = step > 0 && Math.abs(Math.round((number - (Number.isNaN(min) ? 0 : min)) / step) * step - (number - (Number.isNaN(min) ? 0 : min))) > 1e-7; }
        }
      }
    }
    const state = Object.create(ValidityState.prototype);
    for (const [name, value] of Object.entries(flags)) state['_' + name] = value;
    state._valid = !Object.values(flags).includes(true);
    return state;
  };
  const formOf = (node, top) => { const id = node.getAttribute('form'); if (id !== null) { const form = !top && id && node.getRootNode().getElementById?.(id); return form?.localName === 'form' ? form : null; } for (let rec = node._m; rec && rec.tag[0] !== '#'; rec = rec === top ? null : rec.up) if (rec.tag === 'form') return of(rec); return null; };
  class CustomStateSet extends Set {
    _send() { if (this._node) op(11, this._node._id, [...this].join('\0')); }
    add(state) { super.add(String(state)); this._send(); }
    delete(state) { const found = super.delete(String(state)); if (found) this._send(); return found; }
    clear() { super.clear(); this._send(); }
  }
  class ElementInternals {
    constructor(node) { this._node = node; this._value = null; this._flags = {}; this._message = ''; }
    get states() { if (!this._states) { this._states = new CustomStateSet(); this._states._node = this._node; } return this._states; }
    get shadowRoot() { return this._node._shadow ?? null; }
    get form() { return formOf(face(this)._node); }
    get labels() { return Object.getOwnPropertyDescriptor(constraints, 'labels').get.call(face(this)._node); }
    get willValidate() { const node = face(this)._node; return !disabled(node) && !node.hasAttribute('readonly') && !node._m.up?.tag.startsWith('datalist'); }
    get validity() { const state = Object.create(ValidityState.prototype), flags = face(this)._flags; for (const name of Object.keys(messages)) state['_' + name] = !!flags[name]; state._customError = !!flags.customError; state._valid = !(this._node._bits & 128); return state; }
    get validationMessage() { return face(this)._message; }
    setFormValue(value) { face(this)._value = value; }
    setValidity(flags = {}, message = '') {
      const invalid = !!flags.customError || Object.keys(messages).some(name => flags[name]);
      if (invalid && !message) throw new TypeError('An invalid state needs a message');
      face(this)._flags = { ...flags }; this._message = invalid ? String(message) : '';
      flag(this._node, 128, invalid);
    }
    checkValidity() { if (!this.willValidate || this.validity.valid) return true; dispatch(this._node, new Event('invalid', { cancelable: true })); return false; }
    reportValidity() { if (this.checkValidity()) return true; this._node.focus(); return false; }
  }
  const face = internals => { if (!internals._node._face) throw new DOMException('The element is not form-associated', 'NotSupportedError'); return internals; };
  const constraints = {
    get willValidate() { return candidate(this); },
    get validity() { return validity(this); },
    get validationMessage() { if (!candidate(this)) return ''; const state = validity(this); return state.customError ? this._custom : Object.keys(messages).map(name => state[name] && messages[name]).find(Boolean) ?? ''; },
    setCustomValidity(message) { this._custom = String(message); revalidate(this); },
    checkValidity() { if (!candidate(this) || validity(this).valid) return true; dispatch(this, new Event('invalid', { cancelable: true })); return false; },
    reportValidity() { if (this.checkValidity()) return true; this.focus(); return false; },
    get form() { return formOf(this); },
    get labels() {
      if (this.type === 'hidden') return null;
      return live(this, 'labels', NodeList, () => {
      const result = [], root = this.getRootNode(), id = this.getAttribute('id');
      if (id && root.querySelectorAll) for (const label of root.querySelectorAll('label[for=' + quote(id) + ']')) if (labelControl(label) === this) result.push(label);
      for (let rec = this._m.up; rec; rec = rec.up) if (rec.tag === 'label' && !rec.attrs.has('for') && labelControl(of(rec)) === this) result.push(of(rec));
      return result.sort((a, b) => a.compareDocumentPosition(b) & 4 ? -1 : 1);
      }, { tags: ['label', 'button', 'input', 'meter', 'output', 'progress', 'select', 'textarea'], attrs: ['for', 'id', 'type'] });
    },
    get name() { return this.getAttribute('name') ?? ''; }, set name(value) { this.setAttribute('name', value); },
  };
  class HTMLInputElement extends HTMLElement {
    get type() { const type = (this.getAttribute('type') ?? '').toLowerCase(); return inputTypes.has(type) ? type : 'text'; } set type(value) { this.setAttribute('type', value); }
    get value() { const type = this.type; return /^(hidden|submit|image|reset|button)$/.test(type) ? this.getAttribute('value') ?? '' : type === 'checkbox' || type === 'radio' ? this.getAttribute('value') ?? 'on' : type === 'file' ? (this._files?.length ? 'C:\\fakepath\\' + this._files[0].name : '') : type === 'range' ? rangeValue(this, this._value ?? this.getAttribute('value')) : this._value ?? sanitize(type, this.getAttribute('value') ?? ''); }
    set value(value) { value = String(value ?? ''); if (/^(hidden|submit|image|reset|button|checkbox|radio)$/.test(this.type)) this.setAttribute('value', value); else if (this.type !== 'file' || !value) { if (!value && this._files) setFiles(this._files, []); assign(this, sanitize(this.type, value)); } }
    get defaultValue() { return this.getAttribute('value') ?? ''; } set defaultValue(value) { this.setAttribute('value', value); }
    get checked() { return this._checked ?? this.hasAttribute('checked'); }
    set checked(value) { setChecked(this, !!value); this._dirty = true; }
    get defaultChecked() { return this.hasAttribute('checked'); } set defaultChecked(value) { this.toggleAttribute('checked', !!value); }
    get indeterminate() { return !!(this._bits & 64); } set indeterminate(value) { flag(this, 64, !!value); }
    get valueAsNumber() { const kind = stepKinds[this.type]; return kind ? kind[3](this.value) : NaN; }
    set valueAsNumber(value) { const kind = stepKinds[this.type]; if (!kind) throw new DOMException('This input has no number value', 'InvalidStateError'); value = Number(value); if (!Number.isFinite(value) && !Number.isNaN(value)) throw new TypeError('The value is not finite'); this.value = Number.isNaN(value) ? '' : kind[4](value); }
    get valueAsDate() { const kind = stepKinds[this.type], value = kind?.[5] ? this.valueAsNumber : NaN; return Number.isNaN(value) ? null : new Date(this.type === 'month' ? Date.UTC(1970 + Math.floor(value / 12), value % 12) : value); }
    set valueAsDate(date) { if (!stepKinds[this.type]?.[5]) throw new DOMException('This input has no date value', 'InvalidStateError'); if (date !== null && !(date instanceof Date)) throw new TypeError('Expected a Date'); const time = date?.getTime() ?? NaN; this.valueAsNumber = this.type === 'month' && !Number.isNaN(time) ? (date.getUTCFullYear() - 1970) * 12 + date.getUTCMonth() : time; }
    stepUp(n = 1) { stepBy(this, n); } stepDown(n = 1) { stepBy(this, -n); }
    get files() { return this.type === 'file' ? this._files ??= fileList() : null; }
    set files(value) { if (value == null) return; if (!(value instanceof FileList)) throw new TypeError("Failed to set the 'files' property on 'HTMLInputElement': The provided value is not of type 'FileList'."); if (this.type === 'file') { this._files = value; showFiles(this); } }
    get list() { const id = this.getAttribute('list'); return id ? document.getElementById(id) : null; }
    // Like Chrome, the selection API works only on text fields. Editing code uses the fields, as number and email
    // inputs keep a selection too.
    get selectionStart() { return selectable(this) ? this._start ?? 0 : null; } set selectionStart(value) { this.setSelectionRange(value, Math.max(value, this.selectionEnd)); }
    get selectionEnd() { return selectable(this) ? this._end ?? 0 : null; } set selectionEnd(value) { this.setSelectionRange(Math.min(value, this.selectionStart), value); }
    get selectionDirection() { return selectable(this) ? this._direction ?? 'none' : null; } set selectionDirection(value) { this.setSelectionRange(this.selectionStart, this.selectionEnd, value); }
    setSelectionRange(start, end, direction = 'none', user = false) {
      if (!user && !selectable(this)) throw new DOMException(`The input element's type ('${this.type}') does not support selection.`, 'InvalidStateError');
      const length = this.value.length;
      end = Math.min(Number(end) >>> 0, length); start = Math.min(Number(start) >>> 0, end);
      direction = ['forward','backward'].includes(direction) ? direction : 'none';
      const changed = start !== (this._start ?? 0) || end !== (this._end ?? 0) || direction !== (this._direction ?? 'none');
      this._start = start; this._end = end; this._direction = direction;
      if (active === this) call('dom.selection', this._id, start, end);
      // Selections by the user fire `select` only when they select text, as in Chrome.
      if (changed) { selectionChanged(this); if (!user || start !== end) setTimeout(() => dispatch(this, new Event('select', { bubbles: true })), 0); }
    }
    // Chromium and WebKit focus the field in `select()`; editors rely on it to move typing into the field.
    select() { this.focus(); if (editable(this)) this.setSelectionRange(0, this.value.length, 'none', !selectable(this)); }
    setRangeText(text, start = this.selectionStart, end = this.selectionEnd, mode = 'preserve') {
      if (!selectable(this)) throw new DOMException(`The input element's type ('${this.type}') does not support selection.`, 'InvalidStateError');
      text = String(text); start = Number(start) >>> 0; end = Number(end) >>> 0;
      if (start > end) throw new DOMException('Invalid replacement range', 'IndexSizeError');
      if (!['select','start','end','preserve'].includes(mode)) throw new TypeError('Invalid selection mode');
      const value = this.value, oldStart = this._start ?? 0, oldEnd = this._end ?? 0;
      start = Math.min(start, value.length); end = Math.min(end, value.length);
      this.value = value.slice(0, start) + text + value.slice(end);
      const after = start + text.length, delta = text.length - (end - start);
      this.setSelectionRange(mode === 'select' || mode === 'start' ? start : mode === 'end' ? after : oldStart > end ? oldStart + delta : oldStart > start ? start : oldStart,
        mode === 'select' || mode === 'end' ? after : mode === 'start' ? start : oldEnd > end ? oldEnd + delta : oldEnd > start ? after : oldEnd);
    }
  }
  class HTMLTextAreaElement extends HTMLElement {
    get type() { return 'textarea'; }
    get value() { return this._value ?? this.textContent.replace(/\r\n?/g, '\n'); } set value(value) { assign(this, String(value ?? '').replace(/\r\n?/g, '\n')); }
    get defaultValue() { return this.textContent; } set defaultValue(value) { this.textContent = value; }
    get textLength() { return this.value.length; }
  }
  for (const name of ['selectionStart', 'selectionEnd', 'selectionDirection', 'setSelectionRange', 'select', 'setRangeText']) Object.defineProperty(HTMLTextAreaElement.prototype, name, Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, name));
  for (const attr of ['accept', 'alt', 'autocomplete', 'max', 'min', 'pattern', 'step']) Object.defineProperty(HTMLInputElement.prototype, attr, { get() { return this.getAttribute(attr) ?? ''; }, set(value) { this.setAttribute(attr, value); } });
  // Cached by `version`, which every tree and attribute change bumps. Never mutate the result.
  const options = select => { if (select._optionsAt === version) return select._options; const result = []; for (const kid of select._m.kids) { if (kid.tag === 'option') result.push(of(kid)); else if (kid.tag === 'optgroup') for (const sub of kid.kids) if (sub.tag === 'option') result.push(of(sub)); } select._optionsAt = version; return select._options = result; };
  // Getters pass `cached`: every write of `_selected` or `_none` runs a full pass after it.
  const selectedness = (select, cached) => {
    const list = options(select);
    if (cached && select._selectedAt === version) return list;
    if (select.multiple || select.size > 1) for (const option of list) option._current = option._selected ?? option.hasAttribute('selected');
    else {
      let chosen = null;
      for (const option of list) if (option._selected ?? option.hasAttribute('selected')) chosen = option;
      // Like browsers, the option picked when none is selected stays selected when options are added before it.
      if (!chosen && !(select._none?.length === list.length && select._none.every((option, i) => option === list[i]))) { chosen = list.find(option => !disabled(option)) ?? null; if (chosen) chosen._selected = true; }
      for (const option of list) option._current = option === chosen;
    }
    for (const option of list) formState(option, option._current ? 3 : 2);
    select._selectedAt = version;
    return list;
  };
  class HTMLOptionsCollection extends HTMLCollection {
    get selectedIndex() { return this[owner].selectedIndex; } set selectedIndex(value) { this[owner].selectedIndex = value; }
    add(element, before) { this[owner].add(element, before); }
    remove(index) { this[owner].remove(index); }
  }
  class LiveOptions extends Live { set(list, key, value) { const select = list[owner], i = tokenIndex(key); if (key === 'length') select.length = value; else if (i < 0) list[key] = value; else if (value === null) options(select)[i]?.remove(); else { if (i > select.length) select.length = i; const old = options(select)[i]; if (old) old.replaceWith(value); else select.append(value); } return true; } }
  class HTMLSelectElement extends HTMLElement {
    get type() { return this.multiple ? 'select-multiple' : 'select-one'; }
    get options() { return live(this, 'options', HTMLOptionsCollection, () => options(this), { root: this._m, tags: ['option', 'optgroup'] }, first, LiveOptions); }
    get length() { return options(this).length; }
    set length(value) { const list = options(this); value >>>= 0; for (let i = list.length; i < value; i++) this.append(document.createElement('option')); for (const option of list.slice(value)) option.remove(); }
    get size() { return Math.max(0, parseInt(this.getAttribute('size'), 10) || 0); } set size(value) { this.setAttribute('size', value >>> 0); }
    get selectedOptions() { return live(this, 'selectedOptions', HTMLCollection, () => selectedness(this, true).filter(option => option._current)); }
    get selectedIndex() { return selectedness(this, true).findIndex(option => option._current); }
    set selectedIndex(value) { value = Number(value); const list = options(this); this._none = list[value] ? null : list; for (const [index, option] of list.entries()) option._selected = index === value; selectedness(this); revalidate(this); }
    get value() { return selectedness(this, true).find(option => option._current)?.value ?? ''; }
    set value(value) { value = String(value); const list = options(this); let found = false; for (const option of list) option._selected = !found && option.value === value && (found = true); this._none = found ? null : list; selectedness(this); revalidate(this); }
    item(index) { return options(this)[index] ?? null; }
    showPicker() { if (!native) throw new DOMException('showPicker() needs a user gesture', 'NotAllowedError'); openSelect(this); }
    namedItem(name) { return this.options.namedItem(name); }
    add(element, before = null) { const reference = typeof before === 'number' ? options(this)[before] ?? null : before; if (reference) reference.parentNode.insertBefore(element, reference); else this.append(element); }
    remove(index) { if (!arguments.length) super.remove(); else options(this)[index]?.remove(); }
  }
  class HTMLOptionElement extends HTMLElement {
    get select() { for (let rec = this._m.up, depth = 0; rec && depth < 2; rec = rec.up, depth++) { if (rec.tag === 'select') return of(rec); if (rec.tag !== 'optgroup') return null; } return null; }
    get value() { return this.getAttribute('value') ?? this.text; } set value(value) { this.setAttribute('value', value); }
    get text() { return this.textContent.replace(/[\t\n\f\r ]+/g, ' ').trim(); } set text(value) { this.textContent = value; }
    get label() { return this.getAttribute('label') ?? this.text; } set label(value) { this.setAttribute('label', value); }
    get selected() { const select = this.select; if (select) selectedness(select, true); return this._current ?? this._selected ?? this.hasAttribute('selected'); }
    set selected(value) { const select = this.select; this._selected = !!value; if (select) { if (value && !select.multiple) for (const option of options(select)) if (option !== this) option._selected = false; select._none = null; selectedness(select); revalidate(select); } else this._current = !!value; }
    get defaultSelected() { return this.hasAttribute('selected'); } set defaultSelected(value) { this.toggleAttribute('selected', !!value); }
    get index() { const select = this.select; return select ? options(select).indexOf(this) : 0; }
    get form() { return this.select?.form ?? null; }
  }
  class HTMLOptGroupElement extends HTMLElement { get label() { return this.getAttribute('label') ?? ''; } set label(value) { this.setAttribute('label', value); } }
  class HTMLButtonElement extends HTMLElement {
    get type() { const type = (this.getAttribute('type') ?? '').toLowerCase(); return /^(submit|reset|button)$/.test(type) ? type : this.hasAttribute('commandfor') ? 'button' : 'submit'; } set type(value) { this.setAttribute('type', value); }
    get value() { return this.getAttribute('value') ?? ''; } set value(value) { this.setAttribute('value', value); }
  }
  class HTMLFieldSetElement extends HTMLElement { get elements() { return live(this, 'elements', HTMLCollection, () => all(this._m, rec => listed.test(rec.tag) || associated(rec)), { root: this._m, tags: controlTags }); } get type() { return 'fieldset'; } }
  class HTMLOutputElement extends HTMLElement { get value() { return this.textContent; } set value(value) { this.textContent = value; } get defaultValue() { return this._default ?? this.textContent; } set defaultValue(value) { this._default = String(value); } get type() { return 'output'; } }
  for (const Type of [HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLButtonElement, HTMLFieldSetElement, HTMLOutputElement]) for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(constraints))) if (!Object.hasOwn(Type.prototype, name)) Object.defineProperty(Type.prototype, name, descriptor);
  class HTMLFormControlsCollection extends HTMLCollection { namedItem(name) { return this[lookup](String(name)) ?? null; } }
  class RadioNodeList extends NodeList {
    get value() { return (this[raw] ?? this).find(node => node.type === 'radio' && node.checked)?.value ?? ''; }
    set value(value) { const radio = (this[raw] ?? this).find(node => node.type === 'radio' && node.value === String(value)); if (radio) radio.checked = true; }
  }
  const controlTags = ['button', 'fieldset', 'input', 'object', 'output', 'select', 'textarea', '-', 'form'];
  const associated = rec => faces > 0 && !!definitions.get(rec.tag)?.formAssociated;
  const controlsOf = form => {
    const id = form.getAttribute('id'), root = form.getRootNode(), control = rec => listed.test(rec.tag) ? !(rec.tag === 'input' && rec.attrs.get('type')?.toLowerCase() === 'image') && formOf(of(rec)) === form : associated(rec) && formOf(of(rec)) === form;
    const result = all(form._m, control), outside = id && root !== form ? all(root._m, rec => rec.attrs.get('form') === id && control(rec)).filter(node => !form.contains(node)) : [];
    if (outside.length) { result.push(...outside); result.sort((a, b) => a.compareDocumentPosition(b) & 4 ? -1 : 1); }
    return result;
  };
  const submitForm = (form, submitter) => {
    if (form._submitting || !form.isConnected) return;
    for (const control of controlsOf(form)) if (/^(input|select|textarea)$/.test(control.localName)) interacted(control);
    if (!form.noValidate && !submitter?.formNoValidate) { const invalid = controlsOf(form).filter(control => !((control._internals ?? control).checkValidity?.() ?? true)); if (invalid.length) { invalid[0].focus(); return; } }
    form._submitting = true;
    let allowed;
    try { allowed = dispatch(form, new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter })); } finally { form._submitting = false; }
    if (allowed && (submitter?.getAttribute('formmethod') ?? form.method) === 'dialog') form.closest('dialog')?.close(submitter?.value);
  };
  class HTMLFormElement extends HTMLElement {
    get elements() {
      return live(this, 'elements', HTMLFormControlsCollection, () => controlsOf(this), { tags: controlTags, attrs: ['form', 'type', 'form id'] }, name => live(this, 'named ' + name, RadioNodeList, () => controlsOf(this).filter(({ _m: { attrs } }) => attrs.get('id') === name || attrs.get('name') === name), { tags: controlTags, attrs: ['form', 'type', 'form id', 'id', 'name'] }, null));
    }
    get length() { return controlsOf(this).length; }
    get name() { return this.getAttribute('name') ?? ''; } set name(value) { this.setAttribute('name', value); }
    get method() { const value = (this.getAttribute('method') ?? '').toLowerCase(); return value === 'post' || value === 'dialog' ? value : 'get'; } set method(value) { this.setAttribute('method', value); }
    get action() { const value = this.getAttribute('action'); return value ? resolved(value) : document.URL; } set action(value) { this.setAttribute('action', value); }
    get enctype() { const value = this.getAttribute('enctype')?.toLowerCase(); return value === 'multipart/form-data' || value === 'text/plain' ? value : 'application/x-www-form-urlencoded'; } set enctype(value) { this.setAttribute('enctype', value); }
    get encoding() { return this.enctype; } set encoding(value) { this.enctype = value; }
    get autocomplete() { return this.getAttribute('autocomplete')?.toLowerCase() === 'off' ? 'off' : 'on'; } set autocomplete(value) { this.setAttribute('autocomplete', value); }
    get acceptCharset() { return this.getAttribute('accept-charset') ?? ''; } set acceptCharset(value) { this.setAttribute('accept-charset', value); }
    get target() { return this.getAttribute('target') ?? ''; } set target(value) { this.setAttribute('target', value); }
    submit() { if (this.method === 'dialog') this.closest('dialog')?.close(); }
    requestSubmit(submitter = null) { if (submitter !== null && (!(submitter instanceof HTMLElement) || submitter.form !== this || !(submitter.localName === 'button' ? submitter.type === 'submit' : /^(submit|image)$/.test(submitter.type)))) throw submitter?.form !== this ? new DOMException('The submitter is not owned by this form', 'NotFoundError') : new TypeError('The submitter is not a submit button'); submitForm(this, submitter); }
    reset() {
      if (!dispatch(this, new Event('reset', { bubbles: true, cancelable: true }))) return;
      const radios = [];
      for (const control of controlsOf(this)) {
        if (control.localName === 'select') { for (const option of options(control)) option._selected = undefined; control._none = null; selectedness(control); }
        else if (control.localName === 'input' || control.localName === 'textarea') { const old = control.value; control._value = control._checked = control._edited = control._dirty = undefined; if (control._files) setFiles(control._files, []); op(7, control._id, '', 1); formState(control, control._bits & ~3); if (control.value !== old) { const end = control._start = control._end = control.value.length; control._direction = 'none'; if (active === control) call('dom.selection', control._id, end, end); } if (control.type === 'radio' && control.hasAttribute('checked')) setChecked(control, true); }
        else if (control.localName === 'output') control.textContent = control.defaultValue;
        else if (control._face) enqueue(control, ['formResetCallback']);
        flag(control, 256, false);
        if (control.localName === 'input' && control.type === 'radio') radios.push(control); else revalidate(control);
      }
      const done = new Set();
      for (const radio of radios) if (!done.has(radio)) { const members = group(radio); for (const member of members) done.add(member); revalidate(radio, members); }
    }
    checkValidity() { return controlsOf(this).map(control => (control._internals ?? control).checkValidity?.() ?? true).every(Boolean); }
    reportValidity() { const invalid = controlsOf(this).filter(control => !((control._internals ?? control).checkValidity?.() ?? true)); invalid[0]?.focus(); return !invalid.length; }
    _entries(submitter, data) {
      if (this._building) throw new DOMException('The form is already building its entry list', 'InvalidStateError');
      for (const control of controlsOf(this)) {
        const tag = control.localName, type = control.type, name = control.getAttribute('name'), value = control._internals?._value;
        if (!listed.test(tag)) { if (value instanceof FormData) for (const [key, item] of value) data.append(key, item); else if (name && value != null && !control.matches(':disabled')) data.append(name, value); continue; }
        let inList = false; for (let rec = control._m.up; rec && !inList; rec = rec.up) inList = rec.tag === 'datalist';
        if (!name || disabled(control) || inList || tag === 'button' && control !== submitter || tag === 'input' && /^(submit|image|reset|button)$/.test(type) && control !== submitter || /^(checkbox|radio)$/.test(type) && !control.checked || tag === 'object' || tag === 'output' || tag === 'fieldset') continue;
        if (tag === 'select') { for (const option of selectedness(control, true)) if (option._current) data.append(name, option.value); }
        else if (type === 'file') for (const file of control.files.length ? control.files : [new File([], '', { type: 'application/octet-stream' })]) data.append(name, file);
        else data.append(name, control.value);
      }
      this._building = true;
      try { dispatch(this, trusted(new FormDataEvent('formdata', { bubbles: true, formData: data }))); } finally { this._building = false; }
    }
  }
  // Match Rust's top-layer protocol: op 9 for order, op 8 flags 4 (modal), 8 (popover), 16 (fullscreen).
  const layer = (node, on) => { const i = topLayer.indexOf(node); if (i >= 0) topLayer.splice(i, 1); if (on) topLayer.push(node); op(9, node._id, on ? 1 : 0); };
  const flag = (node, bit, on) => formState(node, on ? node._bits | bit : node._bits & ~bit);
  const drop = (list, node) => { const i = list.indexOf(node); if (i >= 0) list.splice(i, 1); };
  const toggled = (node, oldState, newState, source = null) => {
    if (node._toggle) { Object.assign(node._toggle, { newState, source }); return; }
    const state = node._toggle = { oldState, newState, source };
    setTimeout(() => { node._toggle = null; dispatch(node, new ToggleEvent('toggle', state)); }, 0);
  };
  const popoverState = value => value === null ? null : /^(auto|)$/i.test(value) ? 'auto' : /^hint$/i.test(value) ? 'hint' : 'manual';
  const stackOf = node => node._popover === 'hint' ? hints : autos;
  const position = node => { const i = autos.indexOf(node); return i >= 0 ? i : hints.includes(node) ? autos.length + hints.indexOf(node) : -1; };
  const nearestPopover = (node, list) => { for (const item of node ? path(node) : []) if (list.includes(item)) return item; return null; };
  const ancestorPopover = (node, list, invoker) => [path(node)[1], invoker].map(item => nearestPopover(item, list)).reduce((a, b) => b && (!a || list.indexOf(b) > list.indexOf(a)) ? b : a, null);
  const hideAll = (list, end) => { for (let guard = 0; list.length && list.at(-1) !== end && guard < 1000; guard++) hidePopover(list.at(-1), false, true); };
  const hideUntil = end => { if (hints.includes(end)) return hideAll(hints, end); hideAll(hints, null); if (end === document || autos.includes(end)) hideAll(autos, end === document ? null : end); };
  const popoverReady = (node, showing) => {
    if (popoverState(node.getAttribute('popover')) === null) throw new DOMException('The element is not a popover', 'NotSupportedError');
    if (!!node._popover !== showing) return false;
    if (!node.isConnected || node.localName === 'dialog' && node._modal || fullscreens.includes(node)) throw new DOMException('The popover is not connected, or is a modal dialog or fullscreen', 'InvalidStateError');
    return true;
  };
  const showPopover = (node, source) => {
    if (!popoverReady(node, false) || !dispatch(node, new ToggleEvent('beforetoggle', { cancelable: true, oldState: 'closed', newState: 'open', source })) || !popoverReady(node, false)) return;
    const mode = popoverState(node.getAttribute('popover'));
    if (mode === 'hint') hideAll(hints, ancestorPopover(node, hints, source));
    else if (mode === 'auto') hideUntil(ancestorPopover(node, autos, source) ?? document);
    if (popoverState(node.getAttribute('popover')) !== mode) throw new DOMException('The popover type changed while it opened', 'InvalidStateError');
    if (!popoverReady(node, false)) return;
    node._popover = mode; node._returnFocus = active;
    if (mode !== 'manual') { stackOf(node).push(node); watchers.push(node); }
    if (source || node._source) { node._source = source; call('dom.popoverSource', node._id, source?._id ?? 0); }
    flag(node, 8, true); layer(node, true);
    (node.hasAttribute('autofocus') ? node : [...node.querySelectorAll('[autofocus]')].find(focusable))?.focus();
    toggled(node, 'closed', 'open', source);
  };
  const hidePopover = (node, focus, events, source = null) => {
    if (!node._popover) return;
    if (node._popover !== 'manual') hideAll(stackOf(node), node);
    if (events) { dispatch(node, new ToggleEvent('beforetoggle', { oldState: 'open', newState: 'closed', source })); if (node._popover && node._popover !== 'manual') hideAll(stackOf(node), node); }
    if (!node._popover) return;
    drop(stackOf(node), node); drop(watchers, node);
    node._popover = null; flag(node, 8, false); layer(node, false);
    if (events) toggled(node, 'open', 'closed', source);
    const previous = node._returnFocus; node._returnFocus = null;
    if (focus && previous?.isConnected && active && node.contains(active)) previous.focus();
  };
  const closeDialog = dialog => {
    drop(dialogs, dialog); drop(watchers, dialog);
    if (dialog._modal) { dialog._modal = false; flag(dialog, 4, false); layer(dialog, false); }
  };
  const openDialog = (dialog, modal) => {
    if (!dispatch(dialog, new ToggleEvent('beforetoggle', { cancelable: true, oldState: 'closed', newState: 'open' })) || dialog.open || modal && (!dialog.isConnected || dialog._popover)) return;
    toggled(dialog, 'closed', 'open');
    dialog.setAttribute('open', '');
    dialog._focus = active; dialogs.push(dialog); watchers.push(dialog);
    if (modal) { dialog._modal = true; flag(dialog, 4, true); layer(dialog, true); }
    hideUntil(ancestorPopover(dialog, hints, null) ?? ancestorPopover(dialog, autos, null) ?? document);
    ([...dialog.querySelectorAll('[autofocus]')].find(focusable) ?? [...dialog.querySelectorAll('*')].find(focusable) ?? dialog).focus();
  };
  class HTMLDialogElement extends HTMLElement {
    get open() { return this.hasAttribute('open'); } set open(value) { this.toggleAttribute('open', !!value); }
    get returnValue() { return this._returnValue ?? ''; } set returnValue(value) { this._returnValue = String(value); }
    get closedBy() { const value = (this.getAttribute('closedby') ?? '').toLowerCase(); return /^(any|closerequest|none)$/.test(value) ? value : this._modal ? 'closerequest' : 'none'; } set closedBy(value) { this.setAttribute('closedby', value); }
    show() { if (this.open) { if (this._modal) throw new DOMException('The dialog is modal', 'InvalidStateError'); return; } openDialog(this, false); }
    showModal() {
      if (this.open) { if (this._modal) return; throw new DOMException('The dialog is already open', 'InvalidStateError'); }
      if (!this.isConnected) throw new DOMException('The dialog is not connected', 'InvalidStateError');
      if (this._popover) throw new DOMException('The dialog is a shown popover', 'InvalidStateError');
      openDialog(this, true);
    }
    close(result) {
      if (!this.open) return;
      dispatch(this, new ToggleEvent('beforetoggle', { oldState: 'open', newState: 'closed' }));
      if (!this.open) return;
      // Chrome fires `close` before `toggle`.
      setTimeout(() => dispatch(this, new Event('close')), 0);
      toggled(this, 'open', 'closed');
      this.removeAttribute('open');
      if (result !== undefined && result !== null) this.returnValue = result;
      const previous = this._focus; this._focus = null;
      if (previous?.isConnected && active && this.contains(active)) previous.focus();
    }
    requestClose(result) { if (this.open && dispatch(this, new Event('cancel', { cancelable: true }))) this.close(result); }
  }
  const referenced = (node, name, key) => { const set = node[key]?.deref(); if (set && node.hasAttribute(name)) return set; const id = node.getAttribute(name); return id ? node.getRootNode().getElementById?.(id) ?? null : null; };
  const reference = (node, name, key, value) => { if (value === null || value === undefined) { node[key] = null; node.removeAttribute(name); } else { if (!(value instanceof Element)) throw new TypeError('Expected an Element'); node[key] = new WeakRef(value); node.setAttribute(name, ''); } };
  const commands = /^(toggle-popover|show-popover|hide-popover|show-modal|close|request-close)$/;
  const invoked = node => {
    if (!(node instanceof HTMLButtonElement || node instanceof HTMLInputElement && /^(button|submit|reset|image)$/.test(node.type)) || disabled(node)) return null;
    const target = node.localName === 'button' && node.commandForElement;
    if (target) return /popover$/.test(node.command) && target.popover !== null ? target : null;
    if (node.form && (node.localName === 'button' ? node.type === 'submit' : /^(submit|image)$/.test(node.type))) return null;
    const popover = node.popoverTargetElement;
    return popover?.popover ? popover : null;
  };
  const invoke = (button, target) => {
    const element = button.localName === 'button' ? button.commandForElement : null;
    if (element) {
      const command = button.command, custom = command.startsWith('--');
      if (!command || !custom && element.popover === null && !(element.localName === 'dialog' && (command === 'show-modal' ? !element.open : /close$/.test(command) && element.open))) return;
      if (!dispatch(element, new CommandEvent('command', { cancelable: true, command, source: button })) || !element.isConnected || custom) return;
      if (command === 'hide-popover' || command === 'toggle-popover' && element._popover) hidePopover(element, true, true, button);
      else if (/popover$/.test(command)) { if (!element._popover && element.popover !== null && !(element.localName === 'dialog' && element._modal) && !fullscreens.includes(element)) showPopover(element, button); }
      else if (element.localName !== 'dialog') return;
      else if (command === 'show-modal') { if (!element.open && !element._popover) element.showModal(); }
      else if (command === 'close') element.close(button.value);
      else element.requestClose(button.value);
      return;
    }
    const popover = invoked(button);
    if (!popover || popover.contains(target) && button !== popover && popover.contains(button)) return;
    const action = button.popoverTargetAction;
    if (popover._popover) { if (action !== 'show') hidePopover(popover, true, true, button); }
    else if (action !== 'hide' && popover.isConnected && !(popover.localName === 'dialog' && popover._modal) && !fullscreens.includes(popover)) showPopover(popover, button);
  };
  class CommandEvent extends Event { constructor(type, options = {}) { super(type, options); this.command = String(options.command ?? ''); this.source = options.source ?? null; } }
  for (const Type of [HTMLButtonElement, HTMLInputElement]) Object.defineProperties(Type.prototype, {
    popoverTargetElement: { configurable: true, get() { return referenced(this, 'popovertarget', '_popoverTarget'); }, set(value) { reference(this, 'popovertarget', '_popoverTarget', value); } },
    popoverTargetAction: { configurable: true, get() { const value = (this.getAttribute('popovertargetaction') ?? '').toLowerCase(); return value === 'show' || value === 'hide' ? value : 'toggle'; }, set(value) { this.setAttribute('popovertargetaction', value); } },
  });
  Object.defineProperties(HTMLButtonElement.prototype, {
    commandForElement: { configurable: true, get() { return referenced(this, 'commandfor', '_commandFor'); }, set(value) { reference(this, 'commandfor', '_commandFor', value); } },
    command: { configurable: true, get() { const value = this.getAttribute('command') ?? ''; return value.startsWith('--') ? value : commands.test(value.toLowerCase()) ? value.toLowerCase() : ''; }, set(value) { this.setAttribute('command', value); } },
  });
  Object.defineProperty(HTMLElement.prototype, 'popover', { configurable: true, get() { return popoverState(this.getAttribute('popover')); }, set(value) { if (value === null) this.removeAttribute('popover'); else this.setAttribute('popover', value); } });
  Object.assign(HTMLElement.prototype, {
    showPopover(options = {}) { popoverReady(this, false); showPopover(this, options?.source ?? null); },
    hidePopover() { if (popoverReady(this, true)) hidePopover(this, true, true); },
    togglePopover(options) {
      const force = typeof options === 'boolean' ? options : options?.force, source = typeof options === 'object' ? options?.source ?? null : null;
      if (this._popover && force !== true) { if (popoverReady(this, true)) hidePopover(this, true, true, source); }
      else if (!this._popover && force !== false) { popoverReady(this, false); showPopover(this, source); }
      else popoverReady(this, !this._popover);
      return !!this._popover;
    },
  });
  let pressedPopover = null, pressedDialog = null;
  const exitFullscreen = (all, native = false) => {
    const exited = all ? fullscreens.splice(0) : fullscreens.splice(-1);
    for (const node of exited.reverse()) { flag(node, 16, false); layer(node, false); }
    if (!native && exited.length && !fullscreens.length) call('dom.fullscreen', false);
    return new Promise(resolve => setTimeout(() => { for (const node of exited) dispatch(node.isConnected ? node : document, trusted(new Event('fullscreenchange', { bubbles: true, composed: true }))); resolve(); }, 0));
  };
  const requestFullscreen = function () {
    return new Promise((resolve, reject) => {
      if (!this.isConnected || this.localName === 'dialog' && this._modal || this.namespaceURI !== XHTML && this.localName !== 'svg' && this.localName !== 'math') {
        setTimeout(() => { dispatch(this.isConnected ? this : document, trusted(new Event('fullscreenerror', { bubbles: true, composed: true }))); reject(new TypeError('The element cannot enter fullscreen')); }, 0);
        return;
      }
      if (fullscreens.at(-1) === this) return resolve();
      if (this._popover) hidePopover(this, true, true);
      hideUntil(document);
      const first = !fullscreens.length;
      drop(fullscreens, this); fullscreens.push(this); flag(this, 16, true); layer(this, true);
      if (first) call('dom.fullscreen', true);
      setTimeout(() => { dispatch(this, trusted(new Event('fullscreenchange', { bubbles: true, composed: true }))); resolve(); }, 0);
    });
  };
  Object.assign(Element.prototype, { requestFullscreen, webkitRequestFullscreen: requestFullscreen, webkitRequestFullScreen: requestFullscreen });
  class HTMLDetailsElement extends HTMLElement { get open() { return this.hasAttribute('open'); } set open(value) { this.toggleAttribute('open', !!value); } }
  class HTMLLabelElement extends HTMLElement { get htmlFor() { return this.getAttribute('for') ?? ''; } set htmlFor(value) { this.setAttribute('for', value); } get control() { return labelControl(this); } get form() { return this.control?.form ?? null; } }
  const resolved = value => { try { return new URL(value, document.baseURI).href; } catch { return value; } };
  class HTMLTemplateElement extends HTMLElement {
    get content() { if (!this._content) { this._content = new DocumentFragment(); this._content.append(...this._m.kids.map(of)); } return this._content; }
    get innerHTML() { return serialize(this.content._m, true); } set innerHTML(value) { setHTML(this.content, value, true); }
    cloneNode(deep = false) { const clone = super.cloneNode(false); if (deep) clone.content.append(this.content.cloneNode(true)); return clone; }
  }
  class HTMLImageElement extends HTMLElement {
    get naturalWidth() { return this._dimensions?.[0] ?? 0; } get naturalHeight() { return this._dimensions?.[1] ?? 0; }
    get width() { return Number(this.getAttribute('width') ?? this.naturalWidth); } set width(value) { this.setAttribute('width',value); }
    get height() { return Number(this.getAttribute('height') ?? this.naturalHeight); } set height(value) { this.setAttribute('height',value); }
    get complete() { return !this.src || !!this._complete; }
    get currentSrc() { return this.src; }
    setAttribute(name,value) { super.setAttribute(name,value); if (name === 'src') this._loadImage(); }
    _loadImage() {
      const raw = this.getAttribute('src'), source = this.src, load = this._load = {}, current = () => this._load === load && this.getAttribute('src') === raw; this._complete = false; if (this._bits & 32) flag(this, 32, false);
      this._loading = (async () => {
        const data = await resource(source, 'Image');
        if (!current()) return;
        const result = await decode(this, data);
        if (!current()) return;
        if (typeof result === 'string') throw new Error(result);
        this._dimensions=result; this._complete=true; dispatch(this, new Event('load'));
      })();
      this._loading.catch(()=>{ if (current()) {this._complete=true; flag(this, 32, true); dispatch(this, new Event('error'));} });
    }
    decode() { if (!this._loading && this.src) this._loadImage(); return this._loading ?? Promise.resolve(); }
  }
  const resource = async (source, kind) => {
    if (source.startsWith('data:')) { const comma = source.indexOf(','), payload = source.slice(comma + 1); return source.slice(0, comma).endsWith(';base64') ? Uint8Array.fromBase64(payload) : new TextEncoder().encode(decodeURIComponent(payload)); }
    if (/^(apps|file):$/.test(new URL(source, globalThis.location?.href).protocol)) return source;
    const response = await fetch(source); if (!response.ok) throw new Error(`${kind} request failed: ${response.status}`); return new Uint8Array(await response.arrayBuffer());
  };
  const decode = (node, data) => new Promise(resolve => decodes.set(call(typeof data === 'string' ? 'dom.imageURL' : 'dom.image', node._id, data), resolve));
  const decodes = new Map();
  globalThis.__appsDecoded = () => {
    if (decodes.size) for (const [request, result] of host('dom.decoded')) { decodes.get(request)?.(result); decodes.delete(request); }
    if (placeholders) for (const [node, width, height] of call('dom.frames')) { const canvas = wrap(node); if (canvas.width !== width) canvas.setAttribute('width', width); if (canvas.height !== height) canvas.setAttribute('height', height); }
  };
  globalThis.__appsImages = root => { find(root._m, rec => { if (rec.tag === 'img' && rec.attrs.has('src')) { const image = of(rec); if (!image._loading) image._loadImage?.(); } else if ((rec.tag === 'video' || rec.tag === 'audio') && !of(rec)._network) of(rec)._select(); return false; }); };
  // Retain playing media after page code drops it, as browsers do.
  const players = new Map(), playing = new Set(), mediaFeature = globalThis.__appsMedia;
  // Node ids can come back, so a collected element closes its player only when no live element has the id.
  const released = new FinalizationRegistry(id => { if (!players.get(id)?.deref()) { players.delete(id); call('media.close', id); } });
  delete globalThis.__appsMedia;
  const [PLAY, PAUSE, SEEK, VOLUME, MUTED, SPEED] = [0, 1, 2, 3, 4, 5];
  const control = (el, op, value = 0) => { if (el._open) call('media.control', el._id, op, +value); };
  // Without the media feature, the built-in player plays only PCM WAV in <audio>.
  const mediaTypes = mediaFeature ? /^(video|audio)\/(mp4|webm|ogg|quicktime|x-matroska|mpeg|mp3|x-m4a|aac|flac|x-flac|wav|wave|x-wav)$/ : /^audio\/(wav|wave|x-wav)$/, mediaCodecs = mediaFeature ? /^(avc[13]|hvc1|hev1|vp8|vp9|vp09|av01|mp4a|mp3|opus|vorbis|flac|alac|1)(\.|$)/ : /^1$/;
  globalThis.__appsMediaPoll = () => {
    if (!players.size) return;
    for (const el of playing) if (el._connected && !el.isConnected) el.pause();
    for (const [id, kind, ...values] of call('media.poll')) { const el = players.get(id)?.deref(); if (el?._open) el._event(kind, values); else players.delete(id); }
  };
  class TimeRanges { constructor(ranges = []) { this._ranges = ranges; } get length() { return this._ranges.length; } start(i) { return this._range(i)[0]; } end(i) { return this._range(i)[1]; } _range(i) { const range = this._ranges[i]; if (!range) throw new DOMException('Index out of range', 'IndexSizeError'); return range; } }
  class MediaError { constructor(code, message) { this.code = code; this.message = message; } }
  Object.assign(MediaError, { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 });
  class HTMLMediaElement extends HTMLElement {
    get paused() { return this._paused ?? true; } get ended() { return !!this._ended; } get seeking() { return !!this._seeking; }
    get readyState() { return this._ready ?? 0; } get networkState() { return this._network ?? 0; } get error() { return this._error ?? null; }
    get duration() { return this._duration ?? NaN; } get currentSrc() { return this._source ?? ''; }
    get currentTime() { if (!this._open || !this._ready) return this._time ?? 0; const time = call('media.time', this._id); return this._duration < time ? this._duration : time; }
    set currentTime(value) { value = +value; if (!Number.isFinite(value)) throw new TypeError('The provided double value is non-finite.'); this._seek(value); }
    fastSeek(time) { this.currentTime = time; }
    get volume() { return this._volume ?? 1; }
    set volume(value) { value = +value; if (!(value >= 0 && value <= 1)) throw new DOMException('The volume must be between 0 and 1', 'IndexSizeError'); if (value === this.volume) return; this._volume = value; control(this, VOLUME, value); this._fire('volumechange'); }
    get muted() { return this._muted ?? this.hasAttribute('muted'); }
    set muted(value) { value = !!value; if (value === this.muted) return; this._muted = value; control(this, MUTED, value); this._fire('volumechange'); }
    get playbackRate() { return this._rate ?? 1; }
    set playbackRate(value) { value = +value; if (!(value >= 0.0625 && value <= 16)) throw new DOMException('The playback rate is not supported', 'NotSupportedError'); if (value === this.playbackRate) return; this._rate = value; control(this, SPEED, value); this._fire('ratechange'); }
    get defaultPlaybackRate() { return this._defaultRate ?? 1; } set defaultPlaybackRate(value) { this._defaultRate = +value; this._fire('ratechange'); }
    get preservesPitch() { return false; } set preservesPitch(value) {}
    get preload() { const value = this.getAttribute('preload')?.toLowerCase(); return value === undefined ? 'metadata' : value === 'none' || value === 'metadata' ? value : 'auto'; } set preload(value) { this.setAttribute('preload', value); }
    get buffered() { return new TimeRanges(this._ready ? [[0, this.duration]] : []); } get seekable() { return this.buffered; } get played() { return new TimeRanges(); }
    get srcObject() { return null; } set srcObject(value) { if (value !== null) throw new DOMException('Media streams are not supported', 'NotSupportedError'); }
    canPlayType(type) {
      if (!this._enabled) return '';
      const [mime, ...params] = String(type).toLowerCase().split(';').map(part => part.trim()), codecs = params.find(part => part.startsWith('codecs='))?.slice(7).replace(/"/g, '');
      return !mediaTypes.test(mime) ? '' : codecs === undefined ? 'maybe' : codecs.split(',').every(codec => mediaCodecs.test(codec.trim())) ? 'probably' : '';
    }
    get _enabled() { return mediaFeature || this.localName === 'audio'; }
    setAttribute(name, value) { super.setAttribute(name, value); if (name === 'src') this._select(); else if (name === 'poster') this._poster(); }
    insertBefore(child, before) { const node = super.insertBefore(child, before); if (!this._network) this._select(); return node; }
    load() { this._select(true); }
    play() {
      if (this._error?.code === 4) return Promise.reject(new DOMException(this._error.message, 'NotSupportedError'));
      if (!this._open && this._network !== 2) this._select(true);
      if (this._ended) this._seek(0);
      const promise = new Promise((resolve, reject) => (this._plays ??= []).push([resolve, reject]));
      if (this.paused) { this._paused = false; this._connected = this.isConnected; playing.add(this); control(this, PLAY); this._fire('play'); if (this._ready > 2) this._fire('playing'); }
      if (this._ready > 2) this._settle();
      return promise;
    }
    pause() {
      if (this.paused) return;
      this._paused = true; playing.delete(this); control(this, PAUSE);
      this._fire('timeupdate'); this._fire('pause'); this._settle(new DOMException('The play() request was interrupted by a call to pause()', 'AbortError'));
    }
    _fire(type) { setTimeout(() => dispatch(this, new Event(type)), 0); }
    _settle(error) { const plays = this._plays ?? []; this._plays = []; for (const [resolve, reject] of plays) error ? reject(error) : resolve(); }
    _fail(code, message) {
      this._error = new MediaError(code, message); if (code === 4) this._network = 3;
      playing.delete(this); this._fire('error'); this._settle(new DOMException(message, 'NotSupportedError'));
    }
    _seek(time) {
      if (!this._open || !this._ready) { this._time = time; return; }
      time = Math.max(0, Math.min(time, this._duration));
      this._seeking = true; this._ended = false; control(this, SEEK, time); this._fire('seeking');
    }
    _poster() {
      const source = this.getAttribute('poster'), token = this._posterToken = {};
      if (this.localName === 'video' && source && !(this._ready > 1)) resource(resolved(source), 'Poster').then(data => this._posterToken === token && decode(this, data)).catch(() => {});
    }
    _select(force = false) {
      const element = this.getAttribute('src') === null ? [...this.children].find(child => child.localName === 'source' && child.hasAttribute('src') && (!child.type || this.canPlayType(child.type))) : this;
      if (!element && !this._network) return;
      this._close();
      const token = this._token = {};
      if (this._network) this._fire('emptied');
      if (!this.paused) { this._paused = true; playing.delete(this); this._fire('pause'); }
      this._settle(new DOMException('The play() request was interrupted by a new load request', 'AbortError'));
      Object.assign(this, { _ready: 0, _time: 0, _seeking: false, _ended: false, _error: null, _duration: NaN, _size: null, _source: element?.src ?? '', _network: element ? 2 : 0 });
      this._poster();
      if (!element) return;
      this._fire('loadstart');
      if (!this._enabled) return this._fail(4, 'The media feature is disabled. Add "media" to "features" in package.json');
      if (!this._source) return this._fail(4, 'The media has no source');
      if (this.preload === 'none' && !this.autoplay && !force) { this._network = 1; this._fire('suspend'); return; }
      resource(this._source, 'Media').then(data => {
        if (this._token !== token) return;
        call('media.open', this._id, data, this.localName === 'video');
        if (!players.has(this._id)) released.register(this, this._id);
        this._open = true; players.set(this._id, new WeakRef(this));
        if (this.volume !== 1) control(this, VOLUME, this.volume);
        if (this.muted) control(this, MUTED, 1);
        if (this.playbackRate !== 1) control(this, SPEED, this.playbackRate);
        if (this.autoplay && this.paused) this.play().catch(() => {});
        else if (!this.paused) control(this, PLAY);
      }).catch(error => { if (this._token === token) this._fail(4, error.message); });
    }
    // Frees the native player and its thread. A later play() or load() opens it again.
    _close() { this._token = null; if (this._open) { this._open = false; call('media.close', this._id); } }
    _event(kind, values) {
      if (kind === 'meta') {
        const [duration, width, height] = values; this._duration = duration ?? Infinity; this._size = [width, height]; this._ready = 1;
        this._fire('durationchange'); if (width) this._fire('resize'); this._fire('loadedmetadata');
        if (this._time) { const time = this._time; this._time = 0; this._seek(time); }
      } else if (kind === 'loaded' || kind === 'seeked') {
        const first = this._ready < 2; this._ready = 4; this._network = 1;
        if (this._seeking) { this._seeking = false; this._fire('timeupdate'); this._fire('seeked'); }
        if (first) { this._fire('loadeddata'); this._fire('canplay'); this._fire('canplaythrough'); if (!this.paused) this._fire('playing'); }
        if (!this.paused) this._settle();
      } else if (kind === 'time') this._fire('timeupdate');
      else if (kind === 'ended') {
        if (this.loop) { this._seek(0); control(this, PLAY); return; }
        this._ended = true; this._paused = true; playing.delete(this);
        this._fire('timeupdate'); this._fire('pause'); this._fire('ended');
      } else if (kind === 'error') this._fail(values[0], values[1]);
    }
  }
  for (const [key, value] of Object.entries({ NETWORK_EMPTY: 0, NETWORK_IDLE: 1, NETWORK_LOADING: 2, NETWORK_NO_SOURCE: 3, HAVE_NOTHING: 0, HAVE_METADATA: 1, HAVE_CURRENT_DATA: 2, HAVE_FUTURE_DATA: 3, HAVE_ENOUGH_DATA: 4 })) HTMLMediaElement[key] = HTMLMediaElement.prototype[key] = value;
  for (const [name, attribute] of [['autoplay', 'autoplay'], ['controls', 'controls'], ['loop', 'loop'], ['defaultMuted', 'muted']]) Object.defineProperty(HTMLMediaElement.prototype, name, { configurable: true, get() { return this.hasAttribute(attribute); }, set(value) { this.toggleAttribute(attribute, !!value); } });
  class HTMLVideoElement extends HTMLMediaElement {
    get videoWidth() { return this._size?.[0] ?? 0; } get videoHeight() { return this._size?.[1] ?? 0; }
    get width() { return Number(this.getAttribute('width') ?? 0); } set width(value) { this.setAttribute('width', value); }
    get height() { return Number(this.getAttribute('height') ?? 0); } set height(value) { this.setAttribute('height', value); }
    get poster() { const value = this.getAttribute('poster'); return value ? resolved(value) : ''; } set poster(value) { this.setAttribute('poster', value); }
    get playsInline() { return this.hasAttribute('playsinline'); } set playsInline(value) { this.toggleAttribute('playsinline', !!value); }
  }
  class HTMLAudioElement extends HTMLMediaElement {}
  function Audio(src) { const audio = document.createElement('audio'); audio.preload = 'auto'; if (src !== undefined) audio.src = src; return audio; }
  Audio.prototype = HTMLAudioElement.prototype;
  class DOMRect { constructor(x = 0, y = 0, width = 0, height = 0) { this.x = x; this.y = y; this.width = width; this.height = height; } get left() { return Math.min(this.x, this.x + this.width); } get right() { return Math.max(this.x, this.x + this.width); } get top() { return Math.min(this.y, this.y + this.height); } get bottom() { return Math.max(this.y, this.y + this.height); } toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }; } static fromRect(r = {}) { return new this(r.x, r.y, r.width, r.height); } }
  class UIEvent extends Event { constructor(type, options = {}) { super(type, options); this.view = options.view ?? null; this.detail = options.detail ?? 0; } get which() { return 0; } }
  const mouseFields = { screenX: 0, screenY: 0, clientX: 0, clientY: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, button: 0, buttons: 0, relatedTarget: null, movementX: 0, movementY: 0 };
  const layerOffset = (event, axis) => {
    for (let node = event.target; node?.nodeType === 1; node = node.parentElement) if (getComputedStyle(node).position !== 'static') return Math.floor((axis ? event.clientY : event.clientX) - node.getBoundingClientRect()[axis ? 'top' : 'left']);
    return axis ? event.pageY : event.pageX;
  };
  class MouseEvent extends UIEvent {
    constructor(type, options = {}) {
      super(type, options); for (const key in mouseFields) this[key] = options[key] ?? mouseFields[key];
      // Like Chrome, mouse events have whole coordinates, and their offsets are rounded from the exact point. Pointer events keep fractions.
      if ((!(this instanceof PointerEvent) || /^(click|auxclick|contextmenu)$/.test(type)) && (this.clientX % 1 || this.clientY % 1 || this.screenX % 1 || this.screenY % 1)) { Object.assign(eventState(this), { x: this.clientX, y: this.clientY }); for (const key of ['clientX', 'clientY', 'screenX', 'screenY']) this[key] = Math.floor(this[key]); }
    }
    get x() { return this.clientX; } get y() { return this.clientY; }
    get pageX() { return this.clientX + scrollX; } get pageY() { return this.clientY + scrollY; }
    get offsetX() { const target = this.target, x = eventState(this).x ?? this.clientX, offset = target?.getBoundingClientRect ? x - target.getBoundingClientRect().left - target.clientLeft * zoomOf(target) : x; return this instanceof PointerEvent && !/^(click|auxclick|contextmenu)$/.test(this.type) ? offset : Math.round(offset); }
    get offsetY() { const target = this.target, y = eventState(this).y ?? this.clientY, offset = target?.getBoundingClientRect ? y - target.getBoundingClientRect().top - target.clientTop * zoomOf(target) : y; return this instanceof PointerEvent && !/^(click|auxclick|contextmenu)$/.test(this.type) ? offset : Math.round(offset); }
    get which() { return eventState(this).which ?? this.button + 1; }
    // Like Chrome, relative to the nearest positioned box, or to the page.
    get layerX() { return layerOffset(this, 0); } get layerY() { return layerOffset(this, 1); }
    initMouseEvent(type, bubbles, cancelable, view, detail, screenX, screenY, clientX, clientY, ctrlKey, altKey, shiftKey, metaKey, button, relatedTarget) { this.initEvent(type, bubbles, cancelable); Object.assign(this, { view, detail, screenX, screenY, clientX, clientY, ctrlKey, altKey, shiftKey, metaKey, button, relatedTarget }); }
    getModifierState(key) { return !!this[({ Control: 'ctrlKey', Meta: 'metaKey', Alt: 'altKey', Shift: 'shiftKey' })[key]]; }
  }
  class WheelEvent extends MouseEvent { constructor(type, options = {}) { super(type, options); this.deltaX = options.deltaX ?? 0; this.deltaY = options.deltaY ?? 0; this.deltaZ = options.deltaZ ?? 0; this.deltaMode = options.deltaMode ?? 0; } }
  for (const [name, value] of Object.entries({ DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 })) { WheelEvent[name] = value; WheelEvent.prototype[name] = value; }
  class ToggleEvent extends Event { constructor(type, options = {}) { super(type, options); this.oldState = options.oldState ?? ''; this.newState = options.newState ?? ''; this.source = options.source ?? null; } }
  class ContentVisibilityAutoStateChangeEvent extends Event { constructor(type, options = {}) { super(type, options); this.skipped = !!options.skipped; } }
  class SubmitEvent extends Event { constructor(type, options = {}) { super(type, options); this.submitter = options.submitter ?? null; } }
  class TransitionEvent extends Event { constructor(type, options = {}) { super(type, options); this.propertyName = String(options.propertyName ?? ''); this.elapsedTime = Number(options.elapsedTime ?? 0); this.pseudoElement = String(options.pseudoElement ?? ''); } }
  class AnimationEvent extends Event { constructor(type, options = {}) { super(type, options); this.animationName = String(options.animationName ?? ''); this.elapsedTime = Number(options.elapsedTime ?? 0); this.pseudoElement = String(options.pseudoElement ?? ''); } }
  class HashChangeEvent extends Event { constructor(type, options = {}) { super(type, options); this.oldURL = String(options.oldURL ?? ''); this.newURL = String(options.newURL ?? ''); } }
  class PopStateEvent extends Event { constructor(type, options = {}) { super(type, options); this.state = options.state ?? null; this.hasUAVisualTransition = !!options.hasUAVisualTransition; } }
  const snapTargets = new WeakMap();
  class SnapEvent extends Event {
    constructor(token, type, init) { if (token !== snapTargets) throw new TypeError("Failed to construct 'SnapEvent': Illegal constructor"); super(type, init); }
    get snapTargetBlock() { return snapTargets.get(this)[0]; }
    get snapTargetInline() { return snapTargets.get(this)[1]; }
  }
  class PageTransitionEvent extends Event { constructor(type, options = {}) { super(type, options); this.persisted = !!options.persisted; } }
  class StorageEvent extends Event { constructor(type, options = {}) { super(type, options); this.key = options.key ?? null; this.oldValue = options.oldValue ?? null; this.newValue = options.newValue ?? null; this.url = String(options.url ?? ''); this.storageArea = options.storageArea ?? null; } }
  class FormDataEvent extends Event { constructor(type, options = {}) { if (!(options?.formData instanceof FormData)) throw new TypeError('formData is required'); super(type, options); this.formData = options.formData; } }
  class BeforeUnloadEvent extends Event { returnValue = ''; }
  class KeyboardEvent extends UIEvent { constructor(type, options = {}) { super(type, options); const { bubbles, cancelable, composed, which, view, detail, ...fields } = options; Object.assign(this, { key: '', code: '', location: 0, keyCode: 0, charCode: 0, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, repeat: false, isComposing: false }, fields); if (which !== undefined) eventState(this).which = which; } get which() { return eventState(this).which ?? this.keyCode; } getModifierState(key) { return !!this[({ Control: 'ctrlKey', Meta: 'metaKey', Alt: 'altKey', Shift: 'shiftKey' })[key]]; } }
  for (const [name, value] of Object.entries({ DOM_KEY_LOCATION_STANDARD: 0, DOM_KEY_LOCATION_LEFT: 1, DOM_KEY_LOCATION_RIGHT: 2, DOM_KEY_LOCATION_NUMPAD: 3 })) { KeyboardEvent[name] = value; KeyboardEvent.prototype[name] = value; }
  class PointerEvent extends MouseEvent { constructor(type, options = {}) { super(type, options); Object.assign(this, { pointerId: options.pointerId ?? 0, width: options.width ?? 1, height: options.height ?? 1, pressure: options.pressure ?? (this.buttons ? 0.5 : 0), tangentialPressure: options.tangentialPressure ?? 0, tiltX: options.tiltX ?? 0, tiltY: options.tiltY ?? 0, twist: options.twist ?? 0, altitudeAngle: options.altitudeAngle ?? Math.PI / 2, azimuthAngle: options.azimuthAngle ?? 0, pointerType: options.pointerType ?? '', isPrimary: options.isPrimary ?? false }); } getCoalescedEvents() { return []; } getPredictedEvents() { return []; } }
  class FocusEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.relatedTarget = options.relatedTarget ?? null; } }
  class InputEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.data = options.data ?? null; this.inputType = options.inputType ?? ''; this.isComposing = !!options.isComposing; this.dataTransfer = options.dataTransfer ?? null; this._targetRanges = Array.from(options.targetRanges ?? []); }
    getTargetRanges() { return this._targetRanges.slice(); }
  }
  const droppedDirectories = new WeakSet();
  let setFiles, addFile;
  class FileList {
    #files = [];
    static {
      const define = (list, value, i) => Object.defineProperty(list, i, { value, enumerable: true, configurable: true });
      setFiles = (list, files) => { const old = list.#files; for (let i = files.length; i < old.length; i++) delete list[i]; list.#files = files; files.forEach((value, i) => old[i] !== value && define(list, value, i)); return list; };
      addFile = (list, file) => define(list, file, list.#files.push(file) - 1);
    }
    constructor(key) { if (key !== setFiles) throw new TypeError("Failed to construct 'FileList': Illegal constructor"); }
    get length() { return this.#files.length; }
    item(index) { return this.#files[index >>> 0] ?? null; }
  }
  Object.defineProperty(FileList.prototype, Symbol.iterator, { configurable: true, writable: true, value: Array.prototype.values });
  const fileList = (files = []) => setFiles(new FileList(setFiles), files);
  class DataTransfer {
    get files(){return this._files;}
    constructor(){this.dropEffect='none';this.effectAllowed='all';this._data=new Map();this._files=fileList();this.items=[];this.items.add=(value,type)=>{if(value instanceof File){addFile(this._files,value);const directory=droppedDirectories.has(value),item={kind:'file',type:value.type,getAsFile:()=>value,getAsString:()=>{},webkitGetAsEntry:()=>({isFile:!directory,isDirectory:directory,name:value.name,fullPath:'/'+value.name})};this.items.push(item);return item;}this.setData(type,String(value));return this.items.at(-1);};this.items.remove=index=>{const item=this.items[index];if(item?.kind==='file'){const files=[...this._files];files.splice(files.indexOf(item.getAsFile()),1);setFiles(this._files,files);}this.items.splice(index,1);};this.items.clear=()=>{this.items.length=0;setFiles(this._files,[]);this._data.clear();};}
    get types(){return [...this._data.keys(),...(this.items.some(item=>item.kind==='file')?['Files']:[])];}getData(type){return this._data.get(type==='text'?'text/plain':type)??'';}setData(type,data){type=type==='text'?'text/plain':type;this._data.set(type,String(data));const item={kind:'string',type,getAsFile:()=>null,getAsString:callback=>queueMicrotask(()=>callback(this.getData(type)))};const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index<0)this.items.push(item);else this.items[index]=item;}clearData(type){if(type===undefined){this._data.clear();this.items.splice(0,this.items.length,...this.items.filter(item=>item.kind==='file'));}else{this._data.delete(type);const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index>=0)this.items.splice(index,1);}}
  }
  class ClipboardEvent extends Event {constructor(type,options={}){super(type,options);this.clipboardData=options.clipboardData??null;}}
  class DragEvent extends MouseEvent {constructor(type,options={}){super(type,options);this.dataTransfer=options.dataTransfer??null;}}
  class CompositionEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.data = options.data ?? ''; } }
  class DocumentFragment extends Node { constructor(id, tag) { super(tag ? id : create('#fragment'), '#fragment'); } }
  const ruleText = data => {
    // Like Chrome, rules inside a block go on their own lines, two spaces in.
    const block = list => '\n' + list.map(rule => ruleText(rule).replace(/^/gm, '  ') + '\n').join(''), declarations = list => declarationText(specifiedList(declare(list.map(([name, value, priority]) => `${written(name, value)}:${value}${priority ? ' !important' : ''}`).join(';'))));
    switch (data[0]) {
      case 'style': { const text = declarations(data[2]); return data[3].length ? `${data[1]} {${text && '\n  ' + text}${block(data[3])}}` : `${data[1]} { ${text}${text && ' '}}`; }
      case 'media': return `@media ${data[1].join(', ')} {${block(data[2])}}`;
      case 'supports': return `@supports ${data[1]} {${block(data[2])}}`;
      case 'layer': return `@layer ${data[1]} {${block(data[2])}}`;
      case 'layers': return `@layer ${data[1].join(', ')};`;
      case 'container': return `@container ${data[1]} {${block(data[2])}}`;
      case 'keyframes': return `@keyframes ${data[1]} { \n${data[2].map(frame => `  ${frame[0]} { ${declarations(frame[1])} }\n`).join('')}}`;
      case 'font-face': return `@font-face { ${declarationText(specifiedList(data[1]))} }`;
      case 'position-try': { const text = declarationText(specifiedList(data[2])); return `@position-try ${data[1]} { ${text}${text && ' '}}`; }
      case 'property': return `@property ${data[1]} { syntax: ${JSON.stringify(data[2])}; inherits: ${data[3]};${data[4] == null ? '' : ` initial-value: ${data[4]};`} }`;
      case 'import': return `@import url("${data[1]}");`;
      default: return data[1];
    }
  };
  class CSSRuleList extends List {}
  class StyleSheetList extends List {}
  class MediaList {
    constructor(read, write) { this._read = read; this._write = write; }
    get mediaText() { return this._read().join(', '); } set mediaText(value) { this._write?.(String(value)); }
    get length() { return this._read().length; } item(index) { return this._read()[index] ?? null; }
    appendMedium(medium) { if (!this._read().includes(medium)) this.mediaText = [...this._read(), medium].join(', '); }
    deleteMedium(medium) { const list = this._read(); if (!list.includes(medium)) throw new DOMException('Medium not found', 'NotFoundError'); this.mediaText = list.filter(item => item !== medium).join(', '); }
    toString() { return this.mediaText; }
  }
  const mediaList = text => text ? text.split(',').map(item => item.trim()).filter(Boolean) : [];
  class CSSRule {
    constructor(sheet, parent, data) { this._sheet = sheet; this._parent = parent; this._data = data; }
    get parentStyleSheet() { return this._sheet; }
    get parentRule() { return this._parent === this._sheet ? null : this._parent; }
    get type() { return { style: 1, import: 3, media: 4, 'font-face': 5, keyframes: 7, keyframe: 8, supports: 12 }[this._data[0]] ?? 0; }
    get cssText() { return ruleText(this._data); } set cssText(_) {}
    _path(...tail) { const index = this._parent.cssRules.indexOf(this); return index < 0 ? null : this._parent === this._sheet ? [index, ...tail] : this._parent._path(index, ...tail); }
  }
  class CSSGroupingRule extends CSSRule {
    get _children() { return this._data[this._data[0] === 'style' ? 3 : 2]; }
    get cssRules() { return this._rules ??= CSSRuleList.from(this._children, data => makeRule(this._sheet, this, data)); }
    insertRule(rule, index = 0) { index = Number(index) >>> 0; if (index > this._children.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); const data = this._sheet._edit('insert', this._path(index), String(rule)); this._children.splice(index, 0, data); this.cssRules.splice(index, 0, makeRule(this._sheet, this, data)); return index; }
    deleteRule(index) { index = Number(index) >>> 0; if (index >= this._children.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); this._sheet._edit('delete', this._path(index)); this._children.splice(index, 1); this.cssRules.splice(index, 1); }
  }
  class CSSStyleRule extends CSSGroupingRule {
    get selectorText() { return this._data[1]; }
    set selectorText(value) { try { this._data[1] = this._sheet._edit('selector', this._path(), String(value)); } catch {} }
    get style() { return this._style ??= declaration({ rule: this, read: () => this._list ??= declare(this._data[2].map(([name, value, priority]) => `${name}:${value}${priority ? ' !important' : ''}`).join(';')), write: list => { this._list = list; this._data[2] = list; this._sheet._edit('style', this._path(), declarationText(list)); } }); }
    set style(value) { this.style.cssText = value; }
  }
  class CSSMediaRule extends CSSGroupingRule { get media() { return this._media ??= new MediaList(() => this._data[1], value => { this._sheet._edit('replace', this._path(), `@media ${value}{}`); this._data[1] = mediaList(value); }); } get conditionText() { return this._data[1].join(', '); } }
  class CSSSupportsRule extends CSSGroupingRule { get conditionText() { return this._data[1]; } }
  class CSSContainerRule extends CSSGroupingRule { get conditionText() { return this._data[1]; } get containerQuery() { return this._data[1].replace(/^(?!not\b|\()[\w-]+\s+/, ''); } get containerName() { return /^(?!not\b|\()([\w-]+)\s/.exec(this._data[1])?.[1] ?? ''; } }
  class CSSLayerBlockRule extends CSSGroupingRule { get name() { return this._data[1]; } }
  class CSSLayerStatementRule extends CSSRule { get nameList() { return Object.freeze(this._data[1].slice()); } }
  class CSSKeyframeRule extends CSSRule {
    get keyText() { return this._data[0]; } set keyText(value) { value = keyText(value); if (!/^(\d+|\d*\.\d+)%$/.test(value) || parseFloat(value) > 100) throw new DOMException('Invalid keyframe selector', 'SyntaxError'); this._data[0] = parseFloat(value) + '%'; this._parent._replace(); }
    get type() { return 8; } get cssText() { return `${this._data[0]} { ${declarationText(this._data[1])} }`; }
    get style() { return this._style ??= declaration({ rule: this, raw: true, read: () => this._data[1], write: list => { this._data[1] = list; this._parent._replace(); } }); }
  }
  const keyText = key => String(key).trim().toLowerCase().replace(/^from$/, '0%').replace(/^to$/, '100%');
  class CSSKeyframesRule extends CSSRule {
    get name() { return this._data[1]; } set name(value) { const name = String(value); this._sheet._edit('replace', this._path(), ruleText(['keyframes', name, this._data[2]])); this._data[1] = name; }
    get cssRules() { return this._rules ??= CSSRuleList.from(this._data[2], data => new CSSKeyframeRule(this._sheet, this, data)); }
    _replace() { this._sheet._edit('replace', this._path(), ruleText(this._data)); }
    get length() { return this.cssRules.length; }
    findRule(key) { key = keyText(key); return this.cssRules.findLast(rule => rule._data[0] === key) ?? null; }
    appendRule(text) { const frames = this._sheet._edit('keyframe', this._path(), String(text)); for (const data of frames ?? []) { this._data[2].push(data); this.cssRules.push(new CSSKeyframeRule(this._sheet, this, data)); } }
    deleteRule(key) { const rule = this.findRule(key); if (!rule) return; const index = this.cssRules.indexOf(rule); this._sheet._edit('keyframe', this._path(), String(index)); this._data[2].splice(index, 1); this.cssRules.splice(index, 1); }
  }
  class CSSFontFaceRule extends CSSRule { get style() { return this._style ??= declaration({ rule: this, raw: true, read: () => this._data[1], write: list => { this._data[1] = list; this._sheet._edit('style', this._path(), declarationText(list)); } }); } }
  class CSSPositionTryDescriptors extends CSSStyleDeclaration {}
  class CSSPositionTryRule extends CSSRule { get name() { return this._data[1]; } get style() { return this._style ??= declaration({ rule: this, read: () => this._list ??= declare(this._data[2].map(([name, value]) => `${name}:${value}`).join(';')), write: list => { this._list = this._data[2] = list.filter(([name, , priority]) => !priority && tryDescriptor.test(name)); this._sheet._edit('style', this._path(), declarationText(this._list)); } }, CSSPositionTryDescriptors); } }
  const tryDescriptor = /^(top|right|bottom|left|inset.*|margin.*|(min-|max-)?(width|height|inline-size|block-size)|(align|justify|place)-self|position-(anchor|area))$/;
  class CSSPropertyRule extends CSSRule { get name() { return this._data[1]; } get syntax() { return this._data[2]; } get inherits() { return this._data[3]; } get initialValue() { return this._data[4] ?? null; } }
  class CSSImportRule extends CSSRule { get href() { return this._data[1]; } get media() { return this._media ??= new MediaList(() => []); } get styleSheet() { return null; } get layerName() { return null; } get supportsText() { return null; } }
  const ruleTypes = { style: CSSStyleRule, media: CSSMediaRule, supports: CSSSupportsRule, container: CSSContainerRule, layer: CSSLayerBlockRule, layers: CSSLayerStatementRule, keyframes: CSSKeyframesRule, 'font-face': CSSFontFaceRule, property: CSSPropertyRule, 'position-try': CSSPositionTryRule, import: CSSImportRule };
  const makeRule = (sheet, parent, data) => new (ruleTypes[data[0]] ?? CSSRule)(sheet, parent, data);
  let sheetIds = 1;
  class StyleSheet {
    get type() { return 'text/css'; } get href() { return null; } get parentStyleSheet() { return null; } get ownerRule() { return null; }
    get ownerNode() { return this._node; } get title() { return this._node?.getAttribute('title') ?? null; }
    get media() { return this._media ??= new MediaList(() => mediaList(this._node ? this._node.getAttribute('media') : this._mediaText), value => { if (this._node) this._node.setAttribute('media', value); else { this._mediaText = value; this._query = null; for (const root of this._owners) adopt(root); } }); }
    get disabled() { return this._disabled; }
    set disabled(value) { value = !!value; if (value === this._disabled) return; this._disabled = value; if (this._node) call('dom.sheet', this._node._id, 'disabled', value); else for (const root of this._owners) adopt(root); }
  }
  class CSSStyleSheet extends StyleSheet {
    constructor(options = {}, node = null) { super(); this._id = sheetIds++; this._node = node; this._text = null; this._source = null; this._tree = []; this._rules = null; this._owners = new Set(); this._live = false; this._disabled = !!options.disabled; this._mediaText = typeof options.media === 'string' ? options.media : Array.from(options.media ?? []).join(', '); }
    // Rules of a constructed sheet are parsed from `_source` when first read. `_source` is the text Rust has, until a rule edit.
    get _data() { return this._tree ??= call('dom.constructed', 0, 'parse', this._source); }
    _sync() {
      if (!this._node) return;
      const value = text(this._node._m);
      if (value === this._text) return;
      this._text = value; this._tree = call('dom.css', this._node._id);
      if (this._rules) this._rules.splice(0, this._rules.length, ...this._data.map(data => makeRule(this, this, data)));
    }
    _edit(kind, path, value = '') {
      let result = null;
      this._source = null;
      if (path && this._node) result = call('dom.sheet', this._node._id, kind, value, path);
      else if (path && this._live) result = call('dom.constructed', this._id, kind, value, path);
      else if (kind === 'insert') { const parsed = call('dom.constructed', 0, 'parse', value); result = parsed.length === 1 && parsed[0][0] !== 'other' ? parsed[0] : { error: 'SyntaxError' }; }
      else if (kind === 'keyframe' && value.includes('{')) result = call('dom.constructed', 0, 'parse', `@keyframes x{${value}}`)[0]?.[2] ?? { error: 'SyntaxError' };
      else if (kind === 'selector') { const parsed = call('dom.constructed', 0, 'parse', `${value}{}`); result = parsed.length === 1 && parsed[0][0] === 'style' ? parsed[0][1] : { error: 'SyntaxError' }; }
      if (result?.error) throw new DOMException(`Failed to ${kind} the CSS rule`, result.error);
      return result;
    }
    get cssRules() { this._sync(); return this._rules ??= CSSRuleList.from(this._data, data => makeRule(this, this, data)); }
    get rules() { return this.cssRules; }
    insertRule(rule, index = 0) { const rules = this.cssRules; index = Number(index) >>> 0; if (index > rules.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); const data = this._edit('insert', [index], String(rule)); this._data.splice(index, 0, data); rules.splice(index, 0, makeRule(this, this, data)); return index; }
    deleteRule(index) { const rules = this.cssRules; index = Number(index) >>> 0; if (index >= rules.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); this._edit('delete', [index]); this._data.splice(index, 1); rules.splice(index, 1); }
    addRule(selector = 'undefined', style = 'undefined', index) { this.insertRule(`${selector}{${style}}`, index ?? this.cssRules.length); return -1; }
    removeRule(index = 0) { this.deleteRule(index); }
    replaceSync(value) { if (this._node) throw new DOMException('Only constructed sheets can be replaced', 'NotAllowedError'); value = String(value).replace(/@import\s[^;]*;?/g, ''); this._source = value; this._tree = null; if (this._live) this._tree = call('dom.constructed', this._id, this._rules ? 'replace' : 'load', value); if (this._rules) this._rules.splice(0, this._rules.length, ...this._data.map(data => makeRule(this, this, data))); }
    replace(value) { try { this.replaceSync(value); return Promise.resolve(this); } catch (error) { return Promise.reject(error); } }
  }
  const adopt = root => call('dom.adopt', root === document ? 0 : root._id, (root._adopted ?? []).filter(sheet => {
    if (sheet._disabled) return false;
    if (!sheet._mediaText.trim()) return true;
    if (!sheet._query) { const query = sheet._query = matchMedia(sheet._mediaText); query.addEventListener('change', () => { if (sheet._query === query) for (const root of sheet._owners) adopt(root); }); }
    return sheet._query.matches;
  }).map(sheet => sheet._id));
  const adoptedSheets = root => {
    const apply = list => {
      for (const sheet of list) if (!(sheet instanceof CSSStyleSheet) || sheet._node) throw new DOMException('Only constructed sheets can be adopted', 'NotAllowedError');
      const previous = root._adopted ?? [];
      root._adopted = list;
      for (const sheet of previous) if (!list.includes(sheet)) sheet._owners.delete(root);
      for (const sheet of list) { sheet._owners.add(root); if (!sheet._live) { sheet._live = true; call('dom.constructed', sheet._id, 'load', sheet._source ?? sheet._data.map(ruleText).join('\n')); } }
      for (const sheet of previous) if (!sheet._owners.size && sheet._live) { sheet._live = false; call('dom.constructed', sheet._id, 'drop'); }
      adopt(root);
    };
    const list = [];
    return { list, proxy: new Proxy(list, { set(target, key, value) { if (key !== 'length' && !(value instanceof CSSStyleSheet)) throw new TypeError('Expected a CSSStyleSheet'); const next = target.slice(); next[key] = value; const list = next.filter(Boolean), previous = root._adopted ?? []; if (list.length !== previous.length || list.some((sheet, i) => sheet !== previous[i])) apply(list); Reflect.set(target, key, value); return true; }, deleteProperty(target, key) { Reflect.deleteProperty(target, key); apply(target.filter(Boolean)); return true; } }), apply };
  };
  const adoptedProperty = { configurable: true, get() { return (this._adoption ??= adoptedSheets(this)).proxy; }, set(sheets) { const adoption = this._adoption ??= adoptedSheets(this), list = Array.from(sheets); adoption.apply(list); adoption.list.splice(0, adoption.list.length, ...list); } };
  const styleSheetsOf = root => {
    const sheets = new StyleSheetList(), stack = [...root._m.kids].reverse();
    while (stack.length) { const rec = stack.pop(); if (rec.tag === 'style' || rec.tag === 'link') { const sheet = of(rec).sheet; if (sheet) sheets.push(sheet); } if (rec.tag !== 'template' && rec.tag !== 'noscript') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); }
    return sheets;
  };
  // The renderer gets the text of each range of a highlight, again when the range changes. Ranges over more nodes,
  // static ones and ones outside the document are loose: a change of the tree can also change their text.
  let highlightOrder = '[]', highlightIds = 0, rangeIds = 0;
  const rangeParts = range => {
    const { startContainer: node, startOffset: start, endContainer: end, endOffset: to } = range;
    if (!node.isConnected || end !== node && end.getRootNode() !== node.getRootNode()) return [];
    if (node === end && node.nodeType === 3) return to > start ? [[node._id, start, to]] : [];
    return call('dom.rangeNodes', node._id, start, end._id, to).filter(part => part[2] > part[1]);
  };
  const queueHighlights = () => {
    if (highlightsQueued) return;
    highlightsQueued = true;
    microtask(() => {
      highlightsQueued = false;
      const tree = highlightShape !== shape, updates = [], order = JSON.stringify([...highlights].sort((a, b) => a[1]._priority - b[1]._priority).map(([name, highlight]) => [name, highlight._id]));
      highlightShape = shape;
      for (const highlight of new Set(highlights.values())) {
        if (tree) for (const range of highlight._loose) highlight._dirty.add(range);
        for (const range of highlight._dirty) {
          const inside = highlight.has(range), parts = inside ? rangeParts(range) : [], sent = parts.length === 1 ? parts[0].join() : parts.length ? JSON.stringify(parts) : '';
          const loose = inside && (range instanceof StaticRange || range.startContainer !== range.endContainer || range.startContainer.nodeType !== 3 || !range.startContainer.isConnected);
          if (loose !== highlight._loose.has(range)) { looseRanges += loose ? 1 : -1; if (loose) highlight._loose.add(range); else highlight._loose.delete(range); }
          if ((highlight._sent.get(range) ?? '') !== sent) { updates.push([highlight._id, range._highlightId ??= ++rangeIds, parts]); if (sent) highlight._sent.set(range, sent); else highlight._sent.delete(range); }
        }
        highlight._dirty.clear();
      }
      if (updates.length || order !== highlightOrder) call('dom.highlights', order === highlightOrder ? null : json(order), updates);
      highlightOrder = order;
    });
  };
  class Highlight extends Set {
    constructor(...ranges) { super(); this._id = ++highlightIds; this._priority = 0; this._names = 0; this._dirty = new Set(); this._loose = new Set(); this._sent = new Map(); this.type = 'highlight'; for (const range of ranges) this.add(range); }
    get priority() { return this._priority; } set priority(value) { this._priority = Number(value) | 0; if (this._names) queueHighlights(); }
    _touch(range) { this._dirty.add(range); if (this._names) queueHighlights(); }
    add(range) { if (!(range instanceof Range) && !(range instanceof StaticRange)) throw new TypeError('Expected a range'); if (!this.has(range)) { super.add(range); (range._lit ??= new Set()).add(this); this._touch(range); } return this; }
    delete(range) { const found = super.delete(range); if (found) { range._lit.delete(this); this._touch(range); } return found; }
    clear() { for (const range of [...this]) this.delete(range); }
  }
  const nameHighlight = (highlight, by) => {
    highlight._names += by;
    // The renderer forgets a highlight without a name.
    if (!highlight._names) { looseRanges -= highlight._loose.size; highlight._loose.clear(); highlight._sent.clear(); for (const range of highlight) highlight._dirty.add(range); }
    queueHighlights();
  };
  class HighlightRegistry extends Map {
    set(name, highlight) { if (!(highlight instanceof Highlight)) throw new TypeError('Expected a Highlight'); const old = this.get(name = String(name)); if (old !== highlight) { super.set(name, highlight); nameHighlight(highlight, 1); if (old) nameHighlight(old, -1); } return this; }
    delete(name) { const old = this.get(name = String(name)), found = super.delete(name); if (found) nameHighlight(old, -1); return found; }
    clear() { for (const name of [...this.keys()]) this.delete(name); }
  }
  highlights = new HighlightRegistry();
  const CSS = {
    highlights,
    supports(name, value) { return call('dom.supports', String(name), arguments.length > 1 ? String(value) : null); },
    escape(value) {
      value = String(value); let out = '';
      for (let i = 0; i < value.length; i++) {
        const c = value.charCodeAt(i);
        if (c === 0) out += '�';
        else if (c <= 31 || c === 127 || (i === 0 || i === 1 && value.charCodeAt(0) === 45) && c >= 48 && c <= 57) out += '\\' + c.toString(16) + ' ';
        else if (i === 0 && c === 45 && value.length === 1) out += '\\-';
        else if (c >= 128 || c === 45 || c === 95 || c >= 48 && c <= 57 || c >= 65 && c <= 90 || c >= 97 && c <= 122) out += value[i];
        else out += '\\' + value[i];
      }
      return out;
    },
    registerProperty(definition) {
      if (definition?.name === undefined || definition.inherits === undefined) throw new TypeError('Expected a name and inherits');
      const result = call('dom.registerProperty', String(definition.name), String(definition.syntax ?? '*'), !!definition.inherits, definition.initialValue === undefined ? null : String(definition.initialValue));
      if (result?.error) throw new DOMException('Invalid property registration', result.error);
    },
  };
  class ShadowRoot extends DocumentFragment {
    get innerHTML(){return serialize(this._m,true);} set innerHTML(value){setHTML(this,value);}
    getHTML() { return this.innerHTML; } setHTMLUnsafe(html) { this.innerHTML = html; attachDeclarative(); }
    get activeElement() { return within(active, this); }
    get fullscreenElement() { return within(fullscreens.at(-1), this); }
    get styleSheets() { return styleSheetsOf(this); }
    elementFromPoint(x, y) { return Document.prototype.elementFromPoint.call(this, x, y); }
    elementsFromPoint(x, y) { return Document.prototype.elementsFromPoint.call(this, x, y); }
  }
  // `define` resolves `whenDefined` before the upgrades it queued run, so a throwing upgrade can't leave it pending.
  // Tags with a `-` that any record had, so `define` skips the document walk when no element can upgrade.
  const customTags = new Set();
  const customElements = {
    define: scoped((name, Type) => {
      if (typeof Type !== 'function') throw new TypeError('Expected a constructor');
      if (!/^[a-z][a-z0-9._-]*-[a-z0-9._-]*$/.test(name) || /^(annotation-xml|color-profile|font-face(-src|-uri|-format|-name)?|missing-glyph)$/.test(name)) throw new DOMException(`'${name}' is not a valid custom element name`, 'SyntaxError');
      if (definitions.has(name) || defined.has(Type)) throw new DOMException('Invalid custom element definition', 'NotSupportedError');
      const observed = new Set(typeof Type.prototype.attributeChangedCallback === 'function' ? Array.from(Type.observedAttributes ?? [], String) : []);
      wake(tagReaders.get('-'), document._m); definitions.set(name, Type); defined.set(Type, { name, observed });
      if (customTags.has(name)) reactions(document._m, undefined, name);
      for (const resolve of definitionsWaiting.get(name) ?? []) resolve(Type);
      definitionsWaiting.delete(name);
    }),
    get(name) { return definitions.get(name); },
    getName(Type) { return defined.get(Type)?.name ?? null; },
    whenDefined(name) { if (definitions.has(name)) return Promise.resolve(definitions.get(name)); return new Promise(resolve => { const pending = definitionsWaiting.get(name) ?? []; pending.push(resolve); definitionsWaiting.set(name, pending); }); },
    upgrade: scoped(root => { if (definitions.size) reactions(root._m); }),
  };
  const filterNode = (walker, node) => !(walker.whatToShow & (1 << (node.nodeType - 1))) ? 3 : walker.filter == null ? 1 : +(typeof walker.filter === 'function' ? walker.filter(node) : walker.filter.acceptNode(node));
  class TreeWalker {
    constructor(root, whatToShow = 0xffffffff, filter = null) { Object.assign(this, { root, whatToShow: whatToShow >>> 0, filter, currentNode: root }); }
    parentNode() { for (let node = this.currentNode; node && node !== this.root;) { node = node.parentNode; if (node && filterNode(this, node) === 1) return this.currentNode = node; } return null; }
    firstChild() { return this._children(true); } lastChild() { return this._children(false); }
    nextSibling() { return this._siblings(true); } previousSibling() { return this._siblings(false); }
    _children(first) {
      let node = first ? this.currentNode.firstChild : this.currentNode.lastChild;
      while (node) {
        const result = filterNode(this, node);
        if (result === 1) return this.currentNode = node;
        const child = result === 3 && (first ? node.firstChild : node.lastChild);
        if (child) { node = child; continue; }
        for (;;) { const sibling = first ? node.nextSibling : node.previousSibling; if (sibling) { node = sibling; break; } node = node.parentNode; if (!node || node === this.root || node === this.currentNode) return null; }
      }
      return null;
    }
    _siblings(next) {
      let node = this.currentNode;
      if (node === this.root) return null;
      for (;;) {
        let sibling = next ? node.nextSibling : node.previousSibling;
        while (sibling) {
          node = sibling; const result = filterNode(this, node);
          if (result === 1) return this.currentNode = node;
          sibling = next ? node.firstChild : node.lastChild;
          if (result === 2 || !sibling) sibling = next ? node.nextSibling : node.previousSibling;
        }
        node = node.parentNode;
        if (!node || node === this.root || filterNode(this, node) === 1) return null;
      }
    }
    previousNode() {
      for (let node = this.currentNode; node !== this.root;) {
        for (let sibling = node.previousSibling; sibling; sibling = node.previousSibling) {
          node = sibling; let result = filterNode(this, node);
          for (let child; result !== 2 && (child = node.lastChild);) { node = child; result = filterNode(this, node); }
          if (result === 1) return this.currentNode = node;
        }
        if (node === this.root || !node.parentNode) return null;
        node = node.parentNode;
        if (filterNode(this, node) === 1) return this.currentNode = node;
      }
      return null;
    }
    nextNode() {
      for (let node = this.currentNode, result = 1;;) {
        for (let child; result !== 2 && (child = node.firstChild);) { node = child; result = filterNode(this, node); if (result === 1) return this.currentNode = node; }
        let sibling = null;
        for (let up = node; up && up !== this.root && !(sibling = up.nextSibling); up = up.parentNode);
        if (!sibling) return null;
        node = sibling; result = filterNode(this, node);
        if (result === 1) return this.currentNode = node;
      }
    }
  }
  // Pre-removal steps for `node`, or with `clear` for all its children.
  const iteratorsRemove = (node, clear = false) => {
    for (const ref of iterators) {
      const it = ref.deref(); if (!it) { iterators.delete(ref); continue; }
      const reference = it.referenceNode;
      if (!it.root.contains(node) || !clear && node === it.root || !node.contains(reference) || clear && reference === node) continue;
      if (it.pointerBeforeReferenceNode) {
        let next = null;
        for (let up = node; up && up !== it.root && !(next = up.nextSibling); up = up.parentNode);
        if (next) { it.referenceNode = next; continue; }
        it.pointerBeforeReferenceNode = false;
      }
      let previous = clear ? null : node.previousSibling;
      if (previous) { while (previous.lastChild) previous = previous.lastChild; it.referenceNode = previous; } else it.referenceNode = clear ? node : node.parentNode;
    }
  };
  class NodeIterator {
    constructor(root, whatToShow = 0xffffffff, filter = null) { Object.assign(this, { root, whatToShow: whatToShow >>> 0, filter, referenceNode: root, pointerBeforeReferenceNode: true }); iterators.add(new WeakRef(this)); }
    nextNode() { return this._traverse(true); } previousNode() { return this._traverse(false); } detach() {}
    _traverse(next) {
      let node = this.referenceNode, before = this.pointerBeforeReferenceNode;
      for (;;) {
        if (next === before) before = !before;
        else if (next) {
          let following = node.firstChild;
          for (let up = node; !following && up && up !== this.root; up = up.parentNode) following = up.nextSibling;
          if (!following) return null; node = following;
        } else {
          if (node === this.root) return null;
          let previous = node.previousSibling;
          if (previous) { for (let child; (child = previous.lastChild);) previous = child; node = previous; } else node = node.parentNode;
        }
        if (filterNode(this, node) === 1) break;
      }
      this.referenceNode = node; this.pointerBeforeReferenceNode = before; return node;
    }
  }
  class Document extends Node {
    constructor(id, tag) { super(tag ? id : create('#document'), '#document'); }
    get textContent() { return null; } set textContent(_) {}
    _target(element) { call('dom.target', element?._id ?? null); }
    // As in browsers, a fragment shows its target: each `hidden=until-found` box around it gets `beforematch` and loses `hidden`, and closed details open.
    _reveal(element) { for (let node = element; node; node = node.parentElement) { if (node.getAttribute('hidden')?.toLowerCase() === 'until-found') { node.dispatchEvent(new Event('beforematch', { bubbles: true })); node.removeAttribute('hidden'); } else if (node.localName === 'details' && !node.open && node !== element && !node.querySelector(':scope > summary')?.contains(element)) node.open = true; } }
    createElement(tag) {if(this instanceof XMLDocument)return this._element(create(String(tag)),this.contentType==='application/xhtml+xml'?XHTML:null,String(tag));tag=String(tag).toLowerCase();if(tag[0]==='#')throw new DOMException('Invalid tag','InvalidCharacterError');const Type=definitions.get(tag);if(Type){constructing={tag};try{return new Type();}finally{constructing=null;}}return this===document?wrap(create(tag)):owned(this,wrap(create(tag)));}
    createAttribute(name) {name=badAttr(String(name));return new Attr(null,this instanceof XMLDocument?name:name.toLowerCase(),'');}
    _element(id, namespace, tag) { const Type = namespace === XHTML ? htmlTypes.get(tag.replace(/^.*:/, '')) ?? HTMLElement : namespace === SVG ? tag === 'svg' ? SVGSVGElement : SVGElement : Element, node = new Type(id, tag), colon = tag.indexOf(':'); node.namespaceURI = namespace || null; node.nodeName = node.tagName = tag; if (colon > 0) { node.prefix = tag.slice(0, colon); node.localName = tag.slice(colon + 1); } return owned(this, node); }
    createElementNS(namespace, tag) {namespace=namespace==null?null:String(namespace);tag=String(tag);const colon=tag.indexOf(':'),prefix=colon<0?null:tag.slice(0,colon);if(!colon||colon===tag.length-1)throw new DOMException('Invalid tag','InvalidCharacterError');if(prefix!==null&&!namespace||prefix==='xml'&&namespace!==XML||(tag==='xmlns'||prefix==='xmlns')!==(namespace===XMLNS))throw new DOMException('Invalid namespace','NamespaceError');if(tag[0]==='#')throw new DOMException('Invalid tag','InvalidCharacterError');if(namespace===XHTML&&!(this instanceof XMLDocument))return this.createElement(tag);return this._element(create(tag),namespace,tag);}
    createTextNode(text) { return owned(this, wrap(create('#text', String(text)))); }
    createComment(text) { return owned(this, wrap(create('#comment', String(text)))); }
    createCDATASection(data) { if (!(this instanceof XMLDocument)) throw new DOMException('HTML documents have no CDATA sections', 'NotSupportedError'); return owned(this, new CDATASection(create('#text', String(data)))); }
    createProcessingInstruction(target, data) { return owned(this, new ProcessingInstruction(create('#comment', String(data)), String(target))); }
    createDocumentFragment() { return owned(this, wrap(create('#fragment'))); }
    createRange() { return new Range(); }
    get implementation(){return this._implementation??={createDocumentType:(name,publicId,systemId)=>{name=String(name);if(!/^[A-Za-z_][\w.-]*(:[A-Za-z_][\w.-]*)?$/.test(name))throw new DOMException('Invalid doctype name','InvalidCharacterError');publicId=String(publicId);systemId=String(systemId);return wrap(create('#doctype',publicId||systemId?[name,publicId,systemId].join('\0'):name));},createHTMLDocument:title=>{const doc=new DOMParser().parseFromString('<!doctype html><html><head></head><body></body></html>','text/html');if(title!==undefined)doc.title=title;return doc;},createDocument:(namespace,name,doctype)=>{const doc=new XMLDocument();if(doctype)doc.append(doctype);if(name)doc.append(doc.createElementNS(namespace,name));return doc;},hasFeature:()=>true};}
    createEvent(type) { const Type = { event: Event, events: Event, htmlevents: Event, customevent: CustomEvent, uievent: UIEvent, uievents: UIEvent, focusevent: FocusEvent, mouseevent: MouseEvent, mouseevents: MouseEvent, keyboardevent: KeyboardEvent, compositionevent: CompositionEvent, dragevent: DragEvent, messageevent: MessageEvent, storageevent: StorageEvent, hashchangeevent: HashChangeEvent, beforeunloadevent: BeforeUnloadEvent }[String(type).toLowerCase()]; if (!Type) throw new DOMException('Unsupported event interface', 'NotSupportedError'); return new Type(''); }
    importNode(node, deep = false) { return owned(this, node.cloneNode(deep)); }
    adoptNode(node) { if (node instanceof Attr) { node.ownerElement?.removeAttributeNode(node); return node; } if (node.nodeType === 9) throw new DOMException('A document cannot be adopted', 'NotSupportedError'); node.remove(); node._owner = this === document ? undefined : this; return node; }
    getElementById(id) { id = String(id); const found = idsWithin(this._m, id); return found.length < 2 ? of(found[0]) : find(this._m, rec => rec.attrs?.get('id') === id); }
    getElementsByName(name) { name = String(name); return live(this, 'name ' + name, NodeList, () => all(this._m, rec => rec.attrs.get('name') === name), { root: this._m, tags: ['*'], attrs: ['name'] }, null); }
    get body() { const html = this._m.kids.find(rec => rec.tag[0] !== '#'); return html?.tag === 'html' ? of(html.kids.find(rec => rec.tag === 'body' || rec.tag === 'frameset')) : null; }
    get head() { const html = this._m.kids.find(rec => rec.tag[0] !== '#'); return html?.tag === 'html' ? of(html.kids.find(rec => rec.tag === 'head')) : null; }
    get documentElement() { return of(this._m.kids.find(rec => rec.tag[0] !== '#')); }
    get activeElement() { return within(active, this) ?? this.body ?? this.documentElement; } get defaultView() { return this === document ? globalThis : null; }
    get baseURI() {
      const href = location.href;
      if (!rare.base) return href;
      if (this._baseAt !== version || this._baseFrom !== href) {
        const base = find(this._m, rec => rec.tag === 'base' && rec.attrs.has('href'));
        this._baseAt = version; this._baseFrom = href;
        try { this._base = base ? new URL(base.getAttribute('href'), href).href : href; } catch { this._base = href; }
      }
      return this._base;
    }
    get URL() { return this === document ? location.href : 'about:blank'; } get documentURI() { return this.URL; } get location() { return this === document ? location : null; }
    get characterSet() { return 'UTF-8'; } get charset() { return 'UTF-8'; } get inputEncoding() { return 'UTF-8'; } get referrer() { return ''; } get domain() { return this === document ? location.hostname : ''; } get lastModified() { const d = new Date(), p = n => String(n).padStart(2, '0'); return `${p(d.getMonth() + 1)}/${p(d.getDate())}/${d.getFullYear()} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`; }
    get designMode() { return 'off'; } set designMode(_) {}
    // Cookies live in memory, like a private session with no server.
    get cookie() { return this === document ? [...cookies].map(([name, value]) => name ? name + '=' + value : value).join('; ') : ''; } set cookie(value) { const [pair, ...attrs] = String(value).split(';'), at = pair.indexOf('='), name = at < 0 ? '' : pair.slice(0, at).trim(), expired = attrs.some(attr => { const [key, text = ''] = attr.split('=').map(part => part.trim()); return /^max-age$/i.test(key) ? Number(text) <= 0 : /^expires$/i.test(key) && Date.parse(text) <= Date.now(); }); if (this !== document) return; if (expired) cookies.delete(name); else cookies.set(name, (at < 0 ? pair : pair.slice(at + 1)).trim()); }
    get contentType() { return 'text/html'; } get scrollingElement() { return this.documentElement; }
    get readyState() { return this === document ? readyState : 'complete'; } get currentScript() { return this === document ? currentScript : null; } get compatMode() { return !(this instanceof XMLDocument) && (this._quirks ??= quirks(this.doctype)) ? 'BackCompat' : 'CSS1Compat'; } get doctype() { return of(this._m.kids.find(rec => rec.tag === '#doctype')); } get visibilityState() { return 'visible'; } get hidden() { return false; }
    get title() { return (rare.title && find(this._m, rec => rec.tag === 'title')?.textContent || '').replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, ''); }
    set title(value) {
      let title = rare.title ? find(this._m, rec => rec.tag === 'title') : null;
      if (!title) {
        let head = this.head;
        if (!head) { head = this.createElement('head'); (this.documentElement ?? this).prepend(head); }
        title = this.createElement('title'); head.append(title);
      }
      title.textContent = String(value);
    }
    getSelection() { return selection; } hasFocus() { return windowFocused; }
    createTreeWalker(root, whatToShow, filter) { return new TreeWalker(root, whatToShow, filter); }
    createNodeIterator(root, whatToShow, filter) { return new NodeIterator(root, whatToShow, filter); }
    caretRangeFromPoint(x,y) {
      const node = wrap(call('dom.hitTest',x,y,true,true)); if (!node) return null;
      const range = this.createRange(); range.setStart(node,node.nodeType === 3 ? textCall('dom.textIndex',node._id,x,y) : 0); range.collapse(true); return range;
    }
    caretPositionFromPoint(x,y){const range=this.caretRangeFromPoint(x,y);return range?{offsetNode:range.startContainer,offset:range.startOffset}:null;}
    elementFromPoint(x, y) { const node = hitAt(x, y); return node && retarget(node, this); }
    elementsFromPoint(x, y) { laidOut(); const list = [...new Set(call('dom.hitTest', x, y).map(id => retarget(wrap(id), this)))], root = document.documentElement; if (root && !list.includes(root) && x >= 0 && y >= 0 && x < innerWidth && y < innerHeight) list.push(root); return list; }
    get fullscreenElement() { return within(fullscreens.at(-1), this); }
    get fullscreenEnabled() { return true; } get fullscreen() { return !!this.fullscreenElement; }
    exitFullscreen() { return fullscreens.length ? exitFullscreen(false) : Promise.reject(new TypeError('The document is not in fullscreen')); }
    get webkitFullscreenElement() { return this.fullscreenElement; } get webkitCurrentFullScreenElement() { return this.fullscreenElement; } get webkitFullscreenEnabled() { return true; } get webkitIsFullScreen() { return this.fullscreen; }
    webkitExitFullscreen() { return this.exitFullscreen(); } webkitCancelFullScreen() { return this.exitFullscreen(); }
  }
  class XMLDocument extends Document { constructor(type = 'application/xml') { super(); xmlDocuments++; this._type = type; } get contentType() { return this._type; } get body() { return this.documentElement?.namespaceURI === XHTML ? super.body : null; } }
  let xmlDocuments = 0;
  const inXML = node => xmlDocuments > 0 && node.getRootNode() instanceof XMLDocument;
  DocumentFragment.prototype.getElementById = Document.prototype.getElementById;
  const cookies = new Map();
  for (const [name, selector, tags, attrs] of [['forms', 'form', ['form']], ['images', 'img', ['img']], ['links', 'a[href], area[href]', ['a', 'area'], ['href']], ['scripts', 'script', ['script']], ['anchors', 'a[name]', ['a'], ['name']], ['embeds', 'embed', ['embed']], ['plugins', 'embed', ['embed']]]) Object.defineProperty(Document.prototype, name, { configurable: true, get() { return live(this, name, HTMLCollection, () => this.querySelectorAll(selector), { root: this._m, tags, attrs }); } });
  class ParentNode {
    get children() { return this._elements ??= liveList(this, 1) ?? listOf(HTMLCollection, this._m.kids.filter(rec => rec.tag[0] !== '#').map(of)); }
    get childElementCount() { let count = 0; for (const kid of this._m.kids) if (kid.tag[0] !== '#') count++; return count; }
    get firstElementChild() { return of(this._m.kids.find(rec => rec.tag[0] !== '#')); }
    get lastElementChild() { return of(this._m.kids.findLast(rec => rec.tag[0] !== '#')); }
    append(...children) { insertNodes(this, convert(children), null); }
    prepend(...children) { const nodes = convert(children), before = viable(this.firstChild, nodes); insertNodes(this, nodes, before); }
    replaceChildren(...children) { const nodes = convert(children); replaceAll(this, removed => { release(this); detach(this._m); op(1, this._id, ''); removed(); quiet++; try { for (const node of nodes) this.insertBefore(node, null); } finally { quiet--; } }); }
    moveBefore(node, child) { if (!(node instanceof Node) || node.nodeType === 9 || node.nodeType === 11 || node.getRootNode({ composed: true }) !== this.getRootNode({ composed: true })) throw new DOMException('Cannot move the node here', 'HierarchyRequestError'); reactionDepth++; moving = true; try { this.insertBefore(node, child); } finally { moving = false; leave(); } }
    querySelector(selector) { selector = String(selector); const found = idSelector.test(selector) && idsWithin(this._m, selector.slice(1)); return found?.length < 2 ? of(found[0]) : wrap(selectors('dom.queryWithin', this._id, selector, true)); }
    querySelectorAll(selector) { selector = String(selector); const found = idSelector.test(selector) && idsWithin(this._m, selector.slice(1)); return listOf(NodeList, found?.length < 2 ? found.map(of) : selectors('dom.queryWithin', this._id, selector).map(wrap)); }
    getElementsByTagName(name) { name = String(name); const lower = name.toLowerCase(); return live(this, 'tag ' + name, HTMLCollection, () => all(this._m, rec => name === '*' || rec.tag === lower || rec.tag === name), { root: this._m, tags: new Set([lower, name]), drops: true }); }
    getElementsByTagNameNS(namespace, name) { namespace ||= null; name = String(name); return live(this, `ns ${namespace} ${name}`, HTMLCollection, () => all(this._m, rec => name === '*' || rec.tag.endsWith(name)).filter(node => (name === '*' || node.localName === name) && (namespace === '*' || node.namespaceURI === namespace)), { root: this._m, tags: ['*'], drops: true }); }
    getElementsByClassName(names) {
      names = String(names).split(/[\t\n\f\r ]+/).filter(Boolean);
      const has = rec => { const value = rec.attrs.get('class'); if (!value) return false; if (names.length === 1 && value === names[0]) return true; for (const name of names) if (!value.includes(name)) return false; const own = value.split(/[\t\n\f\r ]+/); return names.every(name => own.includes(name)); };
      return live(this, 'class ' + names.join(' '), HTMLCollection, () => names.length ? all(this._m, has) : [], { root: this._m, classes: names, drops: true });
    }
  }
  const { constructor: _, ...parentNode } = Object.getOwnPropertyDescriptors(ParentNode.prototype);
  for (const Type of [Element, Document, DocumentFragment]) Object.defineProperties(Type.prototype, parentNode);
  const xmlEscapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', '\t': '&#9;', '\n': '&#10;', '\r': '&#13;' }, xmlText = (value, attr) => value.replace(attr ? /[&<>"\t\n\r]/g : /[&<>]/g, c => xmlEscapes[c]);
  class XMLSerializer {
    serializeToString(root) {
      if (!(root instanceof Node)) throw new TypeError('Expected a Node');
      let out = '';
      // Entries: a node, its default namespace, and the prefixes in scope.
      if (root._declaration) out += root._declaration;
      for (const stack = [[root, null, { __proto__: null }]]; stack.length;) {
        const item = stack.pop();
        if (typeof item === 'string') { out += item; continue; }
        const [node, ns, outer] = item, type = node.nodeType;
        if (type === 3) out += xmlText(node.data);
        else if (type === 4) out += '<![CDATA[' + node.data + ']]>';
        else if (type === 7) out += '<?' + node.target + (node.data && ' ' + node.data) + '?>';
        else if (type === 8) out += '<!--' + node.data + '-->';
        else if (type === 2) out += xmlText(node.value, true);
        else if (type === 10) out += '<!DOCTYPE ' + node.name + (node.publicId ? ' PUBLIC "' + node.publicId + '"' : node.systemId ? ' SYSTEM' : '') + (node.systemId ? ' "' + node.systemId + '"' : '') + '>';
        else if (type === 1) {
          const space = node.namespaceURI, html = space === XHTML, name = node.prefix ? node._m.tag : node.localName, attrs = node._m.attrs, prefixes = Object.assign({ __proto__: null }, outer);
          for (const [key, value] of attrs) if (key.startsWith('xmlns:')) prefixes[key.slice(6)] = value;
          out += '<' + name;
          if (node.prefix) { if (prefixes[node.prefix] !== space) { prefixes[node.prefix] = space; out += ' xmlns:' + node.prefix + '="' + xmlText(space ?? '', true) + '"'; } }
          else if (space !== ns && !attrs.has('xmlns')) out += ' xmlns="' + xmlText(space ?? '', true) + '"';
          for (const [key, space] of node._spaces ?? []) { const prefix = key.slice(0, key.indexOf(':')); if (attrs.has(key) && key.includes(':') && prefix !== 'xml' && prefix !== 'xmlns' && prefixes[prefix] !== space) { prefixes[prefix] = space; out += ' xmlns:' + prefix + '="' + xmlText(space, true) + '"'; } }
          for (const [key, value] of attrs) out += ' ' + key + '="' + xmlText(value, true) + '"';
          const kids = (html && name === 'template' ? node.content : node).childNodes;
          if (!kids.length && (!html || voids.test(name))) { out += html ? ' />' : '/>'; continue; }
          out += '>'; stack.push('</' + name + '>');
          for (let i = kids.length; i--;) stack.push([kids[i], node.prefix ? attrs.get('xmlns') ?? ns : space, prefixes]);
        } else { const kids = node.childNodes; for (let i = kids.length; i--;) stack.push([kids[i], ns, outer]); }
      }
      return out;
    }
  }
  // As in Chrome, a document is at most this deep.
  const xmlDepth = 5000, XML = 'http://www.w3.org/XML/1998/namespace', XMLNS = 'http://www.w3.org/2000/xmlns/';
  const parseXML = (source, type) => {
    const doc = new XMLDocument(type), open = [doc], scopes = [{ __proto__: null, '': null, xml: 'http://www.w3.org/XML/1998/namespace', xmlns: 'http://www.w3.org/2000/xmlns/' }], entities = new Map([['amp', '&'], ['lt', '<'], ['gt', '>'], ['quot', '"'], ['apos', "'"]]);
    const link = node => { const parent = open.at(-1), kid = node._m; op(2, parent._id, node._id, -1); kid.up = parent._m; kid.i = parent._m.kids.push(kid) - 1; hold(parent, node); };
    let at = 0, expanded = 0, doctype = false;
    const fail = (message = 'Unexpected token: ' + (source[at] ?? ''), where = at) => { throw [message, where]; };
    const decode = (text, start) => text.includes('&') ? text.replace(/&(?:#x([\da-f]+)|#(\d+)|([^\s;&<]+));|&/gi, (found, hex, digits, name, offset) => {
      const code = hex ? parseInt(hex, 16) : digits ? +digits : -1;
      if (code >= 0) { if (code > 0x10ffff || code >= 0xd800 && code < 0xe000 || code === 0xfffe || code === 0xffff || code < 32 && !/[\t\n\r]/.test(String.fromCharCode(code))) fail('Invalid character U+' + code.toString(16).toUpperCase().padStart(4, '0'), start + offset + found.length - 1); return String.fromCodePoint(code); }
      if (!name) fail('Unexpected token inside entity: ' + (text[offset + 1] ?? ''), start + offset + 1);
      const value = entities.get(name) ?? fail('Unexpected entity: ' + name, start + offset + found.length - 1);
      expanded += value.length;
      if (expanded > 1e7) fail('Too much entity text', start + offset);
      return value;
    }) : text;
    const name = /[A-Za-z_:\u0080-￿][\w:.·\u0080-￿-]*/y, space = /[ \t\n\r]*/y;
    const take = pattern => { pattern.lastIndex = at; const found = pattern.exec(source); if (found) at = pattern.lastIndex; return found?.[0]; };
    const until = (end, message = 'Unexpected end of stream') => { const stop = source.indexOf(end, at); if (stop < 0) { at = source.length; fail(message); } const text = source.slice(at, stop); at = stop + end.length; return text; };
    source = source.replace(/\r\n?/g, '\n');
    if (source[0] === '\ufeff') source = source.slice(1);
    const invalid = source.search(/[\0-\x08\x0b\x0c\x0e-\x1f\ufffe\uffff]/), code = source.charCodeAt(invalid);
    if (invalid >= 0) source = source.slice(0, invalid);
    try {
      if (/^<\?xml\s/.test(source)) { at = 5; until('?>'); doc._declaration = source.slice(0, at); }
      while (at < source.length) {
        const start = at, depth = open.length;
        if (source[at] !== '<') {
          const stop = source.indexOf('<', at), text = source.slice(at, stop < 0 ? source.length : stop);
          if (depth === 1) { const bad = text.search(/[^ \t\n]/); if (bad >= 0) { at += bad; fail('Unexpected characters outside the root element: ' + source[at]); } at += text.length; continue; }
          if (text.includes(']]>')) { at += text.indexOf(']]>'); fail('Unexpected token: ]]>'); }
          const value = decode(text, at);
          at += text.length; link(owned(doc, new Text(create('#text', value), '#text')));
        } else if (source.startsWith('<!--', at)) { at += 4; const data = until('-->'); const dashes = (data + '-').indexOf('--'); if (dashes >= 0) fail(`Unexpected token '--' before '${data[dashes + 2] ?? '-'}'`, start + 4 + dashes); link(doc.createComment(data)); }
        else if (source.startsWith('<![CDATA[', at) && depth > 1) { at += 9; link(doc.createCDATASection(until(']]>'))); }
        else if (source.startsWith('<!DOCTYPE', at) && depth === 1 && !doc.documentElement && !doctype) {
          const found = /<!DOCTYPE\s+([^\s>[]+)(?:\s+PUBLIC\s+(?:"([^"]*)"|'([^']*)')(?:\s+(?:"([^"]*)"|'([^']*)'))?|\s+SYSTEM\s+(?:"([^"]*)"|'([^']*)'))?\s*(?:\[([^\]]*)\]\s*)?>/y;
          found.lastIndex = at; const [whole, kind, p1, p2, s1, s2, s3, s4, subset] = found.exec(source) ?? fail();
          at += whole.length;
          doctype = true;
          const entity = /<!ENTITY\s+([^\s%"']+)\s+(["'])/g;
          for (let found; subset && (found = entity.exec(subset));) { const stop = subset.indexOf(found[2], entity.lastIndex); if (stop < 0) break; if (!entities.has(found[1])) entities.set(found[1], subset.slice(entity.lastIndex, stop).replace(/&#(?:x([\da-f]+)|(\d+));/gi, (_, hex, digits) => String.fromCodePoint(Math.min(hex ? parseInt(hex, 16) : +digits, 0x10ffff)))); entity.lastIndex = stop + 1; }
          link(doc.implementation.createDocumentType(kind, p1 ?? p2 ?? '', s1 ?? s2 ?? s3 ?? s4 ?? ''));
        } else if (source.startsWith('<?', at)) {
          at += 2; const target = take(name) ?? fail();
          if (/^xml$/i.test(target)) fail();
          take(space); link(doc.createProcessingInstruction(target, until('?>')));
        } else if (source[at + 1] === '/') {
          at += 2; const tag = take(name), element = open.at(-1); take(space);
          if (depth === 1) { at = start; fail(); }
          if (source[at] !== '>') fail();
          if (tag !== element._m.tag) fail(`Unexpected closing tag: ${tag} != ${element._m.tag}`);
          at++; open.pop(); scopes.pop();
        } else {
          if (depth === 1 && doc.documentElement) fail();
          at++; const tag = take(name) ?? fail(), end = at, attrs = [], places = [], own = new Set();
          let scope = scopes.at(-1), empty = false;
          if (depth > xmlDepth) fail('Excessive depth in document');
          for (;;) {
            const gap = take(space);
            if (source[at] === '>') { at++; break; }
            if (source.startsWith('/>', at)) { at += 2; empty = true; break; }
            if (at >= source.length) fail('Unexpected end of stream');
            if (!gap) fail();
            const place = at, key = take(name) ?? fail();
            take(space); if (source[at] !== '=') fail();
            at++; take(space); const quote = source[at]; if (quote !== '"' && quote !== "'") fail();
            const from = ++at, value = until(quote);
            if (value.includes('<')) { at = from + value.indexOf('<'); fail(); }
            if (own.has(key)) fail(`Attribute '${key}' is redefined`, place);
            own.add(key); const text = decode(value.replace(/[\t\n]/g, ' '), from);
            attrs.push(key, text); places.push(place);
            if (key === 'xmlns' || key.startsWith('xmlns:')) { const prefix = key.slice(6); if (prefix === 'xmlns' || (prefix === 'xml') !== (text === XML) || text === XMLNS || prefix && !text) fail(`Namespace ${text || 'empty'} can't have the prefix ${prefix || 'xmlns'}`, place); if (scope === scopes.at(-1)) scope = Object.assign({ __proto__: null }, scope); scope[prefix] = text || null; }
          }
          for (let i = 0; i < attrs.length; i += 2) { const colon = attrs[i].indexOf(':'); if (colon < 0) continue; const space = scope[attrs[i].slice(0, colon)], full = space + ' ' + attrs[i].slice(colon + 1); if (space === undefined) fail(`Attribute ${attrs[i]} prefix is unbound`, places[i / 2]); if (!/^[^:]+:[^:]+$/.test(attrs[i])) fail(undefined, places[i / 2]); if (own.has(full)) fail(`Attribute '${attrs[i].slice(colon + 1)}' is redefined`, places[i / 2]); own.add(full); }
          const colon = tag.indexOf(':'), prefix = colon > 0 ? tag.slice(0, colon) : '';
          if (colon >= 0 && !/^[^:]+:[^:]+$/.test(tag)) fail(undefined, end);
          if (!(prefix in scope)) fail(`Element ${tag} prefix is unbound`, end);
          const id = create(tag), element = doc._element(id, scope[prefix], tag);
          if (attrs.length) { element._m.attrs = new Attrs(attrs); for (let i = 0; i < attrs.length; i += 2) { op(4, id, attrs[i], attrs[i + 1]); if (attrs[i] === 'id') idAdd(attrs[i + 1], element._m); } }
          link(element);
          if (!empty) { open.push(element); scopes.push(scope); }
        }
      }
      if (invalid >= 0) fail(`Invalid character U+${code.toString(16).toUpperCase().padStart(4, '0')}`, invalid);
      if (open.length > 1) fail('Unexpected end of stream: still inside the root element');
      if (!doc.documentElement) fail('Unexpected end of stream: no root element found');
    } catch (error) {
      if (!Array.isArray(error)) throw error;
      const [message, where] = invalid >= 0 && error[1] >= invalid ? [`Invalid character U+${code.toString(16).toUpperCase().padStart(4, '0')}`, invalid] : error, lines = source.slice(0, where).split('\n');
      // As in Chrome, the document keeps what was read before the error, and tells about it in its first element.
      const make = (tag, style, ...kids) => { const node = doc.createElementNS(XHTML, tag); if (style) node.setAttribute('style', style); node.append(...kids); return node; };
      const report = make('parsererror', 'display: block; white-space: pre; border: 2px solid #c77; padding: 0 1em 0 1em; margin: 1em; background-color: #fdd; color: black',
        make('h3', '', 'This page contains the following errors:'), make('div', 'font-family:monospace;font-size:12px', `error on line ${lines.length} at column ${lines.at(-1).length + 1}: ${message}`), make('h3', '', 'Below is a rendering of the page up to the first error.'));
      const root = doc.documentElement;
      if (root) root.prepend(report); else doc.append(make('html', '', make('body', '', report)));
    }
    return doc;
  };
  class DOMParser {
    parseFromString(source, type) {
      if (type === 'text/xml' || type === 'application/xml' || type === 'application/xhtml+xml' || type === 'image/svg+xml') return parseXML(String(source), type);
      if (type !== 'text/html') throw new TypeError(`'${type}' is not a supported type`);
      const doc = wrap(create('#document')); load(json(call('dom.html', doc._id, String(source)))); return doc;
    }
  }
  Document.parseHTMLUnsafe = html => { const doc = new DOMParser().parseFromString(html, 'text/html'); attachDeclarative(); return doc; };
  const comparePoints = (a, start, b, end) => {
    if (a === b) return Math.sign(start - end);
    const ra = a._m, rb = b._m;
    for (let child = rb; child.up; child = child.up) if (child.up === ra) return start <= index(child) ? -1 : 1;
    for (let child = ra; child.up; child = child.up) if (child.up === rb) return index(child) < end ? -1 : 1;
    return a.compareDocumentPosition(b) & 4 ? -1 : 1;
  };
  const rangeNodes=(range,elements)=>call('dom.rangeNodes',range.startContainer._id,range.startOffset,range.endContainer._id,range.endOffset,elements).map(([id,start,end])=>({node:wrap(id),start,end}));
  const textPoint=(root,offset)=>{const [id,index]=call('dom.textPoint',root._id,offset);return [wrap(id),index];};
  const textOffset=(root,node,offset)=>call('dom.textOffset',root._id,node?._id,offset);
  const characterData = node => node instanceof CharacterData;
  class Range {
    constructor() { this._ref = new WeakRef(this); this._sc = this._ec = document; this._so = this._eo = 0; }
    get startContainer() { return this._sc; } get startOffset() { return this._so; } get endContainer() { return this._ec; } get endOffset() { return this._eo; }
    get collapsed() { return this._sc === this._ec && this._so === this._eo; }
    get commonAncestorContainer() { const ends = new Set(); for (let rec = this._ec._m; rec; rec = rec.up) ends.add(rec); for (let rec = this._sc._m; rec; rec = rec.up) if (ends.has(rec)) return of(rec); return document; }
    setStart(node, offset) {
      offset = Number(offset) >>> 0;
      if (node.nodeType === 10) throw new DOMException('Invalid node type', 'InvalidNodeTypeError');
      if (offset > (characterData(node) ? node.length : node._m.kids.length)) throw new DOMException('Invalid range offset', 'IndexSizeError');
      if (node.getRootNode() !== this._ec.getRootNode() || comparePoints(node, offset, this._ec, this._eo) > 0) place(this, false, node, offset);
      place(this, true, node, offset);
      if (selection?.ranges[0] === this) selection._setRange(this, selection.direction, true);
    }
    setEnd(node, offset) {
      offset = Number(offset) >>> 0;
      if (node.nodeType === 10) throw new DOMException('Invalid node type', 'InvalidNodeTypeError');
      if (offset > (characterData(node) ? node.length : node._m.kids.length)) throw new DOMException('Invalid range offset', 'IndexSizeError');
      if (node.getRootNode() !== this._sc.getRootNode() || comparePoints(node, offset, this._sc, this._so) < 0) place(this, true, node, offset);
      place(this, false, node, offset);
      if (selection?.ranges[0] === this) selection._setRange(this, selection.direction, true);
    }
    setStartBefore(node) { this.setStart(node.parentNode, index(node._m)); } setStartAfter(node) { this.setStart(node.parentNode, index(node._m) + 1); }
    setEndBefore(node) { this.setEnd(node.parentNode, index(node._m)); } setEndAfter(node) { this.setEnd(node.parentNode, index(node._m) + 1); }
    selectNode(node) { this.setStartBefore(node); this.setEndAfter(node); }
    selectNodeContents(node) { this.setStart(node, 0); this.setEnd(node, characterData(node) ? node.length : node._m.kids.length); }
    collapse(start = false) { if (start) this.setEnd(this._sc, this._so); else this.setStart(this._ec, this._eo); }
    cloneRange() { const range = new Range(); place(range, true, this._sc, this._so); place(range, false, this._ec, this._eo); return range; }
    toString() { if (this._sc === this._ec && this._sc.nodeType === 3) return this._sc.data.slice(this._so, this._eo); return this.cloneContents().textContent; }
    cloneContents() {
      const fragment = document.createDocumentFragment();
      if (this.collapsed) return fragment;
      const sc = this._sc._m, ec = this._ec._m, so = this._so, eo = this._eo, starts = new Map(), ends = new Map();
      for (let rec = sc; rec.up; rec = rec.up) starts.set(rec.up, rec);
      for (let rec = ec; rec.up; rec = rec.up) ends.set(rec.up, rec);
      const copy = (rec, parent) => {
        if (rec.tag === '#text' || rec.tag === '#comment') { const text = own(rec).slice(rec === sc ? so : 0, rec === ec ? eo : undefined); parent.append(rec.tag === '#text' ? document.createTextNode(text) : document.createComment(text)); return; }
        const first = rec === sc ? null : starts.get(rec), last = rec === ec ? null : ends.get(rec), kids = rec.kids;
        const partial = kid => { if (kid.tag === '#text' || kid.tag === '#comment') copy(kid, parent); else { const clone = of(kid).cloneNode(false); copy(kid, clone); parent.append(clone); } };
        if (first) partial(first);
        for (let i = rec === sc ? so : first ? index(first) + 1 : 0, end = rec === ec ? eo : last ? index(last) : kids.length; i < end; i++) parent.append(of(kids[i]).cloneNode(true));
        if (last && last !== first) partial(last);
      };
      copy(this.commonAncestorContainer._m, fragment);
      return fragment;
    }
    deleteContents() {
      if (this.collapsed) return;
      const start = this._sc, end = this._ec, so = this._so, eo = this._eo;
      if (start === end && characterData(start)) { start.deleteData(so, eo - so); return; }
      const common = this.commonAncestorContainer._m, removed = [], tail = [];
      let from = so, to = eo, reference = null;
      if (start._m !== common) {
        let rec = start._m;
        for (let i = so; i < rec.kids.length; i++) removed.push(rec.kids[i]);
        for (; rec.up !== common; rec = rec.up) for (let i = index(rec) + 1; i < rec.up.kids.length; i++) removed.push(rec.up.kids[i]);
        reference = rec; from = index(rec) + 1;
      }
      if (end._m !== common) {
        let rec = end._m;
        tail.push(rec.kids.slice(0, eo));
        for (; rec.up !== common; rec = rec.up) tail.push(rec.up.kids.slice(0, index(rec)));
        to = index(rec);
      }
      for (let i = from; i < to; i++) removed.push(common.kids[i]);
      for (let i = tail.length; i--;) removed.push(...tail[i]);
      reactionDepth++;
      try {
        if (characterData(start)) start.deleteData(so, start.length - so);
        for (const rec of removed) of(rec).remove();
        if (characterData(end)) end.deleteData(0, eo);
      } finally { leave(); }
      if (reference) this.setStart(of(common), index(reference) + 1); else this.setStart(start, so);
      this.collapse(true);
    }
    extractContents() {
      const fragment = document.createDocumentFragment();
      if (this.collapsed) return fragment;
      const sc = this._sc, so = this._so, ec = this._ec, eo = this._eo;
      if (sc === ec && characterData(sc)) { const clone = sc.cloneNode(false); clone.data = sc.data.slice(so, eo); fragment.appendChild(clone); sc.deleteData(so, eo - so); return fragment; }
      const common = this.commonAncestorContainer, kids = common._m.kids;
      let first = sc === common ? null : sc, last = ec === common ? null : ec;
      while (first && first.parentNode !== common) first = first.parentNode;
      while (last && last.parentNode !== common) last = last.parentNode;
      const contained = kids.slice(first ? index(first._m) + 1 : so, last ? index(last._m) : eo).map(of);
      if (contained.some(node => node.nodeType === 10)) throw new DOMException('Cannot extract a doctype', 'HierarchyRequestError');
      const [node, offset] = first ? [common, index(first._m) + 1] : [sc, so];
      const part = (child, start) => {
        const clone = child.cloneNode(false); fragment.appendChild(clone);
        if (characterData(child)) { const from = start ? so : 0, to = start ? child.length : eo; clone.data = child.data.slice(from, to); child.deleteData(from, to - from); return; }
        const range = new Range(); if (start) { range.setStart(sc, so); range.setEnd(child, child._m.kids.length); } else { range.setStart(child, 0); range.setEnd(ec, eo); }
        clone.appendChild(range.extractContents());
      };
      reactionDepth++;
      try {
        if (first) part(first, true);
        for (const child of contained) fragment.appendChild(child);
        if (last) part(last, false);
      } finally { leave(); }
      this.setStart(node, offset); this.collapse(true);
      return fragment;
    }
    insertNode(node) {
      const start = this._sc, text = start.nodeType === 3;
      if (start.nodeType === 8 || text && !start.parentNode || start === node) throw new DOMException('Cannot insert the node here', 'HierarchyRequestError');
      const parent = text ? start.parentNode : start;
      let reference = text ? start.splitText(this._so) : of(start._m.kids[this._so]);
      if (reference === node) reference = node.nextSibling;
      node.parentNode?.removeChild(node);
      const offset = (reference ? index(reference._m) : parent._m.kids.length) + (node.nodeType === 11 ? node._m.kids.length : 1);
      parent.insertBefore(node, reference);
      if (this.collapsed) this.setEnd(parent, offset);
    }
    surroundContents(node) {
      const common = this.commonAncestorContainer;
      for (const end of [this._sc, this._ec]) for (let up = end; up !== common; up = up.parentNode) if (up.nodeType !== 3) throw new DOMException('The range partially selects a non-Text node', 'InvalidStateError');
      if (node.nodeType === 9 || node.nodeType === 10 || node.nodeType === 11) throw new DOMException('Invalid new parent', 'InvalidNodeTypeError');
      const contents = this.extractContents();
      node.replaceChildren();
      this.insertNode(node); node.append(contents); this.selectNode(node);
    }
    createContextualFragment(html) { const node = this.startContainer.nodeType === 1 ? this.startContainer : this.startContainer.parentElement; return parseFragment(html, node && !(node.localName === 'html' && node.namespaceURI === XHTML) ? node : null); }
    getBoundingClientRect() {const rects=this.getClientRects();if(!rects.length)return new DOMRect();const left=Math.min(...rects.map(rect=>rect.left)),top=Math.min(...rects.map(rect=>rect.top));return new DOMRect(left,top,Math.max(...rects.map(rect=>rect.right))-left,Math.max(...rects.map(rect=>rect.bottom))-top);}
    // Like the CSSOM spec: the boxes of the elements that the range holds whole, and of its text.
    getClientRects() {layout();return rangeNodes(this,true).flatMap(({node,start,end})=>node.nodeType===1?[...node.getClientRects()]:textCall('dom.textRect',node._id,start,end).map(rect=>new DOMRect(...rect)));}
    compareBoundaryPoints(how,range){return how===0?comparePoints(this.startContainer,this.startOffset,range.startContainer,range.startOffset):how===1?comparePoints(this.endContainer,this.endOffset,range.startContainer,range.startOffset):how===2?comparePoints(this.endContainer,this.endOffset,range.endContainer,range.endOffset):comparePoints(this.startContainer,this.startOffset,range.endContainer,range.endOffset);}
    isPointInRange(node,offset){if(node.getRootNode()!==this._sc.getRootNode())return false;return comparePoints(node,offset,this.startContainer,this.startOffset)>=0&&comparePoints(node,offset,this.endContainer,this.endOffset)<=0;}
    comparePoint(node,offset){if(node.getRootNode()!==this._sc.getRootNode())throw new DOMException('The node is in another tree','WrongDocumentError');return comparePoints(node,offset,this.startContainer,this.startOffset)<0?-1:comparePoints(node,offset,this.endContainer,this.endOffset)>0?1:0;}
    intersectsNode(node){if(!node.parentNode)return true;const parent=node.parentNode,at=index(node._m);return comparePoints(parent,at+1,this._sc,this._so)>0&&comparePoints(parent,at,this._ec,this._eo)<0;}
    detach() {}
  }
  for (const [name, value] of Object.entries({ START_TO_START: 0, START_TO_END: 1, END_TO_END: 2, END_TO_START: 3 })) { Range[name] = value; Range.prototype[name] = value; }
  class StaticRange {
    constructor({startContainer, startOffset, endContainer, endOffset}) {
      for (const node of [startContainer, endContainer]) {
        if (!(node instanceof Node)) throw new TypeError('Expected a Node');
        if (node.nodeType === 2 || node.nodeType === 10) throw new DOMException('Invalid range node', 'InvalidNodeTypeError');
      }
      for (const [key, value] of Object.entries({startContainer, startOffset: Number(startOffset) >>> 0, endContainer, endOffset: Number(endOffset) >>> 0})) Object.defineProperty(this, key, {value, enumerable:true});
    }
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; }
  }
  const rtlBlock = node => { while (node && (node.nodeType !== 1 || /^(inline|contents)$/.test(getComputedStyle(node).display))) node = node.parentNode; return !!node && getComputedStyle(node).direction === 'rtl'; };
  class Selection {
    constructor() { this.ranges = []; this.direction = 'none'; }
    get anchorNode() { return this.ranges[0]?.[this.direction === 'backward' ? 'endContainer' : 'startContainer'] ?? null; }
    get anchorOffset() { return this.ranges[0]?.[this.direction === 'backward' ? 'endOffset' : 'startOffset'] ?? 0; }
    get focusNode() { return this.ranges[0]?.[this.direction === 'backward' ? 'startContainer' : 'endContainer'] ?? null; }
    get focusOffset() { return this.ranges[0]?.[this.direction === 'backward' ? 'startOffset' : 'endOffset'] ?? 0; }
    get rangeCount() { return this.ranges.length; } get isCollapsed() { return !this.rangeCount || this.ranges[0].collapsed; } get type() { return !this.rangeCount ? 'None' : this.isCollapsed ? 'Caret' : 'Range'; }
    _setRange(range, direction = 'none', changed = false) {
      const previous = this.ranges[0], moved = !previous || direction !== this.direction || ['startContainer','startOffset','endContainer','endOffset'].some(key => previous[key] !== range[key]);
      this.ranges = [range]; this.direction = direction; this._x = undefined;
      if (moved) typing = [];
      if (!changed && !moved) return;
      this._send();
      selectionChanged();
    }
    _send(sync = false) {
      const range = this.ranges[0], caret = !!range?.collapsed && !!active?.isContentEditable && active.contains(range.startContainer);
      if (!range) return;
      const nodes = rangeNodes(range).map(({node,start,end}) => [node._id,start,end]);
      if (caret && !nodes.length) { const { startContainer: parent, startOffset: offset } = range, child = parent.nodeType === 1 ? parent.childNodes[offset] ?? parent.lastChild : null; if (child?.localName === 'br') nodes.push([child._id, 0, 0]); }
      call('dom.selection', 0, nodes, caret, sync);
    }
    addRange(range) { if (!this.rangeCount && range.startContainer.isConnected) this._setRange(range); }
    removeRange(range) { if (this.ranges[0] !== range) throw new DOMException('Range is not selected', 'NotFoundError'); this.removeAllRanges(); }
    getRangeAt(index) { if (!this.ranges[index]) throw new DOMException('Invalid range index', 'IndexSizeError'); return this.ranges[index]; }
    getComposedRanges({shadowRoots = []} = {}) {
      const roots = new Set();
      for (const shadow of shadowRoots) {
        if (!(shadow instanceof ShadowRoot)) throw new TypeError('Expected a ShadowRoot');
        for (let root = shadow; root instanceof ShadowRoot; root = root.host.getRootNode()) roots.add(root);
      }
      if (!this.rangeCount) return [];
      const range = this.ranges[0], points = {};
      for (const side of ['start', 'end']) {
        let node = range[side + 'Container'], offset = range[side + 'Offset'];
        for (let root = node.getRootNode(); root instanceof ShadowRoot && !roots.has(root); root = node.getRootNode()) {
          const host = root.host;
          node = host.parentNode;
          offset = index(host._m) + (side === 'end' ? 1 : 0);
        }
        points[side + 'Container'] = node;
        points[side + 'Offset'] = offset;
      }
      return [new StaticRange(points)];
    }
    removeAllRanges() { if (!this.rangeCount) return; this.ranges = []; this.direction = 'none'; call('dom.selection',0,0,0); selectionChanged(); } empty() { this.removeAllRanges(); }
    collapse(node, offset = 0) { if (!node) return this.removeAllRanges(); const range = new Range(); range.setStart(node, offset); range.collapse(true); if (node.isConnected) this._setRange(range); }
    setPosition(node, offset) { this.collapse(node, offset); }
    collapseToStart() { if (!this.rangeCount) throw new DOMException('Selection is empty', 'InvalidStateError'); const range = this.ranges[0]; this.collapse(range.startContainer, range.startOffset); }
    collapseToEnd() { if (!this.rangeCount) throw new DOMException('Selection is empty', 'InvalidStateError'); const range = this.ranges[0]; this.collapse(range.endContainer, range.endOffset); }
    selectAllChildren(node) { const range = new Range(); range.selectNodeContents(node); if (node.isConnected) this._setRange(range); }
    setBaseAndExtent(anchor, start, focus, end) {
      const range = new Range(); range.setStart(anchor, start); range.setEnd(focus, end);
      if (!anchor.isConnected || anchor.getRootNode() !== focus.getRootNode()) return;
      const order = comparePoints(anchor, start, focus, end);
      if (order > 0) { range.setStart(focus, end); range.setEnd(anchor, start); }
      this._setRange(range, order > 0 ? 'backward' : order < 0 ? 'forward' : 'none');
    }
    extend(node, offset = 0) { if (!this.rangeCount) throw new DOMException('Selection is empty', 'InvalidStateError'); this.setBaseAndExtent(this.anchorNode, this.anchorOffset, node, offset); }
    containsNode(node, partial = false) { if (!this.rangeCount || node.getRootNode() !== this.anchorNode.getRootNode()) return false; const range = this.ranges[0], end = characterData(node) ? node.length : node._m.kids.length; return partial ? range.intersectsNode(node) : comparePoints(range.startContainer, range.startOffset, node, 0) <= 0 && comparePoints(range.endContainer, range.endOffset, node, end) >= 0; }
    deleteFromDocument() { for (const range of this.ranges) range.deleteContents(); }
    modify(alter, direction, granularity) {
      const to = this._point(alter, direction, granularity); if (!to) return;
      if (String(alter).toLowerCase() === 'extend') this.setBaseAndExtent(this.anchorNode, this.anchorOffset, to[0], to[1]); else this.collapse(to[0], to[1]);
      this._x = to[2];
    }
    // The new focus of modify(), and the x to keep for the next vertical move.
    _point(alter, direction, granularity) {
      [alter, direction, granularity] = [alter, direction, granularity].map(value => String(value).toLowerCase());
      if (!this.rangeCount || !/^(move|extend)$/.test(alter) || !/^(forward|backward|left|right)$/.test(direction) || !/^(character|word|sentence|line|paragraph|lineboundary|sentenceboundary|paragraphboundary|documentboundary)$/.test(granularity)) return;
      const focus = this.focusNode, forward = direction === 'forward' || /^(left|right)$/.test(direction) && (direction === 'right') !== rtlBlock(focus), range = this.ranges[0];
      if (granularity === 'character' && alter === 'move' && !range.collapsed) return forward ? [range.endContainer, range.endOffset] : [range.startContainer, range.startOffset];
      let host = focus.nodeType === 1 ? focus : focus.parentElement;
      if (host?.isContentEditable) while (host.parentElement?.isContentEditable) host = host.parentElement; else host = null;
      const root = host ?? document.body; if (!root) return;
      if (/line|paragraph$/.test(granularity)) layout();
      // Read the text near the focus first, and more of it when a move reaches the edge of what was read.
      for (let window = granularity === 'documentboundary' ? -1 : 8; ; window *= 16) {
        const [text, runs, from, first, last, hidden] = call('dom.caretText', root._id, !!host, focus._id, this.focusOffset, window < 0 || window > 1e5 ? null : window);
        if (from < 0) return;
        const n = text.length;
        const run = i => { let lo = 0, hi = runs.length / 3 - 1; while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (runs[mid * 3] <= i) lo = mid; else hi = mid - 1; } return lo * 3; };
        // The DOM point before (side 0) or after (side 1) the character at i.
        const point = (i, side) => { const r = run(i), node = runs[r + 1] < 0 ? null : wrap(runs[r + 1]); return !node ? null : runs[r + 2] < 0 ? [node.parentNode, index(node._m) + side] : [node, runs[r + 2] + i - runs[r] + side]; };
        // Caret geometry takes the box of the character after the point, so a soft wrap belongs to the next line.
        const rect = p => {
          const after = p < n && (text[p] !== '\n' || p === 0 || text[p - 1] === '\n' && runs[run(p) + 1] >= 0), i = after ? p : p - 1, r = run(i), node = runs[r + 1];
          if (i < 0 || node < 0) return null;
          if (runs[r + 2] >= 0) { const at = runs[r + 2] + i - runs[r] + !after, box = after && textCall('dom.textRect', node, at, at + 1)[0] || textCall('dom.textRect', node, at, at)[0]; return box && { x: box[0], y: box[1], h: box[3] }; }
          const box = wrap(node).getBoundingClientRect(); return { x: after ? box.left : box.right, y: box.top, h: box.height };
        };
        // The first index in [lo, hi) where test holds, for tests that hold from some index on.
        const search = (lo, hi, test) => { while (lo < hi) { const mid = (lo + hi) >> 1; if (test(mid)) hi = mid; else lo = mid + 1; } return lo; };
        // The first and last index on the line of p.
        const lineStart = p => { const box = rect(p); return box ? search(0, p, i => !(rect(i)?.y < box.y - box.h / 2)) : p; };
        const lineEnd = p => { const box = rect(p); return box ? search(p + 1, n + 1, i => !(rect(i)?.y < box.y + box.h / 2)) - 1 : p; };
        const vertical = (p, x) => {
          let start, end;
          if (forward) { start = lineEnd(p) + 1; if (start > n) return n; end = lineEnd(start); }
          else { end = lineStart(p) - 1; if (end < 0) return 0; start = lineStart(end); }
          const at = search(start, end + 1, i => !(rect(i)?.x < x));
          return at > start && (at > end || x - (rect(at - 1)?.x ?? -Infinity) <= rect(at).x - x) ? at - 1 : at;
        };
        const segment = (kind, i) => (kind === 'word' ? words ??= new Intl.Segmenter(undefined, { granularity: 'word' }) : kind === 'sentence' ? sentences ??= new Intl.Segmenter(undefined, { granularity: 'sentence' }) : graphemes ??= new Intl.Segmenter(undefined, { granularity: 'grapheme' })).segment(text).containing(i);
        let p = from, x = this._x;
        if (granularity === 'character') p = forward ? (p < n ? (s => s.index + s.segment.length)(segment('grapheme', p)) : p) : p > 0 ? segment('grapheme', p - 1).index : p;
        else if (granularity === 'word') {
          // Like Chrome on macOS: to the far edge of the next word or punctuation, and line breaks count as words.
          const space = s => /^[^\S\n]+$/.test(s.segment);
          if (forward) { for (let s = p < n && segment('word', p); s; s = s.index + s.segment.length < n && segment('word', s.index + s.segment.length)) if (!space(s)) { p = s.index + s.segment.length; break; } }
          else for (let s = p > 0 && segment('word', p - 1); s; s = s.index > 0 && segment('word', s.index - 1)) if (!space(s)) { p = s.index; break; }
        } else if (granularity === 'sentence' || granularity === 'sentenceboundary') {
          if (forward) { for (let at = p; at < n;) { const s = segment('sentence', at), end = s.index + s.segment.length; at = end; if (end - (text[end - 1] === '\n') > p) { p = end - (text[end - 1] === '\n'); break; } if (granularity === 'sentenceboundary') break; } }
          else if (p > 0) p = segment('sentence', granularity === 'sentence' || p === n ? p - 1 : p).index;
        } else if (granularity === 'paragraphboundary') p = forward ? (text.indexOf('\n', p) + 1 || n + 1) - 1 : text.lastIndexOf('\n', p - 1) + 1;
        // As in Chrome, the ends of a page that starts or ends in editable content are no places for the caret.
        else if (granularity === 'documentboundary') { if (hidden[+forward]) return; p = forward ? n : 0; }
        // As in Chrome, editable text ends a line after the space at a soft wrap.
        else if (granularity === 'lineboundary') p = !forward ? lineStart(p) : (end => end >= n || text[end] === '\n' || !host && /[ \t]/.test(text[end]) ? end : end + 1)(lineEnd(p));
        else { x ??= rect(p)?.x ?? 0; const start = text.lastIndexOf('\n', p - 1), end = text.indexOf('\n', p); do { const next = vertical(p, x); if (next === p) break; p = next; } while (granularity === 'paragraph' && p > start && (end < 0 || p <= end)); }
        if (p === 0 && !first || p === n && !last) continue;
        const place = i => (i > 0 && text[i - 1] !== '\n' && point(i - 1, 1)) || (i < n && point(i, 0)) || (i > 0 && point(i - 1, 1)) || [root, 0];
        return [...place(p), /line|paragraph$/.test(granularity) ? x : undefined];
      }
    }
    toString() { return call('dom.selectionText', 0, this.ranges.flatMap(range => rangeNodes(range).map(({node,start,end}) => [node._id,start,end]))); }
  }
  class MutationObserver {
    constructor(callback) { if (typeof callback !== 'function') throw new TypeError('Expected a callback'); this.callback = callback; this.targets = new Map(); this.records = []; this._ref = null; }
    observe(target, options = {}) {
      if (!(target instanceof Node)) throw new TypeError('Expected a Node');
      const attributes = options.attributes ?? (options.attributeOldValue !== undefined || options.attributeFilter !== undefined), characterData = options.characterData ?? options.characterDataOldValue !== undefined;
      if (!options.childList && !attributes && !characterData) throw new TypeError('The options must set childList, attributes or characterData');
      if (!attributes && (options.attributeOldValue || options.attributeFilter) || !characterData && options.characterDataOldValue) throw new TypeError('Old values and filters need their kind of record');
      watch(this, target, { ...options, attributes, characterData, attributeFilter: options.attributeFilter === undefined ? undefined : Array.from(options.attributeFilter, String) }, observers);
      if (this.targets.size > (this._limit ?? 64)) { for (const key of this.targets.keys()) if (!key.deref()) this.targets.delete(key); this._limit = Math.max(64, this.targets.size * 2); }
    }
    disconnect() { unwatchAll(this, observers); this.records = []; } takeRecords() { return this.records.splice(0); }
  }
  const intersectionObservers=new Set();
  class IntersectionObserver {
    constructor(callback,options={}){if(typeof callback!=='function')throw new TypeError('Expected a callback');this.callback=callback;this.root=options.root??null;if(this.root!==null&&!(this.root instanceof Element||this.root instanceof Document))throw new TypeError('Expected an Element or Document root');const margins=String(options.rootMargin??'0px').trim().split(/\s+/).map(value=>{const match=/^([-+]?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?)(px|%)?$/i.exec(value);if(!match||!match[2]&&+match[1]!==0)throw new DOMException('Invalid rootMargin','SyntaxError');return [+match[1],match[2]==='%'];});if(margins.length>4)throw new DOMException('Invalid rootMargin','SyntaxError');const [top,right=top,bottom=top,left=right]=margins;this._margins=[top,right,bottom,left];this.rootMargin=this._margins.map(([v,p])=>v+(p?'%':'px')).join(' ');this.thresholds=[...new Set(Array.isArray(options.threshold)?options.threshold:[options.threshold??0])].sort((a,b)=>a-b);if(this.thresholds.some(v=>v<0||v>1))throw new RangeError('Invalid intersection threshold');this.targets=new Map();this.records=[];this._ref=null;}
    observe(target){if(this.targets.has(target._m.ref))return;watch(this,target,null,intersectionObservers);host('dom.requestFrame');}unobserve(target){unwatch(this,target,intersectionObservers);}disconnect(){unwatchAll(this,intersectionObservers);this.records=[];}takeRecords(){return this.records.splice(0);}
  }
  const resizeObservers = new Set();
  const atomicTags = /^(img|svg|canvas|video|audio|iframe|embed|object|input|select|textarea|button|math)$/;
  class ResizeObserver { constructor(callback) { if (typeof callback !== 'function') throw new TypeError('Expected a callback'); this.callback = callback; this.targets = new Map(); this._ref = null; } observe(target, options) { const box = String(options?.box ?? 'content-box'); if (this.targets.has(target._m.ref) && (this._boxes ??= new WeakMap()).get(target) === box) return; (this._boxes ??= new WeakMap()).set(target, box); watch(this, target, null, resizeObservers); host('dom.requestFrame'); } unobserve(target) { unwatch(this, target, resizeObservers); } disconnect() { unwatchAll(this, resizeObservers); } }
  const breakout = /^(b|big|blockquote|body|br|center|code|dd|div|dl|dt|em|embed|h[1-6]|head|hr|i|img|li|listing|menu|meta|nobr|ol|p|pre|ruby|s|small|span|strong|strike|sub|sup|table|tt|u|ul|var)$/;
  const wrap = id => {
    let rec = records[id], node = rec?.ref?.deref();
    if (node || !rec) return node ?? null;
    const chain = [];
    for (; rec && !(node = rec.ref?.deref()); rec = rec.up) chain.push(rec);
    for (let i = chain.length; i--;) {
      const { id, tag, attrs } = chain[i], above = node, svg = tag === 'svg' || (above instanceof SVGElement ? !integration.test(above.localName) : !!above?._svg && !breakout.test(tag) && !(tag === 'font' && (attrs.has('color') || attrs.has('face') || attrs.has('size'))));
      const Type = tag === '#document' ? Document : tag === '#fragment' ? DocumentFragment : tag === '#text' ? Text : tag === '#comment' ? Comment : tag === '#doctype' ? DocumentType : svg ? (tag === 'svg' ? SVGSVGElement : SVGElement) : htmlTypes.get(tag) ?? HTMLElement;
      node = new Type(id, tag); if (above) hold(above, node);
    }
    return node;
  };
  let sweepAt = 0, cursor = 0, canary = new WeakRef({});
  globalThis.__appsDomSweep = (force = false) => {
    scheduled = false; flush();
    if (force && !size && codes.length > 1024) codes = new Int32Array(1024);
    // Release removed, unwrapped trees next turn so row churn cannot fill the arena between sweeps.
    const collected = !canary.deref(), timed = force || collected || performance.now() >= sweepAt, dead = [];
    if (!timed && !detached) return;
    detached = false;
    if (collected) canary = new WeakRef({});
    if (timed) {
      sweepAt = performance.now() + 250;
      for (const [rec, set] of anchors) { for (const ref of set) if (!ref.deref()) set.delete(ref); if (!set.size) anchors.delete(rec); }
    }
    if (force || collected) cursor = 0;
    for (let budget = force || collected ? wrapped.length : timed ? 4096 : 0; budget-- && wrapped.length;) {
      if (cursor >= wrapped.length) cursor = 0;
      const rec = wrapped[cursor];
      if (rec.ref?.deref()) { cursor++; continue; }
      dead.push(rec.id); rec.ref = null; wrapped[cursor] = wrapped[wrapped.length - 1]; wrapped.pop();
    }
    if (!dead.length && !registered.length && !changed) return;
    changed = false;
    for (const id of call('dom.release', dead, registered.splice(0))[1]) { const rec = records[id]; if (!rec) continue; records[id] = undefined; const name = rec.attrs?.get('id'); if (name) idDrop(name, rec); }
  };

  const matrixFields = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'], plane = [0, 1, 4, 5, 12, 13];
  const flat2D = m => m[2] === 0 && m[3] === 0 && m[6] === 0 && m[7] === 0 && m[8] === 0 && m[9] === 0 && m[10] === 1 && m[11] === 0 && m[14] === 0 && m[15] === 1;
  class DOMMatrixReadOnly {
    constructor(init) {
      if (init !== undefined && typeof init?.[Symbol.iterator] !== 'function' || typeof init === 'string') { init = String(init); const match = /^\s*matrix(?:3d)?\(([^)]*)\)\s*$/.exec(init); if (!match && !/^\s*(none)?\s*$/.test(init)) throw new DOMException(`Failed to parse '${init}'`, 'SyntaxError'); init = match?.[1].split(','); }
      const v = init === undefined ? [1, 0, 0, 1, 0, 0] : Array.from(init, Number);
      if (v.length === 6) { this._m = [v[0], v[1], 0, 0, v[2], v[3], 0, 0, 0, 0, 1, 0, v[4], v[5], 0, 1]; this._2d = true; }
      else if (v.length === 16) { this._m = v; this._2d = false; }
      else throw new TypeError('A matrix needs 6 or 16 values');
    }
    static fromMatrix(init = {}) {
      const read = init instanceof DOMMatrixReadOnly, m = read ? init._m : matrixFields.map((name, i) => +(init[name] ?? init['abcdef'[plane.indexOf(i)]] ?? +(i % 5 === 0)));
      return Object.assign(new this(m), { _2d: read ? init._2d : init.is2D ?? flat2D(m) });
    }
    static fromFloat32Array(array) { return new this(array); } static fromFloat64Array(array) { return new this(array); }
    get is2D() { return this._2d; } get isIdentity() { return this._m.every((v, i) => v === +(i % 5 === 0)); }
    transformPoint(point = {}) { const m = this._m, { x = 0, y = 0, z = 0, w = 1 } = point; return new DOMPoint(m[0] * x + m[4] * y + m[8] * z + m[12] * w, m[1] * x + m[5] * y + m[9] * z + m[13] * w, m[2] * x + m[6] * y + m[10] * z + m[14] * w, m[3] * x + m[7] * y + m[11] * z + m[15] * w); }
    flipX() { return this.multiply(new DOMMatrix([-1, 0, 0, 1, 0, 0])); } flipY() { return this.multiply(new DOMMatrix([1, 0, 0, -1, 0, 0])); }
    toFloat32Array() { return new Float32Array(this._m); } toFloat64Array() { return new Float64Array(this._m); }
    toString() { const m = this._m; return this._2d ? `matrix(${plane.map(i => m[i]).join(', ')})` : `matrix3d(${m.join(', ')})`; }
    toJSON() { const json = {}; for (const name of [...'abcdef', ...matrixFields, 'is2D', 'isIdentity']) json[name] = this[name]; return json; }
  }
  class DOMMatrix extends DOMMatrixReadOnly {
    multiplySelf(other) { const b = other instanceof DOMMatrixReadOnly ? other : DOMMatrix.fromMatrix(other); return this._mul(b._m, b._2d); }
    _mul(m, flat) { const a = this._m, out = new Array(16); for (let i = 0; i < 16; i++) { const r = i & 3, c = i & 12; out[i] = a[r] * m[c] + a[4 + r] * m[c + 1] + a[8 + r] * m[c + 2] + a[12 + r] * m[c + 3]; } this._m = out; this._2d &&= flat; return this; }
    preMultiplySelf(other) { const matrix = DOMMatrix.fromMatrix(other).multiplySelf(this); this._m = matrix._m; this._2d = matrix._2d; return this; }
    translateSelf(x = 0, y = 0, z = 0) { return this._mul([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, Number(x), Number(y), Number(z), 1], z === 0); }
    scaleSelf(x = 1, y = x, z = 1, ox = 0, oy = 0, oz = 0) { return this.translateSelf(ox, oy, oz)._mul([Number(x), 0, 0, 0, 0, Number(y), 0, 0, 0, 0, Number(z), 0, 0, 0, 0, 1], z === 1).translateSelf(-ox, -oy, -oz); }
    scale3dSelf(scale = 1, ox = 0, oy = 0, oz = 0) { return this.scaleSelf(scale, scale, scale, ox, oy, oz); }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      const length = Math.hypot(x, y, z); if (!length) return this; [x, y, z] = [x / length, y / length, z / length];
      const r = angle * Math.PI / 180, s = Math.sin(r), c = Math.cos(r), t = 1 - c;
      const m = [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0, t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0, t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0, 0, 0, 0, 1];
      return this._mul(m, flat2D(m));
    }
    rotateSelf(x = 0, y, z) { if (y === undefined && z === undefined) [x, y, z] = [0, 0, x]; return this.rotateAxisAngleSelf(0, 0, 1, z ?? 0).rotateAxisAngleSelf(0, 1, 0, y ?? 0).rotateAxisAngleSelf(1, 0, 0, x); }
    skewXSelf(angle = 0) { return this.multiplySelf(new DOMMatrix([1, 0, Math.tan(angle * Math.PI / 180), 1, 0, 0])); } skewYSelf(angle = 0) { return this.multiplySelf(new DOMMatrix([1, Math.tan(angle * Math.PI / 180), 0, 1, 0, 0])); }
    invertSelf() {
      const m = this._m, out = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1], work = [...m];
      for (let col = 0; col < 4; col++) {
        let pivot = col; for (let row = col + 1; row < 4; row++) if (Math.abs(work[col * 4 + row]) > Math.abs(work[col * 4 + pivot])) pivot = row;
        const value = work[col * 4 + pivot]; if (!value || !Number.isFinite(value)) { this._m = m.map(() => NaN); this._2d = false; return this; }
        for (const array of [work, out]) for (let k = 0; k < 4; k++) [array[k * 4 + col], array[k * 4 + pivot]] = [array[k * 4 + pivot] / value, array[k * 4 + col]];
        for (let row = 0; row < 4; row++) { const factor = work[col * 4 + row]; if (row !== col && factor) for (const array of [work, out]) for (let k = 0; k < 4; k++) array[k * 4 + row] -= factor * array[k * 4 + col]; }
      }
      this._m = out; return this;
    }
  }
  for (const [name, self] of [['multiply', 'multiplySelf'], ['translate', 'translateSelf'], ['scale', 'scaleSelf'], ['scale3d', 'scale3dSelf'], ['rotate', 'rotateSelf'], ['rotateAxisAngle', 'rotateAxisAngleSelf'], ['skewX', 'skewXSelf'], ['skewY', 'skewYSelf'], ['inverse', 'invertSelf']]) DOMMatrixReadOnly.prototype[name] = function (...args) { return DOMMatrix.fromMatrix(this)[self](...args); };
  for (const [i, name] of [...matrixFields.entries(), ...[...'abcdef'].map((name, i) => [plane[i], name])]) {
    Object.defineProperty(DOMMatrixReadOnly.prototype, name, { configurable: true, enumerable: true, get() { return this._m[i]; } });
    Object.defineProperty(DOMMatrix.prototype, name, { configurable: true, enumerable: true, get() { return this._m[i]; }, set(value) { this._m[i] = +value; if (!plane.includes(i) && +value !== +(i % 5 === 0)) this._2d = false; } });
  }
  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) { this._p = [+x, +y, +z, +w]; }
    static fromPoint(point = {}) { return new this(point.x, point.y, point.z, point.w); }
    matrixTransform(matrix) { return DOMMatrixReadOnly.fromMatrix(matrix).transformPoint(this); }
    toJSON() { const { x, y, z, w } = this; return { x, y, z, w }; }
  }
  class DOMPoint extends DOMPointReadOnly {}
  for (const [i, name] of ['x', 'y', 'z', 'w'].entries()) { Object.defineProperty(DOMPointReadOnly.prototype, name, { configurable: true, enumerable: true, get() { return this._p[i]; } }); Object.defineProperty(DOMPoint.prototype, name, { configurable: true, enumerable: true, get() { return this._p[i]; }, set(value) { this._p[i] = +value; } }); }
  // Match Canvas::draw opcodes and argument counts in render/canvas.rs. Keep f64 precision for large
  // coordinates.
  class CanvasPath {
    arc(x, y, radius, start, end, counter = false) { if (radius < 0) throw new DOMException('The radius is negative', 'IndexSizeError'); this._op(28, x, y, radius, start, end, counter ? 1 : 0); }
    ellipse(x, y, rx, ry, rotation, start, end, counter = false) { if (rx < 0 || ry < 0) throw new DOMException('The radius is negative', 'IndexSizeError'); this._op(29, x, y, rx, ry, rotation, start, end, counter ? 1 : 0); }
    arcTo(x1, y1, x2, y2, radius) { if (radius < 0) throw new DOMException('The radius is negative', 'IndexSizeError'); this._op(30, x1, y1, x2, y2, radius); }
    roundRect(x, y, width, height, radii = 0) { const list = (Array.isArray(radii) ? radii : [radii]).map(v => typeof v === 'object' ? [+v.x || 0, +v.y || 0] : [+v, +v]); if (!list.length || list.length > 4 || list.some(r => r[0] < 0 || r[1] < 0)) throw new RangeError('Invalid corner radii'); const [a, b, c, d] = list.length === 1 ? [list[0], list[0], list[0], list[0]] : list.length === 2 ? [list[0], list[1], list[0], list[1]] : list.length === 3 ? [list[0], list[1], list[2], list[1]] : list; const w = Math.abs(width), h = Math.abs(height), fit = (size, sum) => sum > size ? size / sum : 1, k = Math.min(fit(w, a[0] + b[0]), fit(w, d[0] + c[0]), fit(h, a[1] + d[1]), fit(h, b[1] + c[1])); this._op(31, x, y, width, height, ...[a, b, c, d].flat().map(v => v * k)); }
    closePath() { this._op(12); } moveTo(x, y) { this._op(13, x, y); } lineTo(x, y) { this._op(14, x, y); } rect(x, y, width, height) { this._op(17, x, y, width, height); }
    quadraticCurveTo(x1, y1, x, y) { this._op(15, x1, y1, x, y); } bezierCurveTo(x1, y1, x2, y2, x, y) { this._op(16, x1, y1, x2, y2, x, y); }
  }
  const fillRule = (rule = 'nonzero') => { if (rule !== 'nonzero' && rule !== 'evenodd') throw new TypeError(`'${rule}' is not a valid fill rule`); return +(rule === 'evenodd'); };
  let pathIds = 0;
  class Path2D extends CanvasPath {
    constructor(path) { super(); this._ops = new Float64Array(16); this._size = 0; this._id = ++pathIds; if (path instanceof Path2D) this._push(path._ops.subarray(0, path._size)); else if (path !== undefined) this._push(host('dom.path', String(path))); }
    _op(...values) { this._push(values); }
    _push(values) { const size = this._size + values.length; if (size > this._ops.length) { const grown = new Float64Array(Math.max(size, this._ops.length * 2)); grown.set(this._ops.subarray(0, this._size)); this._ops = grown; } this._ops.set(values, this._size); this._size = size; }
    addPath(path, transform) { if (!(path instanceof Path2D)) throw new TypeError('Expected a Path2D'); const ops = path._ops.subarray(0, path._size), { a, b, c, d, e, f } = DOMMatrix.fromMatrix(transform); if ([a, b, c, d, e, f].every(Number.isFinite)) { this._push([0, 2, a, b, c, d, e, f]); this._push(ops); this._op(1); } }
  }
  class CanvasRenderingContext2D extends CanvasPath {
    constructor(canvas, options) { super(); this.canvas = canvas; this._alpha = options?.alpha !== false; this._ops = new Float64Array(256); this._size = 0; this._texts = []; this._data = []; this._paths = new WeakMap(); this._pathSize = 0; this._reset(); }
    _reset() { this._state = { fillStyle: '#000000', strokeStyle: '#000000', font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', globalAlpha: 1, lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDash: [], lineDashOffset: 0, globalCompositeOperation: 'source-over', letterSpacing: '0px', wordSpacing: '0px', imageSmoothingEnabled: true, imageSmoothingQuality: 'low', shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal', textRendering: 'auto', direction: 'inherit', lang: 'inherit' }; this._stack = []; }
    _op(code, ...args) {
      const size = this._size;
      if (size + args.length >= this._ops.length) { const grown = new Float64Array(this._ops.length * 2 + args.length); grown.set(this._ops); this._ops = grown; }
      this._ops[size] = code; for (let i = 0; i < args.length; i++) this._ops[size + 1 + i] = args[i];
      this._size = size + 1 + args.length;
      if (!size) queueMicrotask(() => this._flush()); else if (this._size > 1 << 18) this._flush();
    }
    // A draw from another 2D canvas reads its pixels when it runs, so that canvas flushes its readers before its pixels change.
    _read(source) { (source._readers ??= new Set()).add(this); (this._sources ??= new Set()).add(source); }
    _release() { const readers = this._readers; if (!readers) return; for (const reader of readers) reader._flush(); this._readers = null; }
    _forget() { if (!this._sources) return; for (const source of this._sources) source._readers?.delete(this); this._sources = null; }
    _flush() { if (!this._size) return; this._release(); this._forget(); const ops = this._ops.subarray(0, this._size), text = this._texts.join(''), data = this._data; this._size = 0; this._texts.length = 0; this._data = []; bitmapDraws.delete(this); if (this.canvas.width && this.canvas.height) call('dom.canvas', this.canvas._id, ops, text, ...data); this.canvas._commit?.(); }
    // Canvas::draw keeps each Path2D it gets until the context resets or sends more than about 8 MB of paths.
    _path(path) {
      if (!(path instanceof Path2D)) throw new TypeError('Expected a Path2D');
      if (this._paths.get(path) === path._size) return path._id;
      const reset = (this._pathSize += path._size) > 1 << 20; if (reset) { this._paths = new WeakMap(); this._pathSize = path._size; }
      this._paths.set(path, path._size); this._data.push(path._ops.subarray(0, path._size)); this._op(41, path._id, this._data.length - 1, +reset); return path._id;
    }
    _resize() { this._release(); this._forget(); this._size = 0; this._texts.length = 0; this._data = []; this._reset(); this._paths = new WeakMap(); this._pathSize = 0; call('dom.canvas', this.canvas._id, null); }
    save() { this._stack.push({ ...this._state }); this._op(0); } restore() { if (this._stack.length) { this._state = this._stack.pop(); this._op(1); } }
    setTransform(a, b, c, d, e, f) { if (arguments.length < 2) ({ a, b, c, d, e, f } = DOMMatrix.fromMatrix(a)); this._op(3, a, b, c, d, e, f); }
    resetTransform() { this._op(3, 1, 0, 0, 1, 0, 0); } transform(a, b, c, d, e, f) { this._op(2, a, b, c, d, e, f); }
    scale(x, y) { this._op(2, x, 0, 0, y, 0, 0); } translate(x, y) { this._op(2, 1, 0, 0, 1, x, y); } rotate(a) { const cos = Math.cos(a), sin = Math.sin(a); this._op(2, cos, sin, -sin, cos, 0, 0); }
    fillRect(x, y, width, height) { this._op(21, x, y, width, height); } strokeRect(x, y, width, height) { this._op(22, x, y, width, height); } clearRect(x, y, width, height) { this._op(23, x, y, width, height); }
    fillText(text, x, y, maxWidth) { this._text(24, text, x, y, maxWidth); } strokeText(text, x, y, maxWidth) { this._text(27, text, x, y, maxWidth); }
    _text(code, text, x, y, maxWidth = Infinity) { text = String(text); if (arguments.length > 4 && arguments[4] !== undefined && !(Number.isFinite(+maxWidth) && maxWidth > 0)) return; this._texts.push(alone(text)); this._op(code, text.length, x, y, maxWidth); }
    measureText(text) { const state = this._state, [width, left, right, top, bottom, ascent, descent, emAscent, emDescent] = call('dom.measureText', this.canvas._id, String(text), this.font, parseFloat(state.letterSpacing), parseFloat(state.wordSpacing)); const base = { top: emAscent, hanging: ascent * 0.8, middle: (emAscent - emDescent) / 2, bottom: -emDescent, ideographic: -descent }[state.textBaseline] ?? 0, anchor = width * ({ center: 0.5, right: 1, end: 1 }[state.textAlign] ?? 0); return Object.assign(Object.create(TextMetrics.prototype), { width, actualBoundingBoxLeft: left === right ? anchor : anchor - left, actualBoundingBoxRight: left === right ? -anchor : right - anchor, actualBoundingBoxAscent: top - base, actualBoundingBoxDescent: bottom + base, fontBoundingBoxAscent: ascent - base, fontBoundingBoxDescent: descent + base, emHeightAscent: emAscent - base, emHeightDescent: emDescent + base, hangingBaseline: ascent * 0.8 - base, alphabeticBaseline: -base, ideographicBaseline: -descent - base }); }
    getContextAttributes() { return { alpha: this._alpha, colorSpace: 'srgb', desynchronized: false, willReadFrequently: false }; } isContextLost() { return false; } drawFocusIfNeeded() {} reset() { this._resize(); }
    getTransform() { this._flush(); return new DOMMatrix(call('dom.canvasQuery', this.canvas._id)); }
    isPointInPath(...args) { const id = args[0] instanceof Path2D ? this._path(args.shift()) : null, [x, y, rule] = args, evenOdd = !!fillRule(rule); if (!Number.isFinite(+x) || !Number.isFinite(+y)) return false; this._flush(); return call('dom.canvasQuery', this.canvas._id, +x, +y, evenOdd, false, id); }
    isPointInStroke(...args) { const id = args[0] instanceof Path2D ? this._path(args.shift()) : null, [x, y] = args; if (!Number.isFinite(+x) || !Number.isFinite(+y)) return false; this._flush(); return call('dom.canvasQuery', this.canvas._id, +x, +y, false, true, id); }
    createLinearGradient(x0, y0, x1, y1) { return new CanvasGradient([x0, y0, x1, y1]); } createRadialGradient(x0, y0, r0, x1, y1, r1) { if (r0 < 0 || r1 < 0) throw new DOMException('The radius is negative', 'IndexSizeError'); return new CanvasGradient([x0, y0, r0, x1, y1, r1]); } createConicGradient(angle, x, y) { return new CanvasGradient([angle, x, y]); }
    createPattern(image, repetition) {
      if (!drawable(image)) throw new TypeError('Expected an image, canvas, video or bitmap');
      repetition = repetition === null || repetition === '' ? 'repeat' : String(repetition); if (!['repeat', 'repeat-x', 'repeat-y', 'no-repeat'].includes(repetition)) throw new DOMException(`'${repetition}' is not a valid repetition`, 'SyntaxError');
      const [width, height] = imageSize(image); if (!width || !height || image instanceof HTMLVideoElement && image.readyState < 2) { if (image instanceof HTMLCanvasElement || image instanceof OffscreenCanvas) throw new DOMException('The canvas has no pixels', 'InvalidStateError'); return null; }
      return Object.assign(Object.create(CanvasPattern.prototype), { _image: image, _size: [width, height], _repeat: [repetition === 'repeat' || repetition === 'repeat-x', repetition === 'repeat' || repetition === 'repeat-y'], _matrix: new DOMMatrix() });
    }
    setLineDash(list) { list = Array.from(list, Number); if (!list.every(v => Number.isFinite(v) && v >= 0)) return; if (list.length % 2) list = [...list, ...list]; this._state.lineDash = list; const text = list.join(); this._texts.push(text); this._op(35, text.length); } getLineDash() { return [...this._state.lineDash]; }
    createImageData(width,height) { if (width instanceof ImageData) return new ImageData(width.width,width.height); return new ImageData(width,height); }
    getImageData(x, y, width, height) { this._flush(); x |= 0; y |= 0; width |= 0; height |= 0; if (!width || !height) throw new DOMException('Image dimensions must be nonzero', 'IndexSizeError'); if (width < 0) { x += width; width = -width; } if (height < 0) { y += height; height = -height; } return new ImageData(new Uint8ClampedArray(call('dom.canvasPixels', this.canvas._id, x, y, width, height)), width, height); }
    putImageData(data, x, y) { if (!(data instanceof ImageData)) throw new TypeError('Expected ImageData'); this._data.push(data.data.slice()); this._op(26, this._data.length - 1, x | 0, y | 0, data.width, data.height); }
    drawImage(image,...args) {
      if (!drawable(image)) throw new TypeError('Expected an image, canvas, video or bitmap');
      if (image instanceof HTMLVideoElement && image.readyState < 2) return;
      image._context?._flush?.();
      const [iw, ih] = imageSize(image); if (!iw||!ih) throw new DOMException('Image is not loaded','InvalidStateError');
      let sx=0,sy=0,sw=iw,sh=ih,dx,dy,dw=iw,dh=ih; if(args.length===2) [dx,dy]=args; else if(args.length===4) [dx,dy,dw,dh]=args; else if(args.length===8) [sx,sy,sw,sh,dx,dy,dw,dh]=args; else throw new TypeError('Invalid drawImage arguments');
      if (sw<0) { sx+=sw; sw=-sw; } if (sh<0) { sy+=sh; sh=-sh; } if (dw<0) { dx+=dw; dw=-dw; } if (dh<0) { dy+=dh; dh=-dh; }
      const left=Math.max(sx,0),top=Math.max(sy,0),right=Math.min(sx+sw,iw),bottom=Math.min(sy+sh,ih),kx=dw/sw,ky=dh/sh;
      if (right<=left||bottom<=top) return;
      dx+=(left-sx)*kx; dy+=(top-sy)*ky; dw=(right-left)*kx; dh=(bottom-top)*ky;
      this._op(25,image===this.canvas?-1:image._id,iw,ih,left,top,right-left,bottom-top,dx,dy,dw,dh);
      if (image instanceof ImageBitmap) bitmapDraws.add(this); else if (image._context instanceof CanvasRenderingContext2D && image !== this.canvas) this._read(image._context); else if (image._context && image!==this.canvas) this._flush();
    }
    // The source rectangle is in CSS pixels from the border box of the element. Without a size, the element draws as
    // large as the page would show it.
    drawElementImage(element, ...args) {
      const options = args.length % 2 ? args.pop() : undefined, image = element instanceof ElementImage, canvas = this.canvas instanceof OffscreenCanvas ? this.canvas._node : this.canvas;
      if (![2, 4, 6, 8].includes(args.length)) throw new TypeError('Invalid drawElementImage arguments');
      const size = snapshotOf(canvas, element), [width, height] = size, [kx, ky] = gridScale(canvas, size);
      let sx = 0, sy = 0, sw = width, sh = height, dx, dy, dw, dh;
      if (args.length < 6) [dx, dy, dw, dh] = args; else [sx, sy, sw, sh, dx, dy, dw, dh] = args;
      dw ??= sw * kx; dh ??= sh * ky;
      const target = image ? element._element : element, to = !options?.preserveElementGeometry && target?.isConnected && this.getTransform().translateSelf(dx, dy).scaleSelf(dw / sw, dh / sh).translateSelf(-sx, -sy);
      if (to) placeElement(this.canvas, target, { canvasTransform: new DOMMatrix().scaleSelf(1 / kx, 1 / ky).multiplySelf(to) });
      if (sw < 0) { sx += sw; sw = -sw; } if (sh < 0) { sy += sh; sh = -sh; } if (dw < 0) { dx += dw; dw = -dw; } if (dh < 0) { dy += dh; dh = -dh; }
      const left = Math.max(sx, 0), top = Math.max(sy, 0), right = Math.min(sx + sw, width), bottom = Math.min(sy + sh, height), fx = dw / sw, fy = dh / sh;
      if (right <= left || bottom <= top) return;
      this._op(25, image ? element._node._id : SNAPSHOT + element._id, width, height, left, top, right - left, bottom - top, dx + (left - sx) * fx, dy + (top - sy) * fy, (right - left) * fx, (bottom - top) * fy);
      if (image) bitmapDraws.add(this);
    }

    beginPath() { this._op(11); }
    fill(path, rule) { if (path instanceof Path2D) this._op(42, this._path(path), 18, fillRule(rule)); else this._op(18, fillRule(path)); }
    stroke(path) { if (path === undefined) this._op(19); else this._op(42, this._path(path), 19, 0); }
    clip(path, rule) { if (path instanceof Path2D) this._op(42, this._path(path), 20, fillRule(rule)); else this._op(20, fillRule(path)); }
  }
  // Both ops depend only on the text, so their answers are kept.
  const canvasColors = new Map(), canvasFonts = new Map(), canvasCurrent = new Map();
  const canvasParsed = (cache, op, value) => { let parsed = cache.get(value); if (parsed === undefined) { if (cache.size > 256) cache.clear(); cache.set(value, parsed = call(op, 0, value)); } return parsed; };
  // As in Chromium, `currentcolor` alone is the `color` of the inline style of a connected canvas element, else black.
  // Inside a color function it is black, and an OffscreenCanvas rejects such a function.
  const canvasColor = (context, value) => {
    if (/currentcolor/i.test(value)) {
      if (/^[\t\n\f\r ]*currentcolor[\t\n\f\r ]*$/i.test(value)) {
        // The inline color comes only from the style attribute, so its color is kept by the attribute text. As in Chromium, a `color-mix()` there is black.
        const style = context.canvas?.isConnected && !(context instanceof OffscreenCanvasRenderingContext2D) && context.canvas.getAttribute('style');
        if (!style) return '#000000';
        let color = canvasCurrent.get(style);
        if (color === undefined) { if (canvasCurrent.size > 256) canvasCurrent.clear(); const inline = context.canvas.style.getPropertyValue('color'); canvasCurrent.set(style, color = (!/^\s*color-mix\(/i.test(inline) && canvasParsed(canvasColors, 'dom.canvasColor', inline)) || '#000000'); }
        return color;
      }
      if (context instanceof OffscreenCanvasRenderingContext2D) return null;
      value = value.replace(/currentcolor/gi, '#000');
    }
    const color = canvasParsed(canvasColors, 'dom.canvasColor', value);
    return color === null || /^\s*(oklch|oklab|lab|lch|color)\(/i.test(value) ? color && value : color;
  };
  for (const [name, code] of [['fillStyle', 4], ['strokeStyle', 5], ['font', 6], ['textAlign', 7], ['textBaseline', 8], ['globalAlpha', 9], ['lineWidth', 10]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { get() { return this._state[name]; }, set(value) {
    if (code < 6 && (value instanceof CanvasGradient || value instanceof CanvasPattern)) { this._state[name] = value; const image = value._image; image?._context?._flush?.(); const text = value._spec(image === this.canvas ? -1 : image?._id); this._texts.push(text); this._op(code, text.length); if (image instanceof ImageBitmap) bitmapDraws.add(this); else if (image?._context instanceof CanvasRenderingContext2D && image !== this.canvas) this._read(image._context); else if (image?._context && image !== this.canvas) this._flush(); return; }
    if (code === 6) { value = canvasParsed(canvasFonts, 'dom.canvasFont', String(value)); if (value === null) return; } else if (code < 9) value = String(value); else if (value = Number(value), !(code === 9 ? value >= 0 && value <= 1 : value > 0 && value < Infinity)) return;
    if (code < 6 && (value = canvasColor(this, value)) === null) return;
    if (this._state[name] === value) return;
    this._state[name] = value; if (code < 9) { this._texts.push(alone(value)); this._op(code, value.length); } else this._op(code, value);
  } });
  for (const [name, code, list] of [['lineCap', 32, 'butt round square'], ['lineJoin', 33, 'miter round bevel'], ['globalCompositeOperation', 37, 'source-over source-in source-out source-atop destination-over destination-in destination-out destination-atop lighter copy xor multiply screen overlay darken lighten color-dodge color-burn hard-light soft-light difference exclusion hue saturation color luminosity']]) { const values = new Set(list.split(' ')); Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { value = String(value); if (!values.has(value) || this._state[name] === value) return; this._state[name] = value; this._texts.push(value); this._op(code, value.length); } }); }
  for (const [name, code, valid] of [['miterLimit', 34, v => v > 0 && v < Infinity], ['lineDashOffset', 36, Number.isFinite]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { value = Number(value); if (valid(value)) { this._state[name] = value; this._op(code, value); } } });
  for (const [name, code] of [['letterSpacing', 38], ['wordSpacing', 39]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { const px = /^(-?[\d.]+)px$/.exec(String(value).trim()); if (px) { this._state[name] = `${+px[1]}px`; this._op(code, +px[1]); } } });
  Object.defineProperty(CanvasRenderingContext2D.prototype, 'imageSmoothingEnabled', { configurable: true, enumerable: true, get() { return this._state.imageSmoothingEnabled; }, set(value) { this._state.imageSmoothingEnabled = !!value; this._op(40, value ? 1 : 0); } });
  for (const [name, valid] of [['shadowColor', () => true], ['shadowBlur', v => v >= 0 && v < Infinity], ['shadowOffsetX', Number.isFinite], ['shadowOffsetY', Number.isFinite]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) {
    value = name === 'shadowColor' ? canvasColor(this, String(value)) : Number(value); if (value === null || !valid(value)) return;
    const state = this._state; state[name] = value; this._texts.push(state.shadowColor); this._op(43, state.shadowColor.length, state.shadowBlur, state.shadowOffsetX, state.shadowOffsetY);
  } });
  // Of the filter functions, only `blur()` draws: other values keep their text and draw without a filter.
  Object.defineProperty(CanvasRenderingContext2D.prototype, 'filter', { configurable: true, enumerable: true, get() { return this._state.filter; }, set(value) { value = String(value); const blur = /^\s*blur\(\s*([\d.]+)(px)?\s*\)\s*$/i.exec(value); if (value.trim() !== 'none' && !blur && !CSS.supports('filter', value)) return; this._state.filter = value; this._op(44, blur ? Number(blur[1]) : 0); } });
  for (const name of ['imageSmoothingQuality', 'fontKerning', 'fontStretch', 'fontVariantCaps', 'textRendering', 'direction', 'lang']) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { this._state[name] = value; } });
  class CanvasGradient {
    constructor(shape) { this._shape = shape.map(Number); this._stops = []; }
    addColorStop(offset, color) { offset = Number(offset); if (!(offset >= 0 && offset <= 1)) throw new DOMException('The offset is out of range', 'IndexSizeError'); this._stops.push([offset, String(color)]); }
    _spec() { return 'gradient:' + JSON.stringify([this._shape, [...this._stops].sort((a, b) => a[0] - b[0])]); }
  }
  class CanvasPattern {
    constructor() { throw new TypeError('Illegal constructor'); }
    setTransform(matrix) { matrix = DOMMatrix.fromMatrix(matrix); if ([matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f].every(Number.isFinite)) this._matrix = matrix; }
    _spec(id) { const { a, b, c, d, e, f } = this._matrix; return 'pattern:' + JSON.stringify([id, ...this._size, ...this._repeat.map(Number), a, b, c, d, e, f]); }
  }
  class TextMetrics {}
  // Web Audio: sources started in one task render to one PCM WAV, played by the <audio> player. Gain and biquad
  // nodes are linear, so each source renders along its own paths and the results add up.
  const audioCap = 10, audioTail = 0.1;
  // The nominal ranges of the spec, as floats like Chromium: 1200 log2(FLT_MAX) cents and 40 log10(FLT_MAX) dB.
  const detuneMax = Math.fround(1200 * Math.log2(3.4028234663852886e38)), gainMax = Math.fround(40 * Math.log10(3.4028234663852886e38));
  class AudioParam {
    constructor(context, value, min = -3.4028234663852886e38, max = 3.4028234663852886e38) { this._context = context; this.defaultValue = this._value = value; this.minValue = min; this.maxValue = max; this.automationRate = 'a-rate'; this._events = []; }
    get value() { return this._events.length ? this._at(this._context.currentTime) : Math.min(this.maxValue, Math.max(this.minValue, this._value)); }
    set value(value) { value = +value; if (!Number.isFinite(value)) throw new TypeError('The value is not finite'); if (this._events.length) this.setValueAtTime(value, this._context.currentTime); else this._value = value; }
    _add(kind, value, time, extra) {
      value = +value; time = +time;
      if (!Number.isFinite(value) || !Number.isFinite(time) || time < 0) throw new RangeError('The value or time is not valid');
      const at = this._events.findLastIndex(event => event[1] <= time) + 1;
      this._events.splice(at, 0, [kind, time, value, extra]); return this;
    }
    setValueAtTime(value, time) { return this._add(0, value, time); }
    linearRampToValueAtTime(value, time) { return this._add(1, value, time); }
    exponentialRampToValueAtTime(value, time) { if (!+value) throw new RangeError('The value must not be 0'); return this._add(2, value, time); }
    setTargetAtTime(value, time, constant) { if (!(+constant >= 0)) throw new RangeError('The time constant must not be negative'); return this._add(3, value, time, +constant); }
    setValueCurveAtTime(values, time, duration) { values = [...values]; this._add(0, values[0], time); values.forEach((value, i) => i && this._add(1, value, time + duration * i / (values.length - 1))); return this; }
    cancelScheduledValues(time) { this._events = this._events.filter(event => event[1] < time); return this; }
    cancelAndHoldAtTime(time) { const value = this._at(time); this.cancelScheduledValues(time); return this._add(0, value, time); }
    // The value at a time, as the spec's automation timeline gives it, clamped to the nominal range.
    _at(time) {
      let value = this._value, from = 0, target = null, out = null;
      const settled = at => target ? target[2] + (value - target[2]) * (target[3] ? Math.exp(-(at - target[1]) / target[3]) : 0) : value;
      for (const event of this._events) {
        const [kind, at, to] = event;
        if (time < at) {
          if (kind === 1 || kind === 2) { const start = settled(from), t = (time - from) / (at - from); out = kind === 1 ? start + (to - start) * t : start * to > 0 ? start * (to / start) ** t : start; }
          break;
        }
        value = kind === 3 ? settled(at) : to; target = kind === 3 ? event : null; from = at;
      }
      return Math.min(this.maxValue, Math.max(this.minValue, out ?? settled(time)));
    }
    // The values of a run of frames. Like `_at`, but it walks the events once, since the times only grow.
    _run(start, rate, length) {
      const out = new Float32Array(length), events = this._events, count = events.length;
      const min = this.minValue, max = this.maxValue;
      if (!count) return out.fill(Math.min(max, Math.max(min, this._value)));
      let value = this._value, from = 0, target = null, next = 0;
      const settled = at => target ? target[2] + (value - target[2]) * (target[3] ? Math.exp(-(at - target[1]) / target[3]) : 0) : value;
      for (let i = 0; i < length; i++) {
        const time = start + i / rate;
        for (; next < count && !(time < events[next][1]); next++) { const event = events[next], kind = event[0], at = event[1]; value = kind === 3 ? settled(at) : event[2]; target = kind === 3 ? event : null; from = at; }
        let v;
        if (next === count) v = settled(time);
        else {
          const [kind, at, to] = events[next];
          if (kind === 1 || kind === 2) { const begin = settled(from), t = (time - from) / (at - from); v = kind === 1 ? begin + (to - begin) * t : begin * to > 0 ? begin * (to / begin) ** t : begin; }
          else v = settled(time);
        }
        out[i] = Math.min(max, Math.max(min, v));
      }
      return out;
    }
  }
  class AudioNode extends EventTarget {
    constructor(context) { super(); this.context = context; this._outputs = new Set(); this.channelCount = 2; this.channelCountMode = 'max'; this.channelInterpretation = 'speakers'; }
    get numberOfInputs() { return 1; } get numberOfOutputs() { return 1; }
    // Connections to an AudioParam do nothing.
    connect(target) { if (target instanceof AudioNode) this._outputs.add(target); return target; }
    disconnect(target) { if (target === undefined) this._outputs.clear(); else this._outputs.delete(target); }
    _process(channels) { return channels; }
  }
  const audioOptions = (node, options = {}) => { for (const [key, value] of Object.entries(options)) if (node[key] instanceof AudioParam) node[key].value = value; else if (key in node) node[key] = value; return node; };
  class GainNode extends AudioNode {
    constructor(context, options) { super(context); this.gain = new AudioParam(context, 1); audioOptions(this, options); }
    _process(channels, start, rate) { const gain = this.gain._run(start, rate, channels[0].length); for (const data of channels) for (let i = 0; i < data.length; i++) data[i] *= gain[i]; return channels; }
  }
  const filterTypes = ['lowpass', 'highpass', 'bandpass', 'lowshelf', 'highshelf', 'peaking', 'notch', 'allpass'];
  class BiquadFilterNode extends AudioNode {
    constructor(context, options) { super(context); this._type = 'lowpass'; Object.assign(this, { frequency: new AudioParam(context, 350, 0, context.sampleRate / 2), detune: new AudioParam(context, 0, -detuneMax, detuneMax), Q: new AudioParam(context, 1), gain: new AudioParam(context, 0, undefined, gainMax) }); audioOptions(this, options); }
    get type() { return this._type; } set type(value) { if (filterTypes.includes(value)) this._type = value; }
    // The coefficients of the spec, from the Audio EQ Cookbook, b0 b1 b2 a1 a2 over a0.
    _coefficients(frequency, detune, q, gain, rate) {
      const f = Math.min(frequency * 2 ** (detune / 1200), rate / 2) / rate, w = 2 * Math.PI * f, cos = Math.cos(w), sin = Math.sin(w), A = 10 ** (gain / 40);
      if (f <= 0 || f >= 0.5) { const open = this._type === 'lowpass' ? f > 0 : this._type === 'highpass' ? f <= 0 : true; return [open ? 1 : 0, 0, 0, 0, 0]; }
      const qdb = sin / (2 * 10 ** (q / 20)), aq = sin / (2 * Math.max(q, 1e-4)), as = sin / 2 * Math.SQRT2, root = 2 * Math.sqrt(A) * as;
      const [b0, b1, b2, a0, a1, a2] = {
        lowpass: [(1 - cos) / 2, 1 - cos, (1 - cos) / 2, 1 + qdb, -2 * cos, 1 - qdb],
        highpass: [(1 + cos) / 2, -(1 + cos), (1 + cos) / 2, 1 + qdb, -2 * cos, 1 - qdb],
        bandpass: [aq, 0, -aq, 1 + aq, -2 * cos, 1 - aq],
        notch: [1, -2 * cos, 1, 1 + aq, -2 * cos, 1 - aq],
        allpass: [1 - aq, -2 * cos, 1 + aq, 1 + aq, -2 * cos, 1 - aq],
        peaking: [1 + aq * A, -2 * cos, 1 - aq * A, 1 + aq / A, -2 * cos, 1 - aq / A],
        lowshelf: [A * (A + 1 - (A - 1) * cos + root), 2 * A * (A - 1 - (A + 1) * cos), A * (A + 1 - (A - 1) * cos - root), A + 1 + (A - 1) * cos + root, -2 * (A - 1 + (A + 1) * cos), A + 1 + (A - 1) * cos - root],
        highshelf: [A * (A + 1 + (A - 1) * cos + root), -2 * A * (A - 1 + (A + 1) * cos), A * (A + 1 + (A - 1) * cos - root), A + 1 - (A - 1) * cos + root, 2 * (A - 1 - (A + 1) * cos), A + 1 - (A - 1) * cos - root],
      }[this._type];
      return [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
    }
    _process(channels, start, rate) {
      const length = channels[0].length, params = [this.frequency, this.detune, this.Q, this.gain].map(param => param._run(start, rate, length));
      for (const data of channels) {
        let x1 = 0, x2 = 0, y1 = 0, y2 = 0, c;
        for (let i = 0; i < length; i++) {
          // New coefficients for each block of 128 frames, as browsers compute them.
          if (i % 128 === 0) c = this._coefficients(params[0][i], params[1][i], params[2][i], params[3][i], rate);
          const x = data[i], y = c[0] * x + c[1] * x1 + c[2] * x2 - c[3] * y1 - c[4] * y2;
          x2 = x1; x1 = x; y2 = y1; y1 = Math.abs(y) < 1e-30 ? 0 : y; data[i] = y1;
        }
      }
      return channels;
    }
    getFrequencyResponse(frequencies, magnitudes, phases) {
      const rate = this.context.sampleRate, [b0, b1, b2, a1, a2] = this._coefficients(this.frequency.value, this.detune.value, this.Q.value, this.gain.value, rate);
      frequencies.forEach((frequency, i) => {
        const w = 2 * Math.PI * frequency / rate, [c1, s1, c2, s2] = [Math.cos(w), -Math.sin(w), Math.cos(2 * w), -Math.sin(2 * w)];
        const [nr, ni, dr, di] = [b0 + b1 * c1 + b2 * c2, b1 * s1 + b2 * s2, 1 + a1 * c1 + a2 * c2, a1 * s1 + a2 * s2], d = dr * dr + di * di;
        const [re, im] = [(nr * dr + ni * di) / d, (ni * dr - nr * di) / d];
        magnitudes[i] = Math.hypot(re, im); phases[i] = Math.atan2(im, re);
      });
    }
  }
  class AudioDestinationNode extends AudioNode { get maxChannelCount() { return 2; } }
  class AudioBuffer {
    constructor(options, duration) {
      // Decoded files keep their encoded data, so the <audio> player plays them whole.
      if (typeof options === 'string') { this._url = options; this.duration = duration; this.length = Math.round(duration * 48000); this.sampleRate = 48000; this.numberOfChannels = 2; return; }
      const { numberOfChannels = 1, length, sampleRate } = options;
      if (!(numberOfChannels >= 1 && numberOfChannels <= 32 && length >= 1 && sampleRate >= 3000 && sampleRate <= 768000)) throw new DOMException('The buffer size is not supported', 'NotSupportedError');
      Object.assign(this, { numberOfChannels, length: length >>> 0, sampleRate, duration: (length >>> 0) / sampleRate });
      this._data = Array.from({ length: numberOfChannels }, () => new Float32Array(this.length));
    }
    _channel(channel) { if (!this._data) throw new DOMException('A decoded buffer has no samples', 'NotSupportedError'); const data = this._data[channel]; if (!data) throw new DOMException('The channel does not exist', 'IndexSizeError'); return data; }
    getChannelData(channel) { return this._channel(channel); }
    copyFromChannel(destination, channel, start = 0) { destination.set(this._channel(channel).subarray(start, start + destination.length)); }
    copyToChannel(source, channel, start = 0) { this._channel(channel).set(source.subarray(0, this.length - start), start); }
  }
  class AudioScheduledSourceNode extends AudioNode {
    onended = null;
    start(when = 0) {
      if (this._start !== undefined) throw new DOMException('The source has already started', 'InvalidStateError');
      this._start = Math.max(+when, this.context.currentTime); this.context._schedule(this);
    }
    stop(when = 0) {
      if (this._start === undefined) throw new DOMException('The source has not started', 'InvalidStateError');
      this._stop = Math.max(+when, this._start); this._audio?._close(); this._audio = null;
    }
    _ended() { const event = new Event('ended'); this.dispatchEvent(event); }
  }
  const waveTypes = ['sine', 'square', 'sawtooth', 'triangle'];
  class PeriodicWave {
    constructor(context, { real, imag, disableNormalization = false } = {}) {
      const r = real === undefined ? null : Float32Array.from(real), i = imag === undefined ? null : Float32Array.from(imag); this._real = r ?? (i ? new Float32Array(i.length) : Float32Array.of(0, 0)); this._imag = i ?? (r ? new Float32Array(r.length) : Float32Array.of(0, 1)); if (this._real.some(v => !Number.isFinite(v)) || this._imag.some(v => !Number.isFinite(v))) throw new TypeError('The provided float value is non-finite.'); if (this._real.length !== this._imag.length || this._real.length < 2) throw new DOMException(`The lengths of the real (${this._real.length}) and imaginary (${this._imag.length}) arrays must match and be at least 2`, 'IndexSizeError'); this._real[0] = this._imag[0] = 0; this._scale = 1;
      if (disableNormalization) return;
      // As in Chromium, the scale makes the peak of the table with all partials 1. A few partials are summed at each
      // point of the table, more take an inverse FFT of the table size.
      const rate = context.sampleRate, n = rate <= 24000 ? 2048 : rate <= 88200 ? 4096 : 16384, count = Math.min(this._real.length, n / 2), ar = this._real, ai = this._imag;
      let peak = 0;
      if (count - 1 <= Math.log2(n)) {
        const c1 = Math.cos(2 * Math.PI / n), s1 = Math.sin(2 * Math.PI / n);
        for (let i = 0, wr = 1, wi = 0; i < n; i++) {
          let sum = 0, zr = wr, zi = wi;
          for (let k = 1; k < count; k++) { sum += ar[k] * zr + ai[k] * zi; const t = zr * wr - zi * wi; zi = zr * wi + zi * wr; zr = t; }
          peak = Math.max(peak, Math.abs(sum));
          const t = wr * c1 - wi * s1; wi = wr * s1 + wi * c1; wr = t;
        }
      } else {
        const re = new Float64Array(n), im = new Float64Array(n);
        for (let k = 1; k < count; k++) { re[k] = ar[k]; im[k] = -ai[k]; }
        for (let i = 1, j = 0; i < n; i++) { let bit = n >> 1; for (; j & bit; bit >>= 1) j ^= bit; j ^= bit; if (i < j) { let t = re[i]; re[i] = re[j]; re[j] = t; t = im[i]; im[i] = im[j]; im[j] = t; } }
        for (let size = 2; size <= n; size <<= 1) for (let k = 0, half = size / 2; k < half; k++) {
          const c = Math.cos(2 * Math.PI * k / size), s = Math.sin(2 * Math.PI * k / size);
          for (let a = k; a < n; a += size) { const b = a + half, tr = re[b] * c - im[b] * s, ti = re[b] * s + im[b] * c; re[b] = re[a] - tr; im[b] = im[a] - ti; re[a] += tr; im[a] += ti; }
        }
        for (let i = 0; i < n; i++) peak = Math.max(peak, Math.abs(re[i]));
      }
      if (peak) this._scale = 1 / peak;
    }
  }
  class OscillatorNode extends AudioScheduledSourceNode {
    constructor(context, options) { super(context); this._type = 'sine'; this.frequency = new AudioParam(context, 440, -context.sampleRate / 2, context.sampleRate / 2); this.detune = new AudioParam(context, 0, -detuneMax, detuneMax); audioOptions(this, options); }
    get type() { return this._type; }
    set type(value) { if (value === 'custom') throw new DOMException('Use setPeriodicWave()', 'InvalidStateError'); if (waveTypes.includes(value)) { this._type = value; this._wave = null; } }
    setPeriodicWave(wave) { this._wave = wave; this._type = 'custom'; }
    _render(rate, length, frames) {
      const out = new Float32Array(length), frequency = this.frequency._run(this._start, rate, frames), detune = this.detune._run(this._start, rate, frames), wave = this._wave;
      // Square and sawtooth waves smooth their steps (polyBLEP), so they don't alias.
      const blep = (t, dt) => t < dt ? (t /= dt, t + t - t * t - 1) : t > 1 - dt ? (t = (t - 1) / dt, t * t + t + t + 1) : 0;
      let phase = 0;
      for (let i = 0; i < frames; i++) {
        const dt = Math.min(0.5, Math.abs(frequency[i] * 2 ** (detune[i] / 1200) / rate)), t = phase;
        let value;
        switch (wave ? 'custom' : this._type) {
          case 'sine': value = Math.sin(2 * Math.PI * t); break;
          case 'square': value = (t < 0.5 ? 1 : -1) + blep(t, dt) - blep((t + 0.5) % 1, dt); break;
          case 'sawtooth': value = 2 * t - 1 - blep(t, dt); break;
          case 'triangle': value = 1 - 4 * Math.abs(t - 0.5); break;
          default: value = 0; for (let k = 1; k < Math.min(wave._real.length, 1 / (2 * dt)); k++) value += (wave._real[k] ?? 0) * Math.cos(2 * Math.PI * k * t) + (wave._imag[k] ?? 0) * Math.sin(2 * Math.PI * k * t); value *= wave._scale;
        }
        out[i] = value; phase = (phase + Math.sign(frequency[i]) * dt + 1) % 1;
      }
      return [out];
    }
  }
  class AudioBufferSourceNode extends AudioScheduledSourceNode {
    buffer = null; loop = false; loopStart = 0; loopEnd = 0;
    constructor(context, options) { super(context); this.playbackRate = new AudioParam(context, 1); this.detune = new AudioParam(context, 0); audioOptions(this, options); }
    start(when = 0, offset = 0, duration) {
      this._offset = Math.max(0, +offset || 0);
      super.start(when);
      if (duration !== undefined) this._stop = this._start + Math.max(0, +duration);
    }
    _render(rate, length, frames) {
      const buffer = this.buffer, speed = this.playbackRate._run(this._start, rate, frames), detune = this.detune._run(this._start, rate, frames), step = buffer.sampleRate / rate;
      const [low, high] = this.loop && this.loopEnd > this.loopStart ? [this.loopStart * buffer.sampleRate, Math.min(this.loopEnd * buffer.sampleRate, buffer.length)] : [0, buffer.length];
      const input = buffer._data.slice(0, 2), out = input.map(() => new Float32Array(length));
      let at = this._offset * buffer.sampleRate;
      for (let i = 0; i < frames; i++) {
        if (this.loop && at >= high) at = low + (at - low) % Math.max(1, high - low);
        if (at >= buffer.length || at < 0) break;
        const index = at | 0, t = at - index;
        for (let c = 0; c < input.length; c++) { const data = input[c]; out[c][i] = data[index] + ((data[index + 1] ?? (this.loop ? data[low | 0] : 0)) - data[index]) * t; }
        at += step * speed[i] * 2 ** (detune[i] / 1200);
      }
      return out;
    }
    // The playing time of the source, or undefined when it plays until stopped.
    get _length() { const buffer = this.buffer; return !buffer || this.loop ? undefined : (buffer.duration - this._offset) / Math.max(1e-3, this.playbackRate.value); }
  }
  // A 32-bit float WAV file of interleaved channels.
  const wav = (channels, rate) => {
    const frames = channels[0].length, bytes = new DataView(new ArrayBuffer(44 + frames * channels.length * 4)), text = (at, value) => [...value].forEach((char, i) => bytes.setUint8(at + i, char.charCodeAt(0)));
    text(0, 'RIFF'); bytes.setUint32(4, 36 + frames * channels.length * 4, true); text(8, 'WAVEfmt '); bytes.setUint32(16, 16, true);
    bytes.setUint16(20, 3, true); bytes.setUint16(22, channels.length, true); bytes.setUint32(24, rate, true); bytes.setUint32(28, rate * channels.length * 4, true);
    bytes.setUint16(32, channels.length * 4, true); bytes.setUint16(34, 32, true); text(36, 'data'); bytes.setUint32(40, frames * channels.length * 4, true);
    // Every platform is little-endian, as WAV is.
    const samples = new Float32Array(bytes.buffer, 44), count = channels.length;
    for (let c = 0; c < count; c++) { const data = channels[c]; for (let i = 0, at = c; i < frames; i++, at += count) samples[at] = data[i]; }
    return bytes.buffer;
  };
  class AudioContext extends EventTarget {
    state = 'running'; sampleRate = 48000; baseLatency = 0.01; outputLatency = 0.02; onstatechange = null; _start = performance.now(); _pending = [];
    constructor(options) { super(); if (options?.sampleRate) this.sampleRate = +options.sampleRate; this.destination = new AudioDestinationNode(this); this.listener = {}; }
    get currentTime() { return (performance.now() - this._start) / 1000; }
    _set(state) { if (this.state === 'closed') return Promise.reject(new DOMException('The context is closed', 'InvalidStateError')); this.state = state; const event = new Event('statechange'); this.dispatchEvent(event); return Promise.resolve(); }
    resume() { return this.state === 'running' ? Promise.resolve() : this._set('running'); } suspend() { return this._set('suspended'); } close() { return this._set('closed'); }
    getOutputTimestamp() { return { contextTime: this.currentTime, performanceTime: performance.now() }; }
    createGain() { return new GainNode(this); } createBufferSource() { return new AudioBufferSourceNode(this); }
    createOscillator() { return new OscillatorNode(this); } createBiquadFilter() { return new BiquadFilterNode(this); }
    createBuffer(numberOfChannels, length, sampleRate) { return new AudioBuffer({ numberOfChannels, length, sampleRate }); }
    createPeriodicWave(real, imag, options) {
      if (arguments.length < 2) throw new TypeError(`Failed to execute 'createPeriodicWave' on 'BaseAudioContext': 2 arguments required, but only ${arguments.length} present.`);
      if (typeof real?.[Symbol.iterator] !== 'function' || typeof imag?.[Symbol.iterator] !== 'function') throw new TypeError("Failed to execute 'createPeriodicWave' on 'BaseAudioContext': The provided value cannot be converted to a sequence."); return new PeriodicWave(this, { real, imag, disableNormalization: options?.disableNormalization }); }
    decodeAudioData(data, done, failed) {
      const url = URL.createObjectURL(new Blob([data])), audio = new Audio(url);
      const result = new Promise((resolve, reject) => { audio.addEventListener('loadedmetadata', () => { audio._close(); resolve(new AudioBuffer(url, audio.duration)); }); audio.addEventListener('error', () => reject(new DOMException('The audio cannot be decoded', 'EncodingError'))); audio.load(); });
      if (done || failed) result.then(done, failed);
      return result;
    }
    _schedule(source) {
      if (source.buffer?._url) return this._playFile(source);
      if (source instanceof AudioBufferSourceNode && !source.buffer) return;
      if (!this._pending.length) queueMicrotask(() => this._render());
      this._pending.push(source);
    }
    _playFile(source) {
      let volume = 1, node = [...source._outputs][0];
      while (node && !(node instanceof AudioDestinationNode)) { volume *= node.gain?.value ?? 1; node = [...node._outputs][0]; }
      const audio = source._audio = new Audio(source.buffer._url);
      audio.volume = node ? Math.min(1, Math.max(0, volume)) : 0; audio.loop = source.loop; audio.playbackRate = source.playbackRate.value; audio.currentTime = source._offset;
      audio.addEventListener('ended', () => { audio._close(); source._ended(); });
      setTimeout(() => source._audio === audio && audio.play().catch(() => {}), Math.max(0, source._start - this.currentTime) * 1000);
    }
    // Renders the sources that started in this task into one sound.
    _render() {
      const sources = this._pending, rate = this.sampleRate; this._pending = [];
      const begin = Math.min(...sources.map(source => source._start));
      const ends = sources.map(source => source._start + Math.min(audioCap, (source._stop ?? Infinity) - source._start, source._length ?? Infinity));
      const [mix, heard] = this._mix(sources, ends, begin, Math.ceil((Math.max(...ends) + audioTail - begin) * rate), 2);
      sources.forEach((source, i) => setTimeout(() => source._ended(), Math.max(0, ends[i] - this.currentTime) * 1000));
      if (!heard || this.state === 'closed') return;
      const url = URL.createObjectURL(new Blob([wav(mix, rate)], { type: 'audio/wav' })), audio = new Audio(url);
      const done = () => { audio._close(); URL.revokeObjectURL(url); };
      audio.addEventListener('ended', done);
      setTimeout(() => audio.play().catch(done), Math.max(0, begin - this.currentTime) * 1000);
    }
    // Adds each source along its paths to the destination, into channels of `length` frames from `begin`.
    _mix(sources, ends, begin, length, count) {
      const rate = this.sampleRate, mix = Array.from({ length: count }, () => new Float32Array(length));
      let heard = false;
      const walk = (node, channels, start, depth = 0) => {
        if (depth > 64) return;
        if (node instanceof AudioDestinationNode) {
          heard = true; const at = Math.round((start - begin) * rate);
          mix.forEach((data, c) => { const input = channels[Math.min(c, channels.length - 1)]; for (let i = Math.max(0, -at); i < input.length && at + i < data.length; i++) data[at + i] += input[i]; });
          return;
        }
        const outputs = [...node._outputs];
        outputs.forEach((next, i) => walk(next, next._process(i < outputs.length - 1 ? channels.map(data => data.slice()) : channels, start, rate), start, depth + 1));
      };
      sources.forEach((source, i) => {
        const frames = Math.max(0, Math.round((ends[i] - source._start) * rate));
        walk(source, source._render(rate, frames + Math.round(audioTail * rate), frames), source._start);
      });
      return [mix, heard];
    }
  }
  class OfflineAudioContext extends AudioContext {
    oncomplete = null;
    constructor(channels, length, sampleRate) {
      const options = typeof channels === 'object' ? channels : { numberOfChannels: channels, length, sampleRate };
      super(options); this.length = options.length >>> 0; this._channels = options.numberOfChannels ?? 1; this.state = 'suspended';
    }
    get currentTime() { return 0; }
    _schedule(source) { if (source.buffer?._url) throw new DOMException('Decoded files cannot render offline', 'NotSupportedError'); if (source instanceof OscillatorNode || source.buffer) this._pending.push(source); }
    startRendering() {
      const sources = this._pending, end = this.length / this.sampleRate; this._pending = [];
      const ends = sources.map(source => Math.min(end, source._stop ?? end, source._start + (source._length ?? end)));
      const buffer = new AudioBuffer({ numberOfChannels: this._channels, length: this.length, sampleRate: this.sampleRate });
      buffer._data = this._mix(sources, ends, 0, this.length, this._channels)[0];
      this.state = 'closed';
      const event = Object.assign(new Event('complete'), { renderedBuffer: buffer });
      setTimeout(() => { sources.forEach(source => source._ended()); this.dispatchEvent(event); });
      return Promise.resolve(buffer);
    }
  }
  class ImageData {
    constructor(data,width,height) { if (typeof data==='number') {height=width;width=data;data=new Uint8ClampedArray(width*height*4);} else {if (!(data instanceof Uint8ClampedArray)) throw new TypeError('Expected Uint8ClampedArray');height??=data.length/(width*4);} if(!Number.isInteger(width)||!Number.isInteger(height)||width<=0||height<=0||data.length!==width*height*4) throw new DOMException('Invalid image dimensions','IndexSizeError');Object.assign(this,{data,width,height,colorSpace:'srgb'}); }
  }
  // Match render's dimension(): leading HTML non-negative integers after whitespace and an optional +.
  const canvasSize = (value, fallback) => { const digits = /^[\t\n\f\r ]*\+?(\d+)/.exec(value ?? '')?.[1], size = digits ? Number(digits) : NaN; return size <= 2147483647 ? size : fallback; };
  let webglWarned = false;
  class HTMLCanvasElement extends HTMLElement {
    get clientWidth(){const width=this.getBoundingClientRect().width;if(width)return width/zoomOf(this);const value=getComputedStyle(this).width;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.width;}
    get clientHeight(){const height=this.getBoundingClientRect().height;if(height)return height/zoomOf(this);const value=getComputedStyle(this).height;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.height;}
    get width() { return canvasSize(this.getAttribute('width'), 300); } set width(v) { if (this._offscreen) throw transferred(); this.setAttribute('width', Number(v) >>> 0); }
    get height() { return canvasSize(this.getAttribute('height'), 150); } set height(v) { if (this._offscreen) throw transferred(); this.setAttribute('height', Number(v) >>> 0); }
    setAttribute(name, value) { super.setAttribute(name, value); if (name === 'width' || name === 'height') this._context?._resize(); }
    toDataURL(type, quality) { this._context?._flush?.(); return call('dom.canvasURL', this._id, false, String(type).toLowerCase(), quality); }
    toBlob(callback, type, quality) { if (typeof callback !== 'function') throw new TypeError('Expected a callback'); this._context?._flush?.(); const [bytes, mime] = call('dom.canvasURL', this._id, true, String(type).toLowerCase(), quality); setTimeout(() => callback(new Blob([bytes], { type: mime }))); }
    getContext(type, options) { if (this._offscreen) throw transferred(); if (['webgl','webgl2','experimental-webgl','experimental-webgl2'].includes(type)) { if (!webglWarned) { webglWarned = true; console.warn('WebGL is not supported: getContext() returns null. Use WebGPU or Canvas 2D'); } return null; } return canvasContext(this, type, options, CanvasRenderingContext2D); }
    transferControlToOffscreen() { if (this._contextType || this._offscreen) throw new DOMException('The canvas has a context or was already transferred', 'InvalidStateError'); this._offscreen = true; const offscreen = new OffscreenCanvas(0, 0); offscreen._node = this; return offscreen; }
    get content() { return this.getAttribute('content')?.toLowerCase() === 'drawable' ? 'drawable' : 'fallback'; } set content(value) { this.setAttribute('content', value); }
    requestPaint() { paintRequests.add(this); host('dom.requestPaint'); }
    captureElementImage(element) { const node = canvasNode(0, 0), size = snapshotOf(this, element, node._id); node.setAttribute('width', size[2]); node.setAttribute('height', size[3]); return Object.assign(Object.create(ElementImage.prototype), { _node: node, _size: size, _element: element }); }
    updateElementGeometry(element, options = {}) { placeElement(this, drawnElement(this, element), options); }
    clearElementGeometry(element) { call('dom.drawn', drawnElement(this, element)._id, 'clear'); }
    getElementTransform(element) { return new DOMMatrix(call('dom.drawn', drawnElement(this, element)._id, 'get') ?? undefined); }
    getElementImageDefaultSize(element) { const size = snapshotOf(this, element), [kx, ky] = gridScale(this, size); return { width: size[0] * kx, height: size[1] * ky }; }
  }
  // html-in-canvas: a canvas with `content="drawable"` lays out its children, and the page draws those with a
  // `drawable` attribute. The native side keeps a snapshot of each one, and SNAPSHOT plus its node draws it.
  const SNAPSHOT = 2 ** 32, paintRequests = new Set(), geometryUpdates = new Map();
  const canvasOf = element => { const canvas = element.hasAttribute('drawable') ? element.parentElement?.closest('canvas') : null; return canvas?.content === 'drawable' ? canvas : null; };
  const drawnElement = (canvas, element) => {
    const target = element instanceof ElementImage ? element._element : element;
    if (!(target instanceof Element)) throw new TypeError('Expected an element or an ElementImage of this page');
    if (canvasOf(target) !== canvas) throw new DOMException('The element is not a drawable element of this canvas', 'InvalidStateError');
    return target;
  };
  // The size of the snapshot of an element in CSS pixels, then in its own pixels.
  const snapshotOf = (canvas, element, copy) => {
    if (element instanceof ElementImage) { if (!element._node) throw new DOMException('The ElementImage is closed', 'InvalidStateError'); return element._size; }
    const size = call('dom.snapshot', drawnElement(canvas, element)._id, copy);
    if (!size) throw new DOMException('The element has no snapshot yet. Draw it in a paint event', 'InvalidStateError');
    return size;
  };
  // Canvas pixels per CSS pixel. A canvas that the page doesn't show draws a snapshot pixel for pixel.
  const gridScale = (canvas, [width, height, pixelWidth, pixelHeight]) => {
    if (canvas instanceof HTMLCanvasElement && canvas.isConnected) { const [, , clientWidth, clientHeight, top, right, bottom, left] = client(canvas), w = clientWidth - left - right, h = clientHeight - top - bottom; if (w > 0 && h > 0) return [canvas.width / w, canvas.height / h]; }
    return [pixelWidth / width, pixelHeight / height];
  };
  // An OffscreenCanvas places its elements together in a microtask, then tells its canvas element.
  const placeElement = (canvas, element, { canvasTransform, canvasClip, preserveHitTestOrder } = {}) => {
    const place = () => call('dom.drawn', element._id, 'set', canvasTransform == null ? null : DOMMatrix.fromMatrix(canvasTransform)._m, canvasClip == null ? null : [canvasClip.x ?? 0, canvasClip.y ?? 0, canvasClip.width ?? 0, canvasClip.height ?? 0], !!preserveHitTestOrder);
    if (canvas instanceof HTMLCanvasElement) return place();
    const owner = canvas._node;
    if (!(owner instanceof HTMLCanvasElement)) return;
    if (!geometryUpdates.has(owner)) microtask(() => { const updates = geometryUpdates.get(owner); geometryUpdates.delete(owner); for (const update of updates.values()) update(); owner.dispatchEvent(new ElementGeometryUpdateEvent('elementgeometryupdate', { elements: [...updates.keys()].filter(element => element.isConnected) })); });
    (geometryUpdates.get(owner) ?? geometryUpdates.set(owner, new Map()).get(owner)).set(element, place);
  };
  class ElementImage {
    constructor() { throw new TypeError('Illegal constructor'); }
    close() { if (!this._node) return; for (const context of bitmapDraws) context._flush(); call('dom.canvas', this._node._id, null); this._node = null; }
    _detach() { this.close(); }
    _wire() { if (!this._node) throw new DOMException('The ElementImage is closed', 'DataCloneError'); return [this._size, call('dom.canvasPixels', this._node._id, 0, 0, this._size[2], this._size[3], true)]; }
    static _revive(size, bytes) { const node = canvasNode(size[2], size[3]); call('dom.canvasAdopt', node._id, bytes); return Object.assign(Object.create(ElementImage.prototype), { _node: node, _size: size }); }
  }
  class CanvasPaintEvent extends Event { constructor(type, options = {}) { super(type, options); this.changedElements = Object.freeze([...options.changedElements ?? []]); } }
  class ElementGeometryUpdateEvent extends Event { constructor(type, options = {}) { super(type, options); this.elements = Object.freeze([...options.elements ?? []]); } }
  // Runs after a frame with the drawable elements that draw otherwise than before. Canvases inside others paint first.
  globalThis.__appsPaint = ids => {
    const changed = new Map();
    for (const canvas of paintRequests) changed.set(canvas, []);
    paintRequests.clear();
    for (const id of ids) { const element = wrap(id), canvas = element?.isConnected ? canvasOf(element) : null; if (canvas) (changed.get(canvas) ?? changed.set(canvas, []).get(canvas)).push(element); }
    for (const [canvas, changedElements] of [...changed].sort(([a], [b]) => a.compareDocumentPosition(b) & 4 ? 1 : -1)) canvas.dispatchEvent(new CanvasPaintEvent('paint', { changedElements }));
  };
  globalThis.__appsElementImage = source => {
    if (source instanceof ElementImage) return source;
    const canvas = source instanceof Element ? canvasOf(source) : null;
    if (!canvas) throw new DOMException('The source is not a drawable element of a canvas', 'InvalidStateError');
    return canvas.captureElementImage(source);
  };
  const transferred = () => new DOMException('The canvas was transferred', 'InvalidStateError');
  const canvasContext = (canvas, type, options, Context2D) => { if (canvas._contextType && canvas._contextType !== type) return null; if (!['webgpu','2d','bitmaprenderer'].includes(type)) return null; canvas._contextType = type; return canvas._context ??= type === '2d' ? new Context2D(canvas, options) : type === 'bitmaprenderer' ? new ImageBitmapRenderingContext(canvas) : new GPUCanvasContext(canvas, canvas._id); };
  // Flush queued draws before bitmap closure. A flush per draw redraws the canvas for every sprite.
  const bitmapDraws = new Set();
  const drawable = image => image instanceof HTMLImageElement || image instanceof HTMLCanvasElement || image instanceof HTMLVideoElement || image instanceof OffscreenCanvas || image instanceof ImageBitmap;
  const imageSize = image => [image.videoWidth ?? image.naturalWidth ?? image.width, image.videoHeight ?? image.naturalHeight ?? image.height];
  const canvasNode = (width, height) => { const node = document.createElement('apps-canvas'); node.setAttribute('width', width); node.setAttribute('height', height); return node; };
  const bitmapOf = node => Object.assign(Object.create(ImageBitmap.prototype), { _node: node });
  class ImageBitmap {
    constructor() { throw new TypeError('Illegal constructor'); }
    get width() { return this._node ? canvasSize(this._node.getAttribute('width'), 0) : 0; }
    get height() { return this._node ? canvasSize(this._node.getAttribute('height'), 0) : 0; }
    get _id() { return this._node?._id; }
    close() { if (!this._node) return; for (const context of bitmapDraws) context._flush(); call('dom.canvas', this._id, null); this._node = null; }
    _detach() { this.close(); }
    _wire() { if (!this._node) throw new DOMException('The ImageBitmap is closed', 'DataCloneError'); const { width, height } = this; return [width, height, call('dom.canvasPixels', this._id, 0, 0, width, height, true)]; }
    static _revive(width, height, bytes) { const node = canvasNode(width, height); call('dom.canvasAdopt', node._id, bytes); return bitmapOf(node); }
  }
  class ImageBitmapRenderingContext {
    constructor(canvas) { this.canvas = canvas; }
    transferFromImageBitmap(bitmap) {
      if (bitmap !== null && !(bitmap instanceof ImageBitmap)) throw new TypeError('Expected an ImageBitmap');
      if (bitmap && !bitmap.width) throw new DOMException('The ImageBitmap is closed', 'InvalidStateError');
      if (bitmap) { for (const context of bitmapDraws) context._flush(); call('dom.canvasTake', bitmap._id, this.canvas._id); bitmap.close(); }
      else call('dom.canvas', this.canvas._id, null);
      this.canvas._commit?.();
    }
    _resize() { call('dom.canvas', this.canvas._id, null); }
  }
  let placeholders = false;
  class OffscreenCanvas extends EventTarget {
    constructor(width, height) { super(); if (arguments.length < 2) throw new TypeError('OffscreenCanvas needs a width and a height'); this._node = canvasNode(0, 0); this.width = width; this.height = height; }
    get _id() { return this._node?._id; }
    get width() { return this._node ? canvasSize(this._node.getAttribute('width'), 300) : 0; }
    get height() { return this._node ? canvasSize(this._node.getAttribute('height'), 150) : 0; }
    set width(value) { this._node?.setAttribute('width', offscreenSize(value)); this._context?._resize(); }
    set height(value) { this._node?.setAttribute('height', offscreenSize(value)); this._context?._resize(); }
    getContext(type, options) { if (!this._node) throw transferred(); return canvasContext(this, type, options, OffscreenCanvasRenderingContext2D); }
    updateElementGeometry(element, options = {}) { const target = element instanceof ElementImage ? element._element : element; if (target instanceof Element) placeElement(this, drawnElement(this._node, target), options); }
    clearElementGeometry(element) { const target = element instanceof ElementImage ? element._element : element; if (target instanceof Element) this._node.clearElementGeometry(target); }
    getElementImageDefaultSize(element) { const size = snapshotOf(this._node, element), [kx, ky] = gridScale(this._node, size); return { width: size[0] * kx, height: size[1] * ky }; }
    transferToImageBitmap() {
      const { width, height } = this;
      if (!this._context || !width || !height) throw new DOMException('The canvas has no context or no pixels', 'InvalidStateError');
      this._context._flush?.(); this._context._release?.();
      const node = canvasNode(width, height); call('dom.canvasTake', this._id, node._id);
      return bitmapOf(node);
    }
    async convertToBlob(options) {
      if (!this._node) throw transferred();
      if (!this.width || !this.height) throw new DOMException('The canvas has no pixels', 'IndexSizeError');
      this._context?._flush?.();
      const [bytes, mime] = call('dom.canvasURL', this._id, true, String(options?.type).toLowerCase(), options?.quality); return new Blob([bytes], { type: mime });
    }
    _commit() { if (!this._linked || this._committing) return; this._committing = true; setTimeout(() => { this._committing = false; if (this._node) call('dom.canvasCommit', this._id); }, 0); }
    _pump() {
      if (!this._linked) return;
      this._frameAt = performance.now();
      if (this._pumping) return;
      this._pumping = true;
      const pump = () => { if (this._node) call('dom.canvasCommit', this._id); if (this._node && performance.now() - this._frameAt < 100) setTimeout(pump, 4); else this._pumping = false; };
      setTimeout(pump, 4);
    }
    _detach() { this._node = null; }
    _wire() { if (!this._node || this._context) throw new DOMException('A canvas with a context or that was transferred cannot be transferred', 'InvalidStateError'); const linked = this._linked || this._node.localName === 'canvas'; placeholders ||= linked; return [linked ? call('dom.canvasLink', this._id) : null, this.width, this.height]; }
    static _revive(token, width, height) { const canvas = new OffscreenCanvas(width, height); if (token !== null) { call('dom.canvasBind', canvas._id, token); canvas._linked = true; } return canvas; }
  }
  const offscreenSize = value => { value = Number(value); if (!(value >= 0 && value <= Number.MAX_SAFE_INTEGER)) throw new TypeError('Invalid canvas size'); return Math.trunc(value); };
  class OffscreenCanvasRenderingContext2D extends CanvasRenderingContext2D {}
  globalThis.createImageBitmap = async (source, ...rest) => {
    const cropped = rest.length > 1;
    if (cropped && rest.length < 4) throw new TypeError('createImageBitmap needs a whole crop rectangle');
    const { imageOrientation, resizeWidth, resizeHeight, resizeQuality } = (cropped ? rest[4] : rest[0]) ?? {};
    let [sx = 0, sy = 0, sw, sh] = cropped ? rest.slice(0, 4).map(Math.trunc) : [];
    if (sw === 0 || sh === 0) throw new RangeError('The crop rectangle is empty');
    if (resizeWidth === 0 || resizeHeight === 0) throw new DOMException('The resized image is empty', 'InvalidStateError');
    let decoded;
    if (source instanceof Blob) {
      const image = document.createElement('img'), size = await decode(image, new Uint8Array(await source.arrayBuffer()));
      if (typeof size === 'string') throw new DOMException('The source image could not be decoded', 'InvalidStateError');
      image._dimensions = size; source = image; decoded = image;
    } else if (source instanceof ImageData) {
      const canvas = new OffscreenCanvas(source.width, source.height); canvas.getContext('2d').putImageData(source, 0, 0); source = canvas;
    } else if (!drawable(source)) throw new TypeError('Expected an image, canvas, video, bitmap, blob or ImageData');
    else if (source instanceof HTMLVideoElement && source.readyState < 2) throw new DOMException('The video has no frame', 'InvalidStateError');
    const [iw, ih] = imageSize(source);
    if (!iw || !ih) throw new DOMException('The image has no pixels', 'InvalidStateError');
    sw ??= iw; sh ??= ih;
    if (sw < 0) { sx += sw; sw = -sw; } if (sh < 0) { sy += sh; sh = -sh; }
    const width = resizeWidth ?? (resizeHeight ? Math.ceil(sw * resizeHeight / sh) : sw), height = resizeHeight ?? (resizeWidth ? Math.ceil(sh * resizeWidth / sw) : sh);
    const canvas = new OffscreenCanvas(width, height), context = canvas.getContext('2d');
    context.imageSmoothingEnabled = resizeQuality !== 'pixelated' && (width !== sw || height !== sh);
    if (imageOrientation === 'flipY') { context.translate(0, height); context.scale(1, -1); }
    context.drawImage(source, sx, sy, sw, sh, 0, 0, width, height); context._flush();
    if (decoded) call('dom.canvas', decoded._id, null);
    return bitmapOf(canvas._node);
  };
  class ScreenOrientation extends EventTarget {
    get angle() { return 0; }
    get type() { const [width, height] = host('dom.screen'); return width < height ? 'portrait-primary' : 'landscape-primary'; }
    lock() { return Promise.reject(new DOMException('The screen orientation cannot be locked', 'NotSupportedError')); }
    unlock() {}
  }
  class Screen extends EventTarget {
    orientation = new ScreenOrientation();
    get colorDepth() { return 24; } get pixelDepth() { return 24; }
  }
  for (const [index, name] of ['width', 'height', 'availLeft', 'availTop', 'availWidth', 'availHeight'].entries()) Object.defineProperty(Screen.prototype, name, { configurable: true, enumerable: true, get: () => host('dom.screen')[index] });
  globalThis.screen = new Screen();
  for (const [index, names] of [[6, 'screenX screenLeft'], [7, 'screenY screenTop'], [8, 'outerWidth'], [9, 'outerHeight']]) for (const name of names.split(' ')) Object.defineProperty(globalThis, name, { configurable: true, enumerable: true, get: () => host('dom.screen')[index] });
  const htmlTypes = new Map([['canvas', HTMLCanvasElement], ['img', HTMLImageElement], ['video', HTMLVideoElement], ['audio', HTMLAudioElement], ['slot', HTMLSlotElement], ['input', HTMLInputElement], ['textarea', HTMLTextAreaElement], ['template', HTMLTemplateElement], ['select', HTMLSelectElement], ['option', HTMLOptionElement], ['optgroup', HTMLOptGroupElement], ['button', HTMLButtonElement], ['fieldset', HTMLFieldSetElement], ['output', HTMLOutputElement], ['form', HTMLFormElement], ['dialog', HTMLDialogElement], ['details', HTMLDetailsElement], ['label', HTMLLabelElement]]);
  for (const [name, tags] of Object.entries({ Anchor: 'a', Area: 'area', Base: 'base', Body: 'body', BR: 'br', Data: 'data', DataList: 'datalist', Div: 'div', DList: 'dl', Embed: 'embed', Head: 'head', Heading: 'h1 h2 h3 h4 h5 h6', HR: 'hr', Html: 'html', IFrame: 'iframe', Legend: 'legend', LI: 'li', Link: 'link', Menu: 'menu', Meta: 'meta', Meter: 'meter', Mod: 'ins del', Object: 'object', OList: 'ol', Paragraph: 'p', Picture: 'picture', Pre: 'pre', Progress: 'progress', Quote: 'blockquote q', Script: 'script', Source: 'source', Span: 'span', Style: 'style', Table: 'table', TableCaption: 'caption', TableCell: 'td th', TableCol: 'col colgroup', TableRow: 'tr', TableSection: 'tbody tfoot thead', Time: 'time', Title: 'title', Track: 'track', UList: 'ul', Unknown: '' })) {
    const key = 'HTML' + name + 'Element', Type = globalThis[key] = { [key]: class extends HTMLElement {} }[key];
    for (const tag of tags.split(' ')) if (tag) htmlTypes.set(tag, Type);
  }
  for (const [tags, name] of [['a area link base', 'href'], ['img script iframe embed source track audio video input', 'src'], ['object', 'data'], ['blockquote q ins del', 'cite']]) for (const tag of tags.split(' ')) Object.defineProperty(htmlTypes.get(tag).prototype, name, { configurable: true, get() { const value = this.getAttribute(name); return value === null ? '' : resolved(value); }, set(value) { this.setAttribute(name, value); } });
  for (const key of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) for (const tag of ['a', 'area']) Object.defineProperty(htmlTypes.get(tag).prototype, key, { configurable: true, get() { try { return new URL(this.href)[key]; } catch { return key === 'protocol' ? ':' : ''; } }, set(value) { try { const url = new URL(this.href); url[key] = value; this.href = url.href; } catch {} } });
  for (const tag of ['a', 'area']) Object.defineProperty(htmlTypes.get(tag).prototype, 'toString', { configurable: true, writable: true, value() { return this.href; } });
  for (const [tags, names] of [['a area link', 'rel'], ['a area', 'target download'], ['a link script', 'type'], ['link', 'media hreflang'], ['meta', 'name content']]) for (const tag of tags.split(' ')) for (const name of names.split(' ')) Object.defineProperty(htmlTypes.get(tag).prototype, name, { configurable: true, get() { return this.getAttribute(name) ?? ''; }, set(value) { this.setAttribute(name, value); } });
  for (const tag of ['a', 'area', 'link']) Object.defineProperty(htmlTypes.get(tag).prototype, 'relList', { configurable: true, get() { return this._relList ??= new DOMTokenList(this, 'rel'); } });
  {
    // Reflected IDL attributes: `prop` or `prop=default` per kind; the attribute is the lowercase name.
    const define = (tags, names, make) => { for (const tag of tags ? tags.split(' ') : ['']) for (const entry of names.split(' ')) { const [name, fallback] = entry.split('='), attr = name.toLowerCase(); Object.defineProperty(tag ? htmlTypes.get(tag).prototype : HTMLElement.prototype, name, { configurable: true, enumerable: true, ...make(attr, fallback) }); } };
    const string = (attr, fallback = '') => ({ get() { return this.getAttribute(attr) ?? fallback; }, set(value) { this.setAttribute(attr, value); } });
    // `least` is the smallest valid value: a smaller one reads as the default, and setting one throws.
    const long = least => (attr, fallback) => ({ get() { const value = parseInt(this.getAttribute(attr), 10); return value >= least ? value : +fallback; }, set(value) { value = Math.trunc(Number(value)) || 0; if (value < least) throw new DOMException(`The value is less than ${least}`, 'IndexSizeError'); this.setAttribute(attr, value); } });
    const double = (attr, fallback) => ({ get() { const value = parseFloat(this.getAttribute(attr)); return Number.isFinite(value) ? value : +fallback; }, set(value) { this.setAttribute(attr, Number(value)); } });
    const boolean = attr => ({ get() { return this.hasAttribute(attr); }, set(value) { this.toggleAttribute(attr, !!value); } });
    const keyword = values => (attr, fallback = '') => ({ get() { const value = this.getAttribute(attr)?.toLowerCase(); return values.includes(value) ? value : fallback; }, set(value) { this.setAttribute(attr, value); } });
    define('', 'accessKey autocapitalize', string);
    define('', 'enterKeyHint', keyword(['enter', 'done', 'go', 'next', 'previous', 'search', 'send']));
    define('', 'inputMode', keyword(['none', 'text', 'tel', 'url', 'email', 'numeric', 'decimal', 'search']));
    define('', 'dir', keyword(['ltr', 'rtl', 'auto']));
    define('a area', 'hreflang ping', string); define('a title script', 'text', () => ({ get() { return this.textContent; }, set(value) { this.textContent = value; } }));
    define('a area img iframe link script', 'referrerPolicy', keyword(['no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url']));
    define('img link script', 'fetchPriority=auto', keyword(['high', 'low']));
    define('button input', 'formMethod', keyword(['get', 'post', 'dialog'])); define('button input', 'formEnctype', keyword(['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain']));
    define('track', 'kind=subtitles', keyword(['subtitles', 'captions', 'descriptions', 'chapters', 'metadata'])); define('area', 'shape', string);
    define('img', 'alt sizes srcset useMap', string); define('img', 'isMap', boolean);
    define('img', 'loading=auto', keyword(['lazy', 'eager'])); define('img', 'decoding=auto', keyword(['sync', 'async']));
    define('img script link video audio', 'crossOrigin', attr => ({ get() { const value = this.getAttribute(attr); return value === null ? null : value.toLowerCase() === 'use-credentials' ? 'use-credentials' : 'anonymous'; }, set(value) { if (value === null) this.removeAttribute(attr); else this.setAttribute(attr, value); } }));
    define('input textarea', 'maxLength=-1 minLength=-1', long(0)); define('input', 'size=20', long(1));
    define('textarea', 'rows=2 cols=20', long(1)); define('textarea', 'placeholder wrap autocomplete', string);
    define('td th', 'colSpan=1', long(1)); define('td th', 'rowSpan=1', long(0)); define('td th', 'headers scope abbr', string); define('col colgroup', 'span=1', long(1));
    define('ol', 'start=1', long(-Infinity)); define('ol', 'reversed', boolean); define('li', 'value=0', long(-Infinity));
    define('progress', 'max=1 value=0', double); define('meter', 'value=0 min=0 max=1', double);
    define('script', 'async defer noModule', boolean); define('script', 'charset integrity', string); define('style', 'media', string);
    define('link', 'integrity sizes', string); define('link', 'as', keyword(['fetch', 'audio', 'audioworklet', 'document', 'embed', 'font', 'image', 'manifest', 'object', 'paintworklet', 'report', 'script', 'serviceworker', 'sharedworker', 'style', 'track', 'video', 'worker', 'xslt', 'json'])); define('iframe', 'name allow srcdoc', string); define('iframe', 'allowFullscreen', boolean);
    // `draggable`, `spellcheck` and `translate` are true/false keywords with defaults that depend on the element.
    Object.defineProperties(HTMLElement.prototype, {
      draggable: { configurable: true, enumerable: true, get() { const value = this.getAttribute('draggable')?.toLowerCase(); return value === 'true' || value !== 'false' && (this.localName === 'img' || this.localName === 'a' && this.hasAttribute('href')); }, set(value) { this.setAttribute('draggable', !!value); } },
      spellcheck: { configurable: true, enumerable: true, get() { for (let node = this; node; node = node.parentElement) { const value = node.getAttribute('spellcheck')?.toLowerCase(); if (value === '' || value === 'true') return true; if (value === 'false') return false; } return true; }, set(value) { this.setAttribute('spellcheck', !!value); } },
      translate: { configurable: true, enumerable: true, get() { for (let node = this; node; node = node.parentElement) { const value = node.getAttribute('translate')?.toLowerCase(); if (value === '' || value === 'yes') return true; if (value === 'no') return false; } return true; }, set(value) { this.setAttribute('translate', value ? 'yes' : 'no'); } },
      hidden: { configurable: true, enumerable: true, get() { const value = this.getAttribute('hidden'); return value === null ? false : value.toLowerCase() === 'until-found' ? 'until-found' : true; }, set(value) { if (!value) this.removeAttribute('hidden'); else this.setAttribute('hidden', typeof value === 'string' && value.toLowerCase() === 'until-found' ? 'until-found' : ''); } },
      nonce: { configurable: true, enumerable: true, get() { return this._nonce ?? this.getAttribute('nonce') ?? ''; }, set(value) { this._nonce = String(value); } },
    });
    define('progress', 'position', () => ({ get() { return this.hasAttribute('value') ? Math.min(Math.max(this.value, 0), this.max) / this.max : -1; } }));
    Object.defineProperty(Document.prototype, 'dir', { configurable: true, get() { return this.documentElement?.dir ?? ''; }, set(value) { if (this.documentElement) this.documentElement.dir = value; } });
  }
  {
    const kids = (el, ...tags) => el._m.kids.filter(kid => tags.includes(kid.tag)).map(of);
    const rows = el => kids(el, 'tr', 'thead', 'tbody', 'tfoot').flatMap(child => child.localName === 'tr' ? [child] : kids(child, 'tr'));
    const insertRow = (section, index, all = kids(section, 'tr')) => { if (index < -1 || index > all.length) throw new DOMException('Row index out of range', 'IndexSizeError'); const row = document.createElement('tr'); return index === -1 || index === all.length ? (all.at(-1)?.parentNode ?? section).appendChild(row) : all[index].before(row) ?? row; };
    const remove = (list, index) => { const item = list.at(index); if (!item || index < -1 || index >= list.length) throw new DOMException('Index out of range', 'IndexSizeError'); item.remove(); };
    const section = (table, tag, before) => kids(table, tag)[0] ?? table.insertBefore(document.createElement(tag), before?.() ?? null);
    Object.defineProperties(HTMLTableElement.prototype, Object.getOwnPropertyDescriptors({
      get caption() { return kids(this, 'caption')[0] ?? null; },
      get tHead() { return kids(this, 'thead')[0] ?? null; },
      get tFoot() { return kids(this, 'tfoot')[0] ?? null; },
      get tBodies() { return live(this, 'tBodies', HTMLCollection, () => kids(this, 'tbody'), { root: this._m, tags: ['tbody'] }); },
      get rows() { return live(this, 'rows', HTMLCollection, () => rows(this), { root: this._m, tags: ['tr', 'thead', 'tbody', 'tfoot'] }); },
      createCaption() { return section(this, 'caption', () => this.firstChild); },
      createTHead() { return section(this, 'thead', () => kids(this, 'tbody', 'tfoot', 'tr')[0]); },
      createTFoot() { return section(this, 'tfoot'); },
      createTBody() { return this.insertBefore(document.createElement('tbody'), kids(this, 'tbody').at(-1)?.nextSibling ?? null); },
      deleteCaption() { this.caption?.remove(); }, deleteTHead() { this.tHead?.remove(); }, deleteTFoot() { this.tFoot?.remove(); },
      insertRow(index = -1) { const all = rows(this); return all.length || kids(this, 'tbody').length ? insertRow(kids(this, 'tbody').at(-1) ?? this, index, all) : this.appendChild(document.createElement('tbody')).appendChild(document.createElement('tr')); },
      deleteRow(index) { remove(rows(this), index); },
    }));
    Object.defineProperties(HTMLTableSectionElement.prototype, Object.getOwnPropertyDescriptors({
      get rows() { return live(this, 'rows', HTMLCollection, () => kids(this, 'tr'), { root: this._m, tags: ['tr'] }); },
      insertRow(index = -1) { return insertRow(this, index); },
      deleteRow(index) { remove(kids(this, 'tr'), index); },
    }));
    Object.defineProperties(HTMLTableRowElement.prototype, Object.getOwnPropertyDescriptors({
      get cells() { return live(this, 'cells', HTMLCollection, () => kids(this, 'td', 'th'), { root: this._m, tags: ['td', 'th'] }); },
      get rowIndex() { const table = this.closest('table'); return table ? rows(table).indexOf(this) : -1; },
      get sectionRowIndex() { return this.parentNode ? kids(this.parentNode, 'tr').indexOf(this) : -1; },
      insertCell(index = -1) { const cells = kids(this, 'td', 'th'); if (index < -1 || index > cells.length) throw new DOMException('Cell index out of range', 'IndexSizeError'); const cell = document.createElement('td'); return index === -1 || index === cells.length ? this.appendChild(cell) : this.insertBefore(cell, cells[index]); },
      deleteCell(index) { remove(kids(this, 'td', 'th'), index); },
    }));
    Object.defineProperty(HTMLTableCellElement.prototype, 'cellIndex', { get() { return this.parentNode ? kids(this.parentNode, 'td', 'th').indexOf(this) : -1; }, configurable: true });
  }
  load(json(call('dom.tree', 0)));
  const document = globalThis.document = wrap(0);
  globalThis.window = globalThis.self = globalThis.top = globalThis.parent = globalThis.frames = globalThis;
  selection = new Selection(); attachDeclarative();
  for (const name of ['addEventListener', 'removeEventListener', 'dispatchEvent']) globalThis[name] = Node.prototype[name].bind(globalThis);
  Object.assign(globalThis, { CSSPositionTryRule, CSSPositionTryDescriptors, ContentVisibilityAutoStateChangeEvent, SnapEvent, ElementImage, CanvasPaintEvent, ElementGeometryUpdateEvent, ImageBitmap, ImageBitmapRenderingContext, OffscreenCanvas, OffscreenCanvasRenderingContext2D, Screen, ScreenOrientation, CanvasGradient, Path2D, CanvasPattern, DOMMatrix, DOMMatrixReadOnly, DOMPoint, DOMPointReadOnly, TextMetrics, AudioContext, OfflineAudioContext, AudioNode, AudioParam, AudioScheduledSourceNode, GainNode, BiquadFilterNode, OscillatorNode, PeriodicWave, AudioBuffer, AudioBufferSourceNode, AudioDestinationNode, Node, CharacterData, Text, Comment, DocumentType, Attr, NamedNodeMap, NodeList, HTMLCollection, DOMTokenList, Element, HTMLElement, SVGElement, SVGSVGElement, SVGAnimatedString, HTMLSlotElement, HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLOptionElement, HTMLOptGroupElement, HTMLOptionsCollection, HTMLButtonElement, HTMLFieldSetElement, HTMLOutputElement, HTMLFormElement, HTMLFormControlsCollection, HTMLDialogElement, HTMLDetailsElement, HTMLLabelElement, ValidityState, DOMRectList, WheelEvent, ToggleEvent, CommandEvent, SubmitEvent, HTMLDocument: Document, HTMLImageElement, HTMLCanvasElement, HTMLMediaElement, HTMLVideoElement, HTMLAudioElement, Audio, MediaError, TimeRanges, HTMLTemplateElement, Document, DocumentFragment, TreeWalker, NodeIterator, XMLSerializer, Range, Selection, DOMRect, DOMRectReadOnly: DOMRect, MutationObserver, ResizeObserver, IntersectionObserver, UIEvent, MouseEvent, PointerEvent, KeyboardEvent, FocusEvent, InputEvent, CompositionEvent, CanvasRenderingContext2D, ImageData, DataTransfer, FileList, ClipboardEvent, DragEvent, StaticRange, customElements, ShadowRoot, CSSStyleSheet, DOMParser, CSS, StyleSheet, StyleSheetList, MediaList, CSSRule, CSSRuleList, CSSGroupingRule, CSSStyleRule, CSSMediaRule, CSSSupportsRule, CSSContainerRule, CSSLayerBlockRule, CSSLayerStatementRule, CSSKeyframesRule, CSSKeyframeRule, CSSFontFaceRule, CSSPropertyRule, CSSImportRule, CSSStyleDeclaration, AnimationEvent, BeforeUnloadEvent, CustomStateSet, Highlight, HighlightRegistry, XMLDocument, CDATASection, ProcessingInstruction, ElementInternals, FormDataEvent, HashChangeEvent, PageTransitionEvent, PopStateEvent, RadioNodeList, StorageEvent, TransitionEvent });
  Object.defineProperty(ShadowRoot.prototype, 'adoptedStyleSheets', adoptedProperty);
  Object.defineProperty(Document.prototype, 'adoptedStyleSheets', adoptedProperty);
  Object.defineProperty(Document.prototype, 'styleSheets', { configurable: true, get() { return styleSheetsOf(this); } });
  const ownSheet = node => { let inert = !node.isConnected; for (let rec = node._m.up; rec && !inert; rec = rec.up) inert = rec.tag === 'template' || rec.tag === 'noscript'; return inert ? node._sheet = null : node._sheet ??= new CSSStyleSheet({}, node); };
  Object.defineProperty(HTMLStyleElement.prototype, 'sheet', { configurable: true, get() { return ownSheet(this); } });
  Object.defineProperty(HTMLLinkElement.prototype, 'sheet', { configurable: true, get() { return /(^|\s)stylesheet(\s|$)/i.test(this.getAttribute('rel') ?? '') && (this._linked ??= call('dom.css', this._id) !== null) ? ownSheet(this) : null; } });
  Object.defineProperty(Node.prototype, 'baseURI', { configurable: true, get() { return document.baseURI; } });
  globalThis.Option = function Option(text = '', value, defaultSelected = false, selected = false) { const option = document.createElement('option'); if (text !== '') option.text = text; if (value !== undefined) option.value = value; option.defaultSelected = defaultSelected; option.selected = selected; return option; };
  const handlers = new WeakMap();
  const handler = (target, name) => Object.defineProperty(target, 'on' + name, { configurable: true, enumerable: true,
    get() { return handlers.get(this)?.[name]?.value ?? null; },
    set(value) {
      let own = handlers.get(this); if (!own) handlers.set(this, own = { __proto__: null });
      value = typeof value === 'function' || value !== null && typeof value === 'object' ? value : null;
      const entry = own[name];
      if (entry && value) { entry.value = value; return; }
      if (entry) { this.removeEventListener(name, entry.listener); delete own[name]; }
      if (!value) return;
      const created = own[name] = { value, listener(event) {
        const fn = created.value; if (typeof fn !== 'function') return;
        if (name === 'error' && this === globalThis && event instanceof ErrorEvent) { if (fn.call(this, event.message, event.filename, event.lineno, event.colno, event.error) === true) event.preventDefault(); }
        else if (fn.call(this, event) === false) event.preventDefault();
      } };
      this.addEventListener(name, created.listener);
    } });
  const globalHandlers = 'abort auxclick beforeinput beforematch beforetoggle blur cancel canplay canplaythrough change click close command contentvisibilityautostatechange contextlost contextmenu contextrestored copy cuechange cut dblclick drag dragend dragenter dragleave dragover dragstart drop durationchange emptied ended error focus formdata input invalid keydown keypress keyup load loadeddata loadedmetadata loadstart mousedown mouseenter mouseleave mousemove mouseout mouseover mouseup paste pause play playing progress ratechange reset resize scroll scrollend scrollsnapchange scrollsnapchanging securitypolicyviolation seeked seeking select selectionchange selectstart slotchange stalled submit suspend timeupdate toggle volumechange waiting wheel animationstart animationiteration animationend animationcancel transitionrun transitionstart transitionend transitioncancel gotpointercapture lostpointercapture pointerdown pointermove pointerrawupdate pointerup pointercancel pointerover pointerout pointerenter pointerleave'.split(' ');
  for (const name of globalHandlers) for (const target of [HTMLElement.prototype, SVGElement.prototype, Document.prototype]) handler(target, name);
  handler(Document.prototype, 'readystatechange'); handler(Document.prototype, 'visibilitychange');
  handler(Screen.prototype, 'change'); handler(ScreenOrientation.prototype, 'change');
  handler(HTMLCanvasElement.prototype, 'paint'); handler(HTMLCanvasElement.prototype, 'elementgeometryupdate');
  for (const name of ['fullscreenchange', 'fullscreenerror']) for (const target of [Element.prototype, Document.prototype]) handler(target, name);
  for (const name of [...globalHandlers, ...'afterprint beforeprint beforeunload hashchange languagechange message messageerror offline online pagehide pageshow popstate rejectionhandled storage unhandledrejection unload'.split(' ')]) if (!Object.hasOwn(globalThis, 'on' + name)) handler(globalThis, name);
  for (const [name, Type] of Object.entries(globalThis)) if (typeof Type === 'function' && Type.prototype && (Type.prototype instanceof Node || Type === Event || Type.prototype instanceof Event || /^(HTML|SVG|DOM|Node|Text|Comment|Attr|Range|Selection|Tree|Mutation|Resize|Intersection|Validity|Static|XML|CSS|ShadowRoot|Custom|Image|Canvas|Font|Media|Data|Clipboard|Drag|Composition|Input|Focus|Keyboard|Mouse|Pointer|Wheel|UI|Toggle|Command|Submit|Offscreen|Screen|ElementImage|FileList|Highlight)/.test(name)) && !Object.hasOwn(Type.prototype, Symbol.toStringTag)) Object.defineProperty(Type.prototype, Symbol.toStringTag, { configurable: true, value: name === 'HTMLDocument' ? 'Document' : name });
  Object.defineProperty(globalThis, Symbol.toStringTag, { configurable: true, value: 'Window' });
  globalThis.__appsScript = node => { currentScript = node ?? null; };
  globalThis.__appsLoaded = () => {
    readyState = 'interactive'; dispatch(document, trusted(new Event('readystatechange')));
    dispatch(document, trusted(new Event('DOMContentLoaded', { bubbles: true })));
    // Like Chrome, the first focusable `autofocus` element takes focus after the parse, unless a script moved the focus.
    setTimeout(() => { if (!active) [...document.querySelectorAll('[autofocus]')].find(focusable)?.focus(); }, 0);
    const images = []; find(document._m, rec => { if (rec.tag === 'img') { const loading = rec.ref?.deref()?._loading; if (loading) images.push(loading.catch(() => {})); } return false; });
    Promise.all(images).then(() => setTimeout(() => { readyState = 'complete'; dispatch(document, trusted(new Event('readystatechange'))); dispatch(globalThis, new Event('load')); dispatch(globalThis, trusted(new PageTransitionEvent('pageshow'))); }, 0));
  };
  globalThis.postMessage ??= (message, options, transfer) => { const data = structuredClone(message, { transfer: Array.isArray(options) ? options : transfer ?? options?.transfer }); setTimeout(() => dispatch(globalThis, trusted(new MessageEvent('message', { data, origin: location.origin, source: globalThis })))); };
  globalThis.requestIdleCallback = fn => setTimeout(() => { const end = performance.now() + 50; fn({ didTimeout: false, timeRemaining: () => Math.max(0, end - performance.now()) }); }, 1);
  globalThis.cancelIdleCallback = id => clearTimeout(id);
  globalThis.Image = function Image(width,height) { const image=document.createElement('img'); if(width!==undefined) image.width=width; if(height!==undefined) image.height=height; return image; };
  class FontFace {
    constructor(family,source,descriptors={}) { Object.assign(this,{family:String(family),style:'normal',weight:'normal',stretch:'normal',unicodeRange:'U+0-10FFFF',status:'unloaded'},descriptors); this._source=source;this.loaded=new Promise((resolve,reject)=>{this._resolve=resolve;this._reject=reject;});this.loaded.catch(()=>{});if(typeof source!=='string')this.load(); }
    load() { if (this.status!=='unloaded') return this.loaded; this.status='loading'; (async()=>{const source=this._source;let error=new DOMException('No supported font source','SyntaxError');for(const [kind,value] of typeof source==='string'?call('dom.fontSources',0,source):[['data',source]]) try {if(kind==='local'){if(call('dom.font',0,this.family,value))return this._done('loaded');throw new DOMException('Local font unavailable','NetworkError');}let bytes=value;if(kind==='url'){const response=await fetch(value);if(!response.ok)throw new DOMException('Font request failed','NetworkError');bytes=await response.arrayBuffer();}call('dom.font',0,this.family,ArrayBuffer.isView(bytes)?new Uint8Array(bytes.buffer,bytes.byteOffset,bytes.byteLength):new Uint8Array(bytes),[this.unicodeRange,this.weight,this.style,this.stretch].map(String));return this._done('loaded');}catch(caught){error=kind==='data'&&!(caught instanceof DOMException)?new DOMException(caught?.message,'SyntaxError'):caught;}this._done('error',error);})();return this.loaded; }
    _done(status,error) { this.status=status; if(status==='loaded')this._resolve(this); else if(status==='error')this._reject(error); }
  }
  class FontFaceSet extends EventTarget {
    constructor() {super();this._faces=new Set();this._css=[];}
    _all() {const [added,css]=call('dom.fonts',0);if(added){stale=true;staleAt=geometry[2];version++;}this._css=css.map(([family,source,range,status,[weight,style,stretch]],i)=>{let face=this._css[i];if(face?.family!==family||face._source!==source||face.status!==status&&face.status!=='loading'){face=new FontFace(family,source,{...range&&{unicodeRange:range},...weight&&{weight},...style&&{style},...stretch&&{stretch}});face._done(status,new DOMException('Font load failed','NetworkError'));}return face;});return [...this._css,...this._faces];}
    add(face) {if(!(face instanceof FontFace))throw new TypeError('Expected FontFace');this._faces.add(face);return this;} delete(face){return this._faces.delete(face);} clear(){this._faces.clear();} has(face){return this._all().includes(face);} get size(){return this._all().length;} values(){return this._all().values();} keys(){return this.values();} entries(){return this._all().map(face=>[face,face]).values();} forEach(fn,thisArg){for(const face of this._all())fn.call(thisArg,face,face,this);} [Symbol.iterator](){return this.values();}
    get status() {return this._all().some(face=>face.status==='loading')?'loading':'loaded';}
    get ready() {return Promise.all(this._all().map(face=>face.status==='loading'&&face.loaded.catch(()=>{}))).then(()=>this);}
    _match(font,text) {const families=String(font).replace(/^.*?\d+(?:\.\d+)?px\s+/,'').split(',').map(v=>v.trim().replace(/^['"]|['"]$/g,'')),codes=[...String(text)].map(c=>c.codePointAt(0)),faces=this._all().filter(face=>families.includes(face.family));return [families,faces,faces.filter(face=>String(face.unicodeRange).split(',').some(range=>{const [low,high=low]=range.trim().slice(2).split('-');return codes.some(code=>code>=parseInt(low.replace(/\?/g,'0'),16)&&code<=parseInt(high.replace(/\?/g,'f'),16));}))];}
    load(font,text=' ') {const [,,faces]=this._match(font,text);this.dispatchEvent(new Event('loading'));const result=Promise.all(faces.map(face=>face.load()));result.then(()=>this.dispatchEvent(new Event('loadingdone')),()=>this.dispatchEvent(new Event('loadingerror')));return result;}
    check(font,text=' ') {const [families,faces,used]=this._match(font,text);return families.some(family=>faces.some(face=>face.family===family)?used.every(face=>face.family!==family||face.status==='loaded'):call('dom.fontCheck',0,family));}
  }
  Object.assign(globalThis,{FontFace,FontFaceSet});document.fonts=new FontFaceSet();
  globalThis.NodeFilter = { SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024, FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3 };
  Object.defineProperties(globalThis, { innerWidth: { get: () => host('dom.viewport')[0] }, innerHeight: { get: () => host('dom.viewport')[1] }, devicePixelRatio: { get: () => host('dom.viewport')[2] } });
  class VisualViewport extends EventTarget { get width() { return viewSize()[0]; } get height() { return viewSize()[1]; } get scale() { return 1; } get offsetLeft() { return 0; } get offsetTop() { return 0; } get pageLeft() { return scrollX; } get pageTop() { return scrollY; } }
  globalThis.VisualViewport = VisualViewport; globalThis.visualViewport = new VisualViewport();
  const page = axis => ({ get: () => document.documentElement?.[axis] ?? 0, configurable: true });
  Object.defineProperties(globalThis, { scrollX: page('scrollLeft'), scrollY: page('scrollTop'), pageXOffset: page('scrollLeft'), pageYOffset: page('scrollTop') });
  globalThis.scrollTo = globalThis.scroll = (left, top) => document.documentElement?.scrollTo(left, top);
  globalThis.scrollBy = (left, top) => document.documentElement?.scrollBy(left, top);
  globalThis.getSelection = () => selection;
  globalThis.getComputedStyle = (node, pseudo = null) => {
    if (!(node instanceof Element)) throw new TypeError('Expected an element');
    const id = node._id, kind = pseudo == null ? '' : String(pseudo);
    let names = null;
    // Container queries need sizes, so the first read of a page lays it out like Chrome does.
    return declaration({ computed: name => { relayout(name); return call('dom.computed', id, name, kind); }, names: () => names ??= call('dom.computed', id, null, kind) });
  };
  const relayout = name => { if (!laid || /^(width|height|min-|max-|margin|padding|inset|top|right|bottom|left|transform|perspective-origin|grid-template)/.test(name)) layout(); };
  // CSS Typed OM: its globals make the classes of typed_om.js on first use.
  const typedOM = globalThis.__appsTypedOM; delete globalThis.__appsTypedOM;
  let typedMade = null;
  const typed = () => typedMade ??= (() => {
    const made = typedOM.make({
      property: name => { name = String(name); if (name.startsWith('--')) return name; name = name.toLowerCase(); if (name.startsWith('-webkit-') && (cssAliases ??= new Set(call('dom.aliases'))).has(name)) name = name.slice(8); return cssProperty(name) ? name : null; },
      // A custom property takes the text only when nothing of it was dropped.
      parse: (name, text) => { const parts = (shorthands ?? loadShorthands()).get(name), list = declare(name + ':' + text); return list.length && list.every(entry => !entry[2] && (parts ? parts.includes(entry[0]) : entry[0] === name && (!name.startsWith('--') || entry[1] === text.trim() && list.length === 1))) ? parts ? text.trim() : specified(name, list[0][1]) : null; },
      layers, known: word => call('dom.keyword', word),
      supports: (name, text) => call('dom.supports', name, text),
      longhands: name => (shorthands ?? loadShorthands()).get(name),
      computed: (node, name) => { relayout(name); return call('dom.computed', node._id, name, '', true); },
      computedNames: node => call('dom.computed', node._id, null, ''),
    });
    // A name that the page set or deleted keeps what the page gave it.
    for (const [target, values, enumerable] of [[globalThis, made.classes, false], [CSS, made.units, true]]) for (const [name, value] of Object.entries(values)) if (typedGetters.has(Object.getOwnPropertyDescriptor(target, name)?.get)) Object.defineProperty(target, name, { value, writable: true, enumerable, configurable: true });
    return made;
  })();
  const typedGetters = new WeakSet(), styleMaps = new WeakMap(), computedMaps = new WeakMap();
  const cached = (maps, owner, make) => { let map = maps.get(owner); if (!map) maps.set(owner, map = make()); return map; };
  for (const [target, names, enumerable] of [[globalThis, typedOM.names, false], [CSS, typedOM.units, true]]) for (const name of names.split(/[ |]/)) {
    const get = () => (target === CSS ? typed().units : typed().classes)[name];
    typedGetters.add(get);
    Object.defineProperty(target, name, { get, set(value) { Object.defineProperty(target, name, { value, writable: true, enumerable, configurable: true }); }, enumerable, configurable: true });
  }
  Object.defineProperty(Element.prototype, 'attributeStyleMap', { get() { return cached(styleMaps, this, () => typed().map(this)); }, enumerable: true, configurable: true });
  Object.defineProperty(CSSStyleRule.prototype, 'styleMap', { get() { return cached(styleMaps, this, () => typed().map(this)); }, enumerable: true, configurable: true });
  Element.prototype.computedStyleMap = function computedStyleMap() { return cached(computedMaps, this, () => typed().computed(this)); };
  const animationFrames = new Map(); let animationFrameId = 0, animationTimer, frameRequested = false;
  globalThis.__appsAnimationFrame = () => {
    animationTimer = undefined; frameRequested = false;
    const now = performance.now(), callbacks = [...animationFrames];
    timelineTime = now;
    if (pendingAnimations.size) { const ready = [...pendingAnimations]; pendingAnimations.clear(); for (const animation of ready) readyAnimation(animation); }
    for (const [id, callback] of callbacks) {
      if (!animationFrames.delete(id)) continue;
      try { callback(now); } catch (error) { report(error); }
      // As in browsers, microtasks run after each callback.
      drainMicrotasks?.();
    }
    microtask(() => { timelineTime = null; });
  };
  // One request covers every callback until the next frame runs.
  const requestFrame = () => { if (!frameRequested) { frameRequested = true; if (!host('dom.requestFrame') && animationTimer === undefined) animationTimer = setTimeout(__appsAnimationFrame, 16); } };
  globalThis.requestAnimationFrame = fn => {
    if (typeof fn !== 'function') throw new TypeError('Expected an animation callback');
    const id = ++animationFrameId; animationFrames.set(id, fn);
    requestFrame();
    return id;
  };
  globalThis.cancelAnimationFrame = id => animationFrames.delete(id);
  let timelineTime = null;
  const timelineNow = () => { if (timelineTime === null) { timelineTime = performance.now(); microtask(() => { timelineTime = null; }); } return timelineTime; };
  class AnimationTimeline { constructor() { if (new.target === AnimationTimeline) throw new TypeError('Illegal constructor'); } get currentTime() { return timelineNow(); } get duration() { return null; } }
  class DocumentTimeline extends AnimationTimeline { #origin; constructor(options = {}) { super(); this.#origin = Number(options?.originTime ?? 0); } get currentTime() { return timelineNow() - this.#origin; } }
  const documentTimeline = new DocumentTimeline();
  const easings = new Map();
  const easingOf = value => {
    value = String(value).trim();
    if (easings.has(value)) return value;
    // `dom.supports` accepts a list for `animation-timing-function`, so commas outside functions must be
    // rejected here.
    if (!value || value.replace(/\([^)]*\)/g, '').includes(',') || !call('dom.supports', 'animation-timing-function', value)) throw new TypeError(`'${value}' is not a valid easing`);
    if (easings.size > 256) easings.clear();
    easings.set(value, true);
    return value;
  };
  const finite = value => { const number = Number(value); if (!Number.isFinite(number)) throw new TypeError('The provided double value is non-finite'); return number; };
  const defaultTiming = { delay: 0, endDelay: 0, fill: 'auto', iterationStart: 0, iterations: 1, duration: 'auto', direction: 'normal', easing: 'linear' };
  const timingChoices = { fill: ['none', 'forwards', 'backwards', 'both', 'auto'], direction: ['normal', 'reverse', 'alternate', 'alternate-reverse'] };
  const timingOf = (input, base) => {
    const timing = { ...base };
    if (input == null) return timing;
    if (typeof input !== 'object') throw new TypeError('Expected timing options');
    for (const key of Object.keys(defaultTiming)) {
      const raw = input[key];
      if (raw === undefined) continue;
      let value;
      if (timingChoices[key]) { value = String(raw); if (!timingChoices[key].includes(value)) throw new TypeError(`'${value}' is not a valid ${key}`); }
      else if (key === 'easing') value = easingOf(raw);
      else if (key === 'duration' && typeof raw === 'string') { if (raw !== 'auto') throw new TypeError(`'${raw}' is not a valid duration`); value = raw; }
      else {
        value = Number(raw);
        if (Number.isNaN(value) || key !== 'iterations' && key !== 'duration' && !Number.isFinite(value) || (key === 'iterations' || key === 'duration' || key === 'iterationStart') && value < 0) throw new TypeError(`${raw} is not a valid ${key}`);
      }
      timing[key] = value;
    }
    return timing;
  };
  const computedTiming = (timing, local, rate) => {
    const duration = timing.duration === 'auto' ? 0 : timing.duration, fill = timing.fill === 'auto' ? 'none' : timing.fill;
    const activeDuration = duration === 0 || timing.iterations === 0 ? 0 : duration * timing.iterations, endTime = Math.max(timing.delay + activeDuration + timing.endDelay, 0);
    const result = { ...timing, duration, fill, activeDuration, endTime, localTime: local, progress: null, currentIteration: null };
    if (local === null) return result;
    const start = Math.max(Math.min(timing.delay, endTime), 0), stop = Math.max(Math.min(timing.delay + activeDuration, endTime), 0);
    const before = local < start || rate < 0 && local === start, after = !before && (local > stop || rate >= 0 && local === stop);
    if (before && fill !== 'backwards' && fill !== 'both' || after && fill !== 'forwards' && fill !== 'both') return result;
    const active = Math.max(Math.min(local - timing.delay, activeDuration), 0);
    const overall = timing.iterationStart + (duration === 0 ? (before ? 0 : timing.iterations) : active / duration);
    let simple = Number.isFinite(overall) ? overall % 1 : timing.iterationStart % 1;
    if (simple === 0 && !before && active === activeDuration && timing.iterations !== 0 && overall > 0) simple = 1;
    const iteration = after && timing.iterations === Infinity ? Infinity : Math.floor(overall) - (simple === 1 ? 1 : 0);
    const reverse = timing.direction === 'reverse' || timing.direction === 'alternate' && iteration % 2 === 1 || timing.direction === 'alternate-reverse' && iteration % 2 === 0;
    const directed = reverse ? 1 - simple : simple;
    return Object.assign(result, { progress: timing.easing === 'linear' ? directed : call('dom.easing', timing.easing, directed, before && !reverse || after && reverse), currentIteration: iteration });
  };
  const frameKeys = new Set(['offset', 'easing', 'composite']);
  const keyframesOf = input => {
    if (input == null) return [];
    if (typeof input !== 'object') throw new TypeError('Keyframes must be an object or an array');
    let frames;
    if (typeof input[Symbol.iterator] === 'function') {
      frames = Array.from(input, frame => {
        if (frame === null || typeof frame !== 'object') throw new TypeError('A keyframe must be an object');
        return { offset: frame.offset == null ? null : Number(frame.offset), computed: null, easing: easingOf(frame.easing ?? 'linear'), composite: frame.composite == null ? 'auto' : String(frame.composite), props: Object.keys(frame).filter(key => !frameKeys.has(key)).map(key => [styleName(key), String(frame[key])]) };
      });
    } else {
      const points = [];
      for (const key of Object.keys(input)) {
        if (frameKeys.has(key)) continue;
        const values = Array.isArray(input[key]) ? input[key] : [input[key]];
        values.forEach((value, index) => points.push([values.length === 1 ? 1 : index / (values.length - 1), styleName(key), String(value)]));
      }
      points.sort((a, b) => a[0] - b[0]);
      frames = [];
      for (const [offset, name, value] of points) { const last = frames[frames.length - 1]; if (last?.computed === offset) last.props.push([name, value]); else frames.push({ offset: null, computed: offset, easing: 'linear', composite: 'auto', props: [[name, value]] }); }
      const list = (value, map) => value === undefined ? [] : Array.isArray(value) ? value.map(map) : [map(value)];
      list(input.offset, value => value == null ? null : Number(value)).forEach((offset, index) => { if (frames[index]) frames[index].offset = offset; });
      const eases = list(input.easing, easingOf), composites = list(input.composite, String);
      frames.forEach((frame, index) => { if (eases.length) frame.easing = eases[index % eases.length]; if (composites.length) frame.composite = composites[index % composites.length]; });
    }
    let previous = -Infinity;
    for (const { offset } of frames) {
      if (offset === null) continue;
      if (!(offset >= 0 && offset <= 1)) throw new TypeError('Keyframe offsets must be between 0 and 1');
      if (offset < previous) throw new TypeError('Keyframe offsets must be in animationOrder');
      previous = offset;
    }
    const known = frames.map(frame => frame.offset ?? frame.computed);
    if (frames.length === 1) known[0] ??= 1;
    else if (frames.length) { known[0] ??= 0; known[frames.length - 1] ??= 1; }
    for (let index = 1; index < known.length; index++) {
      if (known[index] !== null) continue;
      let end = index;
      while (known[end] === null) end++;
      for (let step = index; step < end; step++) known[step] = known[index - 1] + (known[end] - known[index - 1]) * (step - index + 1) / (end - index + 1);
    }
    frames.forEach((frame, index) => { frame.computed = known[index]; });
    return frames;
  };
  const camelName = name => name.startsWith('--') ? name : name === 'float' ? 'cssFloat' : name.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  const effects = new WeakMap();
  let retimed, animationOrder, replaceable, filling;
  class AnimationEffect {
    constructor() { if (new.target === AnimationEffect) throw new TypeError('Illegal constructor'); }
    getTiming() { return { ...effects.get(this).timing }; }
    getComputedTiming() { const { timing, animation } = effects.get(this); return computedTiming(timing, animation?.currentTime ?? null, animation?.playbackRate ?? 1); }
    updateTiming(input = {}) { const effect = effects.get(this); effect.timing = timingOf(input, effect.timing); if (effect.animation) retimed(effect.animation); }
  }
  class KeyframeEffect extends AnimationEffect {
    constructor(target, keyframes, options) {
      super();
      if (target instanceof KeyframeEffect) { const source = effects.get(target); effects.set(this, { ...source, frames: source.frames.map(frame => ({ ...frame, props: [...frame.props] })), timing: { ...source.timing }, animation: null }); return; }
      if (target != null && !(target instanceof Element)) throw new TypeError('The target must be an element');
      const settings = typeof options === 'number' ? { duration: options } : options ?? {};
      const composite = settings.composite ?? 'replace';
      if (!['replace', 'add', 'accumulate'].includes(composite)) throw new TypeError(`'${composite}' is not a valid composite`);
      effects.set(this, { target: target ?? null, pseudo: settings.pseudoElement ?? null, frames: keyframesOf(keyframes), timing: timingOf(settings, defaultTiming), composite, animation: null });
    }
    get target() { return effects.get(this).target; }
    set target(target) { if (target != null && !(target instanceof Element)) throw new TypeError('The target must be an element'); const effect = effects.get(this); effect.target = target ?? null; if (effect.animation) retimed(effect.animation); }
    get pseudoElement() { return effects.get(this).pseudo; }
    set pseudoElement(value) { const effect = effects.get(this); effect.pseudo = value == null ? null : String(value); if (effect.animation) retimed(effect.animation); }
    get composite() { return effects.get(this).composite; }
    set composite(value) { const effect = effects.get(this); if (['replace', 'add', 'accumulate'].includes(value)) { effect.composite = value; if (effect.animation) retimed(effect.animation); } }
    get iterationComposite() { return 'replace'; }
    getKeyframes() { return effects.get(this).frames.map(frame => { const result = { offset: frame.offset, computedOffset: frame.computed, easing: frame.easing, composite: frame.composite }; for (const [name, value] of frame.props) result[camelName(name)] = value; return result; }); }
    setKeyframes(keyframes) { const effect = effects.get(this); effect.frames = keyframesOf(keyframes); if (effect.animation) retimed(effect.animation); }
  }
  class AnimationPlaybackEvent extends Event { constructor(type, init = {}) { super(type, init); this.currentTime = init?.currentTime ?? null; this.timelineTime = init?.timelineTime ?? null; } }
  const liveAnimations = new Set();
  let animationIds = 0;
  const pendingAnimations = new Set();
  let readyAnimation;
  // As in Chrome, an element with no box can't take committed styles: a disconnected one, one in a `display: none` or `content-visibility: hidden` box of the flat tree, or one with `display: contents`.
  const boxless = target => { if (!target.isConnected) return true; for (let node = target; node; node = node.assignedSlot ?? node.parentElement ?? node.parentNode?.host) { const style = getComputedStyle(node); if (style.display === 'none' || node === target && style.display === 'contents' || node !== target && style.contentVisibility === 'hidden') return true; } return false; };
  class Animation extends EventTarget {
    #id = ++animationIds; #effect = null; #timeline; #start = null; #hold = null; #rate = 1; #pendingRate = null; #playing = false; #pausing = false; #previous = null;
    #finished; #settle; #fail; #done = false; #queued = false; #ready; #readyResolve = null; #readyReject = null; #timer; #replace = 'active'; #synced = null; #sent = null;
    static {
      retimed = animation => animation.#update(false, false);
      animationOrder = animation => animation.#id;
      filling = animation => animation.#fills();
      replaceable = animation => animation.#replace === 'active' && animation.playState === 'finished' && animation.#fills();
      readyAnimation = animation => animation.#commit();
    }
    constructor(effect = null, timeline = documentTimeline) { super(); this.id = ''; this.#timeline = timeline; this.#ready = Promise.resolve(this); this.#reset(); this.effect = effect; }
    #reset() { this.#done = false; this.#finished = new Promise((resolve, reject) => { this.#settle = resolve; this.#fail = reject; }); this.#finished.catch(() => {}); }
    #wait() { this.#ready = new Promise((resolve, reject) => { this.#readyResolve = resolve; this.#readyReject = reject; }); this.#ready.catch(() => {}); }
    #resolve() { this.#readyResolve?.(this); this.#readyResolve = this.#readyReject = null; }
    #schedule() { pendingAnimations.add(this); requestFrame(); }
    #now() { return this.#timeline?.currentTime ?? null; }
    #current() { if (this.#hold !== null) return this.#hold; const now = this.#now(); return this.#start === null || now === null ? null : (now - this.#start) * this.#rate; }
    #effective() { return this.#pendingRate ?? this.#rate; }
    #apply() { if (this.#pendingRate !== null) { this.#rate = this.#pendingRate; this.#pendingRate = null; } }
    #end() { return this.#effect ? computedTiming(effects.get(this.#effect).timing, null, 1).endTime : 0; }
    #fills() { const fill = this.#effect ? effects.get(this.#effect).timing.fill : 'none'; return this.#rate < 0 ? fill === 'backwards' || fill === 'both' : fill === 'forwards' || fill === 'both'; }
    #seek(time) { const now = this.#now(); if (this.#hold !== null || this.#start === null || now === null || this.#rate === 0) this.#hold = time; else this.#start = now - time / this.#rate; if (now === null) this.#start = null; this.#previous = null; }
    get effect() { return this.#effect; }
    set effect(effect) {
      if (effect === this.#effect) return;
      if (effect !== null && !(effect instanceof KeyframeEffect)) throw new TypeError('The effect must be a KeyframeEffect');
      if (this.#effect) effects.get(this.#effect).animation = null;
      if (effect) { const owner = effects.get(effect).animation; if (owner) owner.effect = null; effects.get(effect).animation = this; }
      this.#effect = effect;
      this.#update(false, false);
    }
    get timeline() { return this.#timeline; }
    set timeline(timeline) {
      const time = this.#current(), running = this.#start !== null && this.#hold === null;
      this.#timeline = timeline ?? null;
      if (time !== null) { this.#start = null; this.#hold = time; if (running) { this.#playing = true; this.#schedule(); } }
      this.#update(false, false);
    }
    get startTime() { return this.#start; }
    set startTime(value) {
      const time = value == null ? null : finite(value);
      if (this.#now() === null && time !== null) this.#hold = null;
      const previous = this.#current();
      this.#apply();
      this.#start = time;
      if (time === null) this.#hold = previous; else if (this.#rate !== 0) this.#hold = null;
      if (this.pending) { this.#playing = this.#pausing = false; this.#resolve(); }
      this.#update(true, false);
    }
    get currentTime() { return this.#current(); }
    set currentTime(value) {
      if (value == null) { if (this.#current() !== null) throw new TypeError('The current time cannot be null'); return; }
      const time = finite(value);
      this.#seek(time);
      if (this.#pausing) { this.#hold = time; this.#apply(); this.#start = null; this.#pausing = false; this.#resolve(); }
      this.#update(true, false);
    }
    get playbackRate() { return this.#rate; }
    set playbackRate(value) { value = finite(value); this.#pendingRate = null; const time = this.#current(); this.#rate = value; if (time !== null) this.currentTime = time; else this.#update(false, false); }
    updatePlaybackRate(value) {
      const state = this.playState;
      this.#pendingRate = finite(value);
      if (this.pending) return;
      if (state === 'running') return this.#play(false);
      if (state === 'finished') {
        const now = this.#now(), unconstrained = this.#start !== null && now !== null ? (now - this.#start) * this.#rate : null;
        this.#start = this.#pendingRate === 0 ? now : unconstrained === null || now === null ? null : now - unconstrained / this.#pendingRate;
      }
      this.#apply();
      this.#update(false, false);
    }
    get playState() {
      const time = this.#current();
      if (time === null && this.#start === null && !this.pending) return 'idle';
      if (this.#pausing || this.#start === null && !this.#playing) return 'paused';
      const rate = this.#effective();
      return time !== null && (rate > 0 && time >= this.#end() || rate < 0 && time <= 0) ? 'finished' : 'running';
    }
    get replaceState() { return this.#replace; }
    get pending() { return this.#playing || this.#pausing; }
    get ready() { return this.#ready; }
    get finished() { return this.#finished; }
    // Browsers resolve the promises at once but fire the events in a later task.
    #fire(type, timelineTime, currentTime = this.#current()) { setTimeout(() => this.dispatchEvent(new AnimationPlaybackEvent(type, { currentTime, timelineTime }))); }
    #commit() {
      const now = this.#now();
      if (now === null || !this.pending) return;
      if (this.#playing) {
        this.#playing = false;
        if (this.#hold !== null) { this.#apply(); this.#start = this.#rate === 0 ? now : now - this.#hold / this.#rate; if (this.#rate !== 0) this.#hold = null; }
        else if (this.#start !== null && this.#pendingRate !== null) { const time = (now - this.#start) * this.#rate; this.#apply(); if (this.#rate === 0) this.#hold = time; else this.#start = now - time / this.#rate; }
      } else {
        this.#pausing = false;
        if (this.#start !== null && this.#hold === null) this.#hold = (now - this.#start) * this.#rate;
        this.#apply();
        this.#start = null;
      }
      this.#resolve();
      this.#update(false, false);
    }
    play() { this.#play(true); }
    #play(rewind) {
      const aborted = this.#pausing, time = this.#current(), end = this.#end(), rate = this.#effective();
      let seek = null;
      if (rewind && rate > 0 && (time === null || time < 0 || time >= end)) seek = 0;
      else if (rewind && rate < 0 && (time === null || time <= 0 || time > end)) { if (end === Infinity) throw new DOMException('Cannot play an endless animation in reverse', 'InvalidStateError'); seek = end; }
      else if (rate === 0 && time === null) seek = 0;
      if (seek !== null) this.#hold = seek;
      if (this.#hold !== null) this.#start = null;
      const waiting = this.pending;
      this.#playing = this.#pausing = false;
      if (this.#hold === null && seek === null && !aborted && this.#pendingRate === null) return;
      if (!waiting) this.#wait();
      this.#playing = true;
      this.#schedule();
      this.#update(false, false);
    }
    pause() {
      if (this.#pausing || this.playState === 'paused') return;
      if (this.#current() === null) {
        const end = this.#end();
        if (this.#effective() < 0 && end === Infinity) throw new DOMException('Cannot pause an endless animation in reverse', 'InvalidStateError');
        this.#hold = this.#effective() < 0 ? end : 0;
      }
      if (!this.#playing) this.#wait();
      this.#playing = false;
      this.#pausing = true;
      this.#schedule();
      this.#update(false, false);
    }
    finish() {
      const end = this.#end(), rate = this.#effective();
      if (rate === 0 || rate > 0 && end === Infinity) throw new DOMException('Cannot finish the animation', 'InvalidStateError');
      this.#apply();
      const limit = this.#rate > 0 ? end : 0, now = this.#now();
      this.#seek(limit);
      if (this.#start === null && now !== null) this.#start = now - limit / this.#rate;
      if (this.pending && this.#start !== null) { if (this.#pausing) this.#hold = null; this.#playing = this.#pausing = false; this.#resolve(); }
      this.#update(true, true);
    }
    cancel() {
      if (this.playState !== 'idle') {
        if (this.pending) { this.#playing = this.#pausing = false; this.#apply(); this.#readyReject?.(new DOMException('The animation was canceled', 'AbortError')); this.#readyResolve = this.#readyReject = null; this.#ready = Promise.resolve(this); }
        if (!this.#done) this.#fail(new DOMException('The animation was canceled', 'AbortError'));
        this.#reset();
        this.#fire('cancel', this.#now(), null);
      }
      this.#start = this.#hold = null;
      this.#update(false, false);
    }
    reverse() {
      if (this.#now() === null) throw new DOMException('The animation has no active timeline', 'InvalidStateError');
      const original = this.#pendingRate;
      this.#pendingRate = -this.#effective();
      try { this.#play(true); } catch (error) { this.#pendingRate = original; throw error; }
    }
    persist() { this.#replace = 'persisted'; }
    commitStyles() {
      const effect = this.#effect && effects.get(this.#effect), target = effect?.target;
      if (!target) return;
      if (effect.pseudo) throw new DOMException('The target is not rendered', 'NoModificationAllowedError');
      if (boxless(target)) throw new DOMException('Target element is not rendered.', 'InvalidStateError');
      const synced = this.#synced !== null;
      if (!synced) this.#sync(true);
      const values = call('dom.animated', target._id, this.#id) ?? {};
      if (!synced) this.#sync();
      for (const frame of effect.frames) for (const [name] of frame.props) if (values[name] !== undefined) target.style.setProperty(name, values[name]);
    }
    #update(seek, now) {
      const end = this.#end(), rate = this.#rate, time = this.#now();
      const unconstrained = seek ? this.#current() : this.#start !== null && time !== null ? (time - this.#start) * rate : null;
      if (unconstrained !== null && this.#start !== null && !this.pending) {
        if (rate > 0 && unconstrained >= end) this.#hold = seek ? unconstrained : Math.max(this.#previous ?? end, end);
        else if (rate < 0 && unconstrained <= 0) this.#hold = seek ? unconstrained : Math.min(this.#previous ?? 0, 0);
        else if (rate !== 0 && time !== null) { if (seek && this.#hold !== null) this.#start = time - this.#hold / rate; this.#hold = null; }
      }
      this.#previous = this.#current();
      clearTimeout(this.#timer);
      const state = this.playState;
      if (state === 'finished') {
        if (!this.#done && !this.#queued) {
          this.#queued = true;
          const notify = () => { this.#queued = false; if (this.playState !== 'finished' || this.#done) return; this.#done = true; this.#settle(this); this.#fire('finish', this.#now()); if (this.#fills()) this.#replaceOlder(); };
          if (now) notify(); else microtask(notify);
        }
      } else {
        if (this.#done) this.#reset();
        if (state === 'running' && !this.pending && rate !== 0) {
          const left = rate > 0 ? (end - this.#previous) / rate : this.#previous / -rate;
          if (Number.isFinite(left)) this.#timer = setTimeout(() => this.#update(false, false), Math.max(0, left));
        }
      }
      if (state === 'idle' || this.#replace === 'removed' || state === 'finished' && !this.#fills()) liveAnimations.delete(this); else liveAnimations.add(this);
      this.#sync();
    }
    #replaceOlder() {
      const target = effects.get(this.#effect)?.target, covered = new Set();
      if (!target) return;
      for (const animation of [...liveAnimations].filter(animation => animation.effect && effects.get(animation.effect).target === target && replaceable(animation)).sort((a, b) => animationOrder(b) - animationOrder(a))) {
        const names = effects.get(animation.effect).frames.flatMap(frame => frame.props.map(([name]) => name));
        if (names.every(name => covered.has(name))) {
          animation.#replace = 'removed';
          liveAnimations.delete(animation);
          animation.#sync();
          animation.#fire('remove', animation.#now());
        } else for (const name of names) covered.add(name);
      }
    }
    #sync(force) {
      const effect = this.#effect && effects.get(this.#effect), target = effect?.target, time = this.#current();
      if (!target || effect.pseudo !== null && effect.pseudo !== '::before' && effect.pseudo !== '::after' || time === null || !force && (this.#replace === 'removed' || this.playState === 'finished' && !this.#fills())) { if (this.#synced !== null) { this.#synced = null; call('dom.animation', this.#id, -1, null); } return; }
      const { timing } = effect, duration = timing.duration === 'auto' ? 0 : Math.min(timing.duration, 1e12), sent = this.#synced === target._id && this.#sent === effect.frames;
      this.#synced = target._id; this.#sent = effect.frames;
      call('dom.animation', this.#id, target._id, {
        frames: sent ? undefined : effect.frames.map(frame => [frame.computed, frame.props.map(([name, value]) => `${name}:${value}`).join(';'), frame.easing]),
        timing: [timing.delay, timing.endDelay, duration, timing.iterations === Infinity ? -1 : timing.iterations, timing.iterationStart],
        direction: timing.direction, fill: timing.fill === 'auto' ? 'none' : timing.fill, easing: timing.easing, composite: effect.composite, pseudo: effect.pseudo,
        time, rate: this.#start !== null && this.#hold === null && this.#now() !== null ? this.#rate : null,
      });
    }
  }
  for (const name of ['finish', 'cancel', 'remove']) handler(Animation.prototype, name);
  const cssAnimations = new Map(), cssData = new WeakMap(), cssToken = Symbol();
  class CSSAnimation extends Animation { constructor(token) { if (token !== cssToken) throw new TypeError('Illegal constructor'); super(); } get animationName() { return cssData.get(this).name; } }
  class CSSTransition extends Animation { constructor(token) { if (token !== cssToken) throw new TypeError('Illegal constructor'); super(); } get transitionProperty() { return cssData.get(this).name; } }
  const cssReset = data => { data.settled = false; data.finished = new Promise((resolve, reject) => { data.resolve = resolve; data.reject = reject; }); data.finished.catch(() => {}); };
  const cssSettle = (animation, done) => {
    const data = cssData.get(animation);
    if (data.settled) return false;
    data.settled = true;
    if (done) data.resolve(animation); else data.reject(new DOMException('The animation was canceled', 'AbortError'));
    const currentTime = done ? data.time : null, timelineTime = timelineNow();
    setTimeout(() => animation.dispatchEvent(new AnimationPlaybackEvent(done ? 'finish' : 'cancel', { currentTime, timelineTime })));
    data.ended = done;
    if (!done) data.state = 'idle';
    if (!done || !['forwards', 'both'].includes(effects.get(data.effect).timing.fill)) cssAnimations.delete(data.serial);
    return true;
  };
  const cssState = animation => {
    const data = cssData.get(animation), target = data.effect.target;
    if (data.state !== 'idle') {
      const entry = target && call('dom.cssAnimations', target._id, false, true).find(entry => entry[0] === data.serial);
      if (entry) Object.assign(data, { time: entry[4], state: entry[5] }); else Object.assign(data, data.ended ? { time: computedTiming(effects.get(data.effect).timing, null, 1).endTime, state: 'finished' } : { time: null, state: 'idle' });
    }
    return data;
  };
  const cssControl = (animation, action, time = 0) => { const data = cssData.get(animation); if (call('dom.cssControl', data.serial, action, time)) cssState(animation); return data; };
  const cssEvent = (type, id, data) => {
    const kind = type.startsWith('animation') ? 'animation' : 'transition', name = kind === 'animation' ? data.animationName : data.propertyName;
    if (!/(end|cancel)$/.test(type)) return false;
    let settled = false;
    for (const animation of cssAnimations.values()) {
      const own = cssData.get(animation);
      if (own.kind === kind && own.name === name && own.effect.target?._id === id && (own.effect.pseudoElement ?? '') === (data.pseudoElement ?? '')) settled = cssSettle(animation, type.endsWith('end')) || settled;
    }
    return settled;
  };
  for (const Type of [CSSAnimation, CSSTransition]) Object.defineProperties(Type.prototype, {
    effect: { get() { return cssData.get(this).effect; }, set() {}, configurable: true },
    timeline: { get() { return documentTimeline; }, set() {}, configurable: true },
    currentTime: { get() { return cssState(this).time; }, set(value) { if (value == null) throw new TypeError('The current time cannot be null'); cssControl(this, 'seek', finite(value)); }, configurable: true },
    startTime: { get() { const data = cssState(this); return data.state === 'running' ? timelineNow() - data.time : null; }, set(value) { if (value != null) cssControl(this, 'seek', timelineNow() - finite(value)); }, configurable: true },
    playState: { get() { return cssState(this).state; }, configurable: true },
    pending: { get() { return false; }, configurable: true },
    ready: { get() { return cssData.get(this).ready ??= Promise.resolve(this); }, configurable: true },
    finished: { get() { return cssData.get(this).finished; }, configurable: true },
    playbackRate: { get() { return 1; }, set(value) { if (finite(value) !== 1) throw new DOMException('CSS animations and transitions play at rate 1', 'NotSupportedError'); }, configurable: true },
    replaceState: { get() { return 'active'; }, configurable: true },
    updatePlaybackRate: { value(value) { this.playbackRate = value; }, writable: true, configurable: true },
    reverse: { value() { throw new DOMException('CSS animations and transitions cannot play in reverse', 'NotSupportedError'); }, writable: true, configurable: true },
    persist: { value() {}, writable: true, configurable: true },
    play: { value() { const data = cssData.get(this); if (data.settled && data.state !== 'idle') { cssReset(data); data.ended = false; } cssAnimations.set(data.serial, this); cssControl(this, 'play'); }, writable: true, configurable: true },
    pause: { value() { cssControl(this, 'pause'); }, writable: true, configurable: true },
    finish: { value() { const { activeDuration } = cssData.get(this).effect.getComputedTiming(); if (activeDuration === Infinity) throw new DOMException('Cannot finish an endless animation', 'InvalidStateError'); cssControl(this, 'finish'); }, writable: true, configurable: true },
    cancel: { value() { if (cssState(this).state === 'idle') return; cssControl(this, 'cancel'); cssSettle(this, false); }, writable: true, configurable: true },
    commitStyles: { value() {
      const target = cssData.get(this).effect.target;
      if (boxless(target)) throw new DOMException('Target element is not rendered.', 'InvalidStateError');
      const computed = getComputedStyle(target); for (const frame of effects.get(cssData.get(this).effect).frames) for (const [name] of frame.props) target.style.setProperty(name, computed.getPropertyValue(name)); }, writable: true, configurable: true },
  });
  const cssOf = (node, subtree) => {
    // Animations and transitions start when styles change, which the layout pass applies. Without a
    // style that can start one, the styles alone answer.
    let found = call('dom.cssAnimations', node._id, subtree, subtree, true);
    if (found === null) { layout(); found = call('dom.cssAnimations', node._id, subtree, subtree, false); }
    const list = found.map(([serial, id, kind, name, time, state, timing, direction, fill, easing, frames, pseudo]) => {
      let animation = cssAnimations.get(serial);
      if (!animation) {
        animation = new (kind === 'animation' ? CSSAnimation : CSSTransition)(cssToken);
        const keyframes = [];
        for (const [offset, property, value, ease] of frames) {
          let frame = keyframes.find(frame => frame.offset === offset && frame.easing === ease);
          if (!frame) keyframes.push(frame = { offset, easing: ease });
          frame[camelName(property)] = value;
        }
        keyframes.sort((a, b) => a.offset - b.offset);
        const effect = new KeyframeEffect(wrap(id), keyframes, { delay: timing[0], endDelay: timing[1], duration: timing[2], iterations: timing[3] < 0 ? Infinity : timing[3], iterationStart: timing[4], direction, fill, easing, pseudoElement: pseudo || null });
        effects.get(effect).animation = animation;
        const data = { serial, kind, name, effect };
        cssReset(data);
        cssData.set(animation, data);
        cssAnimations.set(serial, animation);
      }
      Object.assign(cssData.get(animation), { time, state });
      return animation;
    });
    if (node._id === 0) { const live = new Set(list); for (const [serial, animation] of cssAnimations) if (!live.has(animation)) cssAnimations.delete(serial); }
    return list;
  };
  const animationsOf = test => [...liveAnimations].filter(animation => (animation.playState !== 'finished' || filling(animation)) && animation.effect && test(effects.get(animation.effect))).sort((a, b) => animationOrder(a) - animationOrder(b));
  Object.defineProperties(Element.prototype, {
    animate: { value(keyframes, options) { const animation = new Animation(new KeyframeEffect(this, keyframes, options), options?.timeline === undefined ? documentTimeline : options.timeline); if (options?.id !== undefined) animation.id = String(options.id); animation.play(); return animation; }, writable: true, configurable: true },
    getAnimations: { value(options) { const subtree = !!options?.subtree; return [...cssOf(this, subtree), ...animationsOf(({ target, pseudo }) => target === this && (!pseudo || subtree) || subtree && !!target && this.contains(target))]; }, writable: true, configurable: true },
  });
  Object.defineProperties(Document.prototype, {
    timeline: { get() { return documentTimeline; }, configurable: true },
    getAnimations: { value() { return [...cssOf(this, true), ...animationsOf(({ target }) => !!target?.isConnected)]; }, writable: true, configurable: true },
  });
  Object.assign(globalThis, { Animation, AnimationEffect, KeyframeEffect, AnimationTimeline, DocumentTimeline, AnimationPlaybackEvent, CSSAnimation, CSSTransition });
  const mediaQueries = new Set();
  let mediaViewport = '';
  class MediaQueryListEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.media = String(options.media ?? ''); this.matches = !!options.matches; }
  }
  class MediaQueryList extends EventTarget {
    constructor(media) { super(); this._media = String(media); this._matches = this.matches; this.onchange = null; mediaQueries.add(new WeakRef(this)); }
    get media() { return this._media; }
    get matches() { return host('dom.mediaMatches', this.media); }
    addListener(callback) { this.addEventListener('change', callback); }
    removeListener(callback) { this.removeEventListener('change', callback); }
  }
  Object.assign(globalThis, { MediaQueryList, MediaQueryListEvent, matchMedia: query => new MediaQueryList(query) });
  const webviewRefs = new Set(), attachedWebviews = new Map(), webviewReplies = new Map();
  let nextWebviewReply = 0;
  const webviewCall = (guest, data) => call('dom.open', 'webview', 'call', guest, data);
  class HTMLWebViewElement extends HTMLElement {
    _guest = null; _bounds = ''; _src = null;
    constructor(...args) { super(...args); webviewRefs.add(new WeakRef(this)); host('dom.requestFrame'); }
    #call(method, ...args) { return this._guest === null ? null : webviewCall(this._guest, { method, args }); }
    #promise(method, ...args) {
      if (this._guest === null) return Promise.reject(new Error('The WebView must be attached to the DOM'));
      return new Promise((resolve, reject) => { const reply = ++nextWebviewReply; webviewReplies.set(reply, [resolve, reject]); webviewCall(this._guest, { method, args, reply }); });
    }
    get src() { return this.getAttribute('src') ?? ''; }
    set src(value) { this.setAttribute('src', value); }
    get partition() { return this.getAttribute('partition') ?? ''; }
    set partition(value) { this.setAttribute('partition', value); }
    getWebContentsId() { if (this._guest === null) throw new Error('The WebView must be attached to the DOM'); return this._guest; }
    loadURL(url) { return this.#promise('loadURL', String(url)); }
    getURL() { return this.#call('getURL') ?? ''; }
    getTitle() { return this.#call('getTitle') ?? ''; }
    isLoading() { return !!this.#call('isLoading'); }
    isLoadingMainFrame() { return !!this.#call('isLoading'); }
    canGoBack() { return !!this.#call('canGoBack'); }
    canGoForward() { return !!this.#call('canGoForward'); }
    goBack() { this.#call('goBack'); }
    goForward() { this.#call('goForward'); }
    reload() { this.#call('reload'); }
    reloadIgnoringCache() { this.#call('reload'); }
    stop() { this.#call('stop'); }
    focus() { this.#call('focus'); }
    openDevTools() { this.#call('openDevTools'); }
    closeDevTools() {}
    isDevToolsOpened() { return false; }
    setZoomFactor(value) { this.#call('setZoomFactor', value); }
    getZoomFactor() { return this.#call('getZoomFactor') ?? 1; }
    setZoomLevel(value) { this.#call('setZoomLevel', value); }
    getZoomLevel() { return this.#call('getZoomLevel') ?? 0; }
    executeJavaScript(code) { return this.#promise('executeJavaScript', String(code)); }
    insertCSS(css) { return this.#promise('insertCSS', String(css)); }
    getUserAgent() { return this.#call('getUserAgent') ?? navigator.userAgent; }
  }
  globalThis.HTMLWebViewElement = HTMLWebViewElement;
  htmlTypes.set('webview', HTMLWebViewElement);
  // A `<terminal>` element shows the terminal of its session, which a preload script creates.
  const terminalMods = event => (event.shiftKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.altKey ? 4 : 0) | (event.metaKey ? 8 : 0);
  const terminalMouse = (session, kind, button, data) => call('terminal.mouse', session, kind, button, terminalMods(data), data.clientX, data.clientY, !!(data.buttons & 1));
  let terminalDrag = null;
  class HTMLTerminalElement extends HTMLElement {
    _preedit = '';
    get session() { return terminalOf(this) ? terminalSessions.get(this) : null; }
    set session(value) {
      value ??= null;
      const id = value && terminalSession?.(value);
      if (id === undefined) throw new TypeError('Expected a terminal session');
      call('terminal.attach', this._id, id);
      terminalViews.delete(terminalSessions.get(this)); terminalSessions.delete(this);
      if (!value) return;
      // A session shows in one element.
      terminalSessions.delete(terminalViews.get(value));
      terminalViews.set(value, this); terminalSessions.set(this, value);
    }
  }
  globalThis.HTMLTerminalElement = HTMLTerminalElement;
  htmlTypes.set('terminal', HTMLTerminalElement);
  globalThis.__appsWebviewEvent = (id, name, detail) => attachedWebviews.get(id)?.dispatchEvent(Object.assign(new Event(name), detail));
  globalThis.__appsWebviewReply = (id, value, error) => { const reply = webviewReplies.get(id); webviewReplies.delete(id); if (error === null) reply?.[0](value); else reply?.[1](new Error(error)); };
  globalThis.__appsMediaChanged=()=>{
    if (!mediaQueries.size) return;
    const viewport = host('dom.viewport').slice(0, 4).join(':');
    if (viewport === mediaViewport) return;
    mediaViewport = viewport;
    for (const reference of mediaQueries) {
      const query = reference.deref();
      if (!query) { mediaQueries.delete(reference); continue; }
      const matches = query.matches;
      if (matches === query._matches) continue;
      query._matches = matches;
      query.dispatchEvent(new MediaQueryListEvent('change', { media: query.media, matches }));
    }
  };
  // Besides the border box, a content box reads the styles, the width of the parent for percentages and the scroll
  // range for scroll bars. A transformed box measures its layout box anew after each layout.
  const inputs = node => {
    const id = node._id, i = 4 + id * 8, up = node._m.up?.id, j = 4 + up * 8;
    if (i >= limit || marks[id] & 128) return 'l' + geometry[2];
    return moves[1] + ':' + (j < limit ? geometry[j + 2] : '') + (marks[id] & 2 ? ':' + geometry[i + 6] + ':' + geometry[i + 7] : '');
  };
  // Use the last frame's observer flags. A forced layout here adds work every frame.
  globalThis.__appsLayout = () => {
    if (active) { observing = true; const lost = !focusable(active); observing = false; if (lost) active.blur(); }
    if (webviewScan) { webviewScan = false; document.querySelectorAll('webview'); }
    for (const ref of webviewRefs) {
      const view = ref.deref();
      if (!view) { webviewRefs.delete(ref); continue; }
      if (!view.isConnected) {
        if (view._guest !== null) { attachedWebviews.delete(view._guest); call('dom.open', 'webview', 'detach', view._guest, null); view._guest = null; view._bounds = ''; }
        continue;
      }
      const src = view.getAttribute('src');
      if (view._guest === null) {
        const guest = call('dom.open', 'webview', 'attach', null, { src: src ?? '', partition: view.partition });
        if (guest === null) { webviewRefs.delete(ref); continue; }
        view._guest = guest; view._src = src;
        attachedWebviews.set(guest, view);
        view.dispatchEvent(new Event('did-attach'));
      } else if (src !== view._src) { view._src = src; if (src) view.loadURL(src).catch(() => {}); }
      const rect = view.getBoundingClientRect(), bounds = { x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height) };
      const visible = bounds.width > 0 && bounds.height > 0 && getComputedStyle(view).visibility !== 'hidden', key = `${bounds.x},${bounds.y},${bounds.width},${bounds.height},${visible}`;
      if (key !== view._bounds) { view._bounds = key; call('dom.open', 'webview', 'bounds', view._guest, { bounds, visible }); }
    }
    // Before the first frame there is no last layout, so observers would see empty boxes.
    if ((resizeObservers.size || intersectionObservers.size) && !geometry[2]) laidOut();
    for (const ref of resizeObservers) { const observer = ref.deref(); if (!observer) { resizeObservers.delete(ref); continue; } const entries = []; observing = true; try { for (const [key, previous] of observer.targets) { const target = key.deref(); if (!target) { observer.targets.delete(key); continue; } const rect = target.getBoundingClientRect(), at = inputs(target); if (previous?.[0] === rect.width && previous[1] === rect.height && previous[2] === at) continue; let [, , clientWidth, clientHeight, top, right, bottom, left, borderWidth, borderHeight, , , inline] = client(target), width = Math.max(0, clientWidth - left - right), height = Math.max(0, clientHeight - top - bottom);
      // As in Chrome, an SVG element other than `<svg>` reports its bounding box, and a non-replaced inline box nothing.
      if (target instanceof SVGElement && !(target instanceof SVGSVGElement)) { const box = target.getBBox(); top = left = 0; borderWidth = width = box.width; borderHeight = height = box.height; } else if (!atomicTags.test(target.localName)) { if (inline) top = left = borderWidth = borderHeight = width = height = 0; }
      // An atomic inline box like `<math>` has no client box, so its content box comes from its border box.
      else if (!clientWidth && !clientHeight && (borderWidth || borderHeight)) { const s = getComputedStyle(target), side = name => parseFloat(s['padding' + name]) + parseFloat(s['border' + name + 'Width']); left = parseFloat(s.paddingLeft); top = parseFloat(s.paddingTop); width = Math.max(0, borderWidth - side('Left') - side('Right')); height = Math.max(0, borderHeight - side('Top') - side('Bottom')); }
      const z = zoomOf(target); if (z !== 1) [top, left, borderWidth, borderHeight, width, height] = [top, left, borderWidth, borderHeight, width, height].map(value => value / z);
      // Sizes are in 1/64 pixels, like Chrome's layout units, so float noise reports no change.
      const unit = value => Math.round(value * 64) / 64; [borderWidth, borderHeight, width, height] = [borderWidth, borderHeight, width, height].map(unit);
      const value = observer._boxes?.get(target) === 'border-box' ? borderWidth + ':' + borderHeight : width + ':' + height; observer.targets.set(key, [rect.width, rect.height, at, value]); if (value !== previous?.[3]) { entries.push({ target, contentRect: new DOMRect(left, top, width, height), borderBoxSize: [{ inlineSize: borderWidth, blockSize: borderHeight }], contentBoxSize: [{ inlineSize: width, blockSize: height }], devicePixelContentBoxSize: [{ inlineSize: Math.round(width * z * devicePixelRatio), blockSize: Math.round(height * z * devicePixelRatio) }] }); } } } finally { observing = false; } if (entries.length) try { observer.callback(entries, observer); } catch (error) { report(error); } }
    // Resize observers measure the last frame before scroll listeners change the page, so its styles are still computed.
    // `scrollend` fires when a box has not scrolled for 100 ms and is not on the way to a snap area, and only pages that listen for it pay for the timers.
    for (const event of call('dom.scrollEvents')) {
      // The last item can list `content-visibility: auto` boxes, each with whether it skips its contents now.
      if (event.skips) { for (const [id, skipped] of event.skips) wrap(id)?.dispatchEvent(new ContentVisibilityAutoStateChangeEvent('contentvisibilityautostatechange', { bubbles: true, skipped })); continue; }
      const [id, kind, block, inline] = event, node = wrap(id); if (!node) continue;
      if (kind === 2 || kind === 3) { const event = new SnapEvent(snapTargets, kind === 3 ? 'scrollsnapchange' : 'scrollsnapchanging', { bubbles: !id }); snapTargets.set(event, [block, inline].map(area => area === null ? null : wrap(area))); node.dispatchEvent(event); continue; }
      if (kind === 4) { clearTimeout(scrollEnds.get(node)); scrollEnds.delete(node); node.dispatchEvent(new Event('scrollend', { bubbles: !id })); continue; }
      hoverVersion = -1; node.dispatchEvent(new Event('scroll', { bubbles: !id })); if (!kind && ending) { clearTimeout(scrollEnds.get(node)); const end = () => { if (call('dom.snapping', id)) { scrollEnds.set(node, setTimeout(end, 100)); return; } scrollEnds.delete(node); node.dispatchEvent(new Event('scrollend', { bubbles: !id })); }; scrollEnds.set(node, setTimeout(end, 100)); }
    }
    __appsMediaChanged();
    if(!intersectionObservers.size)return;
    call('dom.layout',false,true);
    let viewport;
    for(const ref of intersectionObservers){
      const observer=ref.deref();
      if(!observer){intersectionObservers.delete(ref);continue;}
      observing=true;
      try{
      const m=layout(),root=observer.root==null||observer.root.nodeType===9?new DOMRect(0,0,...(viewport??=host('dom.viewport'))):observer.root.getBoundingClientRect();
      const [mt,mr,mb,ml]=observer._margins,top=mt[1]?mt[0]*root.height/100:mt[0],right=mr[1]?mr[0]*root.width/100:mr[0],bottom=mb[1]?mb[0]*root.height/100:mb[0],left=ml[1]?ml[0]*root.width/100:ml[0],stop=observer.root?._m;
      const rootBounds=new DOMRect(root.x-left,root.y-top,root.width+left+right,root.height+top+bottom),rl=rootBounds.left,rt=rootBounds.top,rr=rootBounds.right,rb=rootBounds.bottom;
      for(const [key,previous]of observer.targets){
        const target=key.deref();
        if(!target){observer.targets.delete(key);continue;}
        const i=4+target._id*8,inside=i<limit&&(marks[target._id]&17)===1,bx=inside?m[i]:0,by=inside?m[i+1]:0,bw=inside?m[i+2]:0,bh=inside?m[i+3]:0;
        let x=Math.max(bx,rl),y=Math.max(by,rt),r=Math.min(bx+bw,rr),b=Math.min(by+bh,rb);
        for(let rec=target._m.up;rec&&rec.tag[0]!=='#'&&rec!==stop;rec=rec.up){if(marks[rec.id]&4){const j=4+rec.id*8;x=Math.max(x,m[j]);y=Math.max(y,m[j+1]);r=Math.min(r,m[j]+m[j+2]);b=Math.min(b,m[j+1]+m[j+3]);}}
        const isIntersecting=inside&&target.isConnected&&r>=x&&b>=y,area=bw*bh,ratio=isIntersecting?(area?Math.max(0,r-x)*Math.max(0,b-y)/area:1):0;
        let changed=previous===null||previous.isIntersecting!==isIntersecting;
        for(let k=0;!changed&&k<observer.thresholds.length;k++)changed=(previous.ratio<observer.thresholds[k])!==(ratio<observer.thresholds[k]);
        if(changed)observer.records.push({time:performance.now(),target,rootBounds,boundingClientRect:new DOMRect(bx,by,bw,bh),intersectionRect:new DOMRect(x,y,Math.max(0,r-x),Math.max(0,b-y)),isIntersecting,intersectionRatio:ratio});
        if(previous===null)observer.targets.set(key,{ratio,isIntersecting});else{previous.ratio=ratio;previous.isIntersecting=isIntersecting;}
      }
      }catch(error){report(error);}finally{observing=false;}
      const entries=observer.takeRecords();if(entries.length)try{observer.callback(entries,observer);}catch(error){report(error);}
    }
  };

  globalThis.__appsEditing = () => terminalOf(active) ? { id: active._id, text: active._preedit, start: active._preedit.length, end: active._preedit.length, marked: composing } : editable(active) ? { id: active._id, text: active.value, start: active._direction === 'backward' ? active._end ?? 0 : active._start ?? 0, end: active._direction === 'backward' ? active._start ?? 0 : active._end ?? 0, marked: composing } : active?.isContentEditable ? { id: active._id, text: active.textContent, start: textOffset(active,selection.anchorNode,selection.anchorOffset), end: textOffset(active,selection.focusNode,selection.focusOffset), node: selection.focusNode?._id, offset: selection.focusOffset, marked: composing } : null;
  // The formatting that the next typed text gets, like Chrome's typing style: the inline elements around the
  // last deleted text, which typing copies. A selection change clears it.
  let typing = [];
  const blockTag = /^(address|article|aside|blockquote|dd|details|dialog|div|dl|dt|fieldset|figcaption|figure|footer|form|h[1-6]|header|hr|li|main|nav|ol|p|pre|section|table|tbody|td|th|thead|tr|ul)$/;
  // Warning: new nodes, even detached ones, make the next style read a full style pass, so the hot path of
  // editing keeps references and HTML text instead of copies.
  const formats = node => { const list = []; for (let up = node?.parentNode; up && up !== active && up.nodeType === 1 && !blockTag.test(up.localName); up = up.parentNode) list.unshift(up); return list; };
  const emptyInline = node => node.nodeType === 1 && !blockTag.test(node.localName) && !/^(br|img|input|select|textarea|video|audio|canvas|iframe|svg|object|embed)$/.test(node.localName) && !node.textContent && !node.querySelector('br,img,input,select,textarea,video,audio,canvas,iframe,svg,object,embed');
  // Whether nothing shows before (or after) the node on its line.
  const lineEdge = (node, after) => {
    for (let at = node; at !== active && at; at = at.parentNode) {
      if (at.nodeType === 1 && blockTag.test(at.localName)) return true;
      for (let next = after ? at.nextSibling : at.previousSibling; next; next = after ? next.nextSibling : next.previousSibling) {
        if (next.localName === 'br' || next.nodeType === 1 && blockTag.test(next.localName)) return true;
        if (/[^ \t\n\r]/.test(next.textContent) || next.nodeType === 1 && (/^(img|input|select|textarea|video|audio|canvas|iframe|svg|object|embed)$/.test(next.localName) || next.querySelector('img,input,select,textarea,video,audio,canvas,iframe,svg,object,embed'))) return false;
      }
    }
    return true;
  };
  // Like Chrome, the run of spaces at an edit alternates no-break spaces and spaces, so that all of them show. A
  // single space between other characters stays a space, and a run at the end of its text node ends with a
  // no-break space. Spaces that did not show before the edit stay
  // hidden: they go at the start of a line and stay as they are at its end. `typed` says that the edit wrote
  // spaces of the run. Returns the new place of `at`.
  const rebalance = (node, at, typed, deleted) => {
    let start = at, end = at; const data = node.data, white = c => c === ' ' || c === ' ';
    while (start > 0 && white(data[start - 1])) start--;
    while (end < data.length && white(data[end])) end++;
    if (start === end || /^(pre|pre-wrap|break-spaces)$/.test(getComputedStyle(node.parentNode).whiteSpace)) return at;
    const hidden = !typed && !data.slice(start, end).includes(' ');
    const first = start === 0 && lineEdge(node, false), last = end === data.length && lineEdge(node, true);
    if (hidden && first && !(deleted && at === start)) { node.deleteData(0, end); return Math.max(0, at - end); }
    if (hidden && last && !(deleted && at === end)) return at;
    const edge = end === data.length;
    let run = '';
    for (let i = start; i < end; i++) run += end - start === 1 && start > 0 && !edge || (i - start) % 2 && !(edge && i === end - 1) ? ' ' : ' ';
    if (run !== data.slice(start, end)) node.replaceData(start, end - start, run);
    return at;
  };
  // Undo for editable content inverts the DOM changes of each edit, which an observer records while the edit
  // runs. So the nodes stay the same objects, and page changes elsewhere in the host don't matter.
  const recorder = new MutationObserver(() => {});
  let step = null;
  // The observer runs only while an edit runs, so page changes between the updates of a composition stay out.
  const begin = (host, range, typed, backward, continuing) => {
    if (step && !(continuing && step.host === host)) finish();
    if (!step) {
      const ends = [range.startContainer, range.startOffset, range.endContainer, range.endOffset];
      // Like Chrome, undo selects what the edit replaced.
      const sel = range.collapsed || selection.isCollapsed ? backward ? [ends[2], ends[3], ends[0], ends[1]] : ends : [selection.anchorNode, selection.anchorOffset, selection.focusNode, selection.focusOffset];
      step = { host, sel, typed, records: [] };
    }
    recorder.observe(host, { subtree: true, childList: true, characterData: true, characterDataOldValue: true, attributes: true, attributeOldValue: true });
  };
  const pause = () => { const records = recorder.takeRecords(); recorder.disconnect(); step?.records.push(...records); };
  // A text change keeps only the changed part: `[node, at, removed, inserted]`.
  const diff = (node, before, after) => {
    let at = 0, end = 0;
    const most = Math.min(before.length, after.length);
    // Long texts compare in slices first, which is much faster than one character at a time.
    for (let step = 4096; step >= 1; step >>= 4) {
      while (at + step <= most && before.slice(at, at + step) === after.slice(at, at + step)) at += step;
      while (end + step <= most - at && before.slice(before.length - end - step, before.length - end) === after.slice(after.length - end - step, after.length - end)) end += step;
    }
    return { node, at, removed: before.slice(at, before.length - end), inserted: after.slice(at, after.length - end) };
  };
  const finish = () => {
    pause();
    if (!step) return;
    const { host, sel, typed, records } = step, ops = [];
    step = null;
    for (const record of records) {
      const last = ops.at(-1);
      if (record.type === 'characterData') { if (!(last?.text && last.node === record.target)) ops.push({ text: true, node: record.target, old: record.oldValue }); }
      else if (record.type === 'childList') ops.push({ parent: record.target, added: [...record.addedNodes], removed: [...record.removedNodes], next: record.nextSibling });
      else ops.push({ element: record.target, name: record.attributeName, old: record.oldValue });
    }
    // Each text change runs to the next change of its node, or to its text now.
    for (let i = ops.length; i--;) {
      const op = ops[i];
      if (op.text) { const later = ops.slice(i + 1).find(next => 'before' in next && next.node === op.node); ops[i] = diff(op.node, op.old, later ? later.before : op.node.data); ops[i].before = op.old; }
      else if (op.element) { const later = ops.slice(i + 1).find(next => next.element === op.element && next.name === op.name); op.value = later ? later.old : op.element.getAttribute(op.name); }
    }
    for (const op of ops) delete op.before;
    ops.splice(0, ops.length, ...ops.filter(op => !('at' in op) || op.removed || op.inserted));
    if (!ops.length) return;
    const history = histories.get(host) ?? histories.set(host, { undo: [], redo: [], size: 0 }).get(host), last = history.undo.at(-1), after = [selection.anchorNode, selection.anchorOffset, selection.focusNode, selection.focusOffset];
    history.redo.length = 0;
    // Typing in a row adds to one undo step.
    if (typed && last?.typed && ops.length === 1 && last.ops.length === 1 && 'at' in ops[0] && last.ops[0].node === ops[0].node && last.after.every((value, i) => value === sel[i])) {
      const [one, two] = [last.ops[0], ops[0]], data = one.node.data, middle = data.slice(0, two.at) + two.removed + data.slice(two.at + two.inserted.length);
      last.ops[0] = diff(one.node, middle.slice(0, one.at) + one.removed + middle.slice(one.at + one.inserted.length), data);
      last.after = after;
      return;
    }
    // History is capped at 100 steps and about 8 MB of changed text.
    const size = ops.reduce((sum, op) => sum + (op.removed?.length ?? 0) + (op.inserted?.length ?? 0) + (op.parent ? [...op.added, ...op.removed].reduce((count, node) => count + node.textContent.length, 0) : 0), 0);
    history.undo.push({ ops, sel, after, typed, size }); history.size += size;
    while (history.undo.length > 100 || history.size > 8e6 && history.undo.length > 1) history.size -= history.undo.shift().size;
  };
  // Redoes (or undoes) one change, after checking that the page left its nodes as the step left them.
  const apply = (op, forward) => {
    const check = value => { if (!value) throw new Error('The page changed the content'); };
    if ('at' in op) {
      const [from, to] = forward ? [op.removed, op.inserted] : [op.inserted, op.removed];
      check(op.node.isConnected && op.node.data.substr(op.at, from.length) === from);
      op.node.replaceData(op.at, from.length, to);
    } else if (op.parent) {
      const [gone, back] = forward ? [op.removed, op.added] : [op.added, op.removed];
      check(gone.every(node => node.parentNode === op.parent) && (!op.next || op.next.parentNode === op.parent && !gone.includes(op.next)));
      for (const node of gone) node.remove();
      for (const node of back) op.parent.insertBefore(node, op.next);
    } else {
      const value = forward ? op.value : op.old;
      if (value === null) op.element.removeAttribute(op.name); else op.element.setAttribute(op.name, value);
    }
  };
  // Undoes (or redoes) the changes of a step, or none of them: a failed check puts back the ones it made.
  const replay = (ops, forward) => {
    const done = [];
    try { for (const op of forward ? ops : [...ops].reverse()) { apply(op, forward); done.push(op); } }
    catch (error) { for (const op of done.reverse()) apply(op, !forward); throw error; }
  };

  const paragraph = (host, range) => {
    let block = range.startContainer.nodeType === 3 ? range.startContainer.parentNode : range.startContainer;
    while (block !== host && !/^(div|p|li|h[1-6]|blockquote|pre)$/.test(block.localName)) block = block.parentNode;
    const tail = document.createRange();
    tail.setStart(range.startContainer, range.startOffset);
    if (range.startContainer.nodeType === 3 && range.startOffset === range.startContainer.length) tail.setStartAfter(range.startContainer);
    // The split copies the inline ancestors of the caret. Like Chrome, the copies drop their id. A new empty line
    // keeps them around its <br>, and otherwise empty copies go.
    const extract = () => {
      let depth = 0; for (let node = range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentNode; node !== block && node !== host; node = node.parentNode) depth++;
      const fragment = tail.extractContents(), clones = [];
      for (let node = fragment.firstChild; node?.nodeType === 1 && clones.length < depth; node = node.firstChild) clones.push(node);
      if (clones.length && !fragment.textContent && !fragment.querySelector('br,img')) clones.at(-1).replaceChildren(document.createElement('br'));
      for (const node of clones.reverse()) { node.removeAttribute('id'); if (!node.textContent && !node.querySelector('br,img')) node.remove(); }
      return fragment;
    };
    if (block === host) {
      let top = range.startContainer; while (top.parentNode !== host) top = top.parentNode;
      let next = top === host ? null : top.nextSibling; while (next && !/^(div|p|li|h[1-6]|blockquote|pre|ul|ol|table)$/.test(next.localName)) next = next.nextSibling;
      if (next) tail.setEndBefore(next); else tail.setEnd(host, host.childNodes.length);
      // Warning: the new line goes into the page before its content, so that undo records where the content went.
      const fragment = extract(), line = document.createElement('div'); host.insertBefore(line, next); line.append(fragment);
      if (!line.textContent && !line.querySelector('br')) line.replaceChildren(document.createElement('br'));
      if (!host.textContent && !host.querySelector('br')) host.prepend(document.createElement('br'));
      return line;
    }
    tail.setEndAfter(block.lastChild ?? block);
    const fragment = extract(), line = document.createElement(/^(li|p)$/.test(block.localName) ? block.localName : 'div'); block.after(line); line.append(fragment);
    if (!line.textContent && !line.querySelector('br')) line.replaceChildren(document.createElement('br'));
    if (!block.textContent && !block.querySelector('br')) block.append(document.createElement('br'));
    return line;
  };
  const insertText = (range, text) => {
    const node = range.startContainer, at = range.startOffset;
    if (node.nodeType === 3) { node.insertData(at, text); return [node, at]; }
    const before = node.childNodes[at - 1], after = node.childNodes[at], created = document.createTextNode(text);
    if (before?.nodeType === 3) { const length = before.length; before.appendData(text); return [before, length]; }
    if (after?.nodeType === 3) { after.insertData(0, text); return [after, 0]; }
    const placeholder = [after, before].find(br => br?.localName === 'br' && !br.nextSibling && !br.previousSibling);
    if (placeholder) node.replaceChild(created, placeholder); else node.insertBefore(created, after ?? null);
    return [created, 0];
  };
  const histories = new WeakMap();
  const remember = (target, at, removed, inserted, typed) => {
    const history = histories.get(target) ?? histories.set(target, { undo: [], redo: [] }).get(target), last = history.undo.at(-1);
    history.redo.length = 0;
    if (typed && last?.typed && last.at + last.inserted.length === at) last.inserted += inserted;
    else if (history.undo.push({ at, removed, inserted, typed }) > 100) history.undo.shift();
  };
  let composed = null;
  // Call before active changes. End composition with its text intact and only a compositionend event.
  const finishComposition = () => {
    if (composing === null) return;
    const target = active, [at, removed, text] = composed;
    composing = null; call('dom.endComposition');
    if (terminalOf(target)) { target._preedit = ''; call('terminal.preedit', terminalOf(target), ''); return; }
    finish();
    if (!text || !target?.isConnected) return;
    if (editable(target)) remember(target, at, removed, text, !removed);
    dispatch(target, trusted(new CompositionEvent('compositionend', { bubbles: true, composed: true, data: text })));
  };
  globalThis.__appsFinishComposition = finishComposition;
  globalThis.__appsInput = (text, start, end, mark = false, selected = null, inputType, dataTransfer = null, explicit = null) => {
    const target = active;
    if (!target || disabled(target) || target.readOnly) return finishComposition();
    // Plain edits of editable content take the selection, since the text of the host walks all of it.
    const lazy = start == null && end == null && composing === null && !mark && target.isContentEditable && !editable(target) && !terminalOf(target) && selection.rangeCount > 0;
    const state = lazy ? { text: '', start: 0, end: 0 } : __appsEditing(); if (!state) return finishComposition();
    text = String(text);
    // A terminal shows the text that an input method composes at its cursor, and sends the result to its program.
    const session = terminalOf(target);
    if (session) {
      const began = composing !== null;
      if (mark && !began) dispatch(target, trusted(new CompositionEvent('compositionstart', { bubbles: true, composed: true, data: '' })));
      if (mark) dispatch(target, trusted(new CompositionEvent('compositionupdate', { bubbles: true, composed: true, data: text })));
      composing = mark ? [0, text.length] : null; composed = [0, '', mark ? text : ''];
      call('terminal.preedit', session, target._preedit = mark ? text : '');
      if (!mark && text) call('terminal.write', session, 0, text);
      if (!mark && began) dispatch(target, trusted(new CompositionEvent('compositionend', { bubbles: true, composed: true, data: text })));
      return true;
    }
    start ??= composing?.[0] ?? Math.min(state.start,state.end); end ??= composing?.[1] ?? Math.max(state.start,state.end);
    start = Math.max(0, Math.min(start, state.text.length)); end = Math.max(start, Math.min(end, state.text.length));
    const isComposing = mark || composing !== null, continuing = composing !== null, control = editable(target);
    inputType ??= isComposing ? 'insertCompositionText' : text ? 'insertText' : 'deleteContentBackward';
    if (mark && !composing) {
      composing = [start,end]; composed = [start, state.text.slice(start,end), ''];
      dispatch(target, trusted(new CompositionEvent('compositionstart', { bubbles: true, composed: true, data: '' })));
    }
    if (isComposing) dispatch(target, trusted(new CompositionEvent('compositionupdate', { bubbles: true, composed: true, data: text })));
    // Offsets at block boundaries are ambiguous. Reuse a matching selection, or locate the first replaced
    // character.
    const replaced = () => {
      if (explicit) return explicit.cloneRange();
      if (lazy) return selection.getRangeAt(0).cloneRange();
      if (selection.rangeCount && start === Math.min(state.start,state.end) && end === Math.max(state.start,state.end) && target.contains(selection.getRangeAt(0).startContainer)) return selection.getRangeAt(0).cloneRange();
      const range = document.createRange(), [node,at] = textPoint(target,start + (start < end));
      range.setStart(node,at - (start < end)); range.setEnd(...textPoint(target,end));
      return range;
    };
    const targetRanges = control ? [] : [new StaticRange(replaced())];
    const data = inputType.startsWith('delete') || ['insertParagraph','insertLineBreak','historyUndo','historyRedo'].includes(inputType) || (inputType === 'insertFromPaste' && !control) ? null : text;
    const options = { bubbles: true, composed: true, data, inputType, isComposing, dataTransfer: control ? null : dataTransfer };
    if (!dispatch(target, trusted(new InputEvent('beforeinput', { ...options, cancelable: !isComposing, targetRanges })))) return;
    if (active !== target || !target.isConnected) return finishComposition();
    // Like Chrome, a delete with nothing to delete fires no `input`.
    if ((explicit || lazy ? replaced().collapsed : start === end) && inputType.startsWith('delete')) return true;
    if (control) {
      edited(target);
      setValue(target, state.text.slice(0,start) + text + state.text.slice(end));
      target.setSelectionRange(start + (selected?.[0] ?? text.length), start + (selected?.[1] ?? text.length), target._direction, true);
    } else {
      const range = replaced(), plain = target.getAttribute('contenteditable')?.toLowerCase() === 'plaintext-only';
      begin(target, range, !isComposing && inputType === 'insertText' && !!text && range.collapsed, /Backward$/.test(inputType), continuing);
      try {
        let style = typing;
        if (!range.collapsed) {
          const block = node => { let up = node.nodeType === 3 ? node.parentNode : node; while (up !== target && getComputedStyle(up).display.startsWith('inline')) up = up.parentNode; return up; };
          const first = block(range.startContainer), last = block(range.endContainer);
          // Text that the range only touches at its edge stays as it is, so observers see no change of it.
          const origin = [range.startContainer, range.startOffset], whole = range.startContainer;
          if (whole === range.endContainer && whole.nodeType === 3 && !range.startOffset && range.endOffset === whole.length) range.selectNode(whole);
          if (range.startContainer.nodeType === 3 && range.startOffset === range.startContainer.length) range.setStartAfter(range.startContainer);
          if (range.endContainer.nodeType === 3 && !range.endOffset) range.setEndBefore(range.endContainer);
          const { startContainer: start, startOffset: offset } = range;
          style = formats(rangeNodes(range).find(part => part.end > part.start)?.node);
          range.deleteContents();
          // Like Chrome, a delete across blocks joins what is left of the last block to the first, after the inline
          // boxes that end at the join.
          if (last !== first && !last.contains(first) && last.isConnected && start.isConnected) {
            // Like Chrome, the <br> that ends the last block stays behind.
            if (last.lastChild?.localName === 'br') last.lastChild.remove();
            const moved = [...last.childNodes]; let gone = last;
            while (gone.parentNode !== target && !gone.parentNode.contains(first) && gone.parentNode.childNodes.every(node => node === gone || node.nodeType === 3 && !/[^ \t\n\r]/.test(node.data))) gone = gone.parentNode;
            let after = start.nodeType === 3 ? start : start.childNodes[offset - 1];
            while (after && !after.nextSibling && after.parentNode !== first && after.parentNode !== target) after = after.parentNode;
            // Warning: move the nodes before the block goes, so that undo records the move.
            if (after) after.after(...moved); else start.prepend(...moved);
            gone.remove();
            const next = after?.nextSibling;
            if (after?.nodeType === 3 && next?.nodeType === 3) { after.appendData(next.data); next.remove(); }
            range.setStart(start, offset); range.collapse(true);
          }
          if (origin[0].isConnected && origin[1] <= origin[0].length) { range.setStart(...origin); range.collapse(true); }
          // Text and inline elements that the delete emptied go, and the caret moves into the text beside them.
          let node = range.startContainer, at = range.startOffset;
          while (node !== target && (node.nodeType === 3 ? !node.length : emptyInline(node))) { at = index(node._m); const parent = node.parentNode; node.remove(); node = parent; }
          for (let next = node.nodeType === 1 ? node.childNodes[at] : node.nextSibling; next && (emptyInline(next) || next.nodeType === 3 && !next.length);) { const gone = next; next = next.nextSibling; gone.remove(); }
          // Like Chrome, a block that the delete emptied keeps a <br> for its line.
          if (node !== target && node.nodeType === 1 && blockTag.test(node.localName) && !node.firstChild) node.append(document.createElement('br'));
          let before = node.nodeType === 1 && node.childNodes[at - 1], after = node.nodeType === 1 && node.childNodes[at];
          while (before?.nodeType === 1 && !blockTag.test(before.localName) && before.lastChild) before = before.lastChild;
          while (after?.nodeType === 1 && !blockTag.test(after.localName) && after.firstChild) after = after.firstChild;
          if (before?.nodeType === 3) [node, at] = [before, before.length]; else if (after?.nodeType === 3) [node, at] = [after, 0];
          range.setStart(node, at); range.collapse(true);
        }
        if (inputType === 'insertParagraph' && !plain) {
          const { startContainer: before, startOffset: at } = range, inside = before.nodeType === 3 && at > 0 && at < before.length;
          const line = paragraph(target, range);
          if (before.nodeType === 3 && before.isConnected) rebalance(before, at, false, true);
          let first = line; while (first.firstChild && first.firstChild.localName !== 'br') first = first.firstChild;
          // Like Chrome, spaces that a split moves to the start of the new line become one no-break space, but a
          // text of only spaces keeps them after it. Spaces that a split leaves at the start of a text don't show,
          // so the caret goes after them.
          const lead = first.nodeType === 3 ? /^[ \t\n]*/.exec(first.data)[0].length : 0;
          if (inside && lead) first.replaceData(0, lead === first.length ? 0 : lead, ' ');
          selection.collapse(first, inside ? 0 : lead);
        }
        else if (inputType === 'insertLineBreak' && !plain) { const br = document.createElement('br'); range.insertNode(br); if (!br.nextSibling || br.nextSibling.nodeType === 3 && !br.nextSibling.data && !br.nextSibling.nextSibling) br.after(document.createElement('br')); selection.collapse(br.parentNode, br.parentNode.childNodes.indexOf(br) + 1); style = []; }
        else {
          let [node, at] = text ? insertText(range, text) : [range.startContainer, range.startOffset];
          if (node.nodeType === 3 && !plain && !mark) {
            const white = c => c === ' ' || c === ' ';
            // Like Chrome, a delete at the end of a text node rebalances the spaces that start the next text on the line.
            let next = node; while (next !== target && !next.nextSibling) next = next.parentNode;
            for (next = next !== target && at === node.length && !text && next.nextSibling; next?.firstChild;) next = next.firstChild;
            const inline = (from, to) => { for (let up = from.parentNode; !up.contains(to); up = up.parentNode) if (!getComputedStyle(up).display.startsWith('inline')) return false; return true; };
            if (next?.nodeType === 3 && inline(next, node) && inline(node, next)) rebalance(next, 0, false, true);
            else at = rebalance(node, at, white(text[0]), !text);
            if (text) rebalance(node, at + text.length, white(text.at(-1)), false);
          }
          // The typing style wraps the new text in the formatting that is not around it yet.
          const names = text && !isComposing && node.nodeType === 3 && new Set(formats(node).map(copy => copy.localName)), wrap = names ? style.filter(copy => !names.has(copy.localName)) : [];
          if (wrap.length) {
            const copy = element => { const node = element.cloneNode(false); node.removeAttribute('id'); return node; }, wrapper = copy(wrap[0]); let inner = wrapper;
            for (const element of wrap.slice(1)) inner = inner.appendChild(copy(element));
            const middle = at ? node.splitText(at) : node;
            if (middle.length > text.length) middle.splitText(text.length);
            middle.before(wrapper); inner.append(middle); node = middle; at = 0;
          }
          if (node.nodeType === 3) selection.setBaseAndExtent(node, at + (selected?.[0] ?? text.length), node, at + (selected?.[1] ?? text.length)); else selection.collapse(node, at);
          if (text) style = [];
        }
        typing = style;
      } finally { if (mark) pause(); else finish(); }
    }
    if (!mark && control && !inputType.startsWith('history')) {
      const [at, removed] = isComposing ? composed ?? [start, ''] : [start, state.text.slice(start,end)];
      remember(target, at, removed, control || !/^insert(Paragraph|LineBreak)$/.test(inputType) ? text : '', !removed && /^insert(Text|CompositionText)$/.test(inputType));
    }
    if (isComposing) { composing = [start,start+text.length]; composed[2] = text; }
    dispatch(target, trusted(new InputEvent('input', options)));
    if (isComposing && !mark) {
      composing = null;
      dispatch(target, trusted(new CompositionEvent('compositionend', { bubbles: true, composed: true, data: text })));
    }
    return true;
  };
  // Entries hold offsets into the text, so a change that bypassed the history clears it.
  const undo = redo => {
    const host = active, history = host && histories.get(host), entry = history?.[redo ? 'redo' : 'undo'].pop(), inputType = redo ? 'historyRedo' : 'historyUndo';
    if (entry?.ops) {
      if (!dispatch(host, trusted(new InputEvent('beforeinput', { bubbles: true, cancelable: true, composed: true, inputType, data: null })))) { history[redo ? 'redo' : 'undo'].push(entry); return false; }
      try { replay(entry.ops, redo); } catch { history.undo.length = history.redo.length = history.size = 0; return false; }
      const [anchor, at, focus, to] = redo ? entry.after : entry.sel;
      try { selection.setBaseAndExtent(anchor, at, focus, to); } catch { selection.collapse(host, 0); }
      entry.typed = false;
      history[redo ? 'undo' : 'redo'].push(entry);
      dispatch(host, trusted(new InputEvent('input', { bubbles: true, composed: true, inputType, data: null })));
      return true;
    }
    const state = __appsEditing();
    if (!entry || !state) return false;
    const [from, to] = redo ? [entry.removed, entry.inserted] : [entry.inserted, entry.removed];
    if (state.text.slice(entry.at, entry.at + from.length) !== from) { history.undo.length = history.redo.length = 0; return false; }
    const done = !!__appsInput(to, entry.at, entry.at + from.length, false, null, redo ? 'historyRedo' : 'historyUndo');
    entry.typed = false;
    history[done !== redo ? 'redo' : 'undo'].push(entry);
    return done;
  };
  // The editing state without the text of editable content, which costs a walk of the whole host.
  const editingState = () => active?.isContentEditable && !editable(active) && !terminalOf(active) ? { text: '', start: 0, end: selection.rangeCount && !selection.isCollapsed ? 1 : 0, rich: true } : __appsEditing();
  const edit = (command, text = '') => {
    const target = active, state = editingState();
    if (command === 'undo' || command === 'redo') return undo(command === 'redo');
    if (command === 'selectAll') {
      if (editable(target)) target.select();
      else if (target?.isContentEditable) { selection.collapse(target, 0); selection.modify('move', 'backward', 'documentboundary'); selection.modify('extend', 'forward', 'documentboundary'); }
      else if (state) selection.selectAllChildren(target);
      else { const root = selection.anchorNode?.getRootNode(); selection.selectAllChildren(root?.host ? root : document.body); }
      return true;
    }
    if (command === 'unselect') { if (editable(target)) target.setSelectionRange(target._end ?? 0, target._end ?? 0, 'none', true); else selection.removeAllRanges(); return true; }
    if (!state) return false;
    const forward = command === 'forwardDelete', [start, end] = state.rich ? [] : [Math.min(state.start,state.end), Math.max(state.start,state.end)];
    if (command === 'insertText' || command === 'replace') return !!__appsInput(text, start, end, false, null, command === 'replace' ? 'insertReplacementText' : 'insertText');
    if (command === 'insertParagraph' || command === 'insertLineBreak') return active.localName !== 'input' && !!__appsInput('\n', start, end, false, null, command);
    return remove(state, forward, 'character');
  };
  // Deletes the selection, or else what the same move with Shift would select. In editable content that is a
  // move by the layout, so it joins blocks like Chrome.
  const remove = (state, forward, unit) => {
    const text = state.text, start = Math.min(state.start, state.end), end = Math.max(state.start, state.end), line = unit === 'lineboundary';
    const type = 'delete' + (line ? 'SoftLine' : unit === 'word' ? 'Word' : 'Content') + (forward ? 'Forward' : 'Backward');
    if (state.rich) {
      if (!selection.rangeCount) return false;
      let range = selection.getRangeAt(0).cloneRange();
      const point = range.collapsed && selection._point('extend', forward ? 'forward' : 'backward', unit), caret = [selection.focusNode, selection.focusOffset];
      if (point) { range = document.createRange(); range.setStart(...forward ? caret : point.slice(0, 2)); range.setEnd(...forward ? point.slice(0, 2) : caret); }
      return !!__appsInput('', null, null, false, null, type, null, range);
    }
    let from = start, to = end;
    if (start === end && forward) to = line ? (text.indexOf('\n', end) + 1 || text.length + 1) - 1 : textBoundary(text, end, true, unit === 'word');
    else if (start === end) from = line ? text.lastIndexOf('\n', start - 1) + 1 : textBoundary(text, start, false, unit === 'word');
    return !!__appsInput('', from, to, false, null, type);
  };

  globalThis.__appsEdit = (command, text = '') => command === 'copy' || command === 'cut' ? __appsClipboard(command) : void (command === 'paste' || command === 'pasteAndMatchStyle' ? __appsClipboard('paste', text) : edit(command, text));
  const editCommands = new Map(['copy', 'cut', 'paste', 'selectAll', 'insertText', 'insertParagraph', 'insertLineBreak', 'delete', 'forwardDelete', 'undo', 'redo'].map(name => [name.toLowerCase(), name]));
  // Like browsers, page scripts reach the clipboard only while they handle user input.
  const enabled = command => {
    const state = editingState(), ranged = state ? state.start !== state.end : !selection.isCollapsed, writable = !!state && !active.readOnly && !disabled(active);
    return (command === 'selectAll' || (command === 'undo' || command === 'redo' ? !!histories.get(active)?.[command].length : command === 'copy' ? ranged : writable && (command !== 'cut' || ranged))) && (!/^(copy|cut|paste)$/.test(command) || native > 0);
  };
  const openWindow = globalThis.open = (url = '', target = '', features = '') => {
    [url, target, features] = [url, target, features].map(String);
    try { url = url && new URL(url, document.baseURI).href; } catch { throw new DOMException(`Invalid URL '${url}'`, 'SyntaxError'); }
    if (/^_(self|top|parent)$/i.test(target)) { if (url) location.assign(url); return globalThis; }
    const id = call('dom.open', 'open', url, target, features, features ? 'new-window' : 'foreground-tab');
    return id === null ? null : Object.freeze({ get closed() { return call('dom.open', 'closed', id); }, close() { call('dom.open', 'close', id); } });
  };
  Object.assign(Document.prototype, {
    execCommand(name, _ui, value) {
      const command = editCommands.get(String(name).toLowerCase());
      if (!command || !enabled(command)) return false;
      if (command === 'paste') __appsClipboard('paste', call('dom.clipboard') ?? '');
      else if (command === 'copy' || command === 'cut') { const text = __appsClipboard(command); if (text) call('dom.clipboard', text); }
      else edit(command, value === undefined ? '' : String(value));
      return true;
    },
    queryCommandSupported: name => editCommands.has(String(name).toLowerCase()),
    queryCommandEnabled(name) { const command = editCommands.get(String(name).toLowerCase()); return !!command && enabled(command); },
  });
  globalThis.__appsNativeControl = (id, value, done) => {
    const target = wrap(id);
    if (!target || disabled(target)) return;
    if (value === null) return void fireClick(target, { detail: 1 }, true);
    if (target.type === 'range') value = rangeValue(target, value);
    if (value !== target.value) { edited(target); setValue(target, value); dispatch(target, trusted(new Event('input', { bubbles: true, composed: true }))); }
    if (done) commit(target);
  };
  globalThis.__appsFrameEvent=(id,event)=>{const target=wrap(id);if(event.type==='did-finish-load')dispatch(target,new Event('load'));else if(event.type==='did-fail-load')dispatch(target,new Event('error'));else if(event.type==='message')dispatch(target,new MessageEvent('message',{data:event.data,origin:'null'}));};
  globalThis.__appsClipboard = (type, text = '') => {
    const target = active ?? document.body, data = new DataTransfer();
    if (type !== 'paste' && target.localName === 'input' && target.type === 'password') return '';
    if (type === 'paste') data.setData('text/plain', text);
    if (dispatch(target, trusted(new ClipboardEvent(type, { bubbles: true, cancelable: true, composed: true, clipboardData: data })))) {
      const state = __appsEditing();
      if (terminalOf(target)) { if (type === 'paste') call('terminal.write', terminalOf(target), 1, text); else data.setData('text/plain', call('terminal.text', terminalOf(target), true) ?? ''); }
      else if (type === 'paste') { if (state) __appsInput(text, undefined, undefined, false, null, 'insertFromPaste', data); }
      else {
        data.setData('text/plain', state ? state.text.slice(Math.min(state.start,state.end), Math.max(state.start,state.end)) : selection.toString());
        if (type === 'cut' && state && state.start !== state.end) __appsInput('', undefined, undefined, false, null, 'deleteByCut');
      }
    }
    return data.getData('text/plain');
  };
  const fileType=name=>({'png':'image/png','jpg':'image/jpeg','jpeg':'image/jpeg','webp':'image/webp','gif':'image/gif','svg':'image/svg+xml','txt':'text/plain','md':'text/markdown','json':'application/json','pdf':'application/pdf','html':'text/html','htm':'text/html','css':'text/css','js':'text/javascript','csv':'text/csv','xml':'text/xml','zip':'application/zip','mp3':'audio/mpeg','wav':'audio/wav','mp4':'video/mp4','webm':'video/webm'})[name.split('.').at(-1).toLowerCase()]??'application/octet-stream';
  const picks=new Map();let nextPick=0;
  pickFiles=input=>{if(!native)return;const id=++nextPick;picks.set(id,input);call('dom.filePicker',{id,multiple:input.multiple,directory:input.hasAttribute('webkitdirectory'),accept:input.getAttribute('accept')??''});};
  globalThis.__appsFilesPicked=(id,files)=>{const input=picks.get(id);picks.delete(id);if(!input)return;if(!files){dispatch(input,new Event('cancel',{bubbles:true}));return;}input._files=fileList(files.map(({name,lastModified,data})=>new File([Uint8Array.fromBase64(data)],name,{lastModified,type:fileType(name)})));showFiles(input);dispatch(input,new Event('input',{bubbles:true,composed:true}));dispatch(input,new Event('change',{bubbles:true}));};
  const pickOption=(select,option)=>{if(option._current)return;for(const other of options(select))other._selected=other===option;select._none=false;selectedness(select);revalidate(select);dispatch(select,trusted(new Event('input',{bubbles:true,composed:true})));interacted(select);dispatch(select,trusted(new Event('change',{bubbles:true})));};
  openSelect=select=>{
    if(!native||!navigator.platform.includes('Mac')||select.multiple||select.size>1||disabled(select))return;
    const id=++nextPick,list=[],items=[],rect=select.getBoundingClientRect();let selected=-1;
    const add=(option,group)=>{if(option.hidden)return;if(option._current)selected=items.length;list[items.length]=option;items.push([(group?'  ':'')+option.label,+disabled(option)]);};
    selectedness(select);
    for(const kid of select.children){if(kid.localName==='option')add(kid);else if(kid.localName==='hr')items.push(null);else if(kid.localName==='optgroup'&&!kid.hidden){items.push([kid.label,1]);for(const option of kid.children)if(option.localName==='option')add(option,true);}}
    picks.set(id,[select,list]);call('dom.selectPopup',{id,x:rect.x,y:rect.y,width:rect.width,selected,items});
  };
  // The menu takes the pointerup of the press that opened it.
  globalThis.__appsSelectPicked=(id,index)=>{const [select,list]=picks.get(id)??[];picks.delete(id);if(press&&select?.contains(press.target)){press=null;pointers.delete(1);call('dom.active',[]);}if(list?.[index]&&select.isConnected)pickOption(select,list[index]);};
  class ClipboardItem {
    constructor(items) { this._items = { ...items }; this.types = Object.keys(this._items); this.presentationStyle = 'unspecified'; }
    async getType(type) { if (!(type in this._items)) throw new DOMException('The type was not found', 'NotFoundError'); const value = await this._items[type]; return value instanceof Blob ? value : new Blob([value], { type }); }
    static supports(type) { return type === 'text/plain'; }
  }
  const clipboardText = text => { const result = call('dom.clipboard', ...(text === undefined ? [] : [text])); if (result === null) throw new DOMException('The clipboard needs user input', 'NotAllowedError'); return result; };
  class Clipboard extends EventTarget {
    async writeText(text) { clipboardText(String(text)); }
    async readText() { return clipboardText(); }
    write(items) { const item = items[0]; if (!item?.types.includes('text/plain')) return Promise.reject(new DOMException('Only text can be written to the clipboard', 'NotAllowedError')); return item.getType('text/plain').then(value => value.text()).then(text => { clipboardText(text); }); }
    async read() { const text = clipboardText(); return [new ClipboardItem({ 'text/plain': new Blob([text], { type: 'text/plain' }) })]; }
  }
  Object.defineProperty(navigator, 'clipboard', { value: new Clipboard(), configurable: true, enumerable: true });
  globalThis.ClipboardItem = ClipboardItem; globalThis.Clipboard = Clipboard;
  let dragTarget = null, dragData = null, dragAccepted = false;
  const dragEvent = (type, target, x, y, relatedTarget = null, dataTransfer = dragData) => { const event = trusted(new DragEvent(type, { bubbles: true, cancelable: type !== 'dragleave', composed: true, clientX: x, clientY: y, relatedTarget, dataTransfer })); dispatch(target, event); return event; };
  globalThis.__appsDrag = (kind, x, y, names) => {
    if (kind === 'leave') { if (dragTarget) dragEvent('dragleave', dragTarget, x, y); dragTarget = dragData = null; return; }
    if (kind === 'enter') { dragData = new DataTransfer(); for (const name of names) dragData.items.push({ kind: 'file', type: fileType(name), getAsFile: () => null, getAsString: () => {}, webkitGetAsEntry: () => null }); dragTarget = null; }
    if (!dragData) return;
    const target = hitAt(x, y) ?? document.body;
    if (target !== dragTarget) { const previous = dragTarget; dragEvent('dragenter', target, x, y, previous); if (previous) dragEvent('dragleave', previous, x, y, target); dragTarget = target; }
    dragAccepted = dragEvent('dragover', dragTarget, x, y).defaultPrevented;
  };
  globalThis.__appsDrop = (files, x, y) => {
    const target = hitAt(x, y) ?? document.body, previous = dragTarget, accepted = dragAccepted;
    dragTarget = dragData = null; dragAccepted = false;
    if (!accepted) { if (previous) dragEvent('dragleave', previous, x, y, null, new DataTransfer()); return; }
    const data = new DataTransfer();
    for (const value of files) { const file = new File([new Uint8Array(value.bytes)], value.name, { lastModified: value.lastModified, type: value.directory ? '' : fileType(value.name) }); droppedFiles.set(file, value.file); if (value.directory) droppedDirectories.add(file); data.items.add(file); }
    dragEvent('drop', target, x, y, null, data);
  };
  const nativeKeys = { backspace:'Backspace', delete:'Delete', enter:'Enter', escape:'Escape', tab:'Tab', left:'ArrowLeft', right:'ArrowRight', up:'ArrowUp', down:'ArrowDown', home:'Home', end:'End', pageup:'PageUp', pagedown:'PageDown', insert:'Insert', space:' ', shift:'Shift', control:'Control', ctrl:'Control', alt:'Alt', command:'Meta', super:'Meta' };
  const keyCodes = { Backspace:8, Tab:9, Enter:13, Shift:16, Control:17, Alt:18, Escape:27, ' ':32, PageUp:33, PageDown:34, End:35, Home:36, ArrowLeft:37, ArrowUp:38, ArrowRight:39, ArrowDown:40, Insert:45, Delete:46, Meta:91, Process:229 };
  let modifiers = {}, graphemes, words, sentences;
  const space = /\s/;
  const textBoundary = (text, offset, forward, word = false) => {
    if (forward ? offset >= text.length : offset <= 0) return offset;
    if (word) {
      let at = offset;
      if (forward) { while (at < text.length && space.test(text[at])) at++; while (at < text.length && !space.test(text[at])) at++; }
      else { while (at > 0 && space.test(text[at - 1])) at--; while (at > 0 && !space.test(text[at - 1])) at--; }
      return at;
    }
    graphemes ??= new Intl.Segmenter(undefined, { granularity: 'grapheme' });
    const segment = graphemes.segment(text).containing(forward ? offset : offset - 1);
    return forward ? segment.index + segment.segment.length : segment.index;
  };
  const selectionExtent = (text, offset, granularity) => {
    if (granularity === 3) {
      const end = text.indexOf('\n',offset);
      return [offset ? text.lastIndexOf('\n',offset - 1) + 1 : 0, end < 0 ? text.length : end + 1];
    }
    words ??= new Intl.Segmenter(undefined, { granularity: 'word' });
    const word = words.segment(text).containing(Math.max(0,Math.min(offset,text.length - 1)));
    return word ? [word.index,word.index + word.segment.length] : [offset,offset];
  };
  const selectionRange = (node, offset, granularity, mode, boundary) => {
    const range = document.createRange();
    if (mode === 'all') range.selectNodeContents(wrap(boundary));
    else if (mode === 'none') range.selectNode(wrap(boundary));
    else if (granularity > 1) {
      let block = node.nodeType === 3 ? node.parentElement : node;
      while (block.parentElement && (getComputedStyle(block).display || (/^(span|a|b|strong|em|i|code|small)$/.test(block.localName) ? 'inline' : 'block')).startsWith('inline')) block = block.parentElement;
      const [start,end] = selectionExtent(block.textContent,textOffset(block,node,offset),granularity);
      range.setStart(...textPoint(block,start)); range.setEnd(...textPoint(block,end));
    } else { range.setStart(node,offset); range.collapse(true); }
    return range;
  };
  // As in Chrome, a label shares its hover and press with its control.
  const labeled = list => { const label = list.find(node => node.localName === 'label'), control = label && labelControl(label); return control && !list.includes(control) ? [...list, control] : list; };
  // `between` runs after the pointer events, before the mouse events.
  const hover = (target, data, between) => {
    if (target === hovered) return;
    const previous = hovered, from = previous ? path(previous) : [], to = target ? path(target) : [];
    hovered = target;
    call('dom.hover', labeled(to).map(node => node._id));
    for (const [kind, Type] of [['pointer', PointerEvent], ['mouse', MouseEvent]]) {
      const fire = (node, type, ancestors, relatedTarget, bubbles) => { const event = trusted(new Type(kind + type, { ...data, button: kind === 'pointer' ? -1 : 0, detail: 0, bubbles, cancelable: bubbles, composed: bubbles, relatedTarget })); eventState(event).which = 0; return dispatch(node, event, ancestors); };
      if (previous) fire(previous, 'out', from.slice(), target, true);
      from.forEach((node, index) => { if (node.nodeType !== 11 && !to.includes(node)) fire(node, 'leave', from.slice(index), target, false); });
      if (target) fire(target, 'over', to.slice(), previous, true);
      for (let index = to.length; index--;) if (to[index].nodeType !== 11 && !from.includes(to[index])) fire(to[index], 'enter', to.slice(index), previous, false);
      if (kind === 'pointer') between?.();
    }
  };
  let press = null, last = null, sliding = null, hoverCheck = null, scrollStart = null;
  // As in Chrome, capture changes take effect before the next pointer event, and after `pointerup` ends the capture.
  const settleCapture = data => {
    const pending = captures.get(1), current = gotten.get(1);
    if (pending === current) return;
    if (current) { gotten.delete(1); dispatch(current, trusted(new PointerEvent('lostpointercapture', mouseInit(data, { cancelable: false })))); }
    if (pending) { gotten.set(1, pending); dispatch(pending, trusted(new PointerEvent('gotpointercapture', mouseInit(data, { cancelable: false })))); }
  };
  // As in Chrome, the hovered element follows DOM, layout and scroll changes under a still pointer, at most every 100 ms.
  // A layout without DOM changes, like a transition, checks again until the layouts stop. During a press, a DOM change
  // lets a pending pointer capture take effect.
  let hoverLayout = 0;
  const checkHover = () => {
    hoverCheck = null;
    if (!last || !hovered || pointers.size && !captures.has(1)) return;
    const data = { clientX: last[0], clientY: last[1], buttons: pointers.size }, capture = captures.get(1), hit = capture ?? hitAt(last[0], last[1]);
    native++; try { if (!capture) settleCapture(data); hover(hit?.nodeType === 3 ? hit.parentElement : hit, mouseInit(data), () => settleCapture(data)); settleCapture(data); } finally { native--; }
    hoverVersion = version; hoverLayout = moves[0];
  };
  const pointerLayout = globalThis.__appsLayout;
  globalThis.__appsLayout = () => {
    pointerLayout();
    if (hoverCheck || !last || !hovered || hoverVersion === version && (pointers.size || hoverLayout === moves[0] && !geometry[0]) || pointers.size && !captures.has(1)) return;
    hoverCheck = setTimeout(checkHover, 100);
  };
  const slide = (node, x) => { const rect = node.getBoundingClientRect(), thumb = Math.min(rect.height, 16), [min, max] = rangeBounds(node); setRange(node, min + Math.min(1, Math.max(0, (x - rect.left - thumb / 2) / (rect.width - thumb || 1))) * (max - min)); };
  const mouseInit = (data, extra) => ({ bubbles: true, cancelable: true, composed: true, view: window, clientX: data.clientX, clientY: data.clientY, screenX: data.screenX ?? data.clientX, screenY: data.screenY ?? data.clientY, ctrlKey: !!data.ctrlKey, metaKey: !!data.metaKey, altKey: !!data.altKey, shiftKey: !!data.shiftKey, button: data.button ?? 0, buttons: data.buttons ?? 0, detail: data.detail ?? 0, movementX: last ? data.clientX - last[0] : 0, movementY: last ? data.clientY - last[1] : 0, pointerId: 1, pointerType: 'mouse', isPrimary: true, ...extra });
  // Queue the end event after finished-promise reactions, as browsers do.
  globalThis.__appsDispatch = (name, ids, data = {}) => { const run = () => { native++; try { return nativeEvent(name, ids, data); } finally { native--; } }; if (/^(animation|transition)(end|cancel)$/.test(name) && cssEvent(name, ids[0], data)) { microtask(run); return true; } return run(); };
  const nativeEvent = (name, ids, data) => {
    if (name === 'pointerexit') { hover(null, data); last = null; return true; }
    if (name === 'fullscreen:exit') { if (fullscreens.length) exitFullscreen(true, true); return true; }
    if (name === 'window:focus' || name === 'window:blur') {
      const focused = name === 'window:focus', target = active;
      if (windowFocused === focused) return true;
      windowFocused = focused;
      if (!focused) finishComposition();
      if (target) call('dom.focus', path(target).map(node => node._id), ring, !focused);
      if (data.quiet) return true;
      if (target && !focused) { commit(target); dispatch(target, trusted(new FocusEvent('blur', { view: window, composed: true }))); if (active === target) dispatch(target, trusted(new FocusEvent('focusout', { view: window, bubbles: true, composed: true }))); }
      dispatch(globalThis, new Event(name.slice(7)));
      if (target && focused && active === target) { dispatch(target, trusted(new FocusEvent('focus', { view: window, composed: true }))); if (active === target) dispatch(target, trusted(new FocusEvent('focusin', { view: window, bubbles: true, composed: true }))); }
      return true;
    }
    if (name.startsWith('window:')) return dispatch(globalThis, new Event(name.slice(7)));
    if (/^(animation|transition)(start|end|run|cancel|iteration)$/.test(name)) { const target = wrap(ids[0]); return !!target && dispatch(target, trusted(new (name[0] === 'a' ? AnimationEvent : TransitionEvent)(name, { bubbles: true, composed: true, cancelable: name[0] === 't', ...data }))); }
    if (name === 'modifiers') {
      const previous = modifiers; modifiers = data;
      for (const [field,key] of Object.entries({ shiftKey:'Shift', ctrlKey:'Control', altKey:'Alt', metaKey:'Meta' })) {
        if (!!previous[field] !== !!data[field]) nativeEvent(data[field] ? 'keydown' : 'keyup', [], { ...data, key, code: key + 'Left', location: 1, keyCode: keyCodes[key] });
      }
      return true;
    }
    if (name === 'keydown' || name === 'keyup') {
      if (name === 'keydown' && !hadKey && !data.metaKey && !data.ctrlKey && !data.altKey) { hadKey = true; if (active && !ring) call('dom.focus', path(active).map(node => node._id), ring = true, !windowFocused); }
      const target = active ?? document.body; if (!target) return false;
      if (data.nativeKey !== undefined) {
        const raw = data.nativeKey, upper = raw.toUpperCase(), named = nativeKeys[raw] ?? (/^f\d{1,2}$/.test(raw) ? upper : null);
        const code = /^[a-z]$/i.test(raw) ? 'Key' + upper : /^\d$/.test(raw) ? 'Digit' + raw : ({ ' ':'Space', space:'Space', '-':'Minus', '=':'Equal', '[':'BracketLeft', ']':'BracketRight', '\\':'Backslash', ';':'Semicolon', "'":'Quote', ',':'Comma', '.':'Period', '/':'Slash', '`':'Backquote' })[raw] ?? named ?? '';
        // Like Chrome, a key that the input method took has key code 229, and its key is `Process` except on macOS.
        const { ime, ...rest } = data, key = ime && !navigator.platform.includes('Mac') ? 'Process' : named ?? data.keyChar ?? (data.shiftKey ? upper : raw);
        data = { ...rest, key, code, keyCode: ime ? 229 : keyCodes[key] ?? (/^[a-z0-9]$/i.test(raw) ? upper.charCodeAt(0) : /^F\d+$/.test(code) ? 111 + Number(code.slice(1)) : 0) };
      }
      if (name === 'keydown' && data.key === 'Escape' && fullscreens.length) { exitFullscreen(true); return false; }
      const event = trusted(new KeyboardEvent(name, { bubbles: true, cancelable: true, composed: true, isComposing: composing !== null, ...data }));
      if (!dispatch(target, event)) return false;
      if (event.isComposing || event.keyCode === 229) return true;
      // Like Chrome, keys that type a character, and Enter, fire `keypress`. Canceling it stops the text.
      if (name === 'keydown' && !event.metaKey && !event.ctrlKey && (event.key === 'Enter' || [...event.key].length === 1)) {
        const { key, code, location, repeat, shiftKey, altKey } = event, char = key === 'Enter' ? 13 : key.codePointAt(0);
        if (!dispatch(target, trusted(new KeyboardEvent('keypress', { bubbles: true, cancelable: true, composed: true, key, code, location, repeat, shiftKey, altKey, keyCode: char, charCode: char })))) return false;
      }
      const plain = !event.metaKey && !event.ctrlKey && !event.altKey, tag = target.localName, type = tag === 'input' ? target.type : '';
      const session = data.nativeKey !== undefined && terminalOf(target);
      if (session) {
        const mac = navigator.platform.includes('Mac'), lower = event.key.toLowerCase(), down = name === 'keydown';
        // Copy and paste are Cmd+C and Cmd+V on macOS, and Ctrl+Shift+C and Ctrl+Shift+V elsewhere, because Ctrl+C is for the program.
        if (!mac && down && event.ctrlKey && event.shiftKey && (lower === 'c' || lower === 'v')) { if (lower === 'v') __appsClipboard('paste', call('dom.clipboard') ?? ''); else { const text = __appsClipboard('copy'); if (text) call('dom.clipboard', text); } return false; }
        if (mac && down && event.metaKey && !event.ctrlKey && !event.altKey && lower === 'a') { call('terminal.control', session, 4, 0); return false; }
        if (!(mac && event.metaKey) && call('terminal.key', session, down ? event.repeat ? 2 : 1 : 0, data.nativeKey, data.keyChar ?? '', terminalMods(event) | (target.hasAttribute('option-as-alt') ? 16 : 0))) return false;
        if (!down) return true;
      }
      if (name === 'keyup') {
        if (event.key === ' ' && plain && (tag === 'button' || /^(button|submit|reset|image|checkbox)$/.test(type) || type === 'radio' && !target.checked || tag === 'summary') && !disabled(target)) fireClick(target, { detail: 0 }, true);
        return true;
      }
      const state = editingState(), mac = navigator.platform.includes('Mac'), primary = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey, lower = event.key.toLowerCase();
      if (primary && !event.altKey && lower === 'a') { edit('selectAll'); return false; }
      if (state) {
        const start = Math.min(state.start,state.end), end = Math.max(state.start,state.end);
        if (primary && !event.altKey && (lower === 'z' || !mac && !event.shiftKey && lower === 'y')) { edit(event.shiftKey || lower === 'y' ? 'redo' : 'undo'); return false; }
        const backward = event.key === 'Backspace' || (mac && event.ctrlKey && !event.metaKey && !event.altKey && event.key === 'h');
        const forward = event.key === 'Delete' || (mac && event.ctrlKey && !event.metaKey && !event.altKey && event.key === 'd');
        if (backward || forward) {
          const word = mac ? event.altKey && !event.metaKey && !event.ctrlKey : event.ctrlKey && !event.altKey && !event.metaKey;
          const line = mac && event.metaKey && !event.ctrlKey && !event.altKey;
          if (word || line || (!event.altKey && !event.metaKey && (!event.ctrlKey || mac))) {
            remove(state, forward, line ? 'lineboundary' : word ? 'word' : 'character');
            return false;
          }
        }
        if (event.key === 'Enter' && plain && (tag === 'textarea' || target.isContentEditable)) {
          __appsInput('\n', state.rich ? null : start, state.rich ? null : end, false, null, tag === 'textarea' || event.shiftKey ? 'insertLineBreak' : 'insertParagraph');
          return false;
        }
        if (['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(event.key)) {
          const word = mac ? event.altKey : event.ctrlKey, documentEdge = mac ? event.metaKey : event.ctrlKey;
          const horizontal = ['ArrowLeft','ArrowRight'].includes(event.key), pos = state.end;
          // Like Chrome, left and right follow the direction of the block.
          const right = ['ArrowRight','ArrowDown','End'].includes(event.key) !== (horizontal && rtlBlock(editable(target) ? target : selection.focusNode));
          const unit = documentEdge && !horizontal || mac && ['Home','End'].includes(event.key) ? 'documentboundary' : ['Home','End'].includes(event.key) || mac && event.metaKey && horizontal ? 'lineboundary' : horizontal ? (word ? 'word' : 'character') : 'line';
          if (!editable(target)) { selection.modify(event.shiftKey ? 'extend' : 'move', right ? 'forward' : 'backward', unit); return false; }
          let next = pos;
          if (unit === 'documentboundary') next = right ? state.text.length : 0;
          else if (unit === 'lineboundary') next = right ? (state.text.indexOf('\n',pos) < 0 ? state.text.length : state.text.indexOf('\n',pos)) : pos === 0 ? 0 : state.text.lastIndexOf('\n',pos - 1) + 1;
          else if (!event.shiftKey && start !== end && horizontal && !word) next = right ? end : start;
          else if (horizontal) next = textBoundary(state.text,pos,right,word);
          else {
            const lineStart = state.text.lastIndexOf('\n',pos - 1) + 1, column = pos - lineStart;
            const lineEnd = state.text.indexOf('\n',pos);
            if (right) { const nextEnd = state.text.indexOf('\n',lineEnd + 1); next = lineEnd < 0 ? state.text.length : Math.min(lineEnd + 1 + column,nextEnd < 0 ? state.text.length : nextEnd); }
            else { const previousStart = state.text.lastIndexOf('\n',lineStart - 2) + 1; next = lineStart === 0 ? 0 : Math.min(previousStart + column,lineStart - 1); }
          }
          const anchor = event.shiftKey ? state.start : next;
          target.setSelectionRange(Math.min(anchor,next), Math.max(anchor,next), next < anchor ? 'backward' : next > anchor || !mac ? 'forward' : 'none', true);
          return false;
        }
      }
      if (event.key === 'Enter' && plain && tag === 'input') {
        if (editable(target)) dispatch(target, trusted(new InputEvent('beforeinput', { bubbles: true, cancelable: true, composed: true, inputType: 'insertLineBreak', data: null })));
        commit(target);
        const form = target.form;
        if (form && active === target) { const submit = controlsOf(form).find(control => control.localName === 'button' ? control.type === 'submit' : /^(submit|image)$/.test(control.type)); if (submit) { if (!disabled(submit)) fireClick(submit, { detail: 0 }, true); } else if ([...form.elements].filter(control => editable(control) && control.localName === 'input').length === 1) submitForm(form, null); }
        return false;
      }
      if (event.key === 'Enter' && plain && (tag === 'button' || /^(button|submit|reset|image)$/.test(type) || (tag === 'a' || tag === 'area') && target.hasAttribute('href') || tag === 'summary') && !disabled(target)) { fireClick(target, { detail: 0 }, true); return false; }
      if (event.key === 'Escape' && watchers.length) {
        const top = watchers.at(-1);
        if (top._popover) hidePopover(top, true, true);
        else if (top.closedBy !== 'none' && dispatch(top, trusted(new Event('cancel', { cancelable: true })))) top.close();
        return false;
      }
      if (tag === 'select' && plain && event.key === ' ') { openSelect(target); return false; }
      if (tag === 'select' && plain && !disabled(target) && /^(ArrowUp|ArrowDown|Home|End|PageUp|PageDown)$/.test(event.key) || tag === 'select' && plain && event.key.length === 1 && event.key !== ' ') {
        const list = options(target).filter(option => !disabled(option)), index = list.findIndex(option => option._current);
        const next = event.key.length === 1 ? list.find((option, i) => i > index && option.text.toLowerCase().startsWith(event.key.toLowerCase())) ?? list.find(option => option.text.toLowerCase().startsWith(event.key.toLowerCase())) : list[event.key === 'Home' || event.key === 'PageUp' ? 0 : event.key === 'End' || event.key === 'PageDown' ? list.length - 1 : Math.max(0, Math.min(list.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
        if (next) pickOption(target, next);
        return false;
      }
      if (tag === 'input' && type === 'range' && plain && !disabled(target) && /^(Arrow(Up|Down|Left|Right)|Home|End|Page(Up|Down))$/.test(event.key)) {
        const [min, max, step] = rangeBounds(target), unit = step || (max - min) / 100, key = event.key, before = target.value;
        setRange(target, key === 'Home' ? min : key === 'End' ? max : Number(before) + (/Up|Right/.test(key) ? 1 : -1) * (key.startsWith('Page') ? Math.max(unit, (max - min) / 10) : unit));
        if (target.value !== before) { interacted(target); dispatch(target, trusted(new Event('change', { bubbles: true }))); }
        return false;
      }
      if (type === 'radio' && plain && /^Arrow(Up|Down|Left|Right)$/.test(event.key)) {
        const list = group(target).filter(radio => radio === target || focusable(radio)), back = /Up|Left/.test(event.key) !== (/Left|Right/.test(event.key) && getComputedStyle(target).direction === 'rtl');
        const next = list[(list.indexOf(target) + (back ? -1 : 1) + list.length) % list.length];
        if (next !== target) { next.focus(); fireClick(next, { detail: 0 }, true); }
        return false;
      }
      const sign = plain && (!event.shiftKey || event.key === ' ') && !editable(target) && !target.isContentEditable && tag !== 'select' && !(event.key === ' ' && activation(target)) ? { ' ': event.shiftKey ? -1 : 1, PageDown: 1, PageUp: -1, ArrowDown: 1, ArrowUp: -1, ArrowRight: 1, ArrowLeft: -1, Home: -1, End: 1 }[event.key] : 0;
      if (sign) {
        // Like Chrome: the focused box, or else the box of the last press, or the nearest ancestor that can scroll that way. Then the page.
        const x = /Left|Right/.test(event.key), root = document.scrollingElement;
        let box = active ?? scrollStart;
        for (; box && box !== root && box.nodeType === 1; box = box.parentElement) {
          const style = getComputedStyle(box), at = x ? box.scrollLeft : box.scrollTop, range = x ? box.scrollWidth - box.clientWidth : box.scrollHeight - box.clientHeight;
          // A box that scrolls from its end has offsets from -range to 0.
          const flex = style.display.endsWith('flex'), low = (x ? (style.direction === 'rtl') !== (flex && style.flexDirection === 'row-reverse') : flex && style.flexDirection === 'column-reverse') ? -range : 0;
          if (/auto|scroll/.test(x ? style.overflowX : style.overflowY) && (sign > 0 ? at < low + range : at > low)) break;
        }
        if (!box?.isConnected || box.nodeType !== 1 || box === document.body) box = root;
        const size = x ? box.clientWidth : box.clientHeight, step = /Arrow/.test(event.key) ? 40 : /Home|End/.test(event.key) ? (x ? box.scrollWidth : box.scrollHeight) : Math.max(size * 0.875, size - 20);
        box.scrollBy({ [x ? 'left' : 'top']: sign * step, behavior: 'smooth' });
        return false;
      }
      if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey) {
        // Wrap only records whose tabIndex can be 0 or more: wrappers live until their nodes leave the page.
        // Without a focused element, Tab starts at the last press or the place of the removed focused element: the
        // count of candidates before that place in the composed tree.
        const [at, before] = active ? [] : navStart ?? [], end = {};
        laidOut();
        const found = [], stack = [records[0]], scrollers = new Set();
        let boundary = -1;
        while (stack.length) {
          const rec = stack.pop(), tag = rec.tag;
          if (rec === end) { boundary = found.length; continue; }
          if (rec === before?._m) boundary = found.length;
          if (tag[0] !== '#' && (rec.attrs.has('tabindex') || rec.attrs.has('contenteditable') || tag.includes('-') || /^(input|textarea|button|select|iframe|summary|terminal)$/.test(tag) || (tag === 'a' || tag === 'area') && rec.attrs.has('href'))) { const node = of(rec); if ((/^\s*[+-]?\d/.test(rec.attrs.get('tabindex') ?? '') ? node.tabIndex >= 0 : tag !== 'dialog') && focusable(node) && !(node.isContentEditable && node.parentElement?.isContentEditable)) found.push(node); }
          else if (tag[0] !== '#' && overflows(rec.id)) { const node = of(rec); if (focusable(node)) { scrollers.add(node); found.push(node); } }
          const assigned = tag === 'slot' && of(rec) instanceof HTMLSlotElement ? call('dom.assignedNodes', rec.id, false) : null, kids = assigned?.length ? assigned.map(id => records[id]) : (rec.shadow ?? rec).kids;
          if (rec === at?._m) { if (before === undefined) boundary = found.length; else if (before === null) stack.push(end); }
          for (let index = kids.length; index--;) stack.push(kids[index]);
        }
        // Like Chrome, a scroller is a Tab stop only when nothing in it is one.
        const sorted = found.map((node, order) => [node.tabIndex > 0 ? node.tabIndex : Infinity, order, node]).filter(entry => !scrollers.has(entry[2]) || !entry[2].contains(found[entry[1] + 1] ?? null)).sort((a, b) => a[0] - b[0] || a[1] - b[1]), list = sorted.map(entry => entry[2]), index = list.indexOf(active);
        let next = index < 0 ? (event.shiftKey ? list.length - 1 : 0) : (index + (event.shiftKey ? -1 : 1) + list.length) % list.length;
        if (index < 0 && boundary >= 0) {
          const from = event.shiftKey ? sorted.findLastIndex(entry => entry[1] < boundary) : sorted.findIndex(entry => entry[1] >= boundary);
          if (from >= 0) next = from;
        }
        // Like Chrome, keyboard focus selects all the text of an input. A textarea keeps its selection.
        const node = list[next], all = node instanceof HTMLInputElement && editable(node) && node !== active;
        if (all) node.setSelectionRange(0, node.value.length, 'none', true);
        if (node && node !== active) mouseFocus = false;
        node?.focus();
        return false;
      }
      return true;
    }
    const located = Number.isFinite(data.clientX) && Number.isFinite(data.clientY);
    const hit = captures.get(1) ?? wrap(ids[0]) ?? (located ? hitAt(data.clientX, data.clientY) : null), element = hit?.nodeType === 3 ? hit.parentElement : hit;
    if (!located) data = { ...data, clientX: Number.isFinite(data.clientX) ? data.clientX : last?.[0] ?? 0, clientY: Number.isFinite(data.clientY) ? data.clientY : last?.[1] ?? 0 };
    if (name === 'click') { if (element) { fireClick(element, mouseInit(data), true); if (data.detail === 2) dispatch(element, trusted(new MouseEvent('dblclick', mouseInit(data)))); } return !!element; }
    if (name === 'wheel') {
      const target = element ?? document.body, result = !target || dispatch(target, trusted(new WheelEvent('wheel', mouseInit(data, { deltaX: data.deltaX ?? 0, deltaY: data.deltaY ?? 0, deltaMode: data.deltaMode ?? 0 }))));
      last = [data.clientX, data.clientY];
      return result;
    }
    if (name !== 'pointermove' && name !== 'pointerdown' && name !== 'pointerup') return false;
    if (name === 'pointerdown') finishComposition();
    // Like Chrome, a new capture target gets the pointer boundary events, then `gotpointercapture`, then the mouse ones.
    if (!captures.has(1)) settleCapture(data);
    hover(element, mouseInit(data), () => settleCapture(data)); hoverVersion = version; hoverLayout = moves[0];
    settleCapture(data);
    // A terminal gets the pointer while a button is down, also outside its box, and every move above it.
    if (terminalDrag && name !== 'pointerdown') { if (terminalOf(terminalDrag)) terminalMouse(terminalOf(terminalDrag), name === 'pointerup' ? 1 : 2, name === 'pointerup' ? [1, 3, 2][data.button ?? 0] ?? 0 : 0, data); if (name === 'pointerup' && !data.buttons) terminalDrag = null; }
    else if (name === 'pointermove' && terminalOf(element)) terminalMouse(terminalOf(element), 2, 0, data);
    const target = element ?? sliding?.[0] ?? (selectionDrag ? selectionDrag.host : null), button = data.button ?? 0, init = mouseInit(data, name === 'pointermove' ? { button: -1, detail: 0 } : { detail: 0 });
    last = [data.clientX, data.clientY];
    if (name === 'pointerdown' && target) call('dom.active', labeled(path(target)).map(node => node._id));
    if (name === 'pointerup' && !data.buttons) call('dom.active', []);
    if (!target) return false;
    const first = name === 'pointerdown' && (!press || press.button === button), ended = name === 'pointerup' && !data.buttons;
    if (first) { pointers.add(1); press = { target, button, detail: data.detail ?? 1, mouse: true }; }
    if (first || ended) {
      if (autos.length || hints.length) {
        const clicked = nearestPopover(target, [...autos, ...hints]), invoker = path(target).map(node => node.nodeType === 1 && invoked(node)).find(popover => popover?._popover && popover._popover !== 'manual') ?? null;
        const ancestor = position(clicked) > position(invoker) ? clicked : invoker;
        if (first) pressedPopover = ancestor; else { const same = ancestor === pressedPopover; pressedPopover = null; if (same) hideUntil(ancestor ?? document); }
      }
      if (dialogs.length) {
        let clicked = path(target).find(node => node.localName === 'dialog' && dialogs.includes(node)) ?? null;
        if (clicked === target && clicked._modal) { const box = clicked.getBoundingClientRect(); if (data.clientX < box.left || data.clientY < box.top || data.clientX >= box.right || data.clientY >= box.bottom) clicked = null; }
        if (first) pressedDialog = clicked;
        else { const same = clicked === pressedDialog, top = dialogs.at(-1); pressedDialog = null; if (same && top && top !== clicked && top.closedBy === 'any') top.requestClose(); }
      }
    }
    const result = dispatch(target, trusted(new PointerEvent(first ? 'pointerdown' : ended ? 'pointerup' : 'pointermove', init)));
    if (first && !result) press.mouse = false;
    const blocked = path(target).some(node => node.nodeType === 1 && /^(button|input|select|textarea)$/.test(node.localName) && disabled(node));
    const moved = name === 'pointermove', down = trusted(new MouseEvent(name === 'pointerdown' ? 'mousedown' : name === 'pointerup' ? 'mouseup' : 'mousemove', { ...init, button: moved ? 0 : button, detail: moved ? 0 : press?.detail ?? data.detail ?? 1 }));
    if (moved) eventState(down).which = 0;
    // Disabled controls get no mouse events, but a press on them still moves the focus.
    const mouse = (press?.mouse ?? true) && (blocked || dispatch(target, down));
    // Like Chrome, every press sets the place where Tab starts.
    if (name === 'pointerdown') navStart = [target.nodeType === 1 ? target : target.parentElement];
    if (name === 'pointerdown' && mouse) {
      if (button === 0) selectionDrag = null;
      const focus = path(target).find(node => focusable(node, true) && !(node.isContentEditable && node.parentElement?.isContentEditable));
      const offset = editable(focus) && button === 0 ? textCall('dom.textIndex', focus._id, data.clientX, data.clientY) : 0;
      // Like Chrome, a press that moves the focus forgets the last key, and one that focuses an element makes it a mouse focus.
      if (focus ? focus !== active : button === 0 && active) { hadKey = false; if (focus) mouseFocus = true; }
      if (focus) focus.focus({ preventScroll: true }); else if (button === 0) active?.blur();
      scrollStart = target.nodeType === 1 ? target : target.parentElement;
      if (button === 0 && focus?.localName === 'select') openSelect(focus);
      if (terminalOf(focus)) { terminalDrag = focus; terminalMouse(terminalOf(focus), 0, [1, 3, 2][button] ?? 0, data); }
      else if (button === 0 && focus?.localName === 'input' && focus.type === 'range') { sliding = [focus, focus.value]; slide(focus, data.clientX); }
      else if (button === 0 && editable(focus)) {
        const anchor = data.shiftKey ? (__appsEditing()?.start ?? offset) : offset;
        const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
        const [start,end] = granularity > 1 ? selectionExtent(focus.value,offset,granularity) : [anchor,offset];
        focus.setSelectionRange(Math.min(start,end),Math.max(start,end),end < start ? 'backward' : navigator.platform.includes('Mac') ? 'none' : 'forward',true);
        selectionDrag = { host: focus, offset: start, endOffset: end, granularity, x: data.clientX, y: data.clientY };
      } else if (button === 0 && !blocked && call('dom.userSelect',target._id)[0] !== 'none') {
        const host = focus?.isContentEditable ? focus : target.getRootNode();
        const point = document.caretRangeFromPoint(data.clientX,data.clientY);
        if (point && host.contains(point.startContainer)) {
          const [mode,boundary] = call('dom.userSelect',point.startContainer._id);
          if (mode !== 'none' && dispatch(point.startContainer, trusted(new Event('selectstart', { bubbles: true, cancelable: true })))) {
            const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
            const range = selectionRange(point.startContainer,point.startOffset,granularity,mode,boundary);
            if (data.shiftKey && host.contains(selection.anchorNode)) selection.extend(range.endContainer,range.endOffset);
            else selection.setBaseAndExtent(range.startContainer,range.startOffset,range.endContainer,range.endOffset);
            selectionDrag = { host, node:selection.anchorNode, offset:selection.anchorOffset, endNode:selection.focusNode, endOffset:selection.focusOffset, granularity, x:data.clientX, y:data.clientY };
          }
        }
      }
    }
    if (sliding && name !== 'pointerdown') { slide(sliding[0], data.clientX); if (ended) { if (sliding[0].value !== sliding[1]) { interacted(sliding[0]); dispatch(sliding[0], trusted(new Event('change', { bubbles: true }))); } sliding = null; } }
    if (name === 'pointerdown' && !blocked && (button === 2 || button === 0 && data.ctrlKey && navigator.platform.includes('Mac')) && dispatch(target, trusted(new PointerEvent('contextmenu', { ...init, button: 2 })))) {
      const element = target.nodeType === 1 ? target : target.parentElement, link = element?.closest?.('a[href]'), media = element?.closest?.('img,video,audio,canvas'), field = element?.closest?.('input,textarea,[contenteditable=""],[contenteditable="true"]'), selected = String(getSelection?.() ?? ''), kind = media?.localName;
      host('dom.contextMenu', { x: data.clientX, y: data.clientY, linkURL: link?.href ?? '', linkText: link?.textContent ?? '', pageURL: location.href, frameURL: location.href, srcURL: media?.currentSrc ?? media?.src ?? '', mediaType: kind === 'img' ? 'image' : kind ?? 'none', hasImageContents: kind === 'img', isEditable: !!field, selectionText: selected, titleText: element?.title ?? '', altText: media?.alt ?? '', misspelledWord: '', dictionarySuggestions: [], selectionRect: { x: 0, y: 0, width: 0, height: 0 }, frameCharset: 'UTF-8', formControlType: field?.localName === 'input' ? field.type : 'none', spellcheckEnabled: false, menuSourceType: 'mouse', mediaFlags: {}, editFlags: { canUndo: !!field, canRedo: !!field, canCut: !!field && !!selected, canCopy: !!selected, canPaste: !!field, canDelete: !!field && !!selected, canSelectAll: true, canEditRichly: false } });
    }
    if ((name === 'pointermove' && result || ended) && selectionDrag && (selectionDrag.x !== data.clientX || selectionDrag.y !== data.clientY)) {
      const { host, node, offset, endNode, endOffset, granularity } = selectionDrag;
      selectionDrag.x = data.clientX; selectionDrag.y = data.clientY;
      if (editable(host)) {
        let end = textCall('dom.textIndex',host._id,data.clientX,data.clientY), start = offset;
        if (granularity > 1) {
          const extent = selectionExtent(host.value,end,granularity);
          [start,end] = end < offset ? [endOffset,extent[0]] : [offset,Math.max(endOffset,extent[1])];
        }
        host.setSelectionRange(Math.min(start,end),Math.max(start,end),end < start ? 'backward' : navigator.platform.includes('Mac') ? 'none' : 'forward',true);
      } else {
        const point = document.caretRangeFromPoint(data.clientX,data.clientY);
        if (point && host.contains(point.startContainer)) {
          const [mode,boundary] = call('dom.userSelect',point.startContainer._id);
          const range = selectionRange(point.startContainer,point.startOffset,granularity,mode,boundary);
          const backward = comparePoints(point.startContainer,point.startOffset,node,offset) < 0;
          const side = backward === (mode === 'none') ? 'end' : 'start';
          selection.setBaseAndExtent(backward ? endNode : node,backward ? endOffset : offset,range[side+'Container'],range[side+'Offset']);
        }
      }
    }
    // As in Chrome, the capture ends before `click`.
    if (ended) { captures.delete(1); settleCapture(data); }
    if (name === 'pointerup' && press?.button === button) {
      const down = path(press.target), common = path(target).find(node => node.nodeType === 1 && down.includes(node)), detail = press.detail;
      if (!blocked && common?.isConnected) { if (button === 0) { fireClick(common, { ...init, detail }, true); if (detail === 2) dispatch(common, trusted(new MouseEvent('dblclick', { ...init, detail }))); } else dispatch(common, trusted(new PointerEvent('auxclick', { ...init, detail }))); }
    }
    if (ended) { selectionDrag = null; press = null; pointers.delete(1); }
    // The press changes its own state, so only later changes check the hover.
    if (name === 'pointerdown') hoverVersion = version;
    return result;
  };
})();
