(() => {
  const host = globalThis.__appsDOM, eventState = globalThis.__appsEventState, droppedFiles = globalThis.__appsFiles, microtask = queueMicrotask, json = JSON.parse;
  delete globalThis.__appsDOM; delete globalThis.__appsEventState; delete globalThis.__appsFiles;
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
  class Record { constructor(id, tag, text) { this.id = id; this.tag = tag; this.text = text; this.attrs = tag[0] === '#' ? null : noAttrs; this.up = null; this.kids = tag === '#text' || tag === '#comment' ? noKids : []; this.i = 0; this.shadow = null; this.host = null; this.ref = null; } }
  const records = [], texts = [];
  let detached = false;
  let scheduled = false, changed = false, pool = [], reserve = 32, codes = new Int32Array(1024), size = 0;
  let stale = false, version = 0;
  const rare = { base: 0, title: 0 };
  let retrying = false;
  const flush = () => {
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
    resync();
  };
  const drain = () => { scheduled = false; flush(); };
  // Replace trailing lone high surrogates with U+FFFD before concatenation, or adjacent strings can pair them
  // and change UTF-16 lengths.
  const alone = value => { const last = value.charCodeAt(value.length - 1); return last >= 0xd800 && last < 0xdc00 ? value.slice(0, -1) + '\u{fffd}' : value; };
  const op = (code, a, b = 0, c = 0) => {
    if (size === codes.length) { const grown = new Int32Array(size * 2); grown.set(codes); codes = grown; }
    changed = stale = true; version++; codes[size] = code; codes[size + 1] = a; codes[size + 2] = typeof b === 'string' ? (texts.push(alone(b)), b.length) : b; codes[size + 3] = typeof c === 'string' ? (texts.push(alone(c)), c.length) : c; size += 4;
    if (!scheduled) { scheduled = true; microtask(drain); } else if (size > 1 << 18) flush();
  };
  let webviewScan = true, pickFiles, openSelect;
  const call = (name, ...args) => { flush(); if (name === 'dom.html') { changed = true; webviewScan ||= /<webview/i.test(args[1]); } if (name === 'dom.html' || name === 'dom.scroll' || name === 'dom.focus' || name === 'dom.hover' || name === 'dom.active' || name === 'dom.font' || name === 'dom.animation') { stale = true; version++; } return host(name, ...args); };
  // Keep mirror arrays private. Retain constructors and avoid prototype getters. A nonzero first float means
  // Rust moved the buffer.
  const F32 = Float32Array, U8 = Uint8Array;
  let geometry = new F32(1), marks, limit = 0, flagged = -1; geometry[0] = 1;
  let observing = false;
  const layout = () => {
    if ((stale || geometry[3]) && !observing) { stale = false; call('dom.reflow'); }
    if (geometry[0]) { const buffer = host('dom.layout', true), capacity = new F32(buffer, 0, 4)[1]; limit = 4 + capacity * 8; geometry = new F32(buffer, 0, limit); marks = new U8(buffer, limit * 4, capacity); }
    return geometry;
  };
  const slot = node => { layout(); const i = 4 + node._id * 8; if (i >= limit || marks[node._id] & 16) return -1; if (node._placed !== version) { if (!node.isConnected) return -1; node._placed = version; } return i; };
  const metric = (node, k) => { const i = slot(node); return i < 0 ? 0 : geometry[i + k]; };
  // Do not cache client boxes from observer reads: they use the previous frame's styles.
  const client = node => { if (slot(node) < 0) return [0, 0, 0, 0, 0, 0, 0, 0]; if (observing) return call('dom.client', node._id, true); const cached = node._client; if (cached?.[8] === geometry[2] && cached[9] === version) return cached; return node._client = [...call('dom.client', node._id), geometry[2], version]; };
  // Only scroll boxes have a scroll range, so other boxes take the border boxes of their children as their
  // scrollable overflow, then their end padding.
  const extent = (node, axis) => {
    const box = client(node), i = slot(node);
    if (i < 0 || marks[node._id] & 2) return Math.round(box[2 + axis] + (i < 0 ? 0 : geometry[i + 6 + axis]));
    if (!box[2] && !box[3] && !box[4] && !box[5] && !box[6] && !box[7] && /^(inline|contents)$/.test(getComputedStyle(node).display)) return 0;
    const start = geometry[i + axis] + box[axis], pending = [...(node._m.shadow ?? node._m).kids];
    let end = 0;
    while (pending.length) {
      const kid = pending.pop(), j = 4 + kid.id * 8;
      if (j >= limit || (marks[kid.id] & 49) !== 1) continue;
      if (geometry[j + 2] || geometry[j + 3]) end = Math.max(end, geometry[j + axis] + geometry[j + 2 + axis] - start);
      else if (kid.tag[0] !== '#') pending.push(...(kid.shadow ?? kid).kids);
    }
    return Math.round(Math.max(box[2 + axis], end && end + box[5 + axis]));
  };
  const offset = (node, axis) => {
    if (slot(node) < 0 || node === document.body) return 0;
    const start = node.getBoundingClientRect()[axis ? 'top' : 'left'], parent = node.offsetParent;
    if (!parent || parent.localName === 'body' && !(marks[parent._id] & 8)) return Math.round(start + metric(document, 4 + axis));
    return Math.round(start - parent.getBoundingClientRect()[axis ? 'top' : 'left'] - client(parent)[axis]);
  };
  const rootElement = node => node._m.tag === 'html' && node._m.up === records[0];
  const pageScroller = node => { layout(); return rootElement(node) && !(marks[node._id] & 2); };
  const create = (tag, text = '') => {
    if (!names.has(tag)) { if (!/^[a-z0-9-]+$/i.test(tag) && !types[tag]) throw new DOMException('Invalid tag', 'InvalidCharacterError'); names.set(tag, tag[0] === '#' ? tag === '#fragment' ? '#document-fragment' : tag : tag.toUpperCase()); }
    if (!pool.length) pool = host('dom.reserve', reserve = Math.min(reserve * 2, 1024)).reverse();
    const id = pool.pop();
    records[id] = new Record(id, tag, text); op(0, id, tag, text); if (tag === 'base' || tag === 'title') rare[tag]++; return id;
  };
  const load = list => {
    version++;
    let media = false;
    for (let i = 0; i < list.length;) {
      const rec = records[list[i]] = new Record(list[i], list[i + 2], list[i + 3]), up = records[list[i + 1]], end = i + 5 + 2 * list[i + 4];
      if (end > i + 5) { rec.attrs = new Attrs(list.slice(i + 5, end)); const id = rec.attrs.get('id'); if (id) idAdd(id, rec); }
      const tag = rec.tag;
      if (tag === 'base' || tag === 'title') rare[tag]++;
      media ||= tag === 'img' || tag === 'video' || tag === 'audio';
      i = end;
      if (up) { rec.up = up; rec.i = up.kids.push(rec) - 1; }
    }
    return media;
  };
  // Reload after failed batches: Rust can skip invalid writes. Preserve record identities because wrappers
  // reference them.
  const resync = () => {
    const lists = [], seen = new Set();
    for (const rec of records) if (rec && !rec.up) try { lists.push(json(host('dom.tree', rec.id))); } catch {}
    if (!lists.length) return;
    version++;
    for (const rec of records) if (rec?.kids.length) rec.kids = [];
    ids.clear();
    for (const list of lists) {
      for (let i = 0; i < list.length;) {
        const id = list[i], tag = list[i + 2], end = i + 5 + 2 * list[i + 4], up = i ? records[list[i + 1]] : null;
        let rec = records[id];
        if (rec?.tag !== tag) rec = records[id] = new Record(id, tag, list[i + 3]); else rec.text = list[i + 3];
        if (rec.attrs) { rec.attrs = end > i + 5 ? new Attrs(list.slice(i + 5, end)) : noAttrs; const value = rec.attrs.get('id'); if (value) idAdd(value, rec); }
        if (up) { rec.up = up; rec.i = up.kids.push(rec) - 1; seen.add(rec); }
        i = end;
      }
    }
    for (const rec of records) if (rec?.up && !seen.has(rec)) rec.up = null;
  };
  const of = rec => rec ? rec.ref?.deref() ?? wrap(rec.id) : null;
  // Keep only one cached list: a list holds the wrappers of its nodes, which keep removed nodes alive.
  const listCache = { nodes: [null, null, -1], elements: [null, null, -1] };
  const cachedList = (cache, rec, make) => { if (cache[0] !== rec || cache[1] !== rec.kids || cache[2] !== version) { cache[0] = rec; cache[1] = rec.kids; cache[2] = version; cache[3] = make(); } return cache[3]; };
  const ids = new Map();
  const idAdd = (id, rec) => { const at = ids.get(id); if (!at) ids.set(id, rec); else if (Array.isArray(at)) { if (!at.includes(rec)) at.push(rec); } else if (at !== rec) ids.set(id, [at, rec]); };
  const idDrop = (id, rec) => { const at = ids.get(id); if (at === rec) ids.delete(id); else if (Array.isArray(at)) { const i = at.indexOf(rec); if (i >= 0) at.splice(i, 1); if (at.length === 1) ids.set(id, at[0]); } };
  const index = rec => { const kids = rec.up.kids; let i = rec.i; if (kids[i] !== rec) rec.i = i = kids[i + 1] === rec ? i + 1 : kids[i - 1] === rec ? i - 1 : kids.indexOf(rec); return i; };
  const own = rec => rec.text ??= host('dom.source', rec.id);
  const text = root => { let value = ''; const stack = [root]; while (stack.length) { const rec = stack.pop(); if (rec !== root && (rec.tag === '#comment' || rec.tag === '#doctype')) continue; value += own(rec); for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); } return value; };
  const find = (root, test) => { const stack = []; for (let i = root.kids.length; i--;) stack.push(root.kids[i]); while (stack.length) { const rec = stack.pop(); if (test(rec)) return of(rec); if (rec.tag !== 'template') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); } return null; };
  const detach = rec => { version++; if (rec.kids.length) { for (const kid of rec.kids) kid.up = null; rec.kids = []; detached = true; } rec.text = ''; };
  const unhold = child => { const parent = child._parent; if (!parent) return; const last = parent._held.pop(); if (last !== child) { parent._held[child._slot] = last; last._slot = child._slot; } };
  const hold = (parent, child) => { unhold(child); child._parent = parent; child._slot = (parent._held ??= []).push(child) - 1; };
  const release = node => { if (node._held) for (const child of node._held) child._parent = null; node._held = null; };
  const definitions = new Map(), defined = new WeakMap(), definitionsWaiting = new Map();
  let constructing = null;
  const wrapped = [], names = new Map(), types = { __proto__: null, '#document': 9, '#text': 3, '#comment': 8, '#fragment': 11, '#doctype': 10 }, events = new WeakMap(), registered = [], pointers = new Set(), captures = new Map(), observers = new Set();
  let readyState = 'loading', currentScript = null, active = null, hovered = null, selection = null, composing = null, mutationQueued = false, selectionDrag = null, quiet = 0, shadows = 0, moving = false, wheeling = false, iterating = false, native = 0;
  const selectionPending = new Set();
  const topLayer = [], dialogs = [], autos = [], hints = [], fullscreens = [], watchers = [];
  const selectionChanged = (target = document) => {
    if (selectionPending.has(target)) return;
    selectionPending.add(target);
    setTimeout(() => { selectionPending.delete(target); target.dispatchEvent(new Event('selectionchange', { bubbles: target !== document })); }, 0);
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
  const watch = (observer, target, value, set) => { observer.targets.set(target._m.ref, value); (target._observers ??= new Set()).add(observer); set.add(observer._ref ??= new WeakRef(observer)); };
  const unwatch = (observer, target, set) => { observer.targets.delete(target._m.ref); target._observers?.delete(observer); if (!observer.targets.size) set.delete(observer._ref); };
  const unwatchAll = (observer, set) => { for (const ref of observer.targets.keys()) ref.deref()?._observers?.delete(observer); observer.targets.clear(); set.delete(observer._ref); };
  const watched = node => {
    if (!observers.size || quiet) return false;
    for (const ref of observers) for (const [key, options] of ref.deref()?.targets ?? []) { const target = key.deref(); if (target && options.childList && (target === node || options.subtree && target.contains(node))) return true; }
    return false;
  };
  const change = record => {
    if (!observers.size || quiet) return;
    for (const ref of observers) {
      const observer = ref.deref();
      if (!observer) { observers.delete(ref); continue; }
      for (const [key, options] of observer.targets) {
        const target = key.deref();
        if (!target) { observer.targets.delete(key); continue; }
        if (target !== record.target && !(options.subtree && target.contains(record.target))) continue;
        if (!options[record.type] || (record.type === 'attributes' && options.attributeFilter && !options.attributeFilter.includes(record.attributeName))) continue;
        const old = record.type === 'attributes' ? options.attributeOldValue : record.type === 'characterData' && options.characterDataOldValue;
        observer.records.push({ addedNodes: [], removedNodes: [], previousSibling: null, nextSibling: null, attributeName: null, attributeNamespace: null, ...record, oldValue: old ? record.oldValue ?? null : null });
        break;
      }
    }
    if (!mutationQueued) {
      mutationQueued = true;
      queueMicrotask(() => { mutationQueued = false; for (const ref of observers) { const observer = ref.deref(), records = observer?.takeRecords(); if (records?.length) try { observer.callback(records, observer); } catch (error) { console.error(error); } } });
    }
  };
  const dispatch = (target, event, ancestors = path(target)) => {
    const state = eventState(event);
    if (state.dispatching) throw new DOMException('Event is already in dispatch', 'InvalidStateError');
    const shadow = ancestors.some(node => node instanceof ShadowRoot);
    if (shadow && !event.composed) { const root = target.getRootNode(); if (root instanceof ShadowRoot) ancestors.length = ancestors.indexOf(root) + 1; }
    if (ancestors.at(-1) === document) ancestors.push(window);
    let seen = target;
    const targets = shadow ? ancestors.map(node => { const value = seen; if (node instanceof ShadowRoot && seen.getRootNode() === node) seen = node.host; return value; }) : null;
    Object.assign(state, { target, path: ancestors, dispatching: true });
    const invoke = (index, capture) => {
      const node = ancestors[index], listeners = events.get(node), list = listeners?.get(event.type);
      if (!list?.length) return;
      state.currentTarget = node; state.target = targets?.[index] ?? target; state.eventPhase = state.target === node ? 2 : capture ? 1 : 3;
      // Removal replaces the list and adding appends to it, so the first `length` items are the snapshot.
      for (let i = 0, length = list.length; i < length && !state.immediate; i++) {
        const item = list[i], current = listeners.get(event.type);
        if (item.capture !== capture || current !== list && !current.includes(item)) continue;
        if (item.once) node.removeEventListener(event.type, item.fn, item.capture);
        state.passive = item.passive;
        try { if (typeof item.fn === 'function') item.fn.call(node, event); else item.fn.handleEvent(event); } catch (error) { console.error(error); }
        state.passive = false;
      }
    };
    try {
      for (let index = ancestors.length; index-- && !state.stopped;) invoke(index, true);
      for (let index = 0; index < ancestors.length && !state.stopped; index++) if (event.bubbles || (targets?.[index] ?? target) === ancestors[index]) invoke(index, false);
    } finally { Object.assign(state, { target: shadow && target.getRootNode() instanceof ShadowRoot ? null : target, currentTarget: null, eventPhase: 0, dispatching: false, stopped: false, immediate: false, passive: false }); }
    return !event.defaultPrevented;
  };
  const trusted = event => { eventState(event).trusted = true; return event; };
  const labelable = rec => /^(button|meter|output|progress|select|textarea)$/.test(rec.tag) || rec.tag === 'input' && rec.attrs.get('type')?.toLowerCase() !== 'hidden';
  const labelControl = label => { const id = label.getAttribute('for'), control = id === null ? find(label._m, labelable) : id && label.getRootNode().getElementById?.(id); return control && labelable(control._m) ? control : null; };
  const disabled = node => {
    if (!/^(button|input|select|textarea|optgroup|option|fieldset)$/.test(node.localName)) return false;
    for (let rec = node._m, child = null; rec && rec.tag[0] !== '#'; child = rec, rec = rec.up) if (rec.attrs.has('disabled') && (rec === node._m || rec.tag === 'fieldset' && rec.kids.find(kid => kid.tag === 'legend') !== child || rec.tag === 'optgroup' && node.localName === 'option')) return true;
    return false;
  };
  const activation = node => { const tag = node.localName; return tag === 'button' || tag === 'label' || tag === 'input' && /^(checkbox|radio|submit|reset|image|button|file)$/.test(node.type) || (tag === 'a' || tag === 'area') && node._m.attrs.has('href') || tag === 'summary' && node._m.up?.tag === 'details' && node._m.up.kids.find(kid => kid.tag === 'summary') === node._m; };
  const group = radio => { const name = radio.name, form = radio.form, root = radio.getRootNode(); return name && root !== radio ? [...root.querySelectorAll('input[name=' + quote(name) + ']')].filter(other => other.type === 'radio' && other.form === form) : [radio]; };
  const focusable = node => {
    if (node?.nodeType !== 1 || !node.isConnected || disabled(node)) return false;
    layout();
    if (!geometry[2] && flagged !== version) { flagged = version; call('dom.layout', false, true); layout(); }
    if (marks[node._id] & 80) return false;
    const rec = node._m, tag = rec.tag;
    return /^\s*[+-]?\d/.test(rec.attrs.get('tabindex') ?? '') || node.isContentEditable || /^(select|textarea|button|iframe|dialog)$/.test(tag) || tag === 'input' && node.type !== 'hidden' || (tag === 'a' || tag === 'area') && rec.attrs.has('href') || tag === 'summary' && activation(node);
  };
  const commit = node => { if (node._edited) { node._edited = false; dispatch(node, trusted(new Event('change', { bubbles: true }))); } };
  const selectors = (name, id, selector, first) => { try { return call(name, id, String(selector), first); } catch (error) { if (/^SyntaxError/.test(error?.message)) throw new DOMException(error.message.replace(/^SyntaxError: /, ''), 'SyntaxError'); throw error; } };
  const toggles = new Map();
  const attributeChanged = (element, name, oldValue, value) => {
    if (shadows && name === 'slot') { signal(element.parentNode, element, oldValue ?? ''); signal(element.parentNode, element); }
    else if (shadows && name === 'name' && element.localName === 'slot') { const root = element.getRootNode(); if (root instanceof ShadowRoot && (slotted(root.host, oldValue ?? '') || slotted(root.host, value ?? ''))) queueSlot(element); }
    else if (name === 'popover' && element._popover && popoverState(oldValue) !== popoverState(value)) hidePopover(element, true, true);
    else if (name === 'open' && element.localName === 'dialog' && value === null) closeDialog(element);
    else if (name === 'open' && element.localName === 'details' && (oldValue === null) !== (value === null)) {
      const pending = toggles.get(element), newState = value === null ? 'closed' : 'open';
      if (pending) pending.newState = newState;
      else { toggles.set(element, { oldState: value === null ? 'open' : 'closed', newState }); setTimeout(() => { const state = toggles.get(element); toggles.delete(element); if (state.oldState !== state.newState) dispatch(element, new ToggleEvent('toggle', state)); }, 0); }
    }
  };
  const fireClick = (target, init = {}, native = false) => {
    const event = new PointerEvent('click', { bubbles: true, cancelable: true, composed: true, pointerId: native ? 1 : -1, pointerType: native ? 'mouse' : '', ...init });
    if (native) trusted(event);
    return activateClick(target, event, init, native);
  };
  // Every `click` MouseEvent runs this, also from `dispatchEvent`, and Base UI's switches and checkboxes rely on that.
  const activateClick = (target, event, init = {}, native = false) => {
    const activator = path(target).find(node => node.nodeType === 1 && activation(node)), kind = activator?.localName === 'input' ? activator.type : '';
    if (activator && disabled(activator)) return dispatch(target, event);
    const checked = node => node._checked ?? node.hasAttribute('checked');
    const previous = kind === 'checkbox' ? checked(activator) : kind === 'radio' ? group(activator).find(checked) ?? null : undefined;
    if (kind === 'checkbox') { setChecked(activator, !previous); activator._indeterminate = false; } else if (kind === 'radio') setChecked(activator, true);
    const allowed = dispatch(target, event);
    if (kind === 'checkbox' || kind === 'radio') {
      if (!allowed) { if (kind === 'checkbox') setChecked(activator, previous); else if (previous) setChecked(previous, true); else setChecked(activator, false); }
      else if (kind === 'checkbox' || previous !== activator) { dispatch(activator, new Event('input', { bubbles: true, composed: true })); dispatch(activator, new Event('change', { bubbles: true })); }
    } else if (allowed && activator) {
      const tag = activator.localName, type = tag === 'button' ? activator.type : kind;
      if (tag === 'label') { const control = labelControl(activator); if (control && !control.contains(target) && !disabled(control)) { if (focusable(control)) control.focus(); fireClick(control, { ...init, detail: 0 }, native); } }
      else if (tag === 'summary') activator.parentElement.open = !activator.parentElement.open;
      else if ((type === 'submit' || type === 'image') && activator.form) activator.form.requestSubmit(activator);
      else if (type === 'file') pickFiles(activator);
      else if (tag === 'button' || tag === 'input') { if (type === 'reset' && activator.form) activator.form.reset(); invoke(activator, target); }
      else if ((tag === 'a' || tag === 'area') && !/^(_self|_top|_parent)?$/i.test(activator.getAttribute('target') ?? '')) openWindow(activator.href, activator.getAttribute('target'));
    }
    return allowed;
  };
  const rawText = /^(style|script|xmp|iframe|noembed|noframes|plaintext|noscript)$/, voids = /^(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr|sf-symbol-[a-zA-Z0-9_]+(?:-[a-zA-Z0-9_]+)*)$/, escapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', '\u00a0': '&nbsp;' };
  const serialize = rec => {
    if (rec.tag === '#text') return rec.up && rawText.test(rec.up.tag) ? own(rec) : own(rec).replace(/[&<>\u00a0]/g, c => escapes[c]);
    if (rec.tag === '#comment') return '<!--' + rec.text + '-->';
    if (rec.tag === '#doctype') return '<!DOCTYPE ' + own(rec).split('\0')[0] + '>';
    if (rec.tag[0] === '#') return rec.kids.map(serialize).join('');
    let attrs = '';
    for (const [name, value] of rec.attrs) attrs += ' ' + name + '="' + value.replace(/[&"<>\u00a0]/g, c => escapes[c]) + '"';
    if (rec.tag.startsWith('sf-symbol-') && voids.test(rec.tag)) return '<' + rec.tag + attrs + '/>';
    return '<' + rec.tag + attrs + '>' + (rec.tag === 'template' ? of(rec).innerHTML : rec.kids.map(serialize).join('') || own(rec)) + (voids.test(rec.tag) ? '' : '</' + rec.tag + '>');
  };
  const reactionQueue = [];
  let reactionDepth = 0, reactionStart = 0;
  const report = error => { const event = Object.assign(new Event('error', { cancelable: true }), { error, message: String(error?.message ?? error) }); dispatch(globalThis, event); if (!event.defaultPrevented) console.error(error); };
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
        else if (name === 'connectedMoveCallback' && typeof node[name] !== 'function') { enqueue(node, ['disconnectedCallback']); enqueue(node, ['connectedCallback']); }
        else if (name && typeof node[name] === 'function') enqueue(node, [name]);
      }
      if (rec.tag !== 'template') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]);
      if (rec.shadow) stack.push(rec.shadow);
    }
  };
  const upgrade = (node, Type) => {
    if (node instanceof Type || node._failed) return;
    constructing = { node };
    try { new Type(); } catch (error) { node._failed = true; node._reactions.length = 0; throw error; } finally { constructing = null; }
    const observed = defined.get(Type)?.observed;
    if (observed?.size) for (const [name, value] of node._m.attrs) if (observed.has(name)) node._reactions.push(['attributeChangedCallback', name, null, value]);
    if (node.isConnected) node._reactions.push(['connectedCallback']);
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
  const anchors = new Map();
  let rangeSync = false;
  const place = (range, start, node, offset) => {
    const old = start ? range._sc : range._ec;
    if (start) { range._sc = node; range._so = offset; } else { range._ec = node; range._eo = offset; }
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
      if (fn(range) && selection?.ranges[0] === range && !rangeSync) { rangeSync = true; queueMicrotask(() => { rangeSync = false; if (selection.ranges[0]?.startContainer.isConnected) selection._setRange(selection.ranges[0], selection.direction, true); }); }
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
  const rangesRemove = (parent, child, index) => { if (!anchors.size) return; inside(child._m, (range, rec) => shift(range, rec, always, parent, () => index)); each(parent._m, range => shift(range, parent._m, at => at > index, parent, at => at - 1)); };
  const rangesInsert = (parent, index, count) => { if (anchors.size) each(parent._m, range => shift(range, parent._m, at => at > index, parent, at => at + count)); };
  const rangesClear = node => { if (anchors.size) inside(node._m, (range, rec) => shift(range, rec, rec === node._m ? at => at > 0 : always, node, () => 0)); };
  const forgetRemoved = () => {
    if (topLayer.length || dialogs.length) {
      if (fullscreens.some(node => !node.isConnected)) exitFullscreen(true);
      for (const node of [...topLayer, ...dialogs]) if (!node.isConnected) { if (node._popover) hidePopover(node, false, false); if (dialogs.includes(node)) closeDialog(node); }
    }
    if (captures.size) for (const [id, node] of captures) if (!node.isConnected) { captures.delete(id); dispatch(document, new PointerEvent('lostpointercapture', { pointerId: id, bubbles: true, composed: true })); }
    if (!active || active.isConnected) return;
    finishComposition();
    const previous = active; active = null; call('dom.focus', []); if (windowFocused) { dispatch(previous, new FocusEvent('blur', { composed: true })); dispatch(previous, new FocusEvent('focusout', { bubbles: true, composed: true })); }
  };
  const ignore = () => {};
  // Call removed after both trees detach the old children, before reinserted children run connected
  // callbacks.
  const replaceAll = scoped((node, replace) => {
    const rec = node._m, observed = watched(node);
    if (!rec.kids.length && !observed && !shadows) { replace(ignore); return; }
    // A wrapper keeps its removed subtree in the arena until a GC, so only an observer's record wraps the old children.
    const kids = definitions.size > 0 && node.isConnected ? rec.kids.slice() : [], old = observed ? rec.kids.map(of) : [];
    if (rec.kids.length) { if (node._shadow) for (const kid of rec.kids) signal(node, of(kid)); rangesClear(node); }
    let pending = kids.length > 0;
    const removed = () => { if (pending) { pending = false; for (const kid of kids) reactions(kid, 'disconnectedCallback'); } };
    replace(removed); removed();
    forgetRemoved();
    if (shadows) { if (node._shadow) for (const kid of rec.kids) signal(node, of(kid)); slotsIn(node.getRootNode(), rec); }
    if (observed && (old.length || rec.kids.length)) change({ type: 'childList', target: node, removedNodes: listOf(NodeList, old), addedNodes: node.childNodes });
  });
  const convert = children => children.map(child => child instanceof Node ? child : document.createTextNode(String(child)));
  const viable = (node, nodes) => { while (node && nodes.includes(node)) node = node.nextSibling; return node; };
  const insertNodes = scoped((parent, nodes, before, replaced = null) => {
    if (!observers.size || quiet || nodes.length < 2 && !replaced) {
      for (const node of nodes) parent.insertBefore(node, before);
      if (replaced && !nodes.includes(replaced) && replaced.parentNode === parent) parent.removeChild(replaced);
      return;
    }
    nodes = nodes.flatMap(node => node.nodeType === 11 ? node.childNodes : [node]);
    for (const node of nodes) node.parentNode?.removeChild(node);
    const removed = replaced?.parentNode === parent ? [replaced] : [], previousSibling = removed.length ? replaced.previousSibling : before ? before.previousSibling : parent.lastChild;
    quiet++;
    try { for (const node of nodes) parent.insertBefore(node, before); if (removed.length) parent.removeChild(replaced); } finally { quiet--; }
    if (nodes.length || removed.length) change({ type: 'childList', target: parent, addedNodes: listOf(NodeList, nodes), removedNodes: listOf(NodeList, removed), previousSibling, nextSibling: before });
  });
  const contextTag = node => !node ? 'body' : node.namespaceURI === SVG ? /^(foreignObject|desc|title)$/.test(node.localName) ? 'body' : 'svg' : node.localName === 'svg' ? 'body' : node.localName;
  const parseFragment = scoped((source, context) => {
    const fragment = document.createDocumentFragment(), tag = contextTag(context);
    if (tag === 'svg') fragment._svg = true;
    if (load(json(call('dom.html', fragment._id, String(source), tag)))) __appsImages(fragment);
    if (definitions.size) reactions(fragment._m);
    return fragment;
  });
  const setHTML = (node, value, inert = false) => replaceAll(node, removed => {
    const list = json(node.nodeType === 1 ? call('dom.html', node._id, String(value), contextTag(node)) : call('dom.html', node._id, String(value)));
    release(node); detach(node._m); const media = load(list); removed();
    if (!inert) { if (media) __appsImages(node); if (definitions.size) for (const kid of node._m.kids) reactions(kid); }
  });
  class List extends Array { static get [Symbol.species]() { return Array; } item(index) { return this[index] ?? null; } }
  class NodeList extends List {}
  class HTMLCollection extends List { namedItem(name) { return this.find(element => element.id === name || element.getAttribute('name') === name) ?? null; } }
  class NamedNodeMap extends List { getNamedItem(name) { return this.find(attr => attr.name === name) ?? null; } }
  class DOMRectList extends List {}
  class Attr {
    constructor(ownerElement, name, value) { Object.assign(this, { ownerElement, name, localName: name, nodeName: name, nodeType: 2, namespaceURI: null, prefix: null, specified: true }); this._value = value; }
    get value() { return this._value; } set value(value) { this._value = String(value); this.ownerElement?.setAttribute(this.name, this._value); }
    get nodeValue() { return this.value; } set nodeValue(value) { this.value = value; }
  }
  const token = value => { value = String(value); if (!value) throw new DOMException('Token is empty', 'SyntaxError'); if (/[\t\n\f\r ]/.test(value)) throw new DOMException('Token contains whitespace', 'InvalidCharacterError'); return value; };
  const tokens = list => { const value = list.value; if (value !== list._source) { list._source = value; list._tokens = [...new Set(value.split(/[\t\n\f\r ]+/).filter(Boolean))]; } return list._tokens; };
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
    add(...values) { const list = rawList(this), next = tokens(list).slice(); for (const value of values.map(token)) if (!next.includes(value)) next.push(value); if (next.length || list._element.hasAttribute(list._name)) list.value = next.join(' '); }
    remove(...values) { const list = rawList(this); values = values.map(token); const next = tokens(list).filter(value => !values.includes(value)); if (next.length || list._element.hasAttribute(list._name)) list.value = next.join(' '); }
    toggle(value, force) { const list = rawList(this); value = token(value); const add = force === undefined ? !list.contains(value) : !!force; if (add) list.add(value); else list.remove(value); return add; }
    replace(old, value) { const list = rawList(this); old = token(old); value = token(value); const next = tokens(list).slice(), index = next.indexOf(old); if (index < 0) return false; next[index] = value; list.value = [...new Set(next)].join(' '); return true; }
    supports() { return true; }
    forEach(fn, thisArg) { tokens(rawList(this)).forEach((value, index) => fn.call(thisArg, value, index, this)); }
    keys() { return tokens(rawList(this)).keys(); } values() { return tokens(rawList(this)).values(); } entries() { return tokens(rawList(this)).entries(); } [Symbol.iterator]() { return this.values(); }
    toString() { return rawList(this).value; }
  }
  class Node {
    constructor(id, tag) { const rec = this._m = records[id]; this._id = id; this._held = null; this._parent = null; this._slot = 0; rec.ref = new WeakRef(this); wrapped.push(rec); registered.push(id); this.nodeType = types[tag] ?? 1; this.nodeName = names.get(tag) ?? (tag[0] === '#' ? tag === '#fragment' ? '#document-fragment' : tag : tag.toUpperCase()); }
    get ownerDocument() { if(this.nodeType===9)return null;const root=this.getRootNode();return root.nodeType===9?root:document; }
    get parentNode() { return of(this._m.up); }
    get parentElement() { const up = this._m.up; return up && up.tag[0] !== '#' ? of(up) : null; }
    get assignedSlot() { const slot = assignedSlot(this); return slot?.getRootNode().mode === 'closed' ? null : slot; }
    get childNodes() { return cachedList(listCache.nodes, this._m, () => listOf(NodeList, this._m.kids.map(of))); }
    get firstChild() { return of(this._m.kids[0]); }
    get lastChild() { const kids = this._m.kids; return of(kids[kids.length - 1]); }
    get nextSibling() { const rec = this._m; if (!rec.up) return null; const next = rec.up.kids[index(rec) + 1]; if (next) next.i = rec.i + 1; return of(next); }
    get previousSibling() { const rec = this._m; if (!rec.up) return null; const previous = rec.up.kids[index(rec) - 1]; if (previous) previous.i = rec.i - 1; return of(previous); }
    get isConnected() { let rec = this._m; while (rec.up ?? rec.host) rec = rec.up ?? rec.host; return rec.tag === '#document'; }
    get textContent() { return text(this._m); }
    set textContent(value) {
      value = String(value ?? '');
      if (this.nodeType === 3 || this.nodeType === 8) { this.data = value; return; }
      replaceAll(this, () => { release(this); detach(this._m); op(1, this._id, ''); if (value) { const text = document.createTextNode(value); op(2, this._id, text._id, -1); text._m.up = this._m; text._m.i = 0; this._m.kids.push(text._m); hold(this, text); } });
    }
    get nodeValue() { return this.nodeType === 3 || this.nodeType === 8 ? this.textContent : null; }
    set nodeValue(value) { if (this.nodeType === 3 || this.nodeType === 8) this.textContent = value; }
    appendChild(child) { return this.insertBefore(child, null); }
    insertBefore(child, before) {
      if (!(child instanceof Node)) throw new TypeError('Expected a Node');
      before ??= null;
      if (child.nodeType === 11) {
        // Move unwrapped children as records to avoid retaining removed trees. Keep SVG wrapping: wrap infers
        // namespaces from parents.
        if (!(observers.size > 0 && !quiet) && !anchors.size && !this._shadow && !child._svg && !this._svg && !(this instanceof SVGElement) && (this.nodeType === 1 || this.nodeType === 11 || this._id === 0) && child._m.kids.some(kid => !kid.ref?.deref() && kid.tag !== 'link')) {
          const rec = this._m, ref = before?._m ?? null;
          if (ref && ref.up !== rec) throw new DOMException('Reference is not a child', 'NotFoundError');
          for (let up = rec; up; up = up.up ?? up.host) if (up === child._m) throw new DOMException('Cannot create a DOM cycle', 'HierarchyRequestError');
          const connected = definitions.size > 0 && this.isConnected, root = shadows ? this.getRootNode() : null;
          for (const kid of child._m.kids.slice()) {
            if (kid.ref?.deref() || kid.tag === 'link') { this.insertBefore(of(kid), before); continue; }
            op(2, this._id, kid.id, ref ? ref.id : -1);
            kid.up.kids.splice(index(kid), 1); kid.up = rec;
            if (ref) rec.kids.splice(kid.i = index(ref), 0, kid); else kid.i = rec.kids.push(kid) - 1;
            if (connected) reactions(kid, 'connectedCallback');
            if (root) slotsIn(root, kid, true);
          }
          forgetRemoved();
          return child;
        }
        const nodes = child._m.kids.map(of), observed = observers.size > 0 && !quiet;
        if (!nodes.length) return child;
        quiet++;
        try { for (const node of nodes) this.insertBefore(node, before); } finally { quiet--; }
        if (observed) { change({ type: 'childList', target: child, removedNodes: listOf(NodeList, nodes) }); change({ type: 'childList', target: this, addedNodes: listOf(NodeList, nodes), previousSibling: nodes[0].previousSibling, nextSibling: before }); }
        return child;
      }
      const rec = this._m, kid = child._m, ref = before?._m ?? null;
      if (child.nodeType === 9 || child.nodeType === 10 && this.nodeType !== 9 || this.nodeType !== 1 && this.nodeType !== 11 && this._id !== 0) throw new DOMException(child.nodeType === 9 || child.nodeType === 10 ? 'Invalid child' : 'Invalid parent', 'HierarchyRequestError');
      if (ref && ref.up !== rec) throw new DOMException('Reference is not a child', 'NotFoundError');
      for (let up = rec; up; up = up.up ?? up.host) if (up === kid) throw new DOMException('Cannot create a DOM cycle', 'HierarchyRequestError');
      const observed = observers.size > 0 && !quiet, connected = definitions.size > 0 && child.isConnected, parented = !!kid.up;
      let removal = null;
      if (kid.up && (observed || shadows || anchors.size)) {
        const from = of(kid.up);
        if (observed) removal = { type: 'childList', target: from, removedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling };
        if (from._shadow) signal(from, child);
        rangesRemove(from, child, index(kid));
      }
      op(2, this._id, child._id, ref ? ref.id : -1);
      if (ref !== kid) { if (kid.up) kid.up.kids.splice(index(kid), 1); kid.up = rec; if (ref) rec.kids.splice(kid.i = index(ref), 0, kid); else kid.i = rec.kids.push(kid) - 1; }
      hold(this, child);
      if (ref) rangesInsert(this, index(kid), 1);
      if (connected && moving && child.isConnected) reactions(kid, 'connectedMoveCallback');
      else { if (connected) reactions(kid, 'disconnectedCallback'); if (definitions.size && child.isConnected) reactions(kid, 'connectedCallback'); }
      if (shadows) { if (this._shadow) signal(this, child); slotsIn(this.getRootNode(), kid, true); }
      if (parented) forgetRemoved();
      if (removal) change(removal);
      if (observed) change({ type: 'childList', target: this, addedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling });
      if (kid.tag === 'link' && kid.attrs.has('href') && /(^|\s)(stylesheet|preload|modulepreload)(\s|$)/i.test(kid.attrs.get('rel') ?? '') && child.isConnected) setTimeout(() => child.dispatchEvent(new Event('error')));
      return child;
    }
    removeChild(child) {
      if (!(child instanceof Node) || child._m.up !== this._m) throw new DOMException('Node is not a child', 'NotFoundError');
      const observed = observers.size > 0 && !quiet, connected = definitions.size > 0 && child.isConnected, kid = child._m, i = index(kid);
      const record = observed ? { type: 'childList', target: this, removedNodes: listOf(NodeList, [child]), previousSibling: child.previousSibling, nextSibling: child.nextSibling } : null;
      if (this._shadow) signal(this, child);
      rangesRemove(this, child, i);
      op(3, this._id, child._id); this._m.kids.splice(i, 1); kid.up = null; unhold(child); child._parent = null;
      if (connected) reactions(kid, 'disconnectedCallback');
      forgetRemoved();
      if (record) change(record);
      return child;
    }
    replaceChild(child, old) {
      if (!(old instanceof Node) || old._m.up !== this._m) throw new DOMException('Node is not a child', 'NotFoundError');
      if (child === old) return old;
      const observed = observers.size > 0 && !quiet, previousSibling = observed ? old.previousSibling : null, nextSibling = observed ? old.nextSibling === child ? child.nextSibling : old.nextSibling : null, addedNodes = observed ? child.nodeType === 11 ? child.childNodes : listOf(NodeList, [child]) : null;
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
    getRootNode(options = {}) { let rec = this._m; while (rec.up ?? (options.composed ? rec.host : null)) rec = rec.up ?? rec.host; return of(rec); }
    cloneNode(deep = false) { const cloned = this.nodeType === 3 ? document.createTextNode(this.data) : this.nodeType === 8 ? document.createComment(this.data) : this.nodeType === 11 ? document.createDocumentFragment() : this.namespaceURI === XHTML ? wrap(create(this.localName)) : document.createElementNS(this.namespaceURI, this.localName); if (this.nodeType === 1) for (const [name, value] of this._m.attrs) cloned.setAttribute(name, value); if (deep) for (const child of this.childNodes) cloned.appendChild(child.cloneNode(true)); const Type = this.nodeType === 1 && definitions.get(cloned.localName); if (Type) enqueue(cloned, Type); return cloned; }
    isSameNode(other) { return this === other; }
    isEqualNode(other) { return other?.nodeType === this.nodeType && serialize(this._m) === serialize(other._m); }
    compareDocumentPosition(other) { if (this === other) return 0; const [a, b] = [this._m, other._m].map(rec => { const chain = []; for (; rec; rec = rec.up) chain.push(rec); return chain.reverse(); }); if (a[0] !== b[0]) return 1 | 32; let i = 0; while (a[i] && a[i] === b[i]) i++; if (i === a.length) return 4 | 16; if (i === b.length) return 2 | 8; return index(a[i]) < index(b[i]) ? 4 : 2; }
    normalize() { for (const child of this.childNodes) { if (child.nodeType === 3) { while (child.nextSibling?.nodeType === 3) { child.data += child.nextSibling.data; child.nextSibling.remove(); } if (!child.length) child.remove(); } else child.normalize(); } }
    addEventListener(name, fn, options = false) { if (!fn || options?.signal?.aborted) return; name = String(name); const capture = typeof options === 'boolean' ? options : !!options?.capture; let listeners = events.get(this); if (!listeners) events.set(this, listeners = new Map()); const list = listeners.get(name) ?? []; if (list.some(item => item.fn === fn && item.capture === capture)) return; const scroll = name === 'wheel' || name === 'mousewheel' || name === 'touchstart' || name === 'touchmove'; if (name === 'wheel' && !wheeling) { wheeling = true; call('dom.wheel'); } if (name === 'animationiteration' && !iterating) { iterating = true; call('dom.iterations'); } list.push({ fn, capture, once: !!options?.once, passive: !!(options?.passive ?? (scroll && (this === globalThis || this === document || this === document.documentElement || this === document.body))) }); listeners.set(name, list); options?.signal?.addEventListener('abort', () => this.removeEventListener(name, fn, capture), { once: true }); }
    removeEventListener(name, fn, options = false) { const capture = typeof options === 'boolean' ? options : !!options?.capture, listeners = events.get(this); listeners?.set(String(name), (listeners.get(String(name)) ?? []).filter(item => item.fn !== fn || item.capture !== capture)); }
    dispatchEvent(event) { if (!(event instanceof Event)) throw new TypeError('Expected an Event'); return event.type === 'click' && event instanceof MouseEvent ? activateClick(this, event) : dispatch(this, event); }
  }
  Object.setPrototypeOf(Node.prototype, EventTarget.prototype); Object.setPrototypeOf(Node, EventTarget);
  for (const [name, value] of Object.entries({ ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4, PROCESSING_INSTRUCTION_NODE: 7, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11, DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32 })) { Node[name] = value; Node.prototype[name] = value; }
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
      rec.text = value.slice(0, offset) + data + value.slice(offset + count); op(1, this._id, rec.text);
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
  class Comment extends CharacterData { constructor(data = '', tag) { super(tag ? data : create('#comment', String(data)), '#comment'); } }
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
  const cssProperty = name => (cssProperties ??= new Set(call('dom.properties'))).has(name) || name === 'float';
  const cssKeys = () => { const names = [...(cssProperties ??= new Set(call('dom.properties')))]; return [...new Set([...names, ...names.map(name => name.replace(/^-(webkit|moz|ms)-/, '$1-').replace(/-([a-z])/g, (_, c) => c.toUpperCase()))])]; };
  const declared = new Map();
  const declare = text => { let list = declared.get(text); if (!list) { if (declared.size > 1023) declared.clear(); const flat = /\S/.test(text) ? host('dom.declare', text) : []; list = []; for (let i = 0; i < flat.length; i += 3) list.push([flat[i], flat[i + 1], flat[i + 2]]); declared.set(text, list); } return list.map(entry => entry.slice()); };
  const sides = (prefix, suffix = '') => ['top', 'right', 'bottom', 'left'].map(side => prefix + side + suffix);
  const shorthands = new Map(Object.entries({
    border: [...sides('border-', '-width'), ...sides('border-', '-style'), ...sides('border-', '-color')], 'border-width': sides('border-', '-width'), 'border-style': sides('border-', '-style'), 'border-color': sides('border-', '-color'),
    'border-top': ['border-top-width', 'border-top-style', 'border-top-color'], 'border-right': ['border-right-width', 'border-right-style', 'border-right-color'], 'border-bottom': ['border-bottom-width', 'border-bottom-style', 'border-bottom-color'], 'border-left': ['border-left-width', 'border-left-style', 'border-left-color'],
    margin: sides('margin-'), padding: sides('padding-'), inset: sides(''), 'scroll-margin': sides('scroll-margin-'), 'scroll-padding': sides('scroll-padding-'), 'border-radius': ['border-top-left-radius', 'border-top-right-radius', 'border-bottom-right-radius', 'border-bottom-left-radius'],
    gap: ['row-gap', 'column-gap'], overflow: ['overflow-x', 'overflow-y'], 'overscroll-behavior': ['overscroll-behavior-x', 'overscroll-behavior-y'], 'flex-flow': ['flex-direction', 'flex-wrap'], flex: ['flex-grow', 'flex-shrink', 'flex-basis'], 'place-content': ['align-content', 'justify-content'], 'place-items': ['align-items', 'justify-items'], 'place-self': ['align-self', 'justify-self'],
    'text-decoration': ['text-decoration-line', 'text-decoration-style', 'text-decoration-color', 'text-decoration-thickness'], 'list-style': ['list-style-type', 'list-style-position', 'list-style-image'], outline: ['outline-width', 'outline-style', 'outline-color'], container: ['container-name', 'container-type'],
  }));
  const shorthandsOf = new Map();
  for (const entry of shorthands) for (const part of entry[1]) shorthandsOf.set(part, [...shorthandsOf.get(part) ?? [], entry]);
  const shorthandValue = (name, values) => {
    if (name === 'border') { const groups = [0, 1, 2, 3].map(side => [values[side], values[side + 4], values[side + 8]]); return groups.every(group => group.join() === groups[0].join()) ? shorthandValue('border-top', groups[0]) : ''; }
    if (values.length === 4 && name !== 'text-decoration') { const [top, right, bottom, left] = values; return left !== right ? values.join(' ') : top !== bottom ? `${top} ${right} ${bottom}` : top !== right ? `${top} ${right}` : top; }
    if (values.length === 2 && !['flex-flow', 'container'].includes(name)) return values[0] === values[1] ? values[0] : values.join(' ');
    if (name === 'container') return values[1] === 'normal' ? values[0] : values.join(' / ');
    const defaults = { 'text-decoration': ['solid', 'currentcolor', 'auto'], 'list-style': ['outside', 'none'], flex: [] }[name] ?? ['currentcolor'], kept = values.filter((value, index) => index === 0 && name !== 'list-style' || !defaults.includes(value));
    return (kept.length ? kept : values.slice(0, 1)).join(' ');
  };
  const declarationText = list => {
    const out = [], done = new Set();
    for (const [name, , priority] of list) {
      if (done.has(name)) continue;
      let text = '';
      for (const [shorthand, parts] of shorthandsOf.get(name) ?? []) {
        if (parts.some(part => done.has(part))) continue;
        const entries = parts.map(part => list.findLast(entry => entry[0] === part));
        if (entries.some(entry => !entry || entry[2] !== priority)) continue;
        const merged = shorthandValue(shorthand, entries.map(entry => entry[1]));
        if (!merged) continue;
        for (const part of parts) done.add(part);
        text = shorthand + ': ' + merged;
        break;
      }
      if (!text) { done.add(name); text = name + ': ' + list.findLast(entry => entry[0] === name)[1]; }
      out.push(text + (priority ? ' !important' : '') + ';');
    }
    return out.join(' ');
  };
  const readOnly = () => { throw new DOMException('Computed styles are read-only', 'NoModificationAllowedError'); };
  let keyboardFocus = true, windowFocused = true, ring = false;
  const focusVisible = node => keyboardFocus || node.isContentEditable || node.localName === 'textarea' || node.localName === 'input' && !/^(button|submit|reset|checkbox|radio|range|color|file|image)$/i.test(node.getAttribute('type') ?? '');
  class CSSStyleDeclaration {
    constructor(owner) { this._owner = owner; }
    get _list() { return this._owner.read(); }
    get length() { return this._owner.computed ? this._owner.names().length : this._list.length; }
    item(index) { return (this._owner.computed ? this._owner.names()[index] : this._list[index]?.[0]) ?? ''; }
    get parentRule() { return this._owner.rule ?? null; }
    getPropertyValue(name) {
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      if (this._owner.computed) return this._owner.computed(name);
      const list = this._list, found = list.findLast(entry => entry[0] === name);
      if (found) return found[1];
      const parts = shorthands.get(name), entries = parts?.map(part => list.findLast(entry => entry[0] === part));
      return entries && entries.every(entry => entry && entry[2] === entries[0][2]) ? shorthandValue(name, entries.map(entry => entry[1])) : '';
    }
    getPropertyPriority(name) {
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      if (this._owner.computed) return '';
      const list = this._list, parts = shorthands.get(name) ?? [name];
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
      const list = owner.read();
      if (added.length === 1) { owner.write([...list.filter(entry => entry[0] !== added[0][0]), added[0]]); return; }
      const names = new Set(added.map(entry => entry[0]));
      owner.write([...list.filter(entry => !names.has(entry[0])), ...added]);
    }
    removeProperty(name) {
      if (this._owner.computed) readOnly();
      name = String(name); if (!name.startsWith('--')) name = name.toLowerCase();
      const old = this.getPropertyValue(name), parts = new Set(shorthands.get(name) ?? [name]), list = this._list, kept = list.filter(entry => !parts.has(entry[0]));
      if (kept.length !== list.length) this._owner.write(kept);
      return old;
    }
    get cssText() { return this._owner.computed ? '' : declarationText(this._list); }
    set cssText(value) { if (this._owner.computed) readOnly(); this._owner.write(this._owner.raw ? String(value).split(';').flatMap(part => { const colon = part.indexOf(':'); return colon > 0 ? [[part.slice(0, colon).trim(), part.slice(colon + 1).trim(), '']] : []; }) : declare(String(value))); }
  }
  const styleKey = key => typeof key === 'string' && !(key.charCodeAt(0) <= 57 && /^\d+$/.test(key));
  const declaration = owner => new Proxy(new CSSStyleDeclaration(owner), {
    get: (target, key) => typeof key !== 'string' || key in target ? Reflect.get(target, key) : /^\d+$/.test(key) ? target.item(Number(key)) : target.getPropertyValue(styleName(key)),
    set: (target, key, value) => { if (typeof key !== 'string' || key in target) return Reflect.set(target, key, value); if (styleKey(key)) target.setProperty(styleName(key), value); return true; },
    has: (target, key) => key in target || typeof key === 'string' && (styleKey(key) ? cssProperty(styleName(key)) : Number(key) < target.length),
    ownKeys: target => [...new Set([...Reflect.ownKeys(target), ...Array.from({ length: target.length }, (_, index) => String(index)), ...cssKeys()])],
    getOwnPropertyDescriptor: (target, key) => Reflect.getOwnPropertyDescriptor(target, key) ?? (typeof key === 'string' && (styleKey(key) ? cssProperty(styleName(key)) : Number(key) < target.length) ? { value: styleKey(key) ? target.getPropertyValue(styleName(key)) : target.item(Number(key)), writable: true, enumerable: true, configurable: true } : undefined),
  });
  const attrName = (element, name) => element.namespaceURI === XHTML ? String(name).toLowerCase() : String(name);
  const datasetOwner = Symbol(), datasetHandler = { get: (target, name) => target[datasetOwner].getAttribute('data-' + cssName(name)) ?? undefined, set: (target, name, value) => { target[datasetOwner].setAttribute('data-' + cssName(name), value); return true; }, has: (target, name) => typeof name === 'string' && target[datasetOwner].hasAttribute('data-' + cssName(name)), deleteProperty: (target, name) => { target[datasetOwner].removeAttribute('data-' + cssName(name)); return true; }, ownKeys: target => target[datasetOwner].getAttributeNames().filter(name => name.startsWith('data-')).map(name => name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())), getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }) };
  class Element extends Node {
    constructor(id, tag) { if(id===undefined){if(constructing?.node){const node=constructing.node;constructing=null;Object.setPrototypeOf(node,new.target.prototype);return node;}tag=constructing?.tag??defined.get(new.target)?.name;if(!tag)throw new TypeError('Illegal element constructor');id=create(tag);constructing=null;} super(id, tag);  this.tagName = this.nodeName; this.localName = tag; this.namespaceURI = XHTML; }
    get attributes() { const attrs = listOf(NamedNodeMap, Array.from(this._m.attrs, ([name, value]) => new Attr(this, name, value))); for (const attr of attrs) if (!(attr.name in attrs)) attrs[attr.name] = attr; return attrs; }
    getAttributeNames() { return [...this._m.attrs.keys()]; }
    hasAttributes() { return this._m.attrs.size > 0; }
    get nextElementSibling() { let n = this.nextSibling; while (n && n.nodeType !== 1) n = n.nextSibling; return n; }
    get previousElementSibling() { let n = this.previousSibling; while (n && n.nodeType !== 1) n = n.previousSibling; return n; }
    get id() { return this.getAttribute('id') ?? ''; } set id(value) { this.setAttribute('id', value); }
    get className() { return this.getAttribute('class') ?? ''; } set className(value) { this.setAttribute('class', value); }
    get classList() { return this._classList ??= new DOMTokenList(this, 'class'); } set classList(value) { this.classList.value = value; }
    get dataset() { return this._dataset ??= new Proxy({ [datasetOwner]: this }, datasetHandler); }
    getAttribute(name) { return this._m.attrs.get(attrName(this, name)) ?? null; }
    getAttributeNS(_, name) { return this.getAttribute(name); }
    hasAttribute(name) { return this._m.attrs.has(attrName(this, name)); }
    hasAttributeNS(_, name) { return this.hasAttribute(name); }
    setAttribute(name, value) { name = attrName(this, name); value = String(value); const attrs = this._m.attrs, oldValue = attrs.get(name) ?? null; if (value !== oldValue) op(4, this._id, name, value); (attrs === noAttrs ? this._m.attrs = new Attrs() : attrs).set(name, value); if (name === 'id' && value !== oldValue) { if (oldValue !== null) idDrop(oldValue, this._m); idAdd(value, this._m); } if (observers.size) change({ type: 'attributes', target: this, attributeName: name, oldValue }); if (definitions.size) attributeReaction(this, name, oldValue, value); if (value !== oldValue && (name === 'slot' || name === 'name' || name === 'open' || name === 'popover')) attributeChanged(this, name, oldValue, value); }
    setAttributeNS(_, name, value) { this.setAttribute(name, value); }
    removeAttribute(name) { name = attrName(this, name); const attrs = this._m.attrs, oldValue = attrs.get(name) ?? null; if (oldValue === null) return; attrs.delete(name); op(5, this._id, name); if (name === 'id') idDrop(oldValue, this._m); if (observers.size) change({ type: 'attributes', target: this, attributeName: name, oldValue }); if (definitions.size) attributeReaction(this, name, oldValue, null); if (name === 'slot' || name === 'name' || name === 'open' || name === 'popover') attributeChanged(this, name, oldValue, null); }
    removeAttributeNS(_, name) { this.removeAttribute(name); }
    toggleAttribute(name, force) { const present = force ?? !this.hasAttribute(name); if (present) this.setAttribute(name, ''); else this.removeAttribute(name); return present; }
    getAttributeNode(name) { name = attrName(this, name); return this._m.attrs.has(name) ? new Attr(this, name, this._m.attrs.get(name)) : null; }
    removeAttributeNode(attr) { if (!(attr instanceof Attr) || attr.ownerElement !== this || !this._m.attrs.has(attr.name)) throw new DOMException('The attribute is not on this element', 'NotFoundError'); attr._value = this._m.attrs.get(attr.name); this.removeAttribute(attr.name); attr.ownerElement = null; return attr; }
    get style() {
      if (this._style) return this._style;
      // The parsed list is shared, not copied, so declaration code must never change its entries.
      let text = null, list = [];
      return this._style = declaration({ read: () => { const now = this.getAttribute('style') ?? ''; if (now !== text) { text = now; list = declare(now); } return list; }, write: next => { text = declarationText(next); list = next; this.setAttribute('style', text); } });
    }
    set style(value) { this.style.cssText = value; }
    get innerHTML() { return this._m.kids.map(serialize).join('') || own(this._m); }
    set innerHTML(value) { setHTML(this, value); }
    get outerHTML() { return serialize(this._m); }
    set outerHTML(value) { const parent = this.parentNode; if (!parent) return; if (parent.nodeType === 9) throw new DOMException('Cannot replace the document element', 'NoModificationAllowedError'); reactionDepth++; try { this.replaceWith(parseFragment(value, parent.nodeType === 1 ? parent : null)); } finally { leave(); } }
    get innerText() { return slot(this) < 0 ? this.textContent : call('dom.innerText', this._id); }
    set innerText(value) { const parts = String(value ?? '').split(/\r\n|\r|\n/); replaceAll(this, () => { release(this); detach(this._m); op(1, this._id, ''); }); quiet++; try { parts.forEach((part, index) => { if (index) this.append(document.createElement('br')); if (part) this.append(part); }); } finally { quiet--; } if (observers.size && this._m.kids.length) change({ type: 'childList', target: this, addedNodes: this.childNodes }); }
    matches(selector) { return selectors('dom.matches', this._id, selector); }
    closest(selector) { return wrap(selectors('dom.closest', this._id, selector)); }
    getBoundingClientRect() { const i = slot(this), m = geometry; return i < 0 ? new DOMRect() : new DOMRect(m[i], m[i + 1], m[i + 2], m[i + 3]); }
    getClientRects() { return slot(this) < 0 ? listOf(DOMRectList, []) : listOf(DOMRectList, [this.getBoundingClientRect()]); }
    get clientWidth() { return rootElement(this) ? innerWidth : Math.round(client(this)[2]); } get clientHeight() { return rootElement(this) ? innerHeight : Math.round(client(this)[3]); }
    get clientLeft() { return Math.round(client(this)[0]); } get clientTop() { return Math.round(client(this)[1]); }
    get scrollTop() { return metric(pageScroller(this) ? document : this, 5); } set scrollTop(value) { call('dom.scroll', this._id, null, Number(value)); }
    get scrollLeft() { return metric(pageScroller(this) ? document : this, 4); } set scrollLeft(value) { call('dom.scroll', this._id, Number(value), null); }
    get scrollWidth() { return pageScroller(this) ? Math.round(innerWidth + metric(document, 6)) : extent(this, 0); } get scrollHeight() { return pageScroller(this) ? Math.round(innerHeight + metric(document, 7)) : extent(this, 1); }
    scrollTo(left, top) { let behavior = null; if (typeof left === 'object') ({ left, top, behavior = null } = left ?? {}); call('dom.scroll', this._id, left ?? null, top ?? null, behavior); }
    scroll(left, top) { this.scrollTo(left, top); }
    scrollBy(left, top) { let behavior = null; if (typeof left === 'object') ({ left, top, behavior = null } = left ?? {}); this.scrollTo({ left: this.scrollLeft + (left ?? 0), top: this.scrollTop + (top ?? 0), behavior }); }
    scrollIntoView(options = {}) {
      if (typeof options === 'boolean') options = { block: options ? 'start' : 'end' };
      const target = this.getBoundingClientRect(), own = getComputedStyle(this), margin = side => parseFloat(own['scrollMargin' + side]) || 0;
      let left = target.left - margin('Left'), top = target.top - margin('Top');
      const width = target.width + margin('Left') + margin('Right'), height = target.height + margin('Top') + margin('Bottom');
      for (let p = this.assignedSlot ?? this.parentElement ?? this.getRootNode().host; p; p = p.assignedSlot ?? p.parentElement ?? p.getRootNode().host) {
        const style = getComputedStyle(p), bounds = p.getBoundingClientRect(), to = [null, null]; let scrollable = false;
        const padding = (side, extent) => { const value = style['scrollPadding' + side]; return value.endsWith('%') ? parseFloat(value) / 100 * extent : parseFloat(value) || 0; };
        for (const [axis, align, start, size, edge, client, near, far] of [
          ['Y', options.block ?? 'start', top, height, bounds.top + p.clientTop, p.clientHeight, 'Top', 'Bottom'],
          ['X', options.inline ?? 'nearest', left, width, bounds.left + p.clientLeft, p.clientWidth, 'Left', 'Right'],
        ]) {
          if (!['auto','scroll','hidden'].includes(style['overflow' + axis] || style.overflow)) continue;
          scrollable = true;
          const inset = padding(near, client), extent = client - inset - padding(far, client);
          const before = start - edge - inset, after = start + size - edge - inset - extent;
          let delta = align === 'center' ? (before + after) / 2 : align === 'end' ? after : before;
          if (align === 'nearest') delta = before < 0 && after > 0 ? 0 : before < 0 ? (size <= extent ? before : after) : after > 0 ? (size <= extent ? after : before) : 0;
          if (delta) to[axis === 'Y' ? 1 : 0] = (axis === 'Y' ? p.scrollTop : p.scrollLeft) + delta;
        }
        if (to[0] !== null || to[1] !== null) {
          const previous = [p.scrollLeft, p.scrollTop], next = call('dom.scroll', p._id, to[0], to[1], options.behavior ?? null);
          left -= next[0] - previous[0]; top -= next[1] - previous[1];
        }
        if (scrollable && options.container === 'nearest') break;
      }
    }
    focus() {
      if (active === this || !focusable(this)) return;
      finishComposition();
      const previous = active; active = this; call('dom.focus', path(this).map(node => node._id), ring = focusVisible(this), !windowFocused);
      if (this.isContentEditable && selection.isCollapsed) selection._send();
      // Defer focus events while the window lacks focus. On window focus, emit them for the current element.
      if (!windowFocused) return;
      const mark = native ? trusted : event => event;
      if (previous) { commit(previous); dispatch(previous, mark(new FocusEvent('blur', { composed: true, relatedTarget: this }))); dispatch(previous, mark(new FocusEvent('focusout', { bubbles: true, composed: true, relatedTarget: this }))); }
      if (active === this) { dispatch(this, mark(new FocusEvent('focus', { composed: true, relatedTarget: previous }))); dispatch(this, mark(new FocusEvent('focusin', { bubbles: true, composed: true, relatedTarget: previous }))); }
    }
    blur() { if (active !== this) return; finishComposition(); const mark = native ? trusted : event => event; active = null; call('dom.focus', []); if (!windowFocused) return; commit(this); dispatch(this, mark(new FocusEvent('blur', { composed: true }))); dispatch(this, mark(new FocusEvent('focusout', { bubbles: true, composed: true }))); }
    click() { if (disabled(this) || this._clicking) return; this._clicking = true; try { fireClick(this); } finally { this._clicking = false; } }
    get contentEditable() { return this.getAttribute('contenteditable') ?? 'inherit'; } set contentEditable(value) { this.setAttribute('contenteditable', value); }
    get isContentEditable() { const value = this.contentEditable.toLowerCase(); return ['true','plaintext-only',''].includes(value) || (value !== 'false' && !!this.parentElement?.isContentEditable); }
    get tabIndex() { const value = parseInt(this.getAttribute('tabindex'), 10), tag = this.localName; return Number.isNaN(value) ? /^(input|textarea|button|select|iframe)$/.test(tag) || (tag === 'a' || tag === 'area') && this.hasAttribute('href') || tag === 'summary' && activation(this) ? 0 : -1 : value; } set tabIndex(value) { this.setAttribute('tabindex', value); }
    attachShadow(options) { if (this._shadow) throw new DOMException('Shadow root already exists', 'NotSupportedError'); if(!['open','closed'].includes(options.mode))throw new TypeError('Invalid shadow mode');const root = document.createDocumentFragment(); shadows++; Object.setPrototypeOf(root,ShadowRoot.prototype);root.host=this;root.mode=options.mode;this._shadow=root;root._m.host=this._m;this._m.shadow=root._m;op(6,this._id,root._id);return root; }
    get shadowRoot() { return this._shadow?.mode === 'open' ? this._shadow : null; }
    setPointerCapture(id) { if (!pointers.has(id)) throw new DOMException('Pointer is not active', 'NotFoundError'); captures.set(id, this); }
    hasPointerCapture(id) { return captures.get(id) === this; }
    releasePointerCapture(id) { if (captures.get(id) === this) { captures.delete(id); dispatch(this, new PointerEvent('lostpointercapture', { pointerId: id, bubbles: true, composed: true })); } }
    insertAdjacentElement(position, node) { position = String(position).toLowerCase(); if (position === 'beforebegin' || position === 'afterend') { if (!this.parentNode) return null; if (position === 'beforebegin') this.before(node); else this.after(node); } else if (position === 'afterbegin') this.prepend(node); else if (position === 'beforeend') this.append(node); else throw new DOMException('Invalid insertion position', 'SyntaxError'); return node; }
    insertAdjacentText(position, text) { this.insertAdjacentElement(position, document.createTextNode(String(text))); }
    insertAdjacentHTML(position, source) { const outside = /^(beforebegin|afterend)$/i.test(position), context = outside ? this.parentNode : this; if (outside && (!context || context.nodeType === 9)) throw new DOMException('The element has no parent', 'NoModificationAllowedError'); this.insertAdjacentElement(position, parseFragment(source, context.nodeType === 1 && !(context.localName === 'html' && context.namespaceURI === XHTML) ? context : null)); }
  }
  for (const [Type, names] of [[Node, ['insertBefore', 'removeChild', 'replaceChild', 'cloneNode']], [Element, ['setAttribute', 'removeAttribute', 'insertAdjacentHTML']]]) for (const name of names) { const fn = Type.prototype[name]; Type.prototype[name] = { [name](a, b) { reactionDepth++; try { return fn.call(this, a, b); } finally { leave(); } } }[name]; }
  const attributeReaction = (node, name, oldValue, value) => { const Type = definitions.get(node._m.tag); if (Type && node instanceof Type && !node._failed && defined.get(Type).observed.has(name)) enqueue(node, ['attributeChangedCallback', name, oldValue, value]); };
  class HTMLElement extends Element {
    get offsetWidth() { return Math.round(metric(this, 2)); } get offsetHeight() { return Math.round(metric(this, 3)); }
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
  class SVGElement extends Element {constructor(id,tag){super(id,tag);this.namespaceURI=SVG;this.nodeName=this.tagName=tag;}getBBox(){layout();const [x,y,width,height]=call('dom.svgBox',this._id);return new DOMRect(x,y,width,height);}get ownerSVGElement(){for(let node=this.parentElement;node;node=node.parentElement)if(node instanceof SVGSVGElement)return node;return null;}get viewportElement(){return this.ownerSVGElement;}}
  class SVGSVGElement extends SVGElement {}
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
  const nativeTypes = new Map([['ns-button','button'],['ns-switch','checkbox'],['ns-range','range'],['ns-text','text'],['ns-search','search'],['ns-color','color'],['ns-date','date']]);
  const editable = node => node instanceof HTMLTextAreaElement || node instanceof HTMLInputElement && /^(text|search|tel|url|email|password|number)$/.test(node.type);
  const setValue = (node, value) => { node._value = value; op(7, node._id, value, 0); const length = value.length; node._start = Math.min(node._start ?? length, length); node._end = Math.min(node._end ?? length, length); };
  const rangeBounds = node => { const number = name => { const value = parseFloat(node.getAttribute(name)); return Number.isFinite(value) ? value : null; }, min = number('min') ?? 0, step = number('step'); return [min, Math.max(min, number('max') ?? 100), node.getAttribute('step')?.trim().toLowerCase() === 'any' ? 0 : step > 0 ? step : 1]; };
  const rangeValue = (node, raw) => {
    const [min, max, step] = rangeBounds(node); let value = String(raw ?? '').trim() ? Number(raw) : NaN;
    value = Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : (min + max) / 2;
    if (step) { let n = Math.round((value - min) / step); if (min + n * step > max) n--; value = +(min + n * step).toPrecision(15); }
    return String(value);
  };
  const setRange = (node, raw) => { const value = rangeValue(node, raw); if (value !== node.value) { setValue(node, value); dispatch(node, trusted(new Event('input', { bubbles: true, composed: true }))); } };
  const formState = (node, bits) => { if (node._bits !== bits) { node._bits = bits; op(8, node._id, bits); } };
  // User edits must change internal state, not value or checked properties: frameworks intercept those
  // properties to detect script writes.
  const setChecked = (node, value) => { node._checked = value; formState(node, node._bits & ~3 | (value ? 3 : 2)); if (value && node.type === 'radio') for (const other of group(node)) if (other !== node && (other._checked ?? other.hasAttribute('checked'))) { other._checked = false; formState(other, other._bits & ~3 | 2); } };
  const formOf = node => { const id = node.getAttribute('form'); if (id !== null) { const form = id && node.getRootNode().getElementById?.(id); return form?.localName === 'form' ? form : null; } for (let rec = node._m.up; rec && rec.tag[0] !== '#'; rec = rec.up) if (rec.tag === 'form') return of(rec); return null; };
  const listed = /^(button|fieldset|input|object|output|select|textarea)$/;
  const candidate = node => !disabled(node) && !node.hasAttribute('readonly') && !node._m.up?.tag?.startsWith('datalist') && (node.localName === 'input' ? !/^(hidden|reset|button)$/.test(node.type) : node.localName === 'button' ? node.type === 'submit' : /^(select|textarea)$/.test(node.localName));
  const emailPattern = /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;
  class ValidityState {}
  for (const name of ['valueMissing','typeMismatch','patternMismatch','tooLong','tooShort','rangeUnderflow','rangeOverflow','stepMismatch','badInput','customError','valid']) Object.defineProperty(ValidityState.prototype, name, { get() { return this['_' + name]; }, enumerable: true });
  const messages = { valueMissing: 'Please fill out this field.', typeMismatch: 'Please enter a valid value.', patternMismatch: 'Please match the requested format.', tooLong: 'Please shorten this text.', tooShort: 'Please lengthen this text.', rangeUnderflow: 'Value is too low.', rangeOverflow: 'Value is too high.', stepMismatch: 'Please enter a valid value.', badInput: 'Please enter a number.' };
  const validity = node => {
    const flags = { valueMissing: false, typeMismatch: false, patternMismatch: false, tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: !!node._custom };
    const tag = node.localName, type = node.type, required = node.hasAttribute('required');
    if (tag === 'select') flags.valueMissing = required && !node.selectedOptions.some(option => option.value !== '' || option.index !== 0 || node.multiple || node.size > 1);
    else if (type === 'checkbox') flags.valueMissing = required && !node.checked;
    else if (type === 'radio') flags.valueMissing = group(node).some(radio => radio.hasAttribute('required')) && !group(node).some(radio => radio.checked);
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
  const constraints = {
    get willValidate() { return candidate(this); },
    get validity() { return validity(this); },
    get validationMessage() { if (!candidate(this)) return ''; const state = validity(this); return state.customError ? this._custom : Object.keys(messages).map(name => state[name] && messages[name]).find(Boolean) ?? ''; },
    setCustomValidity(message) { this._custom = String(message); },
    checkValidity() { if (!candidate(this) || validity(this).valid) return true; dispatch(this, new Event('invalid', { cancelable: true })); return false; },
    reportValidity() { if (this.checkValidity()) return true; this.focus(); return false; },
    get form() { return formOf(this); },
    get labels() {
      const result = [], root = this.getRootNode(), id = this.getAttribute('id');
      if (id && root.querySelectorAll) for (const label of root.querySelectorAll('label[for=' + quote(id) + ']')) if (labelControl(label) === this) result.push(label);
      for (let rec = this._m.up; rec; rec = rec.up) if (rec.tag === 'label' && !rec.attrs.has('for') && labelControl(of(rec)) === this) result.push(of(rec));
      return listOf(NodeList, result.sort((a, b) => a.compareDocumentPosition(b) & 4 ? -1 : 1));
    },
    get name() { return this.getAttribute('name') ?? ''; }, set name(value) { this.setAttribute('name', value); },
  };
  class HTMLInputElement extends HTMLElement {
    get type() { const type = (this.getAttribute('type') ?? '').toLowerCase(); return inputTypes.has(type) ? type : nativeTypes.get(type) ?? 'text'; } set type(value) { this.setAttribute('type', value); }
    get value() { const type = this.type; return /^(hidden|submit|image|reset|button)$/.test(type) ? this.getAttribute('value') ?? '' : type === 'checkbox' || type === 'radio' ? this.getAttribute('value') ?? 'on' : type === 'file' ? '' : type === 'range' ? rangeValue(this, this._value ?? this.getAttribute('value')) : this._value ?? this.getAttribute('value')?.replace(/[\r\n]/g, '') ?? ''; }
    set value(value) { value = String(value ?? ''); if (/^(hidden|submit|image|reset|button|checkbox|radio)$/.test(this.type)) this.setAttribute('value', value); else if (this.type !== 'file' || !value) { if (!value) this._files = null; setValue(this, value); } }
    get defaultValue() { return this.getAttribute('value') ?? ''; } set defaultValue(value) { this.setAttribute('value', value); }
    get checked() { return this._checked ?? this.hasAttribute('checked'); }
    set checked(value) { setChecked(this, !!value); }
    get defaultChecked() { return this.hasAttribute('checked'); } set defaultChecked(value) { this.toggleAttribute('checked', !!value); }
    get indeterminate() { return !!this._indeterminate; } set indeterminate(value) { this._indeterminate = !!value; }
    get valueAsNumber() { return /^(number|range)$/.test(this.type) && this.value !== '' ? Number(this.value) : NaN; } set valueAsNumber(value) { this.value = Number.isNaN(value) ? '' : String(value); }
    get files() { return this.type === 'file' ? this._files ?? [] : null; }
    get list() { const id = this.getAttribute('list'); return id ? document.getElementById(id) : null; }
    get selectionStart() { return this._start ?? 0; } set selectionStart(value) { this.setSelectionRange(value, Math.max(value, this.selectionEnd)); }
    get selectionEnd() { return this._end ?? 0; } set selectionEnd(value) { this.setSelectionRange(Math.min(value, this.selectionStart), value); }
    get selectionDirection() { return this._direction ?? 'none'; } set selectionDirection(value) { this.setSelectionRange(this.selectionStart, this.selectionEnd, value); }
    setSelectionRange(start, end, direction = 'none') {
      const length = this.value.length;
      end = Math.min(Number(end) >>> 0, length); start = Math.min(Number(start) >>> 0, end);
      direction = ['forward','backward'].includes(direction) ? direction : 'none';
      const changed = start !== this.selectionStart || end !== this.selectionEnd || direction !== this.selectionDirection;
      this._start = start; this._end = end; this._direction = direction;
      if (active === this) call('dom.selection', this._id, start, end);
      if (changed) { selectionChanged(this); setTimeout(() => this.dispatchEvent(new Event('select', { bubbles: true })), 0); }
    }
    // Chromium and WebKit focus the field in `select()`; editors rely on it to move typing into the field.
    select() { this.focus(); this.setSelectionRange(0, this.value.length); }
    setRangeText(text, start = this.selectionStart, end = this.selectionEnd, mode = 'preserve') {
      text = String(text); start = Number(start) >>> 0; end = Number(end) >>> 0;
      if (start > end) throw new DOMException('Invalid replacement range', 'IndexSizeError');
      if (!['select','start','end','preserve'].includes(mode)) throw new TypeError('Invalid selection mode');
      const value = this.value, oldStart = this.selectionStart, oldEnd = this.selectionEnd;
      start = Math.min(start, value.length); end = Math.min(end, value.length);
      this.value = value.slice(0, start) + text + value.slice(end);
      const after = start + text.length, delta = text.length - (end - start);
      this.setSelectionRange(mode === 'select' || mode === 'start' ? start : mode === 'end' ? after : oldStart > end ? oldStart + delta : oldStart > start ? start : oldStart,
        mode === 'select' || mode === 'end' ? after : mode === 'start' ? start : oldEnd > end ? oldEnd + delta : oldEnd > start ? after : oldEnd);
    }
  }
  class HTMLTextAreaElement extends HTMLElement {
    get type() { return 'textarea'; }
    get value() { return this._value ?? this.textContent.replace(/\r\n?/g, '\n'); } set value(value) { setValue(this, String(value ?? '').replace(/\r\n?/g, '\n')); }
    get defaultValue() { return this.textContent; } set defaultValue(value) { this.textContent = value; }
    get textLength() { return this.value.length; }
  }
  for (const name of ['selectionStart', 'selectionEnd', 'selectionDirection', 'setSelectionRange', 'select', 'setRangeText']) Object.defineProperty(HTMLTextAreaElement.prototype, name, Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, name));
  for (const attr of ['accept', 'alt', 'autocomplete', 'max', 'min', 'pattern', 'step']) Object.defineProperty(HTMLInputElement.prototype, attr, { get() { return this.getAttribute(attr) ?? ''; }, set(value) { this.setAttribute(attr, value); } });
  const options = select => { const result = []; for (const kid of select._m.kids) { if (kid.tag === 'option') result.push(of(kid)); else if (kid.tag === 'optgroup') for (const sub of kid.kids) if (sub.tag === 'option') result.push(of(sub)); } return result; };
  const selectedness = select => {
    const list = options(select);
    if (select.multiple || select.size > 1) for (const option of list) option._current = option._selected ?? option.hasAttribute('selected');
    else {
      let chosen = null;
      for (const option of list) if (option._selected ?? option.hasAttribute('selected')) chosen = option;
      if (!chosen && !select._none) chosen = list.find(option => !disabled(option)) ?? null;
      for (const option of list) option._current = option === chosen;
    }
    for (const option of list) formState(option, option._current ? 3 : 2);
    return list;
  };
  class HTMLOptionsCollection extends List {
    get selectedIndex() { return this._select.selectedIndex; } set selectedIndex(value) { this._select.selectedIndex = value; }
    add(element, before) { this._select.add(element, before); }
    remove(index) { this._select.remove(index); }
    namedItem(name) { return this.find(option => option.id === name || option.getAttribute('name') === name) ?? null; }
  }
  class HTMLSelectElement extends HTMLElement {
    get type() { return this.multiple ? 'select-multiple' : 'select-one'; }
    get options() { const list = listOf(HTMLOptionsCollection, options(this)); list._select = this; return list; }
    get length() { return options(this).length; }
    set length(value) { const list = options(this); value >>>= 0; for (let i = list.length; i < value; i++) this.append(document.createElement('option')); for (const option of list.slice(value)) option.remove(); }
    get size() { return Math.max(0, parseInt(this.getAttribute('size'), 10) || 0); } set size(value) { this.setAttribute('size', value >>> 0); }
    get selectedOptions() { return listOf(HTMLCollection, selectedness(this).filter(option => option._current)); }
    get selectedIndex() { return selectedness(this).findIndex(option => option._current); }
    set selectedIndex(value) { value = Number(value); const list = options(this); this._none = !list[value]; for (const [index, option] of list.entries()) option._selected = index === value; selectedness(this); }
    get value() { return selectedness(this).find(option => option._current)?.value ?? ''; }
    set value(value) { value = String(value); const list = options(this); let found = false; for (const option of list) option._selected = !found && option.value === value && (found = true); this._none = !found; selectedness(this); }
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
    get selected() { const select = this.select; if (select) selectedness(select); return this._current ?? this._selected ?? this.hasAttribute('selected'); }
    set selected(value) { const select = this.select; this._selected = !!value; if (select) { if (value && !select.multiple) for (const option of options(select)) if (option !== this) option._selected = false; select._none = false; selectedness(select); } else this._current = !!value; }
    get defaultSelected() { return this.hasAttribute('selected'); } set defaultSelected(value) { this.toggleAttribute('selected', !!value); }
    get index() { const select = this.select; return select ? options(select).indexOf(this) : 0; }
    get form() { return this.select?.form ?? null; }
  }
  class HTMLOptGroupElement extends HTMLElement { get label() { return this.getAttribute('label') ?? ''; } set label(value) { this.setAttribute('label', value); } }
  class HTMLButtonElement extends HTMLElement {
    get type() { const type = (this.getAttribute('type') ?? '').toLowerCase(); return /^(submit|reset|button)$/.test(type) ? type : this.hasAttribute('commandfor') ? 'button' : 'submit'; } set type(value) { this.setAttribute('type', value); }
    get value() { return this.getAttribute('value') ?? ''; } set value(value) { this.setAttribute('value', value); }
  }
  class HTMLFieldSetElement extends HTMLElement { get elements() { const result = []; find(this._m, rec => { if (listed.test(rec.tag)) result.push(of(rec)); return false; }); return listOf(HTMLCollection, result); } get type() { return 'fieldset'; } }
  class HTMLOutputElement extends HTMLElement { get value() { return this.textContent; } set value(value) { this.textContent = value; } get defaultValue() { return this._default ?? this.textContent; } set defaultValue(value) { this._default = String(value); } get type() { return 'output'; } }
  for (const Type of [HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLButtonElement, HTMLFieldSetElement, HTMLOutputElement]) for (const [name, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(constraints))) if (!Object.hasOwn(Type.prototype, name)) Object.defineProperty(Type.prototype, name, descriptor);
  class HTMLFormControlsCollection extends HTMLCollection {}
  const submitForm = (form, submitter) => {
    if (form._submitting || !form.isConnected) return;
    if (!form.noValidate && !submitter?.formNoValidate) { const invalid = [...form.elements].filter(control => !control.checkValidity()); if (invalid.length) { invalid[0].focus(); return; } }
    form._submitting = true;
    let allowed;
    try { allowed = dispatch(form, new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter })); } finally { form._submitting = false; }
    if (allowed && (submitter?.getAttribute('formmethod') ?? form.method) === 'dialog') form.closest('dialog')?.close(submitter?.value);
  };
  class HTMLFormElement extends HTMLElement {
    get elements() {
      const id = this.getAttribute('id'), root = this.getRootNode(), result = [], control = rec => listed.test(rec.tag) && !(rec.tag === 'input' && rec.attrs.get('type')?.toLowerCase() === 'image') && of(rec).form === this;
      find(this._m, rec => { if (control(rec)) result.push(of(rec)); return false; });
      const outside = id && root !== this && root.querySelectorAll ? [...root.querySelectorAll('[form=' + quote(id) + ']')].filter(node => !this.contains(node) && control(node._m)) : [];
      if (outside.length) { result.push(...outside); result.sort((a, b) => a.compareDocumentPosition(b) & 4 ? -1 : 1); }
      const list = listOf(HTMLFormControlsCollection, result); for (const control of result) for (const key of [control.id, control.getAttribute('name')]) if (key && !(key in list)) list[key] = control; return list;
    }
    get length() { return this.elements.length; }
    get method() { const value = (this.getAttribute('method') ?? '').toLowerCase(); return value === 'post' || value === 'dialog' ? value : 'get'; } set method(value) { this.setAttribute('method', value); }
    get action() { const value = this.getAttribute('action'); return value ? resolved(value) : document.URL; } set action(value) { this.setAttribute('action', value); }
    get enctype() { return this.getAttribute('enctype') ?? 'application/x-www-form-urlencoded'; } set enctype(value) { this.setAttribute('enctype', value); }
    get target() { return this.getAttribute('target') ?? ''; } set target(value) { this.setAttribute('target', value); }
    submit() { if ((this.method) === 'dialog') this.closest('dialog')?.close(); }
    requestSubmit(submitter = null) { if (submitter !== null && (!(submitter instanceof HTMLElement) || submitter.form !== this || !(submitter.localName === 'button' ? submitter.type === 'submit' : /^(submit|image)$/.test(submitter.type)))) throw submitter?.form !== this ? new DOMException('The submitter is not owned by this form', 'NotFoundError') : new TypeError('The submitter is not a submit button'); submitForm(this, submitter); }
    reset() {
      if (!dispatch(this, new Event('reset', { bubbles: true, cancelable: true }))) return;
      for (const control of this.elements) {
        if (control.localName === 'select') { for (const option of options(control)) option._selected = undefined; control._none = false; selectedness(control); }
        else if (control.localName === 'input' || control.localName === 'textarea') { control._value = undefined; control._checked = undefined; control._edited = undefined; op(7, control._id, '', 1); formState(control, control._bits & ~3); }
        else if (control.localName === 'output') control.textContent = control.defaultValue;
      }
    }
    checkValidity() { return [...this.elements].map(control => control.checkValidity?.() ?? true).every(Boolean); }
    reportValidity() { const invalid = [...this.elements].filter(control => !(control.checkValidity?.() ?? true)); invalid[0]?.focus(); return !invalid.length; }
    _entries(submitter) {
      const entries = [];
      for (const control of this.elements) {
        const tag = control.localName, type = control.type, name = control.getAttribute('name');
        if (!name || disabled(control) || control.closest('datalist') || tag === 'button' && control !== submitter || tag === 'input' && /^(submit|image|reset|button)$/.test(type) && control !== submitter || /^(checkbox|radio)$/.test(type) && !control.checked || tag === 'object' || tag === 'output' || tag === 'fieldset') continue;
        if (tag === 'select') for (const option of control.selectedOptions) entries.push([name, option.value]);
        else if (type === 'file') for (const file of control.files.length ? control.files : [new File([], '', { type: 'application/octet-stream' })]) entries.push([name, file]);
        else entries.push([name, control.value]);
      }
      return entries;
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
      toggled(this, 'open', 'closed');
      this.removeAttribute('open');
      if (result !== undefined && result !== null) this.returnValue = result;
      const previous = this._focus; this._focus = null;
      if (previous?.isConnected && active && this.contains(active)) previous.focus();
      setTimeout(() => dispatch(this, new Event('close')), 0);
    }
    requestClose(result) { if (this.open && dispatch(this, new Event('cancel', { cancelable: true }))) this.close(result); }
  }
  const closeRequest = () => {
    const top = watchers.at(-1);
    if (!top) return false;
    if (top._popover) hidePopover(top, true, true);
    else if (top.closedBy !== 'none' && dispatch(top, trusted(new Event('cancel', { cancelable: true })))) top.close();
    return true;
  };
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
  const lightDismiss = (down, target, x, y) => {
    if (autos.length || hints.length) {
      const clicked = nearestPopover(target, [...autos, ...hints]), button = path(target).map(node => node.nodeType === 1 && invoked(node)).find(popover => popover?._popover && popover._popover !== 'manual') ?? null;
      const ancestor = position(clicked) > position(button) ? clicked : button;
      if (down) pressedPopover = ancestor; else { const same = ancestor === pressedPopover; pressedPopover = null; if (same) hideUntil(ancestor ?? document); }
    }
    if (dialogs.length) {
      let clicked = path(target).find(node => node.localName === 'dialog' && dialogs.includes(node)) ?? null;
      if (clicked === target && clicked._modal) { const box = clicked.getBoundingClientRect(); if (x < box.left || y < box.top || x >= box.right || y >= box.bottom) clicked = null; }
      if (down) pressedDialog = clicked;
      else { const same = clicked === pressedDialog, top = dialogs.at(-1); pressedDialog = null; if (same && top && top !== clicked && top.closedBy === 'any') top.requestClose(); }
    }
  };
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
    get content() { if (!this._content) { this._content = new DocumentFragment(); this._content.append(...this.childNodes); } return this._content; }
    get innerHTML() { return this.content._m.kids.map(serialize).join(''); } set innerHTML(value) { setHTML(this.content, value, true); }
    cloneNode(deep = false) { const clone = super.cloneNode(false); if (deep) clone.content.append(this.content.cloneNode(true)); return clone; }
  }
  class HTMLImageElement extends HTMLElement {
    get naturalWidth() { return this._dimensions?.[0] ?? 0; } get naturalHeight() { return this._dimensions?.[1] ?? 0; }
    get width() { return Number(this.getAttribute('width') ?? this.naturalWidth); } set width(value) { this.setAttribute('width',value); }
    get height() { return Number(this.getAttribute('height') ?? this.naturalHeight); } set height(value) { this.setAttribute('height',value); }
    get complete() { return !this.src || !!this._complete; }
    setAttribute(name,value) { super.setAttribute(name,value); if (name === 'src') this._loadImage(); }
    _loadImage() {
      const raw = this.getAttribute('src'), source = this.src, load = this._load = {}, current = () => this._load === load && this.getAttribute('src') === raw; this._complete = false; if (this._bits & 32) flag(this, 32, false);
      this._loading = (async () => {
        const data = await resource(source, 'Image');
        if (!current()) return;
        const result = await decode(this, data);
        if (!current()) return;
        if (typeof result === 'string') throw new Error(result);
        this._dimensions=result; this._complete=true; this.dispatchEvent(new Event('load'));
      })();
      this._loading.catch(()=>{ if (current()) {this._complete=true; flag(this, 32, true); this.dispatchEvent(new Event('error'));} });
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
  const players = new Map(), playing = new Set(), mediaFeatures = globalThis.__appsMedia ?? {};
  delete globalThis.__appsMedia;
  const [PLAY, PAUSE, SEEK, VOLUME, MUTED, SPEED] = [0, 1, 2, 3, 4, 5];
  const control = (el, op, value = 0) => { if (el._open) call('media.control', el._id, op, +value); };
  // Without the media feature, simpleAudio plays only PCM WAV.
  const mediaTypes = mediaFeatures.media ? /^(video|audio)\/(mp4|webm|ogg|quicktime|x-matroska|mpeg|mp3|x-m4a|aac|flac|x-flac|wav|wave|x-wav)$/ : /^audio\/(wav|wave|x-wav)$/, mediaCodecs = mediaFeatures.media ? /^(avc[13]|hvc1|hev1|vp8|vp9|vp09|av01|mp4a|mp3|opus|vorbis|flac|alac|1)(\.|$)/ : /^1$/;
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
    get preload() { const value = this.getAttribute('preload')?.toLowerCase(); return value === 'none' || value === 'metadata' ? value : 'auto'; } set preload(value) { this.setAttribute('preload', value); }
    get buffered() { return new TimeRanges(this._ready ? [[0, this.duration]] : []); } get seekable() { return this.buffered; } get played() { return new TimeRanges(); }
    get srcObject() { return null; } set srcObject(value) { if (value !== null) throw new DOMException('Media streams are not supported', 'NotSupportedError'); }
    canPlayType(type) {
      if (!this._enabled) return '';
      const [mime, ...params] = String(type).toLowerCase().split(';').map(part => part.trim()), codecs = params.find(part => part.startsWith('codecs='))?.slice(7).replace(/"/g, '');
      return !mediaTypes.test(mime) ? '' : codecs === undefined ? 'maybe' : codecs.split(',').every(codec => mediaCodecs.test(codec.trim())) ? 'probably' : '';
    }
    get _enabled() { return mediaFeatures.media || (mediaFeatures.simpleAudio && this.localName === 'audio'); }
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
    _fire(type) { setTimeout(() => this.dispatchEvent(new Event(type)), 0); }
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
      const token = this._token = {};
      if (this._network) this._fire('emptied');
      if (this._open) { this._open = false; call('media.close', this._id); }
      if (!this.paused) { this._paused = true; playing.delete(this); this._fire('pause'); }
      this._settle(new DOMException('The play() request was interrupted by a new load request', 'AbortError'));
      Object.assign(this, { _ready: 0, _time: 0, _seeking: false, _ended: false, _error: null, _duration: NaN, _size: null, _source: element?.src ?? '', _network: element ? 2 : 0 });
      this._poster();
      if (!element) return;
      this._fire('loadstart');
      if (!this._enabled) return this._fail(4, `The media feature is disabled. Add "media"${this.localName === 'audio' ? ' or, for WAV files only, "simpleAudio"' : ''} to "features" in package.json`);
      if (!this._source) return this._fail(4, 'The media has no source');
      if (this.preload === 'none' && !this.autoplay && !force) { this._network = 1; this._fire('suspend'); return; }
      resource(this._source, 'Media').then(data => {
        if (this._token !== token) return;
        call('media.open', this._id, data, this.localName === 'video');
        this._open = true; players.set(this._id, new WeakRef(this));
        if (this.volume !== 1) control(this, VOLUME, this.volume);
        if (this.muted) control(this, MUTED, 1);
        if (this.playbackRate !== 1) control(this, SPEED, this.playbackRate);
        if (this.autoplay && this.paused) this.play().catch(() => {});
        else if (!this.paused) control(this, PLAY);
      }).catch(error => { if (this._token === token) this._fail(4, error.message); });
    }
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
  class UIEvent extends Event { constructor(type, options = {}) { super(type, options); this.view = options.view ?? window; this.detail = options.detail ?? 0; } }
  const mouseFields = { screenX: 0, screenY: 0, clientX: 0, clientY: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, button: 0, buttons: 0, relatedTarget: null, movementX: 0, movementY: 0 };
  class MouseEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); for (const key in mouseFields) this[key] = options[key] ?? mouseFields[key]; }
    get x() { return this.clientX; } get y() { return this.clientY; }
    get pageX() { return this.clientX + scrollX; } get pageY() { return this.clientY + scrollY; }
    get offsetX() { const target = this.target; return target?.getBoundingClientRect ? this.clientX - target.getBoundingClientRect().left - target.clientLeft : this.clientX; }
    get offsetY() { const target = this.target; return target?.getBoundingClientRect ? this.clientY - target.getBoundingClientRect().top - target.clientTop : this.clientY; }
    get which() { return this.button + 1; }
    getModifierState(key) { return !!this[({ Control: 'ctrlKey', Meta: 'metaKey', Alt: 'altKey', Shift: 'shiftKey' })[key]]; }
  }
  class WheelEvent extends MouseEvent { constructor(type, options = {}) { super(type, options); this.deltaX = options.deltaX ?? 0; this.deltaY = options.deltaY ?? 0; this.deltaZ = options.deltaZ ?? 0; this.deltaMode = options.deltaMode ?? 0; } }
  for (const [name, value] of Object.entries({ DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 })) { WheelEvent[name] = value; WheelEvent.prototype[name] = value; }
  class ToggleEvent extends Event { constructor(type, options = {}) { super(type, options); this.oldState = options.oldState ?? ''; this.newState = options.newState ?? ''; this.source = options.source ?? null; } }
  class SubmitEvent extends Event { constructor(type, options = {}) { super(type, options); this.submitter = options.submitter ?? null; } }
  class KeyboardEvent extends UIEvent { constructor(type, options = {}) { super(type, options); const { bubbles, cancelable, composed, ...fields } = options; Object.assign(this, { key: '', code: '', keyCode: 0, charCode: 0, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, repeat: false, isComposing: false }, fields); } getModifierState(key) { return !!this[({ Control: 'ctrlKey', Meta: 'metaKey', Alt: 'altKey', Shift: 'shiftKey' })[key]]; } }
  class PointerEvent extends MouseEvent { constructor(type, options = {}) { super(type, options); Object.assign(this, { pointerId: options.pointerId ?? 0, width: options.width ?? 1, height: options.height ?? 1, pressure: options.pressure ?? (this.buttons ? 0.5 : 0), tangentialPressure: options.tangentialPressure ?? 0, tiltX: options.tiltX ?? 0, tiltY: options.tiltY ?? 0, twist: options.twist ?? 0, altitudeAngle: options.altitudeAngle ?? Math.PI / 2, azimuthAngle: options.azimuthAngle ?? 0, pointerType: options.pointerType ?? '', isPrimary: options.isPrimary ?? false }); } getCoalescedEvents() { return []; } getPredictedEvents() { return []; } }
  class FocusEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.relatedTarget = options.relatedTarget ?? null; } }
  class InputEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.data = options.data ?? null; this.inputType = options.inputType ?? ''; this.isComposing = !!options.isComposing; this.dataTransfer = options.dataTransfer ?? null; this._targetRanges = Array.from(options.targetRanges ?? []); }
    getTargetRanges() { return this._targetRanges.slice(); }
  }
  const droppedDirectories = new WeakSet();
  class DataTransfer {
    constructor(){this.dropEffect='none';this.effectAllowed='all';this._data=new Map();this.files=[];this.items=[];this.items.add=(value,type)=>{if(value instanceof File){this.files.push(value);const directory=droppedDirectories.has(value),item={kind:'file',type:value.type,getAsFile:()=>value,getAsString:()=>{},webkitGetAsEntry:()=>({isFile:!directory,isDirectory:directory,name:value.name,fullPath:'/'+value.name})};this.items.push(item);return item;}this.setData(type,String(value));return this.items.at(-1);};this.items.remove=index=>{const item=this.items[index];if(item?.kind==='file')this.files.splice(this.files.indexOf(item.getAsFile()),1);this.items.splice(index,1);};this.items.clear=()=>{this.items.length=0;this.files.length=0;this._data.clear();};}
    get types(){return [...this._data.keys(),...(this.items.some(item=>item.kind==='file')?['Files']:[])];}getData(type){return this._data.get(type==='text'?'text/plain':type)??'';}setData(type,data){type=type==='text'?'text/plain':type;this._data.set(type,String(data));const item={kind:'string',type,getAsFile:()=>null,getAsString:callback=>queueMicrotask(()=>callback(this.getData(type)))};const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index<0)this.items.push(item);else this.items[index]=item;}clearData(type){if(type===undefined){this._data.clear();this.items.splice(0,this.items.length,...this.items.filter(item=>item.kind==='file'));}else{this._data.delete(type);const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index>=0)this.items.splice(index,1);}}
  }
  class ClipboardEvent extends Event {constructor(type,options={}){super(type,options);this.clipboardData=options.clipboardData??null;}}
  class DragEvent extends MouseEvent {constructor(type,options={}){super(type,options);this.dataTransfer=options.dataTransfer??null;}}
  class CompositionEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.data = options.data ?? ''; } }
  class DocumentFragment extends Node { constructor(id, tag) { super(tag ? id : create('#fragment'), '#fragment'); } }
  const ruleText = data => {
    const block = list => list.length ? ' ' + list.map(ruleText).join(' ') : '', declarations = list => list.map(([name, value, priority]) => `${name}: ${value}${priority ? ' !important' : ''};`).join(' ');
    switch (data[0]) {
      case 'style': return `${data[1]} { ${declarations(data[2])}${block(data[3])} }`.replace('{  }', '{ }');
      case 'media': return `@media ${data[1].join(', ')} {${block(data[2])} }`;
      case 'supports': return `@supports ${data[1]} {${block(data[2])} }`;
      case 'layer': return `@layer ${data[1]} {${block(data[2])} }`;
      case 'layers': return `@layer ${data[1].join(', ')};`;
      case 'container': return `@container ${data[1]} {${block(data[2])} }`;
      case 'keyframes': return `@keyframes ${data[1]} {${data[2].map(frame => ` ${frame[0]} { ${declarations(frame[1])} }`).join('')} }`;
      case 'font-face': return `@font-face { ${declarations(data[1])} }`;
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
    _path() { const index = this._parent.cssRules.indexOf(this); return this._parent === this._sheet ? [index] : [...this._parent._path(), index]; }
  }
  class CSSGroupingRule extends CSSRule {
    get _children() { return this._data[this._data[0] === 'style' ? 3 : 2]; }
    get cssRules() { return this._rules ??= CSSRuleList.from(this._children, data => makeRule(this._sheet, this, data)); }
    insertRule(rule, index = 0) { index = Number(index) >>> 0; if (index > this._children.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); const data = this._sheet._edit('insert', [...this._path(), index], String(rule)); this._children.splice(index, 0, data); this.cssRules.splice(index, 0, makeRule(this._sheet, this, data)); return index; }
    deleteRule(index) { index = Number(index) >>> 0; if (index >= this._children.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); this._sheet._edit('delete', [...this._path(), index]); this._children.splice(index, 1); this.cssRules.splice(index, 1); }
  }
  class CSSStyleRule extends CSSGroupingRule {
    get selectorText() { return this._data[1]; }
    set selectorText(value) { value = String(value).trim(); if (!value || value === this._data[1]) return; this._sheet._edit('selector', this._path(), value); this._data[1] = value; }
    get style() { return this._style ??= declaration({ rule: this, read: () => this._list ??= declare(this._data[2].map(([name, value, priority]) => `${name}:${value}${priority ? ' !important' : ''}`).join(';')), write: list => { this._list = list; this._data[2] = list; this._sheet._edit('style', this._path(), declarationText(list)); } }); }
    set style(value) { this.style.cssText = value; }
  }
  class CSSMediaRule extends CSSGroupingRule { get media() { return this._media ??= new MediaList(() => this._data[1]); } get conditionText() { return this._data[1].join(', '); } }
  class CSSSupportsRule extends CSSGroupingRule { get conditionText() { return this._data[1]; } }
  class CSSContainerRule extends CSSGroupingRule { get conditionText() { return this._data[1]; } get containerQuery() { return this._data[1].replace(/^(?!not\b|\()[\w-]+\s+/, ''); } get containerName() { return /^(?!not\b|\()([\w-]+)\s/.exec(this._data[1])?.[1] ?? ''; } }
  class CSSLayerBlockRule extends CSSGroupingRule { get name() { return this._data[1]; } }
  class CSSLayerStatementRule extends CSSRule { get nameList() { return Object.freeze(this._data[1].slice()); } }
  class CSSKeyframeRule extends CSSRule {
    get keyText() { return this._data[0]; } set keyText(value) { this._data[0] = String(value); }
    get type() { return 8; } get cssText() { return `${this._data[0]} { ${declarationText(this._data[1])} }`; }
    get style() { return this._style ??= declaration({ rule: this, raw: true, read: () => this._data[1], write: list => { this._data[1] = list; } }); }
  }
  const keyText = key => String(key).trim().toLowerCase().replace(/^from$/, '0%').replace(/^to$/, '100%');
  class CSSKeyframesRule extends CSSRule {
    get name() { return this._data[1]; } set name(value) { this._data[1] = String(value); }
    get cssRules() { return this._rules ??= CSSRuleList.from(this._data[2], data => new CSSKeyframeRule(this._sheet, this, data)); }
    get length() { return this.cssRules.length; }
    findRule(key) { key = keyText(key); return this.cssRules.findLast(rule => rule._data[0] === key) ?? null; }
    appendRule(text) { const frames = this._sheet._edit('keyframe', this._path(), String(text)); for (const data of frames ?? []) { this._data[2].push(data); this.cssRules.push(new CSSKeyframeRule(this._sheet, this, data)); } }
    deleteRule(key) { const rule = this.findRule(key); if (!rule) return; const index = this.cssRules.indexOf(rule); this._sheet._edit('keyframe', this._path(), String(index)); this._data[2].splice(index, 1); this.cssRules.splice(index, 1); }
  }
  class CSSFontFaceRule extends CSSRule { get style() { return this._style ??= declaration({ rule: this, raw: true, read: () => this._data[1], write: list => { this._data[1] = list; } }); } }
  class CSSPropertyRule extends CSSRule { get name() { return this._data[1]; } get syntax() { return this._data[2]; } get inherits() { return this._data[3]; } get initialValue() { return this._data[4] ?? null; } }
  class CSSImportRule extends CSSRule { get href() { return this._data[1]; } get media() { return this._media ??= new MediaList(() => []); } get styleSheet() { return null; } get layerName() { return null; } get supportsText() { return null; } }
  const ruleTypes = { style: CSSStyleRule, media: CSSMediaRule, supports: CSSSupportsRule, container: CSSContainerRule, layer: CSSLayerBlockRule, layers: CSSLayerStatementRule, keyframes: CSSKeyframesRule, 'font-face': CSSFontFaceRule, property: CSSPropertyRule, import: CSSImportRule };
  const makeRule = (sheet, parent, data) => new (ruleTypes[data[0]] ?? CSSRule)(sheet, parent, data);
  let sheetIds = 1;
  class StyleSheet {
    get type() { return 'text/css'; } get href() { return null; } get parentStyleSheet() { return null; } get ownerRule() { return null; }
    get ownerNode() { return this._node; } get title() { return this._node?.getAttribute('title') ?? null; }
    get media() { return this._media ??= new MediaList(() => mediaList(this._node ? this._node.getAttribute('media') : this._mediaText), value => { if (this._node) this._node.setAttribute('media', value); else this._mediaText = value; }); }
    get disabled() { return this._disabled; }
    set disabled(value) { value = !!value; if (value === this._disabled) return; this._disabled = value; if (this._node) call('dom.sheet', this._node._id, 'disabled', value); else for (const root of this._owners) adopt(root); }
  }
  class CSSStyleSheet extends StyleSheet {
    constructor(options = {}, node = null) { super(); this._id = sheetIds++; this._node = node; this._text = null; this._data = []; this._rules = null; this._owners = new Set(); this._live = false; this._disabled = !!options.disabled; this._mediaText = typeof options.media === 'string' ? options.media : Array.from(options.media ?? []).join(', '); }
    _sync() {
      if (!this._node) return;
      const value = text(this._node._m);
      if (value === this._text) return;
      this._text = value; this._data = call('dom.css', this._node._id);
      if (this._rules) this._rules.splice(0, this._rules.length, ...this._data.map(data => makeRule(this, this, data)));
    }
    _edit(kind, path, value = '') {
      let result = null;
      if (this._node) result = call('dom.sheet', this._node._id, kind, value, path);
      else if (this._live) result = call('dom.constructed', this._id, kind, value, path);
      else if (kind === 'insert') { const parsed = call('dom.constructed', 0, 'parse', value); result = parsed.length === 1 && parsed[0][0] !== 'other' ? parsed[0] : { error: 'SyntaxError' }; }
      else if (kind === 'keyframe' && value.includes('{')) result = call('dom.constructed', 0, 'parse', `@keyframes x{${value}}`)[0]?.[2] ?? { error: 'SyntaxError' };
      if (result?.error) throw new DOMException(`Failed to ${kind} the CSS rule`, result.error);
      return result;
    }
    get cssRules() { this._sync(); return this._rules ??= CSSRuleList.from(this._data, data => makeRule(this, this, data)); }
    get rules() { return this.cssRules; }
    insertRule(rule, index = 0) { const rules = this.cssRules; index = Number(index) >>> 0; if (index > rules.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); const data = this._edit('insert', [index], String(rule)); this._data.splice(index, 0, data); rules.splice(index, 0, makeRule(this, this, data)); return index; }
    deleteRule(index) { const rules = this.cssRules; index = Number(index) >>> 0; if (index >= rules.length) throw new DOMException('Invalid rule index', 'IndexSizeError'); this._edit('delete', [index]); this._data.splice(index, 1); rules.splice(index, 1); }
    addRule(selector = 'undefined', style = 'undefined', index) { this.insertRule(`${selector}{${style}}`, index ?? this.cssRules.length); return -1; }
    removeRule(index = 0) { this.deleteRule(index); }
    replaceSync(value) { if (this._node) throw new DOMException('Only constructed sheets can be replaced', 'NotAllowedError'); value = String(value).replace(/@import\s[^;]*;?/g, ''); this._data = call('dom.constructed', this._live ? this._id : 0, this._live ? 'replace' : 'parse', value); if (this._rules) this._rules.splice(0, this._rules.length, ...this._data.map(data => makeRule(this, this, data))); }
    replace(value) { try { this.replaceSync(value); return Promise.resolve(this); } catch (error) { return Promise.reject(error); } }
  }
  const sheetText = sheet => sheet._data.map(ruleText).join('\n');
  const adopt = root => call('dom.adopt', root === document ? 0 : root._id, (root._adopted ?? []).filter(sheet => !sheet._disabled).map(sheet => sheet._id));
  const adoptedSheets = root => {
    const apply = list => {
      for (const sheet of list) if (!(sheet instanceof CSSStyleSheet) || sheet._node) throw new DOMException('Only constructed sheets can be adopted', 'NotAllowedError');
      const previous = root._adopted ?? [];
      root._adopted = list;
      for (const sheet of previous) if (!list.includes(sheet)) sheet._owners.delete(root);
      for (const sheet of list) { sheet._owners.add(root); if (!sheet._live) { sheet._live = true; call('dom.constructed', sheet._id, 'replace', sheetText(sheet)); } }
      for (const sheet of previous) if (!sheet._owners.size && sheet._live) { sheet._live = false; call('dom.constructed', sheet._id, 'drop'); }
      adopt(root);
    };
    const list = [];
    return { list, proxy: new Proxy(list, { set(target, key, value) { const next = target.slice(); next[key] = value; if (key !== 'length' && !(value instanceof CSSStyleSheet)) throw new TypeError('Expected a CSSStyleSheet'); Reflect.set(target, key, value); apply(target.filter(Boolean)); return true; }, deleteProperty(target, key) { Reflect.deleteProperty(target, key); apply(target.filter(Boolean)); return true; } }), apply };
  };
  const adoptedProperty = { configurable: true, get() { return (this._adoption ??= adoptedSheets(this)).proxy; }, set(sheets) { const adoption = this._adoption ??= adoptedSheets(this), list = Array.from(sheets); adoption.apply(list); adoption.list.splice(0, adoption.list.length, ...list); } };
  const styleSheetsOf = root => {
    const sheets = new StyleSheetList(), stack = [...root._m.kids].reverse();
    while (stack.length) { const rec = stack.pop(); if (rec.tag === 'style' || rec.tag === 'link') { const sheet = of(rec).sheet; if (sheet) sheets.push(sheet); } if (rec.tag !== 'template' && rec.tag !== 'noscript') for (let i = rec.kids.length; i--;) stack.push(rec.kids[i]); }
    return sheets;
  };
  const cssEscape = value => {
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
  };
  const CSS = {
    supports(name, value) { return call('dom.supports', String(name), arguments.length > 1 ? String(value) : null); },
    escape: cssEscape,
    registerProperty(definition) {
      if (definition?.name === undefined || definition.inherits === undefined) throw new TypeError('Expected a name and inherits');
      const result = call('dom.registerProperty', String(definition.name), String(definition.syntax ?? '*'), !!definition.inherits, definition.initialValue === undefined ? null : String(definition.initialValue));
      if (result?.error) throw new DOMException('Invalid property registration', result.error);
    },
  };
  class ShadowRoot extends DocumentFragment {
    get innerHTML(){return this._m.kids.map(serialize).join('');} set innerHTML(value){setHTML(this,value);}
    get activeElement() { for (let node = active; node;) { const root = node.getRootNode(); if (root === this) return node; node = root.host ?? null; } return null; }
    get fullscreenElement() { for (let node = fullscreens.at(-1); node;) { const root = node.getRootNode(); if (root === this) return node; node = root.host ?? null; } return null; }
    get styleSheets() { return styleSheetsOf(this); }
  }
  // `define` resolves `whenDefined` before the upgrades it queued run, so a throwing upgrade can't leave it pending.
  const customElements = {
    define: scoped((name, Type) => {
      if (typeof Type !== 'function') throw new TypeError('Expected a constructor');
      if (!/^[a-z][a-z0-9._-]*-[a-z0-9._-]+$/.test(name) || definitions.has(name) || defined.has(Type)) throw new DOMException('Invalid custom element definition', 'NotSupportedError');
      const observed = new Set(typeof Type.prototype.attributeChangedCallback === 'function' ? Array.from(Type.observedAttributes ?? [], String) : []);
      definitions.set(name, Type); defined.set(Type, { name, observed });
      reactions(document._m, undefined, name);
      for (const resolve of definitionsWaiting.get(name) ?? []) resolve(Type);
      definitionsWaiting.delete(name);
    }),
    get(name) { return definitions.get(name); },
    getName(Type) { return defined.get(Type)?.name ?? null; },
    whenDefined(name) { if (definitions.has(name)) return Promise.resolve(definitions.get(name)); return new Promise(resolve => { const pending = definitionsWaiting.get(name) ?? []; pending.push(resolve); definitionsWaiting.set(name, pending); }); },
    upgrade: scoped(root => { if (definitions.size) reactions(root._m); }),
  };
  const filterNode = (walker, node) => !(walker.whatToShow & (1 << (node.nodeType - 1))) ? 3 : walker.filter == null ? 1 : typeof walker.filter === 'function' ? walker.filter(node) : walker.filter.acceptNode(node);
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
  class NodeIterator {
    constructor(root, whatToShow = 0xffffffff, filter = null) { Object.assign(this, { root, whatToShow: whatToShow >>> 0, filter, referenceNode: root, pointerBeforeReferenceNode: true }); }
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
    createElement(tag) {tag=String(tag).toLowerCase();const Type=definitions.get(tag);if(Type){constructing={tag};try{return new Type();}finally{constructing=null;}}return wrap(create(tag));}
    createElementNS(namespace, tag) {namespace=namespace==null?null:String(namespace);tag=String(tag);if(namespace==='http://www.w3.org/1999/xhtml')return this.createElement(tag);const id=create(tag),Type=namespace==='http://www.w3.org/2000/svg'?(tag==='svg'?SVGSVGElement:SVGElement):Element;const node=new Type(id,tag);node.namespaceURI=namespace;return node;}
    createTextNode(text) { return wrap(create('#text', String(text))); }
    createComment(text) { return wrap(create('#comment', String(text))); }
    createDocumentFragment() { return wrap(create('#fragment')); }
    createRange() { return new Range(); }
    get implementation(){return this._implementation??={createDocumentType:(name,publicId,systemId)=>{name=String(name);if(!/^[A-Za-z_][\w.-]*(:[A-Za-z_][\w.-]*)?$/.test(name))throw new DOMException('Invalid doctype name','InvalidCharacterError');publicId=String(publicId);systemId=String(systemId);return wrap(create('#doctype',publicId||systemId?[name,publicId,systemId].join('\0'):name));},createHTMLDocument:title=>{const doc=new DOMParser().parseFromString('<html><head></head><body></body></html>','text/html');if(title)doc.title=title;return doc;},hasFeature:()=>true};}
    createEvent(type) { const Type = { event: Event, events: Event, htmlevents: Event, customevent: CustomEvent, uievent: UIEvent, uievents: UIEvent, focusevent: FocusEvent, mouseevent: MouseEvent, mouseevents: MouseEvent, keyboardevent: KeyboardEvent }[String(type).toLowerCase()]; if (!Type) throw new DOMException('Unsupported event interface', 'NotSupportedError'); return new Type(''); }
    importNode(node, deep = false) { return node.cloneNode(deep); }
    adoptNode(node) { node.remove(); return node; }
    getElementById(id) {
      id = String(id);
      const at = id && ids.get(id), root = this._m;
      if (!at) return null;
      const inside = rec => { if (records[rec.id] !== rec || rec.attrs?.get('id') !== id) return false; for (let up = rec.up; up; up = up.up) { if (up === root) return true; if (up.tag === 'template') return false; } return false; };
      if (!Array.isArray(at)) return inside(at) ? of(at) : null;
      const found = at.filter(inside);
      return found.length < 2 ? of(found[0]) : find(root, rec => rec.attrs?.get('id') === id);
    }
    getElementsByName(name) { return this.querySelectorAll('[name=' + quote(name) + ']'); }
    get body() { const html = this._m.kids.find(rec => rec.tag[0] !== '#'); return html?.tag === 'html' ? of(html.kids.find(rec => rec.tag === 'body' || rec.tag === 'frameset')) : null; }
    get head() { const html = this._m.kids.find(rec => rec.tag[0] !== '#'); return html?.tag === 'html' ? of(html.kids.find(rec => rec.tag === 'head')) : null; }
    get documentElement() { return of(this._m.kids.find(rec => rec.tag[0] !== '#')); }
    get activeElement() { let node = active; while (node && node.getRootNode() !== this) node = node.getRootNode().host ?? null; return node ?? this.body ?? this.documentElement; } get defaultView() { return globalThis; }
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
    get URL() { return location.href; } get documentURI() { return location.href; } get location() { return this === document ? location : null; }
    get characterSet() { return 'UTF-8'; } get contentType() { return 'text/html'; } get scrollingElement() { return this.documentElement; }
    get readyState() { return this === document ? readyState : 'complete'; } get currentScript() { return this === document ? currentScript : null; } get compatMode() { return (this._quirks ??= quirks(this.doctype)) ? 'BackCompat' : 'CSS1Compat'; } get doctype() { return of(this._m.kids.find(rec => rec.tag === '#doctype')); } get visibilityState() { return 'visible'; } get hidden() { return false; }
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
      const range = this.createRange(); range.setStart(node,node.nodeType === 3 ? call('dom.textIndex',node._id,x,y) : 0); range.collapse(true); return range;
    }
    caretPositionFromPoint(x,y){const range=this.caretRangeFromPoint(x,y);return range?{offsetNode:range.startContainer,offset:range.startOffset}:null;}
    elementFromPoint(x, y) { return wrap(call('dom.hitTest', x, y, true)); }
    elementsFromPoint(x, y) { return call('dom.hitTest', x, y).map(wrap); }
    get fullscreenElement() { let node = fullscreens.at(-1) ?? null; while (node && node.getRootNode() !== this) node = node.getRootNode().host ?? null; return node; }
    get fullscreenEnabled() { return true; } get fullscreen() { return !!this.fullscreenElement; }
    exitFullscreen() { return fullscreens.length ? exitFullscreen(false) : Promise.reject(new TypeError('The document is not in fullscreen')); }
    get webkitFullscreenElement() { return this.fullscreenElement; } get webkitCurrentFullScreenElement() { return this.fullscreenElement; } get webkitFullscreenEnabled() { return true; } get webkitIsFullScreen() { return this.fullscreen; }
    webkitExitFullscreen() { return this.exitFullscreen(); } webkitCancelFullScreen() { return this.exitFullscreen(); }
  }
  DocumentFragment.prototype.getElementById = Document.prototype.getElementById;
  class ParentNode {
    get children() { return cachedList(listCache.elements, this._m, () => listOf(HTMLCollection, this._m.kids.filter(rec => rec.tag[0] !== '#').map(of))); }
    get childElementCount() { return this.children.length; }
    get firstElementChild() { return of(this._m.kids.find(rec => rec.tag[0] !== '#')); }
    get lastElementChild() { return of(this._m.kids.findLast(rec => rec.tag[0] !== '#')); }
    append(...children) { insertNodes(this, convert(children), null); }
    prepend(...children) { const nodes = convert(children), before = viable(this.firstChild, nodes); insertNodes(this, nodes, before); }
    replaceChildren(...children) { const nodes = convert(children); replaceAll(this, removed => { release(this); detach(this._m); op(1, this._id, ''); removed(); quiet++; try { for (const node of nodes) this.insertBefore(node, null); } finally { quiet--; } }); }
    moveBefore(node, child) { if (!(node instanceof Node) || node.nodeType === 9 || node.nodeType === 11 || node.getRootNode({ composed: true }) !== this.getRootNode({ composed: true })) throw new DOMException('Cannot move the node here', 'HierarchyRequestError'); reactionDepth++; moving = true; try { this.insertBefore(node, child); } finally { moving = false; leave(); } }
    querySelector(selector) { return wrap(selectors('dom.queryWithin', this._id, selector, true)); }
    querySelectorAll(selector) { return listOf(NodeList, selectors('dom.queryWithin', this._id, selector).map(wrap)); }
    getElementsByTagName(name) { name = String(name); const lower = name.toLowerCase(), result = []; find(this._m, rec => { if (rec.tag[0] !== '#' && (name === '*' || rec.tag === lower || rec.tag === name)) result.push(of(rec)); return false; }); return listOf(HTMLCollection, result); }
    getElementsByClassName(names) { names = String(names).split(/[\t\n\f\r ]+/).filter(Boolean); return listOf(HTMLCollection, names.length ? this.querySelectorAll(names.map(name => '[class~=' + quote(name) + ']').join('')) : []); }
  }
  const { constructor: _, ...parentNode } = Object.getOwnPropertyDescriptors(ParentNode.prototype);
  for (const Type of [Element, Document, DocumentFragment]) Object.defineProperties(Type.prototype, parentNode);
  class XMLSerializer { serializeToString(node) { if (!(node instanceof Node)) throw new TypeError('Expected a Node'); const source = serialize(node._m); return node.nodeType === 1 && node.namespaceURI && !node.hasAttribute('xmlns') ? source.replace(/^<[^\s>]+/, tag => tag + ' xmlns=' + quote(node.namespaceURI)) : source; } }
  class DOMParser {
    parseFromString(source,type){if(type!=='text/html'&&type!=='image/svg+xml')throw new DOMException('Only HTML and SVG parsing is supported','NotSupportedError');const doc=wrap(create('#document'));load(json(call('dom.html',doc._id,String(source))));if(type!=='text/html')Object.defineProperty(doc,'documentElement',{value:doc.querySelector('svg')});return doc;}
  }
  const comparePoints = (a, start, b, end) => {
    if (a === b) return Math.sign(start - end);
    const ra = a._m, rb = b._m;
    for (let child = rb; child.up; child = child.up) if (child.up === ra) return start <= index(child) ? -1 : 1;
    for (let child = ra; child.up; child = child.up) if (child.up === rb) return index(child) < end ? -1 : 1;
    return a.compareDocumentPosition(b) & 4 ? -1 : 1;
  };
  const rangeNodes=range=>call('dom.rangeNodes',range.startContainer._id,range.startOffset,range.endContainer._id,range.endOffset).map(([id,start,end])=>({node:wrap(id),start,end}));
  const textPoint=(root,offset)=>{const [id,index]=call('dom.textPoint',root._id,offset);return [wrap(id),index];};
  const textOffset=(root,node,offset)=>call('dom.textOffset',root._id,node?._id,offset);
  const characterData = node => node.nodeType === 3 || node.nodeType === 8;
  class Range {
    constructor() { this._ref = new WeakRef(this); this._sc = this._ec = document; this._so = this._eo = 0; }
    get startContainer() { return this._sc; } get startOffset() { return this._so; } get endContainer() { return this._ec; } get endOffset() { return this._eo; }
    get collapsed() { return this._sc === this._ec && this._so === this._eo; }
    get commonAncestorContainer() { const ends = new Set(); for (let rec = this._ec._m; rec; rec = rec.up) ends.add(rec); for (let rec = this._sc._m; rec; rec = rec.up) if (ends.has(rec)) return of(rec); return document; }
    setStart(node, offset) {
      offset = Number(offset) >>> 0;
      if (offset > (characterData(node) ? node.length : node._m.kids.length)) throw new DOMException('Invalid range offset', 'IndexSizeError');
      if (node.getRootNode() !== this._ec.getRootNode() || comparePoints(node, offset, this._ec, this._eo) > 0) place(this, false, node, offset);
      place(this, true, node, offset);
      if (selection?.ranges[0] === this) selection._setRange(this, selection.direction, true);
    }
    setEnd(node, offset) {
      offset = Number(offset) >>> 0;
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
    extractContents() { const result = this.cloneContents(); this.deleteContents(); return result; }
    insertNode(node) { const start = this._sc; if (start.nodeType === 3) start.parentNode.insertBefore(node, start.splitText(this._so)); else start.insertBefore(node, of(start._m.kids[this._so])); }
    surroundContents(node) { const contents = this.extractContents(); node.append(contents); this.insertNode(node); this.selectNode(node); }
    createContextualFragment(html) { const node = this.startContainer.nodeType === 1 ? this.startContainer : this.startContainer.parentElement; return parseFragment(html, node && !(node.localName === 'html' && node.namespaceURI === XHTML) ? node : null); }
    getBoundingClientRect() {const rects=this.getClientRects();if(!rects.length)return new DOMRect();const left=Math.min(...rects.map(rect=>rect.left)),top=Math.min(...rects.map(rect=>rect.top));return new DOMRect(left,top,Math.max(...rects.map(rect=>rect.right))-left,Math.max(...rects.map(rect=>rect.bottom))-top);}
    getClientRects() {return rangeNodes(this).flatMap(({node,start,end})=>call('dom.textRect',node._id,start,end).map(rect=>new DOMRect(...rect)));}
    compareBoundaryPoints(how,range){return how===0?comparePoints(this.startContainer,this.startOffset,range.startContainer,range.startOffset):how===1?comparePoints(this.endContainer,this.endOffset,range.startContainer,range.startOffset):how===2?comparePoints(this.endContainer,this.endOffset,range.endContainer,range.endOffset):comparePoints(this.startContainer,this.startOffset,range.endContainer,range.endOffset);}
    isPointInRange(node,offset){return comparePoints(node,offset,this.startContainer,this.startOffset)>=0&&comparePoints(node,offset,this.endContainer,this.endOffset)<=0;}
    comparePoint(node,offset){return comparePoints(node,offset,this.startContainer,this.startOffset)<0?-1:comparePoints(node,offset,this.endContainer,this.endOffset)>0?1:0;}
    intersectsNode(node){if(!node.parentNode)return true;const parent=node.parentNode,at=index(node._m);return comparePoints(parent,at+1,this._sc,this._so)>0&&comparePoints(parent,at,this._ec,this._eo)<0;}
    detach() {}
  }
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
  globalThis.StaticRange = StaticRange;
  class Selection {
    constructor() { this.ranges = []; this.direction = 'none'; }
    get anchorNode() { return this.ranges[0]?.[this.direction === 'backward' ? 'endContainer' : 'startContainer'] ?? null; }
    get anchorOffset() { return this.ranges[0]?.[this.direction === 'backward' ? 'endOffset' : 'startOffset'] ?? 0; }
    get focusNode() { return this.ranges[0]?.[this.direction === 'backward' ? 'startContainer' : 'endContainer'] ?? null; }
    get focusOffset() { return this.ranges[0]?.[this.direction === 'backward' ? 'startOffset' : 'endOffset'] ?? 0; }
    get rangeCount() { return this.ranges.length; } get isCollapsed() { return !this.rangeCount || this.ranges[0].collapsed; } get type() { return !this.rangeCount ? 'None' : this.isCollapsed ? 'Caret' : 'Range'; }
    _setRange(range, direction = 'none', changed = false) {
      const previous = this.ranges[0];
      changed ||= !previous || direction !== this.direction || ['startContainer','startOffset','endContainer','endOffset'].some(key => previous[key] !== range[key]);
      this.ranges = [range]; this.direction = direction;
      if (!changed) return;
      this._send();
      selectionChanged();
    }
    _send() {
      const range = this.ranges[0], caret = !!range?.collapsed && !!active?.isContentEditable && active.contains(range.startContainer);
      if (!range) return;
      const nodes = rangeNodes(range).map(({node,start,end}) => [node._id,start,end]);
      if (caret && !nodes.length) { const { startContainer: parent, startOffset: offset } = range, child = parent.nodeType === 1 ? parent.childNodes[offset] ?? parent.lastChild : null; if (child?.localName === 'br') nodes.push([child._id, 0, 0]); }
      call('dom.selection', 0, nodes, caret);
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
    }
    disconnect() { unwatchAll(this, observers); this.records = []; } takeRecords() { return this.records.splice(0); }
  }
  const intersectionObservers=new Set();
  class IntersectionObserver {
    constructor(callback,options={}){this.callback=callback;this.root=options.root??null;this.rootMargin=options.rootMargin??'0px';this.thresholds=[...new Set(Array.isArray(options.threshold)?options.threshold:[options.threshold??0])].sort((a,b)=>a-b);if(this.thresholds.some(v=>v<0||v>1))throw new RangeError('Invalid intersection threshold');this.targets=new Map();this.records=[];this._ref=null;}
    observe(target){watch(this,target,null,intersectionObservers);}unobserve(target){unwatch(this,target,intersectionObservers);}disconnect(){unwatchAll(this,intersectionObservers);this.records=[];}takeRecords(){return this.records.splice(0);}
  }
  const resizeObservers = new Set();
  class ResizeObserver { constructor(callback) { this.callback = callback; this.targets = new Map(); this._ref = null; } observe(target) { watch(this, target, null, resizeObservers); host('dom.requestFrame'); } unobserve(target) { unwatch(this, target, resizeObservers); } disconnect() { unwatchAll(this, resizeObservers); } }
  const breakout = /^(b|big|blockquote|body|br|center|code|dd|div|dl|dt|em|embed|h[1-6]|head|hr|i|img|li|listing|menu|meta|nobr|ol|p|pre|ruby|s|small|span|strong|strike|sub|sup|table|tt|u|ul|var)$/;
  const wrap = id => {
    let rec = records[id], node = rec?.ref?.deref();
    if (node || !rec) return node ?? null;
    const chain = [];
    for (; rec && !(node = rec.ref?.deref()); rec = rec.up) chain.push(rec);
    for (let i = chain.length; i--;) {
      const { id, tag, attrs } = chain[i], above = node, svg = tag === 'svg' || (above instanceof SVGElement ? !/^(foreignObject|desc|title)$/.test(above.localName) : !!above?._svg && !breakout.test(tag) && !(tag === 'font' && (attrs.has('color') || attrs.has('face') || attrs.has('size'))));
      const Type = tag === '#document' ? Document : tag === '#fragment' ? DocumentFragment : tag === '#text' ? Text : tag === '#comment' ? Comment : tag === '#doctype' ? DocumentType : svg ? (tag === 'svg' ? SVGSVGElement : SVGElement) : htmlTypes.get(tag) ?? HTMLElement;
      node = new Type(id, tag); if (above) hold(above, node);
    }
    return node;
  };
  let sweepAt = 0, cursor = 0, canary = new WeakRef({});
  globalThis.__appsDomSweep = (force = false) => {
    scheduled = false; flush();
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
  const flat2D = m => m.every((v, i) => plane.includes(i) || v === +(i % 5 === 0)), flatMatrix = m => Object.assign(new DOMMatrix(m), { _2d: flat2D(m) });
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
    multiplySelf(other) { const a = this._m, b = DOMMatrix.fromMatrix(other), m = b._m; this._m = a.map((_, i) => a[i & 3] * m[i & 12] + a[4 + (i & 3)] * m[(i & 12) + 1] + a[8 + (i & 3)] * m[(i & 12) + 2] + a[12 + (i & 3)] * m[(i & 12) + 3]); this._2d &&= b._2d; return this; }
    preMultiplySelf(other) { const matrix = DOMMatrix.fromMatrix(other).multiplySelf(this); this._m = matrix._m; this._2d = matrix._2d; return this; }
    translateSelf(x = 0, y = 0, z = 0) { return this.multiplySelf(flatMatrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1])); }
    scaleSelf(x = 1, y = x, z = 1, ox = 0, oy = 0, oz = 0) { return this.translateSelf(ox, oy, oz).multiplySelf(flatMatrix([x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, 0, 0, 0, 0, 1])).translateSelf(-ox, -oy, -oz); }
    scale3dSelf(scale = 1, ox = 0, oy = 0, oz = 0) { return this.scaleSelf(scale, scale, scale, ox, oy, oz); }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      const length = Math.hypot(x, y, z); if (!length) return this; [x, y, z] = [x / length, y / length, z / length];
      const r = angle * Math.PI / 180, s = Math.sin(r), c = Math.cos(r), t = 1 - c;
      return this.multiplySelf(flatMatrix([t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0, t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0, t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0, 0, 0, 0, 1]));
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
    _flush() { if (!this._size) return; const ops = this._ops.subarray(0, this._size), text = this._texts.join(''), data = this._data; this._size = 0; this._texts.length = 0; this._data = []; bitmapDraws.delete(this); if (this.canvas.width && this.canvas.height) call('dom.canvas', this.canvas._id, ops, text, ...data); this.canvas._commit?.(); }
    // Canvas::draw keeps each Path2D it gets until the context resets or sends more than about 8 MB of paths.
    _path(path) {
      if (!(path instanceof Path2D)) throw new TypeError('Expected a Path2D');
      if (this._paths.get(path) === path._size) return path._id;
      const reset = (this._pathSize += path._size) > 1 << 20; if (reset) { this._paths = new WeakMap(); this._pathSize = path._size; }
      this._paths.set(path, path._size); this._data.push(path._ops.subarray(0, path._size)); this._op(41, path._id, this._data.length - 1, +reset); return path._id;
    }
    _resize() { this._size = 0; this._texts.length = 0; this._data = []; this._reset(); this._paths = new WeakMap(); this._pathSize = 0; call('dom.canvas', this.canvas._id, null); }
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
      if (image instanceof ImageBitmap) bitmapDraws.add(this); else if (image._context && image!==this.canvas) this._flush();
    }

    beginPath() { this._op(11); }
    fill(path, rule) { if (path instanceof Path2D) this._op(42, this._path(path), 18, fillRule(rule)); else this._op(18, fillRule(path)); }
    stroke(path) { if (path === undefined) this._op(19); else this._op(42, this._path(path), 19, 0); }
    clip(path, rule) { if (path instanceof Path2D) this._op(42, this._path(path), 20, fillRule(rule)); else this._op(20, fillRule(path)); }
  }
  for (const [name, code] of [['fillStyle', 4], ['strokeStyle', 5], ['font', 6], ['textAlign', 7], ['textBaseline', 8], ['globalAlpha', 9], ['lineWidth', 10]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { get() { return this._state[name]; }, set(value) {
    if (code < 6 && (value instanceof CanvasGradient || value instanceof CanvasPattern)) { this._state[name] = value; const image = value._image; image?._context?._flush?.(); const text = value._spec(image === this.canvas ? -1 : image?._id); this._texts.push(text); this._op(code, text.length); if (image instanceof ImageBitmap) bitmapDraws.add(this); else if (image?._context && image !== this.canvas) this._flush(); return; }
    if (code < 9) value = String(value); else if (value = Number(value), !(code === 9 ? value >= 0 && value <= 1 : value > 0 && value < Infinity)) return;
    if (this._state[name] === value) return;
    this._state[name] = value; if (code < 9) { this._texts.push(alone(value)); this._op(code, value.length); } else this._op(code, value);
  } });
  for (const [name, code, list] of [['lineCap', 32, 'butt round square'], ['lineJoin', 33, 'miter round bevel'], ['globalCompositeOperation', 37, 'source-over source-in source-out source-atop destination-over destination-in destination-out destination-atop lighter copy xor multiply screen overlay darken lighten color-dodge color-burn hard-light soft-light difference exclusion hue saturation color luminosity']]) { const values = new Set(list.split(' ')); Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { value = String(value); if (!values.has(value) || this._state[name] === value) return; this._state[name] = value; this._texts.push(value); this._op(code, value.length); } }); }
  for (const [name, code, valid] of [['miterLimit', 34, v => v > 0 && v < Infinity], ['lineDashOffset', 36, Number.isFinite]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { value = Number(value); if (valid(value)) { this._state[name] = value; this._op(code, value); } } });
  for (const [name, code] of [['letterSpacing', 38], ['wordSpacing', 39]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { const px = /^(-?[\d.]+)px$/.exec(String(value).trim()); if (px) { this._state[name] = `${+px[1]}px`; this._op(code, +px[1]); } } });
  Object.defineProperty(CanvasRenderingContext2D.prototype, 'imageSmoothingEnabled', { configurable: true, enumerable: true, get() { return this._state.imageSmoothingEnabled; }, set(value) { this._state.imageSmoothingEnabled = !!value; this._op(40, value ? 1 : 0); } });
  for (const [name, valid] of [['shadowColor', () => true], ['shadowBlur', v => v >= 0 && v < Infinity], ['shadowOffsetX', Number.isFinite], ['shadowOffsetY', Number.isFinite]]) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) {
    value = name === 'shadowColor' ? String(value) : Number(value); if (!valid(value)) return;
    const state = this._state; state[name] = value; this._texts.push(state.shadowColor); this._op(43, state.shadowColor.length, state.shadowBlur, state.shadowOffsetX, state.shadowOffsetY);
  } });
  for (const name of ['imageSmoothingQuality', 'filter', 'fontKerning', 'fontStretch', 'fontVariantCaps', 'textRendering', 'direction', 'lang']) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { configurable: true, enumerable: true, get() { return this._state[name]; }, set(value) { this._state[name] = value; } });
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
  class AudioNode { constructor(context) { this.context = context; this._target = null; } connect(target) { this._target = target; return target; } disconnect() { this._target = null; } }
  class GainNode extends AudioNode { gain = { value: 1 }; }
  class AudioDestinationNode extends AudioNode {}
  class AudioBuffer { constructor(url, duration) { this._url = url; this.duration = duration; } }
  class AudioBufferSourceNode extends AudioNode {
    buffer = null; loop = false; onended = null; playbackRate = { value: 1 };
    start(when = 0, offset = 0) {
      if (!this.buffer) return;
      let volume = 1, node = this._target;
      while (node && !(node instanceof AudioDestinationNode)) { volume *= node.gain?.value ?? 1; node = node._target; }
      const audio = this._audio = new Audio(this.buffer._url);
      audio.volume = node ? Math.min(1, Math.max(0, volume)) : 0; audio.loop = this.loop; audio.playbackRate = this.playbackRate.value; audio.currentTime = offset;
      audio.addEventListener('ended', () => this.onended?.(new Event('ended')));
      setTimeout(() => audio.play().catch(() => {}), Math.max(0, when - this.context.currentTime) * 1000);
    }
    stop() { this._audio?.pause(); }
  }
  class AudioContext extends EventTarget {
    state = 'running'; sampleRate = 48000; _start = performance.now(); destination = new AudioDestinationNode(this);
    get currentTime() { return (performance.now() - this._start) / 1000; }
    _set(state) { this.state = state; this.dispatchEvent(new Event('statechange')); return Promise.resolve(); }
    resume() { return this.state === 'running' ? Promise.resolve() : this._set('running'); } suspend() { return this._set('suspended'); } close() { return this._set('closed'); }
    createGain() { return new GainNode(this); } createBufferSource() { return new AudioBufferSourceNode(this); }
    decodeAudioData(data, done, failed) {
      const url = URL.createObjectURL(new Blob([data])), audio = new Audio(url);
      const result = new Promise((resolve, reject) => { audio.addEventListener('loadedmetadata', () => resolve(new AudioBuffer(url, audio.duration))); audio.addEventListener('error', () => reject(new DOMException('The audio cannot be decoded', 'EncodingError'))); audio.load(); });
      if (done || failed) result.then(done, failed);
      return result;
    }
  }
  class ImageData {
    constructor(data,width,height) { if (typeof data==='number') {height=width;width=data;data=new Uint8ClampedArray(width*height*4);} else {if (!(data instanceof Uint8ClampedArray)) throw new TypeError('Expected Uint8ClampedArray');height??=data.length/(width*4);} if(!Number.isInteger(width)||!Number.isInteger(height)||width<=0||height<=0||data.length!==width*height*4) throw new DOMException('Invalid image dimensions','IndexSizeError');Object.assign(this,{data,width,height,colorSpace:'srgb'}); }
  }
  // Match render's dimension(): leading HTML non-negative integers after whitespace and an optional +.
  const canvasSize = (value, fallback) => { const digits = /^[\t\n\f\r ]*\+?(\d+)/.exec(value ?? '')?.[1], size = digits ? Number(digits) : NaN; return size <= 2147483647 ? size : fallback; };
  class HTMLCanvasElement extends HTMLElement {
    get clientWidth(){const width=this.getBoundingClientRect().width;if(width)return width;const value=getComputedStyle(this).width;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.width;}
    get clientHeight(){const height=this.getBoundingClientRect().height;if(height)return height;const value=getComputedStyle(this).height;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.height;}
    get width() { return canvasSize(this.getAttribute('width'), 300); } set width(v) { if (this._offscreen) throw transferred(); this.setAttribute('width', Number(v) >>> 0); }
    get height() { return canvasSize(this.getAttribute('height'), 150); } set height(v) { if (this._offscreen) throw transferred(); this.setAttribute('height', Number(v) >>> 0); }
    setAttribute(name, value) { super.setAttribute(name, value); if (name === 'width' || name === 'height') this._context?._resize(); }
    toDataURL(type, quality) { this._context?._flush?.(); return call('dom.canvasURL', this._id, false, String(type).toLowerCase(), quality); }
    toBlob(callback, type, quality) { if (typeof callback !== 'function') throw new TypeError('Expected a callback'); this._context?._flush?.(); const [bytes, mime] = call('dom.canvasURL', this._id, true, String(type).toLowerCase(), quality); setTimeout(() => callback(new Blob([bytes], { type: mime }))); }
    getContext(type, options) { if (this._offscreen) throw transferred(); if (['webgl','webgl2','experimental-webgl','experimental-webgl2'].includes(type))throw new DOMException('WebGL is not supported; use WebGPU or Canvas 2D','NotSupportedError'); return canvasContext(this, type, options, CanvasRenderingContext2D); }
    transferControlToOffscreen() { if (this._contextType || this._offscreen) throw new DOMException('The canvas has a context or was already transferred', 'InvalidStateError'); this._offscreen = true; const offscreen = new OffscreenCanvas(0, 0); offscreen._node = this; return offscreen; }
  }
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
    transferToImageBitmap() {
      const { width, height } = this;
      if (!this._context || !width || !height) throw new DOMException('The canvas has no context or no pixels', 'InvalidStateError');
      this._context._flush?.();
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
  for (const key of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) for (const tag of ['a', 'area']) Object.defineProperty(htmlTypes.get(tag).prototype, key, { configurable: true, get() { try { return new URL(this.href)[key]; } catch { return key === 'origin' ? 'null' : ''; } }, set(value) { try { const url = new URL(this.href); url[key] = value; this.href = url.href; } catch {} } });
  for (const tag of ['a', 'area']) Object.defineProperty(htmlTypes.get(tag).prototype, 'toString', { configurable: true, writable: true, value() { return this.href; } });
  for (const [tags, names] of [['a area link', 'rel'], ['a area', 'target download'], ['a link script', 'type'], ['link', 'media as hreflang'], ['meta', 'name content']]) for (const tag of tags.split(' ')) for (const name of names.split(' ')) Object.defineProperty(htmlTypes.get(tag).prototype, name, { configurable: true, get() { return this.getAttribute(name) ?? ''; }, set(value) { this.setAttribute(name, value); } });
  for (const tag of ['a', 'area', 'link']) Object.defineProperty(htmlTypes.get(tag).prototype, 'relList', { configurable: true, get() { return this._relList ??= new DOMTokenList(this, 'rel'); } });
  {
    const kids = (el, ...tags) => [...el.children].filter(child => tags.includes(child.localName));
    const rows = el => kids(el, 'tr', 'thead', 'tbody', 'tfoot').flatMap(child => child.localName === 'tr' ? [child] : kids(child, 'tr'));
    const insertRow = (section, index, all = kids(section, 'tr')) => { if (index < -1 || index > all.length) throw new DOMException('Row index out of range', 'IndexSizeError'); const row = document.createElement('tr'); return index === -1 || index === all.length ? (all.at(-1)?.parentNode ?? section).appendChild(row) : all[index].before(row) ?? row; };
    const remove = (list, index) => { const item = list.at(index); if (!item || index < -1 || index >= list.length) throw new DOMException('Index out of range', 'IndexSizeError'); item.remove(); };
    const section = (table, tag, before) => kids(table, tag)[0] ?? table.insertBefore(document.createElement(tag), before?.() ?? null);
    Object.defineProperties(HTMLTableElement.prototype, Object.getOwnPropertyDescriptors({
      get caption() { return kids(this, 'caption')[0] ?? null; },
      get tHead() { return kids(this, 'thead')[0] ?? null; },
      get tFoot() { return kids(this, 'tfoot')[0] ?? null; },
      get tBodies() { return listOf(HTMLCollection, kids(this, 'tbody')); },
      get rows() { return listOf(HTMLCollection, rows(this)); },
      createCaption() { return section(this, 'caption', () => this.firstChild); },
      createTHead() { return section(this, 'thead', () => kids(this, 'tbody', 'tfoot', 'tr')[0]); },
      createTFoot() { return section(this, 'tfoot'); },
      createTBody() { return this.insertBefore(document.createElement('tbody'), kids(this, 'tbody').at(-1)?.nextSibling ?? null); },
      deleteCaption() { this.caption?.remove(); }, deleteTHead() { this.tHead?.remove(); }, deleteTFoot() { this.tFoot?.remove(); },
      insertRow(index = -1) { const all = rows(this); return all.length || kids(this, 'tbody').length ? insertRow(kids(this, 'tbody').at(-1) ?? this, index, all) : this.appendChild(document.createElement('tbody')).appendChild(document.createElement('tr')); },
      deleteRow(index) { remove(rows(this), index); },
    }));
    Object.defineProperties(HTMLTableSectionElement.prototype, Object.getOwnPropertyDescriptors({
      get rows() { return listOf(HTMLCollection, kids(this, 'tr')); },
      insertRow(index = -1) { return insertRow(this, index); },
      deleteRow(index) { remove(kids(this, 'tr'), index); },
    }));
    Object.defineProperties(HTMLTableRowElement.prototype, Object.getOwnPropertyDescriptors({
      get cells() { return listOf(HTMLCollection, kids(this, 'td', 'th')); },
      get rowIndex() { const table = this.closest('table'); return table ? rows(table).indexOf(this) : -1; },
      get sectionRowIndex() { return this.parentNode ? kids(this.parentNode, 'tr').indexOf(this) : -1; },
      insertCell(index = -1) { const cells = kids(this, 'td', 'th'); if (index < -1 || index > cells.length) throw new DOMException('Cell index out of range', 'IndexSizeError'); const cell = document.createElement('td'); return index === -1 || index === cells.length ? this.appendChild(cell) : this.insertBefore(cell, cells[index]); },
      deleteCell(index) { remove(kids(this, 'td', 'th'), index); },
    }));
    Object.defineProperty(HTMLTableCellElement.prototype, 'cellIndex', { get() { return this.parentNode ? kids(this.parentNode, 'td', 'th').indexOf(this) : -1; }, configurable: true });
  }
  load(json(call('dom.tree', 0)));
  const document = globalThis.document = wrap(0);
  globalThis.window = globalThis; globalThis.self = globalThis;
  selection = new Selection();
  for (const name of ['addEventListener', 'removeEventListener', 'dispatchEvent']) globalThis[name] = Node.prototype[name].bind(globalThis);
  Object.assign(globalThis, { ImageBitmap, ImageBitmapRenderingContext, OffscreenCanvas, OffscreenCanvasRenderingContext2D, Screen, ScreenOrientation, CanvasGradient, Path2D, CanvasPattern, DOMMatrix, DOMMatrixReadOnly, DOMPoint, DOMPointReadOnly, TextMetrics, AudioContext, AudioNode, GainNode, AudioBuffer, AudioBufferSourceNode, AudioDestinationNode, Node, CharacterData, Text, Comment, DocumentType, Attr, NamedNodeMap, NodeList, HTMLCollection, DOMTokenList, Element, HTMLElement, SVGElement, SVGSVGElement, HTMLSlotElement, HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLOptionElement, HTMLOptGroupElement, HTMLOptionsCollection, HTMLButtonElement, HTMLFieldSetElement, HTMLOutputElement, HTMLFormElement, HTMLFormControlsCollection, HTMLDialogElement, HTMLDetailsElement, HTMLLabelElement, ValidityState, DOMRectList, WheelEvent, ToggleEvent, CommandEvent, SubmitEvent, HTMLDocument: Document, HTMLImageElement, HTMLCanvasElement, HTMLMediaElement, HTMLVideoElement, HTMLAudioElement, Audio, MediaError, TimeRanges, HTMLTemplateElement, Document, DocumentFragment, TreeWalker, NodeIterator, XMLSerializer, Range, Selection, DOMRect, DOMRectReadOnly: DOMRect, MutationObserver, ResizeObserver, IntersectionObserver, UIEvent, MouseEvent, PointerEvent, KeyboardEvent, FocusEvent, InputEvent, CompositionEvent, CanvasRenderingContext2D, ImageData, DataTransfer, ClipboardEvent, DragEvent });
  Object.assign(globalThis,{customElements,ShadowRoot,CSSStyleSheet,DOMParser});
  Object.assign(globalThis, { CSS, StyleSheet, StyleSheetList, MediaList, CSSRule, CSSRuleList, CSSGroupingRule, CSSStyleRule, CSSMediaRule, CSSSupportsRule, CSSContainerRule, CSSLayerBlockRule, CSSLayerStatementRule, CSSKeyframesRule, CSSKeyframeRule, CSSFontFaceRule, CSSPropertyRule, CSSImportRule, CSSStyleDeclaration });
  Object.defineProperty(ShadowRoot.prototype, 'adoptedStyleSheets', adoptedProperty);
  Object.defineProperty(Document.prototype, 'adoptedStyleSheets', adoptedProperty);
  Object.defineProperty(Document.prototype, 'styleSheets', { configurable: true, get() { return styleSheetsOf(this); } });
  const ownSheet = node => { let inert = !node.isConnected; for (let rec = node._m.up; rec && !inert; rec = rec.up) inert = rec.tag === 'template' || rec.tag === 'noscript'; return inert ? node._sheet = null : node._sheet ??= new CSSStyleSheet({}, node); };
  Object.defineProperty(HTMLStyleElement.prototype, 'sheet', { configurable: true, get() { return ownSheet(this); } });
  Object.defineProperty(HTMLLinkElement.prototype, 'sheet', { configurable: true, get() { return /(^|\s)stylesheet(\s|$)/i.test(this.getAttribute('rel') ?? '') && (this._linked ??= call('dom.css', this._id) !== null) ? ownSheet(this) : null; } });
  Object.defineProperty(Node.prototype, 'baseURI', { configurable: true, get() { return document.baseURI; } });
  globalThis.Option = function Option(text = '', value, defaultSelected = false, selected = false) { const option = document.createElement('option'); if (text !== '') option.text = text; if (value !== undefined) option.value = value; option.defaultSelected = defaultSelected; if (selected) option.selected = true; return option; };
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
      const created = own[name] = { value, listener(event) { const fn = created.value; if (typeof fn !== 'function') return; const result = fn.call(this, event); if (result === false) event.preventDefault(); } };
      this.addEventListener(name, created.listener);
    } });
  const globalHandlers = 'abort auxclick beforeinput beforematch beforetoggle blur cancel canplay canplaythrough change click close command contextlost contextmenu contextrestored copy cuechange cut dblclick drag dragend dragenter dragleave dragover dragstart drop durationchange emptied ended error focus formdata input invalid keydown keypress keyup load loadeddata loadedmetadata loadstart mousedown mouseenter mouseleave mousemove mouseout mouseover mouseup paste pause play playing progress ratechange reset resize scroll scrollend securitypolicyviolation seeked seeking select selectionchange selectstart slotchange stalled submit suspend timeupdate toggle volumechange waiting wheel animationstart animationiteration animationend animationcancel transitionrun transitionstart transitionend transitioncancel gotpointercapture lostpointercapture pointerdown pointermove pointerrawupdate pointerup pointercancel pointerover pointerout pointerenter pointerleave'.split(' ');
  for (const name of globalHandlers) for (const target of [HTMLElement.prototype, SVGElement.prototype, Document.prototype]) handler(target, name);
  handler(Document.prototype, 'readystatechange'); handler(Document.prototype, 'visibilitychange');
  handler(Screen.prototype, 'change'); handler(ScreenOrientation.prototype, 'change');
  for (const name of ['fullscreenchange', 'fullscreenerror']) for (const target of [Element.prototype, Document.prototype]) handler(target, name);
  for (const name of [...globalHandlers, ...'afterprint beforeprint beforeunload hashchange languagechange message messageerror offline online pagehide pageshow popstate rejectionhandled storage unhandledrejection unload'.split(' ')]) if (!Object.hasOwn(globalThis, 'on' + name)) handler(globalThis, name);
  for (const [name, Type] of Object.entries(globalThis)) if (typeof Type === 'function' && Type.prototype && (Type.prototype instanceof Node || /^(HTML|SVG|DOM|Node|Text|Comment|Attr|Range|Selection|Tree|Mutation|Resize|Intersection|Validity|Static|XML|CSS|ShadowRoot|Custom|Image|Canvas|Font|Media|Data|Clipboard|Drag|Composition|Input|Focus|Keyboard|Mouse|Pointer|Wheel|UI|Toggle|Command|Submit|Offscreen|Screen)/.test(name)) && !Object.hasOwn(Type.prototype, Symbol.toStringTag)) Object.defineProperty(Type.prototype, Symbol.toStringTag, { configurable: true, value: name === 'HTMLDocument' ? 'Document' : name });
  Object.defineProperty(globalThis, Symbol.toStringTag, { configurable: true, value: 'Window' });
  globalThis.__appsScript = node => { currentScript = node ?? null; };
  globalThis.__appsLoaded = () => {
    readyState = 'interactive'; dispatch(document, trusted(new Event('readystatechange')));
    dispatch(document, trusted(new Event('DOMContentLoaded', { bubbles: true })));
    const images = []; find(document._m, rec => { if (rec.tag === 'img') { const loading = rec.ref?.deref()?._loading; if (loading) images.push(loading.catch(() => {})); } return false; });
    Promise.all(images).then(() => setTimeout(() => { readyState = 'complete'; dispatch(document, trusted(new Event('readystatechange'))); dispatchEvent(trusted(new Event('load'))); }, 0));
  };
  globalThis.Image = function Image(width,height) { const image=document.createElement('img'); if(width!==undefined) image.width=width; if(height!==undefined) image.height=height; return image; };
  class FontFace {
    constructor(family,source,descriptors={}) { Object.assign(this,{family:String(family),style:'normal',weight:'normal',stretch:'normal',unicodeRange:'U+0-10FFFF',status:'unloaded'},descriptors); this._source=source;this.loaded=new Promise((resolve,reject)=>{this._resolve=resolve;this._reject=reject;});this.loaded.catch(()=>{}); }
    load() { if (this.status!=='unloaded') return this.loaded; this.status='loading'; (async()=>{const source=this._source;let error=new DOMException('No supported font source','SyntaxError');for(const [kind,value] of typeof source==='string'?call('dom.fontSources',0,source):[['data',source]]) try {if(kind==='local'){if(call('dom.font',0,this.family,value))return this._done('loaded');throw new DOMException('Local font unavailable','NetworkError');}let bytes=value;if(kind==='url'){const response=await fetch(value);if(!response.ok)throw new DOMException('Font request failed','NetworkError');bytes=await response.arrayBuffer();}call('dom.font',0,this.family,ArrayBuffer.isView(bytes)?new Uint8Array(bytes.buffer,bytes.byteOffset,bytes.byteLength):new Uint8Array(bytes));return this._done('loaded');}catch(caught){error=caught;}this._done('error',error);})();return this.loaded; }
    _done(status,error) { this.status=status; if(status==='loaded')this._resolve(this); else if(status==='error')this._reject(error); }
  }
  class FontFaceSet extends EventTarget {
    constructor() {super();this._faces=new Set();this._css=[];}
    _all() {const [added,css]=call('dom.fonts',0);if(added){stale=true;version++;}this._css=css.map(([family,source,range,status],i)=>{let face=this._css[i];if(face?.family!==family||face._source!==source||face.status!==status&&face.status!=='loading'){face=new FontFace(family,source,range?{unicodeRange:range}:{});face._done(status,new DOMException('Font load failed','NetworkError'));}return face;});return [...this._css,...this._faces];}
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
  const page = axis => ({ get: () => document.documentElement?.[axis] ?? 0, configurable: true });
  Object.defineProperties(globalThis, { scrollX: page('scrollLeft'), scrollY: page('scrollTop'), pageXOffset: page('scrollLeft'), pageYOffset: page('scrollTop') });
  globalThis.scrollTo = globalThis.scroll = (left, top) => document.documentElement?.scrollTo(left, top);
  globalThis.scrollBy = (left, top) => document.documentElement?.scrollBy(left, top);
  globalThis.getSelection = () => selection;
  globalThis.getComputedStyle = (node, pseudo = null) => {
    if (!(node instanceof Element)) throw new TypeError('Expected an element');
    const id = node._id, kind = pseudo == null ? '' : String(pseudo);
    let names = null;
    return declaration({ computed: name => { if (/^(width|height|min-|max-|margin|padding|inset|top|right|bottom|left|transform|perspective-origin)/.test(name)) layout(); return call('dom.computed', id, name, kind); }, names: () => names ??= call('dom.computed', id, null, kind) });
  };
  const animationFrames = new Map(); let animationFrameId = 0, animationTimer, frameRequested = false;
  globalThis.__appsAnimationFrame = () => {
    animationTimer = undefined; frameRequested = false;
    const now = performance.now(), callbacks = [...animationFrames];
    timelineTime = now; microtask(() => { timelineTime = null; });
    if (pendingAnimations.size) { const ready = [...pendingAnimations]; pendingAnimations.clear(); for (const animation of ready) readyAnimation(animation); }
    for (const [id, callback] of callbacks) {
      if (!animationFrames.delete(id)) continue;
      try { callback(now); } catch (error) { console.error(error); }
    }
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
    return Object.assign(result, { progress: timing.easing === 'linear' ? directed : call('dom.easing', timing.easing, directed), currentIteration: iteration });
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
    set composite(value) { if (['replace', 'add', 'accumulate'].includes(value)) effects.get(this).composite = value; }
    get iterationComposite() { return 'replace'; }
    getKeyframes() { return effects.get(this).frames.map(frame => { const result = { offset: frame.offset, computedOffset: frame.computed, easing: frame.easing, composite: frame.composite }; for (const [name, value] of frame.props) result[camelName(name)] = value; return result; }); }
    setKeyframes(keyframes) { const effect = effects.get(this); effect.frames = keyframesOf(keyframes); if (effect.animation) retimed(effect.animation); }
  }
  class AnimationPlaybackEvent extends Event { constructor(type, init = {}) { super(type, init); this.currentTime = init?.currentTime ?? null; this.timelineTime = init?.timelineTime ?? null; } }
  const liveAnimations = new Set();
  let animationIds = 0;
  const pendingAnimations = new Set();
  let readyAnimation;
  class Animation extends EventTarget {
    #id = ++animationIds; #effect = null; #timeline; #start = null; #hold = null; #rate = 1; #pendingRate = null; #playing = false; #pausing = false; #previous = null;
    #finished; #settle; #fail; #done = false; #queued = false; #ready; #readyResolve = null; #readyReject = null; #timer; #replace = 'active'; #synced = false;
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
      if (effect.pseudo || !target.isConnected) throw new DOMException('The target is not rendered', 'NoModificationAllowedError');
      const values = call('dom.animated', target._id) ?? {};
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
      if (state === 'idle' || this.#replace === 'removed') liveAnimations.delete(this); else liveAnimations.add(this);
      this.#sync();
    }
    #replaceOlder() {
      const target = effects.get(this.#effect)?.target, covered = new Set();
      if (!target) return;
      for (const animation of [...liveAnimations].sort((a, b) => animationOrder(b) - animationOrder(a))) {
        const effect = animation.effect && effects.get(animation.effect);
        if (effect?.target !== target || !replaceable(animation)) continue;
        const names = effect.frames.flatMap(frame => frame.props.map(([name]) => name));
        if (animationOrder(animation) < this.#id && names.every(name => covered.has(name))) {
          animation.#replace = 'removed';
          liveAnimations.delete(animation);
          animation.#sync();
          animation.#fire('remove', animation.#now());
        } else for (const name of names) covered.add(name);
      }
    }
    #sync() {
      const effect = this.#effect && effects.get(this.#effect), target = effect?.target, time = this.#current();
      if (!target || effect.pseudo !== null && effect.pseudo !== '::before' && effect.pseudo !== '::after' || time === null || this.#replace === 'removed') { if (this.#synced) { this.#synced = false; call('dom.animation', this.#id, -1, null); } return; }
      const { timing } = effect, duration = timing.duration === 'auto' ? 0 : Math.min(timing.duration, 1e12);
      this.#synced = true;
      call('dom.animation', this.#id, target._id, {
        frames: effect.frames.map(frame => [frame.computed, frame.props.map(([name, value]) => `${name}:${value}`).join(';'), frame.easing]),
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
  const cssEntry = ([serial, id, kind, name, time, state, timing, direction, fill, easing, frames, pseudo]) => {
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
    commitStyles: { value() { const target = cssData.get(this).effect.target, computed = getComputedStyle(target); for (const frame of effects.get(cssData.get(this).effect).frames) for (const [name] of frame.props) target.style.setProperty(name, computed.getPropertyValue(name)); }, writable: true, configurable: true },
  });
  const cssOf = (node, subtree) => {
    const list = call('dom.cssAnimations', node._id, subtree, subtree).map(cssEntry);
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
  const syncWebviews = () => {
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
  };
  globalThis.__appsWebviewEvent = (id, name, detail) => attachedWebviews.get(id)?.dispatchEvent(Object.assign(new Event(name), detail));
  globalThis.__appsWebviewReply = (id, value, error) => { const reply = webviewReplies.get(id); webviewReplies.delete(id); if (error === null) reply?.[0](value); else reply?.[1](new Error(error)); };
  // Use the last frame's observer flags. A forced layout here adds work every frame.
  globalThis.__appsLayout = () => { if (active) { observing = true; const lost = !focusable(active); observing = false; if (lost) active.blur(); } syncWebviews(); for (const ref of resizeObservers) { const observer = ref.deref(); if (!observer) { resizeObservers.delete(ref); continue; } const entries = []; observing = true; try { for (const [key, previous] of observer.targets) { const target = key.deref(); if (!target) { observer.targets.delete(key); continue; } const rect = target.getBoundingClientRect(), at = geometry[2]; if (previous?.[0] === rect.width && previous[1] === rect.height && previous[2] === at) continue; const [, , clientWidth, clientHeight, top, right, bottom, left] = client(target), width = Math.max(0, clientWidth - left - right), height = Math.max(0, clientHeight - top - bottom), value = rect.width + ':' + rect.height + ':' + width + ':' + height; observer.targets.set(key, [rect.width, rect.height, at, value]); if (value !== previous?.[3]) { entries.push({ target, contentRect: new DOMRect(left, top, width, height), borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }], contentBoxSize: [{ inlineSize: width, blockSize: height }], devicePixelContentBoxSize: [{ inlineSize: width * devicePixelRatio, blockSize: height * devicePixelRatio }] }); } } } finally { observing = false; } if (entries.length) observer.callback(entries, observer); } };
  const resizeLayout=globalThis.__appsLayout;
  globalThis.__appsMediaChanged=()=>{
    if (!mediaQueries.size) return;
    const viewport = host('dom.viewport').join(':');
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
  // Resize observers measure the last frame before scroll listeners change the page, so its styles are still computed.
  globalThis.__appsLayout=()=>{resizeLayout();for(const id of call('dom.scrollEvents'))wrap(id)?.dispatchEvent(new Event('scroll',{bubbles:!id}));__appsMediaChanged();};
  const mediaLayout=globalThis.__appsLayout;
  globalThis.__appsLayout=()=>{
    mediaLayout();
    if(!intersectionObservers.size)return;
    call('dom.layout',false,true);
    let viewport;
    for(const ref of intersectionObservers){
      const observer=ref.deref();
      if(!observer){intersectionObservers.delete(ref);continue;}
      observing=true;
      try{
      const m=layout(),root=observer.root==null||observer.root===document?new DOMRect(0,0,...(viewport??=host('dom.viewport'))):observer.root.getBoundingClientRect();
      const margins=observer.rootMargin.trim().split(/\s+/).map(value=>value.endsWith('%')?parseFloat(value)*root.width/100:parseFloat(value));
      if(margins.some(v=>!Number.isFinite(v)))throw new DOMException('Invalid rootMargin','SyntaxError');
      const [top,right=top,bottom=top,left=right]=margins,stop=observer.root?._m;
      const rootBounds=new DOMRect(root.x-left,root.y-top,root.width+left+right,root.height+top+bottom),rl=rootBounds.left,rt=rootBounds.top,rr=rootBounds.right,rb=rootBounds.bottom;
      for(const [key,previous]of observer.targets){
        const target=key.deref();
        if(!target){observer.targets.delete(key);continue;}
        const i=4+target._id*8,inside=i<limit&&!(marks[target._id]&16),bx=inside?m[i]:0,by=inside?m[i+1]:0,bw=inside?m[i+2]:0,bh=inside?m[i+3]:0;
        let x=Math.max(bx,rl),y=Math.max(by,rt),r=Math.min(bx+bw,rr),b=Math.min(by+bh,rb);
        for(let rec=target._m.up;rec&&rec.tag[0]!=='#'&&rec!==stop;rec=rec.up){if(marks[rec.id]&4){const j=4+rec.id*8;x=Math.max(x,m[j]);y=Math.max(y,m[j+1]);r=Math.min(r,m[j]+m[j+2]);b=Math.min(b,m[j+1]+m[j+3]);}}
        const isIntersecting=target.isConnected&&r>=x&&b>=y,area=bw*bh,ratio=isIntersecting?(area?Math.max(0,r-x)*Math.max(0,b-y)/area:1):0;
        let changed=previous===null||previous.isIntersecting!==isIntersecting;
        for(let k=0;!changed&&k<observer.thresholds.length;k++)changed=(previous.ratio<observer.thresholds[k])!==(ratio<observer.thresholds[k]);
        if(changed)observer.records.push({time:performance.now(),target,rootBounds,boundingClientRect:new DOMRect(bx,by,bw,bh),intersectionRect:new DOMRect(x,y,Math.max(0,r-x),Math.max(0,b-y)),isIntersecting,intersectionRatio:ratio});
        if(previous===null)observer.targets.set(key,{ratio,isIntersecting});else{previous.ratio=ratio;previous.isIntersecting=isIntersecting;}
      }
      }finally{observing=false;}
      const entries=observer.takeRecords();if(entries.length)observer.callback(entries,observer);
    }
  };

  globalThis.__appsEditing = () => editable(active) ? { id: active._id, text: active.value, start: active.selectionDirection === 'backward' ? active.selectionEnd : active.selectionStart, end: active.selectionDirection === 'backward' ? active.selectionStart : active.selectionEnd, marked: composing } : active?.isContentEditable ? { id: active._id, text: active.textContent, start: textOffset(active,selection.anchorNode,selection.anchorOffset), end: textOffset(active,selection.focusNode,selection.focusOffset), node: selection.focusNode?._id, offset: selection.focusOffset, marked: composing } : null;
  const blockOf = (host, node) => { let block = node.nodeType === 3 ? node.parentNode : node; while (block !== host && !/^(div|p|li|h[1-6]|blockquote|pre)$/.test(block.localName)) block = block.parentNode; return block; };
  const paragraph = (host, range) => {
    const block = blockOf(host, range.startContainer), tail = document.createRange();
    tail.setStart(range.startContainer, range.startOffset);
    if (block === host) {
      let top = range.startContainer; while (top.parentNode !== host) top = top.parentNode;
      let next = top === host ? null : top.nextSibling; while (next && !/^(div|p|li|h[1-6]|blockquote|pre|ul|ol|table)$/.test(next.localName)) next = next.nextSibling;
      if (next) tail.setEndBefore(next); else tail.setEnd(host, host.childNodes.length);
      const line = document.createElement('div'); line.append(tail.extractContents());
      if (!line.textContent) line.replaceChildren(document.createElement('br'));
      if (!host.textContent && !host.querySelector('br')) host.prepend(document.createElement('br'));
      host.insertBefore(line, next); return line;
    }
    tail.setEndAfter(block.lastChild ?? block);
    const line = document.createElement(/^(li|p)$/.test(block.localName) ? block.localName : 'div'); line.append(tail.extractContents());
    if (!line.textContent && !line.querySelector('br')) line.replaceChildren(document.createElement('br'));
    if (!block.textContent && !block.querySelector('br')) block.append(document.createElement('br'));
    block.after(line); return line;
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
    if (!text || !target?.isConnected) return;
    remember(target, at, removed, text, !removed);
    dispatch(target, trusted(new CompositionEvent('compositionend', { bubbles: true, composed: true, data: text })));
  };
  globalThis.__appsFinishComposition = finishComposition;
  globalThis.__appsInput = (text, start, end, mark = false, selected = null, inputType, dataTransfer = null) => {
    const target = active;
    if (!target || disabled(target) || target.readOnly) return finishComposition();
    const state = __appsEditing(); if (!state) return finishComposition();
    text = String(text);
    start ??= composing?.[0] ?? Math.min(state.start,state.end); end ??= composing?.[1] ?? Math.max(state.start,state.end);
    start = Math.max(0, Math.min(start, state.text.length)); end = Math.max(start, Math.min(end, state.text.length));
    const isComposing = mark || composing !== null, control = editable(target);
    inputType ??= isComposing ? 'insertCompositionText' : text ? 'insertText' : 'deleteContentBackward';
    if (mark && !composing) {
      composing = [start,end]; composed = [start, state.text.slice(start,end), ''];
      dispatch(target, trusted(new CompositionEvent('compositionstart', { bubbles: true, composed: true, data: '' })));
    }
    if (isComposing) dispatch(target, trusted(new CompositionEvent('compositionupdate', { bubbles: true, composed: true, data: text })));
    // Offsets at block boundaries are ambiguous. Reuse a matching selection, or locate the first replaced
    // character.
    const replaced = () => {
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
    if (control) {
      setValue(target, state.text.slice(0,start) + text + state.text.slice(end));
      target._edited = true;
      target.setSelectionRange(start + (selected?.[0] ?? text.length), start + (selected?.[1] ?? text.length));
    } else {
      const range = replaced();
      if (!range.collapsed) range.deleteContents();
      const plain = target.getAttribute('contenteditable')?.toLowerCase() === 'plaintext-only';
      if (inputType === 'insertParagraph' && !plain) { const line = paragraph(target, range), first = line.firstChild; selection.collapse(first?.nodeType === 3 ? first : line, 0); }
      else if (inputType === 'insertLineBreak' && !plain) { const br = document.createElement('br'); range.insertNode(br); if (!br.nextSibling || br.nextSibling.nodeType === 3 && !br.nextSibling.data && !br.nextSibling.nextSibling) br.after(document.createElement('br')); selection.collapse(br.parentNode, br.parentNode.childNodes.indexOf(br) + 1); }
      else { const [node, at] = text ? insertText(range, text) : [range.startContainer, range.startOffset]; if (node.nodeType === 3) selection.setBaseAndExtent(node, at + (selected?.[0] ?? text.length), node, at + (selected?.[1] ?? text.length)); else selection.collapse(node, at); }
    }
    if (!mark && !inputType.startsWith('history')) {
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
    const history = active && histories.get(active), state = __appsEditing(), entry = history?.[redo ? 'redo' : 'undo'].pop();
    if (!entry || !state) return false;
    const [from, to] = redo ? [entry.removed, entry.inserted] : [entry.inserted, entry.removed];
    if (state.text.slice(entry.at, entry.at + from.length) !== from) { history.undo.length = history.redo.length = 0; return false; }
    const done = !!__appsInput(to, entry.at, entry.at + from.length, false, null, redo ? 'historyRedo' : 'historyUndo');
    entry.typed = false;
    history[done !== redo ? 'redo' : 'undo'].push(entry);
    return done;
  };
  const edit = (command, text = '') => {
    const target = active, state = __appsEditing();
    if (command === 'undo' || command === 'redo') return undo(command === 'redo');
    if (command === 'selectAll') {
      if (editable(target)) target.select();
      else if (state) selection.selectAllChildren(target);
      else { const root = selection.anchorNode?.getRootNode(); selection.selectAllChildren(root?.host ? root : document.body); }
      return true;
    }
    if (command === 'unselect') { if (editable(target)) target.setSelectionRange(target.selectionEnd, target.selectionEnd); else selection.removeAllRanges(); return true; }
    if (!state) return false;
    const start = Math.min(state.start,state.end), end = Math.max(state.start,state.end), forward = command === 'forwardDelete';
    if (command === 'insertText' || command === 'replace') return !!__appsInput(text, start, end, false, null, command === 'replace' ? 'insertReplacementText' : 'insertText');
    return !!__appsInput('', start === end && !forward ? textBoundary(state.text,start,false) : start, start === end && forward ? textBoundary(state.text,end,true) : end, false, null, forward ? 'deleteContentForward' : 'deleteContentBackward');
  };
  globalThis.__appsEdit = (command, text = '') => command === 'copy' || command === 'cut' ? __appsClipboard(command) : void (command === 'paste' || command === 'pasteAndMatchStyle' ? __appsClipboard('paste', text) : edit(command, text));
  const editCommands = new Map(['copy', 'cut', 'paste', 'selectAll', 'insertText', 'delete', 'forwardDelete', 'undo', 'redo'].map(name => [name.toLowerCase(), name]));
  // Like browsers, page scripts reach the clipboard only while they handle user input.
  const enabled = command => {
    const state = __appsEditing(), ranged = state ? state.start !== state.end : !selection.isCollapsed, writable = !!state && !active.readOnly && !disabled(active);
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
    if (value !== target.value) { setValue(target, value); target._edited = true; dispatch(target, trusted(new Event('input', { bubbles: true, composed: true }))); }
    if (done) commit(target);
  };
  globalThis.__appsFrameEvent=(id,event)=>{const target=wrap(id);if(event.type==='did-finish-load')dispatch(target,new Event('load'));else if(event.type==='did-fail-load')dispatch(target,new Event('error'));else if(event.type==='message')dispatch(target,new MessageEvent('message',{data:event.data,origin:'null'}));};
  globalThis.__appsClipboard = (type, text = '') => {
    const target = active ?? document.body, data = new DataTransfer();
    if (type !== 'paste' && target.localName === 'input' && target.type === 'password') return '';
    if (type === 'paste') data.setData('text/plain', text);
    if (dispatch(target, trusted(new ClipboardEvent(type, { bubbles: true, cancelable: true, composed: true, clipboardData: data })))) {
      const state = __appsEditing();
      if (type === 'paste') { if (state) __appsInput(text, undefined, undefined, false, null, 'insertFromPaste', data); }
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
  globalThis.__appsFilesPicked=(id,files)=>{const input=picks.get(id);picks.delete(id);if(!input)return;if(!files){dispatch(input,new Event('cancel',{bubbles:true}));return;}input._files=files.map(({name,lastModified,data})=>new File([Uint8Array.fromBase64(data)],name,{lastModified,type:fileType(name)}));setValue(input,files.length?'C:\\fakepath\\'+files[0].name:'');dispatch(input,new Event('input',{bubbles:true,composed:true}));dispatch(input,new Event('change',{bubbles:true}));};
  const pickOption=(select,option)=>{if(option._current)return;for(const other of options(select))other._selected=other===option;select._none=false;selectedness(select);dispatch(select,trusted(new Event('input',{bubbles:true,composed:true})));dispatch(select,trusted(new Event('change',{bubbles:true})));};
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
    const target = document.elementFromPoint(x, y) ?? document.body;
    if (target !== dragTarget) { const previous = dragTarget; dragEvent('dragenter', target, x, y, previous); if (previous) dragEvent('dragleave', previous, x, y, target); dragTarget = target; }
    dragAccepted = dragEvent('dragover', dragTarget, x, y).defaultPrevented;
  };
  globalThis.__appsDrop = (files, x, y) => {
    const target = document.elementFromPoint(x, y) ?? document.body, previous = dragTarget, accepted = dragAccepted;
    dragTarget = dragData = null; dragAccepted = false;
    if (!accepted) { if (previous) dragEvent('dragleave', previous, x, y, null, new DataTransfer()); return; }
    const data = new DataTransfer();
    for (const value of files) { const file = new File([new Uint8Array(value.bytes)], value.name, { lastModified: value.lastModified, type: value.directory ? '' : fileType(value.name) }); droppedFiles.set(file, value.file); if (value.directory) droppedDirectories.add(file); data.items.add(file); }
    dragEvent('drop', target, x, y, null, data);
  };
  const nativeKeys = { backspace:'Backspace', delete:'Delete', enter:'Enter', escape:'Escape', tab:'Tab', left:'ArrowLeft', right:'ArrowRight', up:'ArrowUp', down:'ArrowDown', home:'Home', end:'End', pageup:'PageUp', pagedown:'PageDown', insert:'Insert', space:' ', shift:'Shift', control:'Control', ctrl:'Control', alt:'Alt', command:'Meta', super:'Meta' };
  const keyCodes = { Backspace:8, Tab:9, Enter:13, Shift:16, Control:17, Alt:18, Escape:27, ' ':32, PageUp:33, PageDown:34, End:35, Home:36, ArrowLeft:37, ArrowUp:38, ArrowRight:39, ArrowDown:40, Insert:45, Delete:46, Meta:91, Process:229 };
  let modifiers = {}, graphemes, words;
  const textBoundary = (text, offset, forward, word = false) => {
    const part = forward ? text.slice(offset) : text.slice(0,offset);
    if (!part) return offset;
    if (word) return forward ? offset + (part.match(/^\s*\S+/)?.[0].length ?? part.length) : part.match(/\S+\s*$/)?.index ?? 0;
    graphemes ??= new Intl.Segmenter(undefined, { granularity: 'grapheme' });
    let last = 0;
    for (const segment of graphemes.segment(part)) { if (forward) return offset + segment.segment.length; last = segment.index; }
    return last;
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
  const hover = (target, data) => {
    if (target === hovered) return;
    const previous = hovered, from = previous ? path(previous) : [], to = target ? path(target) : [];
    hovered = target;
    call('dom.hover', to.map(node => node._id));
    for (const [kind, Type] of [['pointer', PointerEvent], ['mouse', MouseEvent]]) {
      const fire = (node, type, ancestors, relatedTarget, bubbles) => dispatch(node, trusted(new Type(kind + type, { ...data, bubbles, cancelable: bubbles, composed: bubbles, relatedTarget })), ancestors);
      if (previous) fire(previous, 'out', from.slice(), target, true);
      from.forEach((node, index) => { if (node.nodeType !== 11 && !to.includes(node)) fire(node, 'leave', from.slice(index), target, false); });
      if (target) fire(target, 'over', to.slice(), previous, true);
      for (let index = to.length; index--;) if (to[index].nodeType !== 11 && !from.includes(to[index])) fire(to[index], 'enter', to.slice(index), previous, false);
    }
  };
  const tabOrder = () => {
    const list = [], stack = [document];
    while (stack.length) { const node = stack.pop(); if (node.nodeType === 1 && node.tabIndex >= 0 && focusable(node) && !(node.isContentEditable && node.parentElement?.isContentEditable)) list.push(node); const kids = (node._shadow ?? node).childNodes; for (let index = kids.length; index--;) stack.push(kids[index]); }
    return list.map((node, order) => [node.tabIndex > 0 ? node.tabIndex : Infinity, order, node]).sort((a, b) => a[0] - b[0] || a[1] - b[1]).map(entry => entry[2]);
  };
  let press = null, last = null, sliding = null;
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
      if (target && !focused) { commit(target); dispatch(target, trusted(new FocusEvent('blur', { composed: true }))); if (active === target) dispatch(target, trusted(new FocusEvent('focusout', { bubbles: true, composed: true }))); }
      dispatchEvent(trusted(new Event(name.slice(7))));
      if (target && focused && active === target) { dispatch(target, trusted(new FocusEvent('focus', { composed: true }))); if (active === target) dispatch(target, trusted(new FocusEvent('focusin', { bubbles: true, composed: true }))); }
      return true;
    }
    if (name.startsWith('window:')) return dispatchEvent(trusted(new Event(name.slice(7))));
    if (/^(animation|transition)(start|end|run|cancel|iteration)$/.test(name)) { const target = wrap(ids[0]); return !!target && dispatch(target, trusted(Object.assign(new Event(name, { bubbles: true, composed: true }), { pseudoElement: '' }, data))); }
    if (name === 'modifiers') {
      const previous = modifiers; modifiers = data;
      for (const [field,key] of Object.entries({ shiftKey:'Shift', ctrlKey:'Control', altKey:'Alt', metaKey:'Meta' })) {
        if (!!previous[field] !== !!data[field]) nativeEvent(data[field] ? 'keydown' : 'keyup', [], { ...data, key, code: key + 'Left', keyCode: keyCodes[key] });
      }
      return true;
    }
    if (name === 'keydown' || name === 'keyup') {
      if (name === 'keydown' && !keyboardFocus && !data.metaKey && !data.ctrlKey && !data.altKey) { keyboardFocus = true; if (active) call('dom.focus', path(active).map(node => node._id), ring = true, !windowFocused); }
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
      const plain = !event.metaKey && !event.ctrlKey && !event.altKey, tag = target.localName, type = tag === 'input' ? target.type : '';
      if (name === 'keyup') {
        if (event.key === ' ' && plain && (tag === 'button' || /^(button|submit|reset|image|checkbox|radio)$/.test(type) || tag === 'summary') && !disabled(target)) fireClick(target, { detail: 0 }, true);
        return true;
      }
      const state = __appsEditing(), mac = navigator.platform.includes('Mac'), primary = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey, lower = event.key.toLowerCase();
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
            const from = start !== end || forward ? start : line ? state.text.lastIndexOf('\n', start - 1) + 1 : textBoundary(state.text,start,false,word);
            const to = start !== end || backward ? end : line ? (state.text.indexOf('\n',end) < 0 ? state.text.length : state.text.indexOf('\n',end)) : textBoundary(state.text,end,true,word);
            __appsInput('', from, to, false, null, 'delete' + (line ? 'SoftLine' : word ? 'Word' : 'Content') + (forward ? 'Forward' : 'Backward'));
            return false;
          }
        }
        if (event.key === 'Enter' && plain && (tag === 'textarea' || target.isContentEditable)) {
          __appsInput('\n', start, end, false, null, tag === 'textarea' || event.shiftKey ? 'insertLineBreak' : 'insertParagraph');
          return false;
        }
        if (['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(event.key)) {
          const word = mac ? event.altKey : event.ctrlKey, documentEdge = mac ? event.metaKey : event.ctrlKey;
          const right = ['ArrowRight','ArrowDown','End'].includes(event.key), pos = state.end;
          let next = pos;
          if ((documentEdge && !['ArrowLeft','ArrowRight'].includes(event.key)) || (mac && ['Home','End'].includes(event.key))) next = right ? state.text.length : 0;
          else if (['Home','End'].includes(event.key) || (mac && event.metaKey && ['ArrowLeft','ArrowRight'].includes(event.key))) next = right ? (state.text.indexOf('\n',pos) < 0 ? state.text.length : state.text.indexOf('\n',pos)) : pos === 0 ? 0 : state.text.lastIndexOf('\n',pos - 1) + 1;
          else if (!event.shiftKey && start !== end && ['ArrowLeft','ArrowRight'].includes(event.key)) next = right ? end : start;
          else if (['ArrowLeft','ArrowRight'].includes(event.key)) next = textBoundary(state.text,pos,right,word);
          else {
            const lineStart = state.text.lastIndexOf('\n',pos - 1) + 1, column = pos - lineStart;
            const lineEnd = state.text.indexOf('\n',pos);
            if (right) { const nextEnd = state.text.indexOf('\n',lineEnd + 1); next = lineEnd < 0 ? state.text.length : Math.min(lineEnd + 1 + column,nextEnd < 0 ? state.text.length : nextEnd); }
            else { const previousStart = state.text.lastIndexOf('\n',lineStart - 2) + 1; next = lineStart === 0 ? 0 : Math.min(previousStart + column,lineStart - 1); }
          }
          const anchor = event.shiftKey ? state.start : next;
          if (editable(target)) target.setSelectionRange(Math.min(anchor,next), Math.max(anchor,next), next < anchor ? 'backward' : 'forward');
          else selection.setBaseAndExtent(...textPoint(target,anchor), ...textPoint(target,next));
          return false;
        }
      }
      if (event.key === 'Enter' && plain && tag === 'input') {
        commit(target);
        const form = target.form;
        if (form && active === target) { const submit = [...form.elements].find(control => control.localName === 'button' ? control.type === 'submit' : /^(submit|image)$/.test(control.type)); if (submit) { if (!disabled(submit)) fireClick(submit, { detail: 0 }, true); } else if ([...form.elements].filter(control => editable(control) && control.localName === 'input').length === 1) submitForm(form, null); }
        return false;
      }
      if (event.key === 'Enter' && plain && (tag === 'button' || /^(button|submit|reset|image)$/.test(type) || (tag === 'a' || tag === 'area') && target.hasAttribute('href') || tag === 'summary') && !disabled(target)) { fireClick(target, { detail: 0 }, true); return false; }
      if (event.key === 'Escape' && closeRequest()) return false;
      if (tag === 'select' && plain && event.key === ' ') { openSelect(target); return false; }
      if (tag === 'select' && plain && !disabled(target) && /^(ArrowUp|ArrowDown|Home|End|PageUp|PageDown)$/.test(event.key) || tag === 'select' && plain && event.key.length === 1 && event.key !== ' ') {
        const list = options(target).filter(option => !disabled(option)), index = list.indexOf(target.selectedOptions[0]);
        const next = event.key.length === 1 ? list.find((option, i) => i > index && option.text.toLowerCase().startsWith(event.key.toLowerCase())) ?? list.find(option => option.text.toLowerCase().startsWith(event.key.toLowerCase())) : list[event.key === 'Home' || event.key === 'PageUp' ? 0 : event.key === 'End' || event.key === 'PageDown' ? list.length - 1 : Math.max(0, Math.min(list.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))];
        if (next) pickOption(target, next);
        return false;
      }
      if (tag === 'input' && type === 'range' && plain && !disabled(target) && /^(Arrow(Up|Down|Left|Right)|Home|End|Page(Up|Down))$/.test(event.key)) {
        const [min, max, step] = rangeBounds(target), unit = step || (max - min) / 100, key = event.key, before = target.value;
        setRange(target, key === 'Home' ? min : key === 'End' ? max : Number(before) + (/Up|Right/.test(key) ? 1 : -1) * (key.startsWith('Page') ? Math.max(unit, (max - min) / 10) : unit));
        if (target.value !== before) dispatch(target, trusted(new Event('change', { bubbles: true })));
        return false;
      }
      if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const list = tabOrder(), index = list.indexOf(active), next = index < 0 ? (event.shiftKey ? list.length - 1 : 0) : (index + (event.shiftKey ? -1 : 1) + list.length) % list.length;
        list[next]?.focus();
        return false;
      }
      return true;
    }
    const located = Number.isFinite(data.clientX) && Number.isFinite(data.clientY);
    const hit = captures.get(1) ?? wrap(ids[0]) ?? (located ? document.elementFromPoint(data.clientX, data.clientY) : null), element = hit?.nodeType === 3 ? hit.parentElement : hit;
    if (!located) data = { ...data, clientX: Number.isFinite(data.clientX) ? data.clientX : last?.[0] ?? 0, clientY: Number.isFinite(data.clientY) ? data.clientY : last?.[1] ?? 0 };
    if (name === 'click') { if (element) { fireClick(element, mouseInit(data), true); if (data.detail === 2) dispatch(element, trusted(new MouseEvent('dblclick', mouseInit(data)))); } return !!element; }
    if (name === 'wheel') {
      const target = element ?? document.body, result = !target || dispatch(target, trusted(new WheelEvent('wheel', mouseInit(data, { deltaX: data.deltaX ?? 0, deltaY: data.deltaY ?? 0, deltaMode: data.deltaMode ?? 0 }))));
      last = [data.clientX, data.clientY];
      return result;
    }
    if (name !== 'pointermove' && name !== 'pointerdown' && name !== 'pointerup') return false;
    if (name === 'pointerdown') finishComposition();
    if (!captures.has(1)) hover(element, mouseInit(data));
    const target = element ?? sliding?.[0] ?? (selectionDrag ? selectionDrag.host : null), button = data.button ?? 0, init = mouseInit(data, name === 'pointermove' ? { button: -1 } : {});
    last = [data.clientX, data.clientY];
    if (name === 'pointerdown' && target) call('dom.active', path(target).map(node => node._id));
    if (name === 'pointerup' && !data.buttons) call('dom.active', []);
    if (!target) return false;
    if (name === 'pointerup' && selectionDrag && !data.buttons) __appsDispatch('pointermove', ids, { ...data, buttons: 1 << [0, 2, 1, 3, 4][button] });
    const first = name === 'pointerdown' && (!press || press.button === button), ended = name === 'pointerup' && !data.buttons;
    if (first) { pointers.add(1); press = { target, button, detail: data.detail ?? 1, mouse: true }; }
    if (name === 'pointerdown') keyboardFocus = false;
    if (first || ended) lightDismiss(first, target, data.clientX, data.clientY);
    const result = dispatch(target, trusted(new PointerEvent(first ? 'pointerdown' : ended ? 'pointerup' : 'pointermove', init)));
    if (first && !result) press.mouse = false;
    const blocked = path(target).some(node => node.nodeType === 1 && /^(button|input|select|textarea)$/.test(node.localName) && disabled(node));
    const mouse = !blocked && (press?.mouse ?? true) && dispatch(target, trusted(new MouseEvent(name === 'pointerdown' ? 'mousedown' : name === 'pointerup' ? 'mouseup' : 'mousemove', init)));
    if (name === 'pointerdown' && mouse) {
      if (button === 0) selectionDrag = null;
      const focus = path(target).find(node => focusable(node) && !(node.isContentEditable && node.parentElement?.isContentEditable));
      const offset = editable(focus) && button === 0 ? call('dom.textIndex', focus._id, data.clientX, data.clientY) : 0;
      if (focus) focus.focus(); else if (button === 0) active?.blur();
      if (button === 0 && focus?.localName === 'select') openSelect(focus);
      if (button === 0 && focus?.localName === 'input' && focus.type === 'range') { sliding = [focus, focus.value]; slide(focus, data.clientX); }
      else if (button === 0 && editable(focus)) {
        const anchor = data.shiftKey ? (__appsEditing()?.start ?? offset) : offset;
        const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
        const [start,end] = granularity > 1 ? selectionExtent(focus.value,offset,granularity) : [anchor,offset];
        focus.setSelectionRange(Math.min(start,end),Math.max(start,end),end < start ? 'backward' : 'forward');
        selectionDrag = { host: focus, offset: start, endOffset: end, granularity, x: data.clientX, y: data.clientY };
      } else if (button === 0 && call('dom.userSelect',target._id)[0] !== 'none') {
        const host = focus?.isContentEditable ? focus : target.getRootNode();
        const point = document.caretRangeFromPoint(data.clientX,data.clientY);
        if (point && host.contains(point.startContainer)) {
          const [mode,boundary] = call('dom.userSelect',point.startContainer._id);
          if (mode !== 'none' && dispatch(target, trusted(new Event('selectstart', { bubbles: true, cancelable: true })))) {
            const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
            const range = selectionRange(point.startContainer,point.startOffset,granularity,mode,boundary);
            if (data.shiftKey && host.contains(selection.anchorNode)) selection.extend(range.endContainer,range.endOffset);
            else selection.setBaseAndExtent(range.startContainer,range.startOffset,range.endContainer,range.endOffset);
            selectionDrag = { host, node:selection.anchorNode, offset:selection.anchorOffset, endNode:selection.focusNode, endOffset:selection.focusOffset, granularity, x:data.clientX, y:data.clientY };
          }
        }
      }
    }
    if (sliding && name !== 'pointerdown') { slide(sliding[0], data.clientX); if (ended) { if (sliding[0].value !== sliding[1]) dispatch(sliding[0], trusted(new Event('change', { bubbles: true }))); sliding = null; } }
    if (name === 'pointerdown' && !blocked && (button === 2 || button === 0 && data.ctrlKey && navigator.platform.includes('Mac')) && dispatch(target, trusted(new PointerEvent('contextmenu', { ...init, button: 2 })))) {
      const element = target.nodeType === 1 ? target : target.parentElement, link = element?.closest?.('a[href]'), media = element?.closest?.('img,video,audio,canvas'), field = element?.closest?.('input,textarea,[contenteditable=""],[contenteditable="true"]'), selected = String(getSelection?.() ?? ''), kind = media?.localName;
      host('dom.contextMenu', { x: data.clientX, y: data.clientY, linkURL: link?.href ?? '', linkText: link?.textContent ?? '', pageURL: location.href, frameURL: location.href, srcURL: media?.currentSrc ?? media?.src ?? '', mediaType: kind === 'img' ? 'image' : kind ?? 'none', hasImageContents: kind === 'img', isEditable: !!field, selectionText: selected, titleText: element?.title ?? '', altText: media?.alt ?? '', misspelledWord: '', dictionarySuggestions: [], selectionRect: { x: 0, y: 0, width: 0, height: 0 }, frameCharset: 'UTF-8', formControlType: field?.localName === 'input' ? field.type : 'none', spellcheckEnabled: false, menuSourceType: 'mouse', mediaFlags: {}, editFlags: { canUndo: !!field, canRedo: !!field, canCut: !!field && !!selected, canCopy: !!selected, canPaste: !!field, canDelete: !!field && !!selected, canSelectAll: true, canEditRichly: false } });
    }
    if (name === 'pointermove' && result && selectionDrag && (selectionDrag.x !== data.clientX || selectionDrag.y !== data.clientY)) {
      const { host, node, offset, endNode, endOffset, granularity } = selectionDrag;
      selectionDrag.x = data.clientX; selectionDrag.y = data.clientY;
      if (editable(host)) {
        let end = call('dom.textIndex',host._id,data.clientX,data.clientY), start = offset;
        if (granularity > 1) {
          const extent = selectionExtent(host.value,end,granularity);
          [start,end] = end < offset ? [endOffset,extent[0]] : [offset,Math.max(endOffset,extent[1])];
        }
        host.setSelectionRange(Math.min(start,end),Math.max(start,end),end < start ? 'backward' : 'forward');
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
    if (name === 'pointerup' && press?.button === button) {
      const down = path(press.target), common = path(target).find(node => node.nodeType === 1 && down.includes(node)), detail = press.detail;
      if (!blocked && common?.isConnected) { if (button === 0) { fireClick(common, { ...init, detail }, true); if (detail === 2) dispatch(common, trusted(new MouseEvent('dblclick', { ...init, detail }))); } else dispatch(common, trusted(new PointerEvent('auxclick', { ...init, detail }))); }
    }
    if (ended) { selectionDrag = null; press = null; pointers.delete(1); captures.get(1)?.releasePointerCapture(1); }
    return result;
  };
})();
