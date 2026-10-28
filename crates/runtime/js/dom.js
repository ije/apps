(() => {
  const call = globalThis.__appsDOM;
  delete globalThis.__appsDOM;
  const definitions=new Map(), definitionsWaiting=new Map();let constructing=null;
  const nodes = new Map(), events = new WeakMap(), registered = [], pointers = new Set(), captures = new Map(), observers = new Set();
  let active = null, hovered = null, selection = null, composing = null, mutationQueued = false, selectionDrag = null;
  const selectionPending = new Set();
  const selectionChanged = (target = document) => {
    if (selectionPending.has(target)) return;
    selectionPending.add(target);
    setTimeout(() => { selectionPending.delete(target); target.dispatchEvent(new Event('selectionchange', { bubbles: target !== document })); }, 0);
  };
  const assignedSlot = node => node.parentNode?._shadow ? wrap(call('dom.assignedSlot', node._id)) : null;
  const path = node => { const result = []; for (; node; node = assignedSlot(node) ?? node.parentNode ?? node.host) result.push(node); return result; };
  const change = record => {
    for (const observer of observers) for (const [target, options] of observer.targets) {
      if (target !== record.target && !(options.subtree && target.contains(record.target))) continue;
      if (!options[record.type] || (record.type === 'attributes' && options.attributeFilter && !options.attributeFilter.includes(record.attributeName))) continue;
      observer.records.push({ addedNodes: [], removedNodes: [], previousSibling: null, nextSibling: null, attributeName: null, attributeNamespace: null, oldValue: null, ...record });
      break;
    }
    if (!mutationQueued) {
      mutationQueued = true;
      queueMicrotask(() => { mutationQueued = false; for (const observer of observers) { const records = observer.takeRecords(); if (records.length) observer.callback(records, observer); } });
    }
  };
  const dispatch = (target, event, ancestors = path(target)) => {
    if(!event.composed){const boundary=ancestors.findIndex(node=>node.host&&node===target.getRootNode());if(boundary>=0)ancestors.length=boundary+1;} const originalStop = event.stopPropagation.bind(event), originalImmediate = event.stopImmediatePropagation.bind(event);
    let stopped = false, immediate = false, current = null, phase = 0;
    Object.defineProperties(event, { target: { configurable: true, get: () => target }, currentTarget: { configurable: true, get: () => current }, eventPhase: { configurable: true, get: () => phase } });
    event.stopPropagation = () => { stopped = true; originalStop(); };
    event.stopImmediatePropagation = () => { immediate = stopped = true; originalImmediate(); };
    event.composedPath = () => [...ancestors, window];
    for (const capture of [true, false]) {
      for (const node of capture ? [...ancestors].reverse() : ancestors) {
        if (!capture && node !== target && !event.bubbles) continue;
        current = node; phase = node === target ? 2 : capture ? 1 : 3;
        const listeners = events.get(node), key = event.type;
        for (const item of [...(listeners?.get(key) ?? [])]) {
          if (immediate) break;
          if (item.capture !== capture || !(listeners?.get(key) ?? []).includes(item)) continue;
          if (item.once) node.removeEventListener(event.type, item.fn, item.capture);
          try { if (typeof item.fn === 'function') item.fn.call(node, event); else item.fn.handleEvent(event); } catch (error) { console.error(error); }
        }
        if (!capture && !immediate && typeof node['on' + event.type] === 'function') try { node['on' + event.type](event); } catch (error) { console.error(error); }
        if (stopped) break;
      }
      if (stopped) break;
    }
    current = null; phase = 0;
    if (!stopped && event.bubbles) globalThis.dispatchEvent(event);
    return !event.defaultPrevented;
  };
  const serialize = node => {
    if (node.nodeType === 3) return node.data.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
    if (node.nodeType === 8) return '<!--' + node.data + '-->';
    if (node.nodeType === 11) return node.childNodes.map(serialize).join('');
    const attrs = node.attributes.map(a => ' ' + a.name + '="' + a.value.replaceAll('&', '&amp;').replaceAll('"', '&quot;') + '"').join('');
    return '<' + node.localName + attrs + '>' + node.innerHTML + (['area','base','br','col','embed','hr','img','input','link','meta','param','source','track','wbr'].includes(node.localName) ? '' : '</' + node.localName + '>');
  };
  class Node {
    constructor(id, tag) { this._id = id; this._children = new Set(); this._parent = null; nodes.set(id, new WeakRef(this)); registered.push(id); this.nodeType = ({ '#document': 9, '#text': 3, '#comment': 8, '#fragment': 11 })[tag] ?? 1; this.nodeName = tag.startsWith('#') ? tag === '#fragment' ? '#document-fragment' : tag : tag.toUpperCase(); }
    get ownerDocument() { if(this.nodeType===9)return null;const root=this.getRootNode();return root.nodeType===9?root:document; }
    get parentNode() { return wrap(call('dom.node', this._id).parent); }
    get parentElement() { const p = this.parentNode; return p?.nodeType === 1 ? p : null; }
    get assignedSlot() { const slot = assignedSlot(this); return slot?.getRootNode().mode === 'closed' ? null : slot; }
    get childNodes() { const result = call('dom.node', this._id).children.map(wrap); result.item = i => result[i] ?? null; return result; }
    get firstChild() { return wrap(call('dom.node', this._id).children[0]); }
    get lastChild() { return wrap(call('dom.node', this._id).children.at(-1)); }
    get nextSibling() { return wrap(call('dom.sibling', this._id, 1)); }
    get previousSibling() { return wrap(call('dom.sibling', this._id, -1)); }
    get isConnected() { return this.getRootNode({ composed: true }).nodeType === 9; }
    get textContent() { return call('dom.text', this._id); }
    set textContent(value) { const observed = observers.size > 0, oldValue = observed ? this.textContent : null, removedNodes = observed ? this.childNodes : []; for (const child of this._children) child._parent = null; this._children.clear(); call('dom.setText', this._id, this.nodeType === 3 || this.nodeType === 8 ? String(value ?? '') : ''); if (this.nodeType === 1 && value != null && String(value)) this.appendChild(document.createTextNode(String(value))); change({ type: this.nodeType === 3 ? 'characterData' : 'childList', target: this, oldValue, removedNodes }); }
    get nodeValue() { return this.nodeType === 3 || this.nodeType === 8 ? this.textContent : null; }
    set nodeValue(value) { if (this.nodeType === 3 || this.nodeType === 8) this.textContent = value; }
    get data() { return this.nodeValue; }
    set data(value) { this.nodeValue = value; }
    get length() { return this.data?.length ?? 0; }
    substringData(offset, count) { return this.data.slice(offset, offset + count); }
    appendData(data) { this.data += data; }
    deleteData(offset, count) { this.replaceData(offset, count, ''); }
    insertData(offset, data) { this.replaceData(offset, 0, data); }
    replaceData(offset, count, data) { if (offset < 0 || offset > this.length) throw new DOMException('Invalid offset', 'IndexSizeError'); this.data = this.data.slice(0, offset) + data + this.data.slice(offset + count); }
    splitText(offset) { if (offset < 0 || offset > this.length) throw new DOMException('Invalid offset', 'IndexSizeError'); const other = document.createTextNode(this.data.slice(offset)); this.data = this.data.slice(0, offset); this.parentNode?.insertBefore(other, this.nextSibling); return other; }
    appendChild(child) { return this.insertBefore(child, null); }
    insertBefore(child, before) {
      if (!(child instanceof Node)) throw new TypeError('Expected a Node');
      if (child.nodeType === 11) { for (const node of [...child.childNodes]) this.insertBefore(node, before); return child; }
      const observed = observers.size > 0, oldParent = observed ? child.parentNode : null, previousSibling = observed ? before?.previousSibling ?? this.lastChild : null;
      call('dom.insert', this._id, child._id, before?._id ?? null); child._parent?._children.delete(child); child._parent = this; this._children.add(child);
      if (definitions.size && child.isConnected) { const stack=[child];while(stack.length){const node=stack.pop();node.connectedCallback?.();stack.push(...node.childNodes);if(node._shadow)stack.push(node._shadow);} }
      if (oldParent && oldParent !== this) change({ type: 'childList', target: oldParent, removedNodes: [child] });
      change({ type: 'childList', target: this, addedNodes: [child], previousSibling, nextSibling: before }); return child;
    }
    removeChild(child) { const observed = observers.size > 0, connected = definitions.size > 0 && child.isConnected, previousSibling = observed ? child.previousSibling : null, nextSibling = observed ? child.nextSibling : null; call('dom.remove', this._id, child._id); this._children.delete(child); child._parent = null; if(connected){const stack=[child];while(stack.length){const node=stack.pop();node.disconnectedCallback?.();stack.push(...node.childNodes);if(node._shadow)stack.push(node._shadow);}} change({ type: 'childList', target: this, removedNodes: [child], previousSibling, nextSibling }); return child; }
    replaceChild(child, old) { this.insertBefore(child, old); return this.removeChild(old); }
    replaceChildren(...children) { for (const child of [...this.childNodes]) this.removeChild(child); call('dom.setText', this._id, ''); this.append(...children); }
    append(...children) { for (const child of children) this.appendChild(child instanceof Node ? child : document.createTextNode(String(child))); }
    prepend(...children) { const before = this.firstChild; for (const child of children) this.insertBefore(child instanceof Node ? child : document.createTextNode(String(child)), before); }
    before(...children) { for (const child of children) this.parentNode?.insertBefore(child instanceof Node ? child : document.createTextNode(String(child)), this); }
    after(...children) { const before = this.nextSibling; for (const child of children) this.parentNode?.insertBefore(child instanceof Node ? child : document.createTextNode(String(child)), before); }
    replaceWith(...children) { this.before(...children); this.remove(); }
    remove() { this.parentNode?.removeChild(this); }
    hasChildNodes() { return this.childNodes.length > 0; }
    contains(node) { for (; node; node = node.parentNode) if (node === this) return true; return false; }
    getRootNode(options = {}) { let node = this; while (node.parentNode || (options.composed && node.host)) node = node.parentNode ?? node.host; return node; }
    cloneNode(deep = false) { const cloned = this.nodeType === 3 ? document.createTextNode(this.data) : this.nodeType === 8 ? document.createComment(this.data) : this.nodeType === 11 ? document.createDocumentFragment() : document.createElementNS(this.namespaceURI, this.localName); if (this.attributes) for (const attr of this.attributes) cloned.setAttribute(attr.name, attr.value); if (deep) for (const child of this.childNodes) cloned.appendChild(child.cloneNode(true)); return cloned; }
    isSameNode(other) { return this === other; }
    isEqualNode(other) { return other?.nodeType === this.nodeType && serialize(this) === serialize(other); }
    compareDocumentPosition(other) { if (this === other) return 0; const [a, b] = [this, other].map(node => { const chain = []; for (; node; node = node.parentNode) chain.push(node); return chain.reverse(); }); if (a[0] !== b[0]) return 1 | 32; let i = 0; while (a[i] && a[i] === b[i]) i++; if (i === a.length) return 4 | 16; if (i === b.length) return 2 | 8; const siblings = call('dom.node', a[i - 1]._id).children; return siblings.indexOf(a[i]._id) < siblings.indexOf(b[i]._id) ? 4 : 2; }
    normalize() { for (const child of [...this.childNodes]) { if (child.nodeType === 3) { while (child.nextSibling?.nodeType === 3) { child.data += child.nextSibling.data; child.nextSibling.remove(); } if (!child.length) child.remove(); } else child.normalize(); } }
    addEventListener(name, fn, options = false) { if (!fn || options?.signal?.aborted) return; const capture = typeof options === 'boolean' ? options : !!options?.capture; let listeners = events.get(this); if (!listeners) events.set(this, listeners = new Map()); const list = listeners.get(name) ?? []; if (!list.some(item => item.fn === fn && item.capture === capture)) list.push({ fn, capture, once: !!options?.once }); listeners.set(name, list); options?.signal?.addEventListener('abort', () => this.removeEventListener(name, fn, capture), { once: true }); }
    removeEventListener(name, fn, options = false) { const capture = typeof options === 'boolean' ? options : !!options?.capture, listeners = events.get(this); listeners?.set(name, (listeners.get(name) ?? []).filter(item => item.fn !== fn || item.capture !== capture)); }
    dispatchEvent(event) { if (!(event instanceof Event)) throw new TypeError('Expected an Event'); return dispatch(this, event); }
  }
  for (const [name, value] of Object.entries({ ELEMENT_NODE: 1, TEXT_NODE: 3, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_FRAGMENT_NODE: 11, DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16 })) { Node[name] = value; Node.prototype[name] = value; }
  const cssName = name => String(name).replace(/[A-Z]/g, c => '-' + c.toLowerCase());
  const styleObject = (read, write) => new Proxy({
    get cssText() { return read(); }, set cssText(value) { write(String(value)); },
    get length() { return this.cssText.split(';').filter(v => v.includes(':')).length; },
    item(index) { return this.cssText.split(';').filter(v => v.includes(':'))[index]?.split(':')[0].trim() ?? ''; },
    getPropertyValue(name) { return read().split(';').findLast(v => v.split(':')[0].trim() === name)?.split(':').slice(1).join(':').trim().replace(/\s*!important$/, '') ?? ''; },
    getPropertyPriority(name) { return read().split(';').findLast(v => v.split(':')[0].trim() === name)?.trim().endsWith('!important') ? 'important' : ''; },
    setProperty(name, value, priority = '') { const list = read().split(';').filter(v => v.split(':')[0].trim() !== name && v.trim()); if (value !== '' && value != null) list.push(name + ':' + value + (priority === 'important' ? ' !important' : '')); write(list.join(';')); },
    removeProperty(name) { const old = this.getPropertyValue(name); this.setProperty(name, ''); return old; },
  }, { get: (target, name) => name in target ? Reflect.get(target, name) : /^\d+$/.test(String(name)) ? target.item(Number(name)) : target.getPropertyValue(cssName(name)), set(target, name, value) { if (name === 'cssText') target.cssText = value; else target.setProperty(cssName(name), value); return true; } });
  class Element extends Node {
    constructor(id, tag) { if(id===undefined){if(constructing?.node){const node=constructing.node;constructing=null;Object.setPrototypeOf(node,new.target.prototype);return node;}tag=constructing?.tag??[...definitions].find(([,ctor])=>ctor===new.target)?.[0];if(!tag)throw new TypeError('Illegal element constructor');id=call('dom.create',tag);constructing=null;} super(id, tag);  this.tagName = this.nodeName; this.localName = tag; this.namespaceURI = 'http://www.w3.org/1999/xhtml'; }
    get attributes() { const attrs = Object.entries(call('dom.snapshot', this._id).attrs).map(([name, value]) => ({ name, value, nodeName: name, nodeValue: value, specified: true })); attrs.item = i => attrs[i] ?? null; attrs.getNamedItem = name => attrs.find(a => a.name === name) ?? null; return attrs; }
    get children() { const list = this.childNodes.filter(n => n.nodeType === 1); list.item = i => list[i] ?? null; return list; }
    get childElementCount() { return this.children.length; }
    get firstElementChild() { return this.children[0] ?? null; }
    get lastElementChild() { return this.children.at(-1) ?? null; }
    get nextElementSibling() { let n = this.nextSibling; while (n && n.nodeType !== 1) n = n.nextSibling; return n; }
    get previousElementSibling() { let n = this.previousSibling; while (n && n.nodeType !== 1) n = n.previousSibling; return n; }
    get id() { return this.getAttribute('id') ?? ''; } set id(value) { this.setAttribute('id', value); }
    get className() { return this.getAttribute('class') ?? ''; } set className(value) { this.setAttribute('class', value); }
    get classList() { const element = this; return { contains: name => element.className.split(/\s+/).includes(name), add(...names) { element.className = [...new Set([...element.className.split(/\s+/).filter(Boolean), ...names])].join(' '); }, remove(...names) { element.className = element.className.split(/\s+/).filter(n => !names.includes(n)).join(' '); }, toggle(name, force) { const present = this.contains(name), add = force ?? !present; if (add) this.add(name); else this.remove(name); return add; }, replace(old, value) { if (!this.contains(old)) return false; this.remove(old); this.add(value); return true; }, get value() { return element.className; }, set value(value) { element.className = value; }, [Symbol.iterator]() { return element.className.split(/\s+/).filter(Boolean)[Symbol.iterator](); } }; }
    get dataset() { return this._dataset ??= new Proxy({}, { get: (_, name) => this.getAttribute('data-' + cssName(name)) ?? undefined, set: (_, name, value) => { this.setAttribute('data-' + cssName(name), value); return true; }, deleteProperty: (_, name) => { this.removeAttribute('data-' + cssName(name)); return true; }, ownKeys: () => this.attributes.filter(a => a.name.startsWith('data-')).map(a => a.name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())), getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }) }); }
    getAttribute(name) { return call('dom.attr', this._id, String(name)); }
    getAttributeNS(_, name) { return this.getAttribute(name); }
    hasAttribute(name) { return this.getAttribute(name) !== null; }
    hasAttributeNS(_, name) { return this.hasAttribute(name); }
    setAttribute(name, value) { name = String(name); const watched = this.constructor.observedAttributes?.includes(name), oldValue = watched || observers.size ? this.getAttribute(name) : null; call('dom.setAttr', this._id, name, String(value)); if(watched)this.attributeChangedCallback?.(name,oldValue,String(value)); change({ type: 'attributes', target: this, attributeName: name, oldValue }); }
    setAttributeNS(_, name, value) { this.setAttribute(name, value); }
    removeAttribute(name) { const watched = this.constructor.observedAttributes?.includes(name), oldValue = watched || observers.size ? this.getAttribute(name) : null; call('dom.removeAttr', this._id, name); if(oldValue!==null&&watched)this.attributeChangedCallback?.(name,oldValue,null); if (oldValue !== null) change({ type: 'attributes', target: this, attributeName: name, oldValue }); }
    removeAttributeNS(_, name) { this.removeAttribute(name); }
    toggleAttribute(name, force) { const present = force ?? !this.hasAttribute(name); if (present) this.setAttribute(name, ''); else this.removeAttribute(name); return present; }
    get style() { return this._style ??= styleObject(() => this.getAttribute('style') ?? '', value => this.setAttribute('style', value)); }
    get innerHTML() { return this.childNodes.map(serialize).join('') || call('dom.snapshot', this._id).text; }
    set innerHTML(value) { const removedNodes = this.childNodes; for (const child of this._children) child._parent = null; this._children.clear(); call('dom.html', this._id, String(value)); __appsImages(this); change({ type: 'childList', target: this, removedNodes, addedNodes: this.childNodes }); }
    get outerHTML() { return serialize(this); }
    set outerHTML(value) { const container = document.createElement('div'); container.innerHTML = value; this.replaceWith(...container.childNodes); }
    get innerText() { return this.textContent; } set innerText(value) { this.textContent = value; }
    querySelector(selector) { return wrap(call('dom.queryWithin', this._id, String(selector), true)); }
    querySelectorAll(selector) { const list = call('dom.queryWithin', this._id, String(selector)).map(wrap); list.item = i => list[i] ?? null; return list; }
    matches(selector) { return call('dom.matches', this._id, String(selector)); }
    closest(selector) { for (let node = this; node; node = node.parentElement) if (node.matches(selector)) return node; return null; }
    getElementsByTagName(tag) { return this.querySelectorAll(tag); }
    getElementsByClassName(names) { return this.querySelectorAll('.' + String(names).trim().split(/\s+/).join('.')); }
    getBoundingClientRect() { const rect = call('dom.rect', this._id); return new DOMRect(rect.x, rect.y, rect.width, rect.height); }
    getClientRects() { return this.isConnected ? [this.getBoundingClientRect()] : []; }
    get clientWidth() { return this.getBoundingClientRect().width; } get clientHeight() { return this.getBoundingClientRect().height; }
    get clientLeft() { return parseFloat(getComputedStyle(this).borderLeftWidth) || 0; } get clientTop() { return parseFloat(getComputedStyle(this).borderTopWidth) || 0; }
    get offsetWidth() { return this.clientWidth; } get offsetHeight() { return this.clientHeight; }
    get offsetParent() { let node = this.parentElement; while (node && (call('dom.style', node._id, ['position']).position ?? 'static') === 'static') node = node.parentElement; return node ?? document.body; }
    get offsetLeft() { return this.getBoundingClientRect().left - (this.offsetParent?.getBoundingClientRect().left ?? 0); }
    get offsetTop() { return this.getBoundingClientRect().top - (this.offsetParent?.getBoundingClientRect().top ?? 0); }
    get scrollTop() { return call('dom.rect', this._id).scrollTop; } set scrollTop(value) { call('dom.scroll', this._id, null, Number(value)); }
    get scrollLeft() { return call('dom.rect', this._id).scrollLeft; } set scrollLeft(value) { call('dom.scroll', this._id, Number(value), null); }
    get scrollWidth() { return call('dom.rect', this._id).scrollWidth; } get scrollHeight() { return call('dom.rect', this._id).scrollHeight; }
    scrollTo(left, top) { if (typeof left === 'object') { top = left.top; left = left.left; } call('dom.scroll', this._id, left ?? null, top ?? null); }
    scroll(left, top) { this.scrollTo(left, top); }
    scrollBy(left, top) { if (typeof left === 'object') { top = left.top; left = left.left; } this.scrollTo(this.scrollLeft + (left ?? 0), this.scrollTop + (top ?? 0)); }
    scrollIntoView(options = {}) {
      if (typeof options === 'boolean') options = { block: options ? 'start' : 'end' };
      const target = this.getBoundingClientRect(); let left = target.left, top = target.top;
      for (let p = this.assignedSlot ?? this.parentElement ?? this.getRootNode().host; p; p = p.assignedSlot ?? p.parentElement ?? p.getRootNode().host) {
        const style = getComputedStyle(p), bounds = p.getBoundingClientRect(); let scrollable = false;
        for (const [axis, align, start, size, edge, extent] of [
          ['Y', options.block ?? 'start', top, target.height, bounds.top + p.clientTop, p.clientHeight],
          ['X', options.inline ?? 'nearest', left, target.width, bounds.left + p.clientLeft, p.clientWidth],
        ]) {
          if (!['auto','scroll','hidden'].includes(style['overflow' + axis] || style.overflow)) continue;
          scrollable = true;
          const before = start - edge, after = start + size - edge - extent;
          let delta = align === 'center' ? (before + after) / 2 : align === 'end' ? after : before;
          if (align === 'nearest') delta = before < 0 && after > 0 ? 0 : before < 0 ? (size <= extent ? before : after) : after > 0 ? (size <= extent ? after : before) : 0;
          if (!delta) continue;
          const property = axis === 'Y' ? 'scrollTop' : 'scrollLeft', previous = p[property];
          p[property] += delta;
          if (axis === 'Y') top -= p[property] - previous; else left -= p[property] - previous;
        }
        if (scrollable && options.container === 'nearest') break;
      }
    }
    focus() { if (active === this || this.disabled) return; const previous = active; active = this; call('dom.focus', path(this).map(node => node._id)); if (previous) { dispatch(previous, new FocusEvent('blur', { composed: true, relatedTarget: this })); dispatch(previous, new FocusEvent('focusout', { bubbles: true, composed: true, relatedTarget: this })); } dispatch(this, new FocusEvent('focus', { composed: true, relatedTarget: previous })); dispatch(this, new FocusEvent('focusin', { bubbles: true, composed: true, relatedTarget: previous })); }
    blur() { if (active !== this) return; active = null; call('dom.focus', []); dispatch(this, new FocusEvent('blur', { composed: true })); dispatch(this, new FocusEvent('focusout', { bubbles: true, composed: true })); }
    click() { if (!this.disabled) dispatch(this, new MouseEvent('click', { bubbles: true, cancelable: true })); }
    get contentEditable() { return this.getAttribute('contenteditable') ?? 'inherit'; } set contentEditable(value) { this.setAttribute('contenteditable', value); }
    get isContentEditable() { const value = this.contentEditable.toLowerCase(); return ['true','plaintext-only',''].includes(value) || (value !== 'false' && !!this.parentElement?.isContentEditable); }
    get tabIndex() { return Number(this.getAttribute('tabindex') ?? (['input','textarea','button','select'].includes(this.localName) ? 0 : -1)); } set tabIndex(value) { this.setAttribute('tabindex', value); }
    attachShadow(options) { if (this._shadow) throw new DOMException('Shadow root already exists', 'NotSupportedError'); if(!['open','closed'].includes(options.mode))throw new TypeError('Invalid shadow mode');const root = document.createDocumentFragment(); Object.setPrototypeOf(root,ShadowRoot.prototype);root.host=this;root.mode=options.mode;this._shadow=root;call('dom.shadow',this._id,root._id);return root; }
    get shadowRoot() { return this._shadow?.mode === 'open' ? this._shadow : null; }
    setPointerCapture(id) { if (!pointers.has(id)) throw new DOMException('Pointer is not active', 'NotFoundError'); captures.set(id, this); }
    hasPointerCapture(id) { return captures.get(id) === this; }
    releasePointerCapture(id) { if (captures.get(id) === this) { captures.delete(id); dispatch(this, new PointerEvent('lostpointercapture', { pointerId: id })); } }
    insertAdjacentHTML(position, source) { const wrapper = document.createElement('div'); wrapper.innerHTML = source; const children = [...wrapper.childNodes]; if (position === 'beforebegin') this.before(...children); else if (position === 'afterend') this.after(...children); else if (position === 'afterbegin') this.prepend(...children); else if (position === 'beforeend') this.append(...children); else throw new DOMException('Invalid insertion position', 'SyntaxError'); }
  }
  class HTMLElement extends Element {}
  class SVGElement extends Element {constructor(id,tag){super(id,tag);this.namespaceURI='http://www.w3.org/2000/svg';this.nodeName=this.tagName=tag;}get ownerSVGElement(){for(let node=this.parentElement;node;node=node.parentElement)if(node instanceof SVGSVGElement)return node;return null;}get viewportElement(){return this.ownerSVGElement;}}
  class SVGSVGElement extends SVGElement {}
  for (const attr of ['title','name','type','src','href','placeholder','role','dir','lang','slot']) Object.defineProperty(Element.prototype, attr, { get() { return this.getAttribute(attr) ?? ''; }, set(value) { this.setAttribute(attr, value); } });
  for (const attr of ['hidden','disabled','readOnly','multiple','checked','selected','autofocus']) Object.defineProperty(Element.prototype, attr, { get() { return this.hasAttribute(attr.toLowerCase()); }, set(value) { this.toggleAttribute(attr.toLowerCase(), !!value); } });
  class HTMLSlotElement extends HTMLElement {
    assignedNodes(options = {}) { return call('dom.assignedNodes', this._id, !!options.flatten).map(wrap); }
    assignedElements(options = {}) { return this.assignedNodes(options).filter(node => node.nodeType === 1); }
  }
  class HTMLInputElement extends HTMLElement {
    get value() { return this._value ?? this.getAttribute('value') ?? (this.localName === 'textarea' ? this.textContent : ''); }
    set value(value) { this._value = String(value); call('dom.setAttr', this._id, 'value', this._value); const length = this._value.length; this._start = Math.min(this._start ?? length, length); this._end = Math.min(this._end ?? length, length); }
    get defaultValue() { return this.getAttribute('value') ?? ''; } set defaultValue(value) { this.setAttribute('value', value); }
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
    select() { this.setSelectionRange(0, this.value.length); }
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
  class HTMLImageElement extends HTMLElement {
    get src() { return this.getAttribute('src') ?? ''; } set src(value) { this.setAttribute('src',value); }
    get naturalWidth() { return this._dimensions?.[0] ?? 0; } get naturalHeight() { return this._dimensions?.[1] ?? 0; }
    get width() { return Number(this.getAttribute('width') ?? this.naturalWidth); } set width(value) { this.setAttribute('width',value); }
    get height() { return Number(this.getAttribute('height') ?? this.naturalHeight); } set height(value) { this.setAttribute('height',value); }
    get complete() { return !this.src || !!this._complete; }
    setAttribute(name,value) { super.setAttribute(name,value); if (name === 'src') this._loadImage(); }
    _loadImage() {
      const source = this.src; this._complete = false;
      this._loading = (async () => {
        let data;
        if (source.startsWith('data:')) { const comma=source.indexOf(','); const payload=source.slice(comma+1); data=source.slice(0,comma).endsWith(';base64') ? Uint8Array.from(atob(payload),c=>c.charCodeAt(0)) : new TextEncoder().encode(decodeURIComponent(payload)); }
        else { const response=await fetch(source); if (!response.ok) throw new Error('Image request failed: '+response.status); data=new Uint8Array(await response.arrayBuffer()); }
        if (this.src !== source) return;
        this._dimensions=call('dom.image',this._id,data); this._complete=true; this.dispatchEvent(new Event('load'));
      })();
      this._loading.catch(error=>{ if (this.src===source) {this._complete=true; this.dispatchEvent(new Event('error'));} });
    }
    decode() { if (!this._loading && this.src) this._loadImage(); return this._loading ?? Promise.resolve(); }
  }
  globalThis.__appsImages = root => { for (const image of root.querySelectorAll('img[src]')) if (!image._loading) image._loadImage(); };
  class DOMRect { constructor(x = 0, y = 0, width = 0, height = 0) { Object.assign(this, { x, y, width, height }); } get left() { return Math.min(this.x, this.x + this.width); } get right() { return Math.max(this.x, this.x + this.width); } get top() { return Math.min(this.y, this.y + this.height); } get bottom() { return Math.max(this.y, this.y + this.height); } toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }; } static fromRect(r = {}) { return new this(r.x, r.y, r.width, r.height); } }
  class UIEvent extends Event { constructor(type, options = {}) { super(type, options); this.view = options.view ?? window; this.detail = options.detail ?? 0; } }
  class MouseEvent extends UIEvent { constructor(type, options = {}) { super(type, options); const { bubbles, cancelable, composed, ...fields } = options; Object.assign(this, { clientX: 0, clientY: 0, screenX: 0, screenY: 0, button: 0, buttons: 0, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, relatedTarget: null }, fields); } }
  class KeyboardEvent extends UIEvent { constructor(type, options = {}) { super(type, options); const { bubbles, cancelable, composed, ...fields } = options; Object.assign(this, { key: '', code: '', keyCode: 0, charCode: 0, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, repeat: false, isComposing: false }, fields); } getModifierState(key) { return !!this[({ Control: 'ctrlKey', Meta: 'metaKey', Alt: 'altKey', Shift: 'shiftKey' })[key]]; } }
  class PointerEvent extends MouseEvent { constructor(type, options = {}) { super(type, options); this.pointerId = options.pointerId ?? 0; this.pointerType = options.pointerType ?? ''; this.isPrimary = options.isPrimary ?? false; } }
  class FocusEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.relatedTarget = options.relatedTarget ?? null; } }
  class InputEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.data = options.data ?? null; this.inputType = options.inputType ?? ''; this.isComposing = !!options.isComposing; this.dataTransfer = options.dataTransfer ?? null; this._targetRanges = Array.from(options.targetRanges ?? []); }
    getTargetRanges() { return this._targetRanges.slice(); }
  }
  class DataTransfer {
    constructor(){this.dropEffect='none';this.effectAllowed='all';this._data=new Map();this.files=[];this.items=[];this.items.add=(value,type)=>{if(value instanceof File){this.files.push(value);const item={kind:'file',type:value.type,getAsFile:()=>value,getAsString:()=>{}};this.items.push(item);return item;}this.setData(type,String(value));return this.items.at(-1);};this.items.remove=index=>{const item=this.items[index];if(item?.kind==='file')this.files.splice(this.files.indexOf(item.getAsFile()),1);this.items.splice(index,1);};this.items.clear=()=>{this.items.length=0;this.files.length=0;this._data.clear();};}
    get types(){return [...this._data.keys(),...(this.files.length?['Files']:[])];}getData(type){return this._data.get(type==='text'?'text/plain':type)??'';}setData(type,data){type=type==='text'?'text/plain':type;this._data.set(type,String(data));const item={kind:'string',type,getAsFile:()=>null,getAsString:callback=>queueMicrotask(()=>callback(this.getData(type)))};const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index<0)this.items.push(item);else this.items[index]=item;}clearData(type){if(type===undefined){this._data.clear();this.items.splice(0,this.items.length,...this.items.filter(item=>item.kind==='file'));}else{this._data.delete(type);const index=this.items.findIndex(item=>item.type===type&&item.kind==='string');if(index>=0)this.items.splice(index,1);}}
  }
  class ClipboardEvent extends Event {constructor(type,options={}){super(type,options);this.clipboardData=options.clipboardData??null;}}
  class DragEvent extends MouseEvent {constructor(type,options={}){super(type,options);this.dataTransfer=options.dataTransfer??null;}}
  class CompositionEvent extends UIEvent { constructor(type, options = {}) { super(type, options); this.data = options.data ?? ''; } }
  class DocumentFragment extends Node { querySelector(selector) { return wrap(call('dom.queryWithin', this._id, String(selector), true)); } querySelectorAll(selector) { return call('dom.queryWithin', this._id, String(selector)).map(wrap); } get children() { return this.childNodes.filter(n => n.nodeType === 1); } get activeElement() { return active && this.contains(active) ? active : null; } }
  class CSSStyleSheet {
    constructor(){this._text='';this._owners=new Map();}
    replaceSync(text){this._text=String(text);for(const node of this._owners.values())node.textContent=this._text;}
    replace(text){this.replaceSync(text);return Promise.resolve(this);}
    get cssRules(){return this._text.split(/(?<=})/).filter(v=>v.trim()).map(cssText=>({cssText}));}
    insertRule(rule,index=this.cssRules.length){const rules=this.cssRules.map(rule=>rule.cssText);if(index<0||index>rules.length)throw new DOMException('Invalid rule index','IndexSizeError');rules.splice(index,0,rule);this.replaceSync(rules.join(''));return index;}
    deleteRule(index){const rules=this.cssRules.map(rule=>rule.cssText);if(index<0||index>=rules.length)throw new DOMException('Invalid rule index','IndexSizeError');rules.splice(index,1);this.replaceSync(rules.join(''));}
  }
  class ShadowRoot extends DocumentFragment {
    get innerHTML(){return this.childNodes.map(serialize).join('');} set innerHTML(value){const removedNodes=this.childNodes;for(const child of this._children)child._parent=null;this._children.clear();call('dom.html',this._id,String(value));__appsImages(this);change({type:'childList',target:this,removedNodes,addedNodes:this.childNodes});}
    get adoptedStyleSheets(){return this._sheets??[];} set adoptedStyleSheets(sheets){for(const sheet of this._sheets??[]){sheet._owners.get(this)?.remove();sheet._owners.delete(this);}this._sheets=Array.from(sheets);for(const sheet of this._sheets){if(!(sheet instanceof CSSStyleSheet))throw new TypeError('Expected CSSStyleSheet');const style=document.createElement('style');style.textContent=sheet._text;this.prepend(style);sheet._owners.set(this,style);}}
    getElementById(id){return this.querySelectorAll('[id]').find(node=>node.id===String(id))??null;}
  }
  const customElements={define(name,Type){if(!/^[a-z][a-z0-9._-]*-[a-z0-9._-]+$/.test(name)||definitions.has(name)||[...definitions.values()].includes(Type))throw new DOMException('Invalid custom element definition','NotSupportedError');definitions.set(name,Type);this.upgrade(document);for(const resolve of definitionsWaiting.get(name)??[])resolve(Type);definitionsWaiting.delete(name);},get(name){return definitions.get(name);},getName(Type){return [...definitions].find(([,ctor])=>ctor===Type)?.[0]??null;},whenDefined(name){if(definitions.has(name))return Promise.resolve(definitions.get(name));return new Promise(resolve=>{const pending=definitionsWaiting.get(name)??[];pending.push(resolve);definitionsWaiting.set(name,pending);});},upgrade(root){const stack=[root];while(stack.length){const node=stack.pop(),Type=definitions.get(node.localName);if(Type&&!(node instanceof Type)){constructing={node};try{new Type();}finally{constructing=null;}for(const attr of Type.observedAttributes??[])if(node.hasAttribute(attr))node.attributeChangedCallback?.(attr,null,node.getAttribute(attr));if(node.isConnected)node.connectedCallback?.();}stack.push(...node.childNodes);if(node._shadow)stack.push(node._shadow);}}};
  class Document extends Node {
    createElement(tag) {tag=String(tag).toLowerCase();const Type=definitions.get(tag);if(Type){constructing={tag};try{return new Type();}finally{constructing=null;}}return wrap(call('dom.create',tag));}
    createElementNS(namespace, tag) {namespace=namespace==null?null:String(namespace);tag=String(tag);if(namespace==='http://www.w3.org/1999/xhtml')return this.createElement(tag);const id=call('dom.create',tag),Type=namespace==='http://www.w3.org/2000/svg'?(tag==='svg'?SVGSVGElement:SVGElement):Element;const node=new Type(id,tag);node.namespaceURI=namespace;return node;}
    createTextNode(text) { return wrap(call('dom.create', '#text', String(text))); }
    createComment(text) { return wrap(call('dom.createSpecial', '#comment', String(text))); }
    createDocumentFragment() { return wrap(call('dom.createSpecial', '#fragment', '')); }
    createRange() { return new Range(); }
    get implementation(){return {createHTMLDocument:title=>{const doc=new DOMParser().parseFromString('<html><head></head><body></body></html>','text/html');if(title)doc.title=title;return doc;}};}
    createEvent(type) { return new (type === 'MouseEvents' ? MouseEvent : type === 'KeyboardEvent' ? KeyboardEvent : Event)(''); }
    importNode(node, deep = false) { return node.cloneNode(deep); }
    adoptNode(node) { node.remove(); return node; }
    querySelector(selector) { return wrap(call('dom.queryWithin', this._id, String(selector), true)); }
    querySelectorAll(selector) { return call('dom.queryWithin',this._id,String(selector)).map(wrap); }
    getElementById(id) { id = String(id); return !id ? null : /['\\()]|^"|"$/.test(id) ? this.querySelectorAll('[id]').find(node => node.id === id) ?? null : this.querySelector("[id='" + id + "']"); }
    getElementsByTagName(tag) { return this.querySelectorAll(tag); }
    getElementsByClassName(names) { return this.querySelectorAll('.' + names.trim().split(/\s+/).join('.')); }
    addEventListener(name, fn, options) { if (name === 'DOMContentLoaded') queueMicrotask(() => fn(new Event(name))); else super.addEventListener(name, fn, options); }
    get body() { return this.querySelector('body'); } get head() { return this.querySelector('head'); } get documentElement() { return this.querySelector('html'); }
    get activeElement() { return active ?? this.body; } get defaultView() { return globalThis; }
    get readyState() { return 'complete'; } get compatMode() { return 'CSS1Compat'; } get visibilityState() { return 'visible'; } get hidden() { return false; }
    get title() { return (this.querySelector('title')?.textContent ?? '').replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, ''); }
    set title(value) {
      let title = this.querySelector('title');
      if (!title) {
        let head = this.head;
        if (!head) { head = this.createElement('head'); (this.documentElement ?? this).prepend(head); }
        title = this.createElement('title'); head.append(title);
      }
      title.textContent = String(value);
    }
    getSelection() { return selection; } hasFocus() { return !!active; }
    createTreeWalker(root, whatToShow = 0xffffffff, filter = null) { const accept = node => !(whatToShow & (1 << (node.nodeType - 1))) ? 3 : typeof filter === 'function' ? filter(node) : filter?.acceptNode(node) ?? 1; const walker = { root, currentNode: root, whatToShow, filter, nextNode() { let node = this.currentNode, result = 1, next; for (;;) { while (result !== 2 && (next = node.firstChild)) { node = next; result = accept(node); if (result === 1) return this.currentNode = node; } next = null; for (let up = node; up && up !== root && !(next = up.nextSibling); up = up.parentNode); if (!next) return null; node = next; result = accept(node); if (result === 1) return this.currentNode = node; } }, parentNode() { if (this.currentNode === root) return null; return this.currentNode = this.currentNode.parentNode; } }; return walker; }
    createNodeIterator(root, whatToShow, filter) { const walker = this.createTreeWalker(root, whatToShow, filter); let first = true; const next = walker.nextNode; walker.nextNode = function () { if (first) { first = false; if (this.whatToShow & (1 << (root.nodeType - 1))) return root; } return next.call(this); }; return walker; }
    caretRangeFromPoint(x,y) {
      const node = wrap(call('dom.hitTest',x,y,true,true)); if (!node) return null;
      const range = this.createRange(); range.setStart(node,node.nodeType === 3 ? call('dom.textIndex',node._id,x,y) : 0); range.collapse(true); return range;
    }
    caretPositionFromPoint(x,y){const range=this.caretRangeFromPoint(x,y);return range?{offsetNode:range.startContainer,offset:range.startOffset}:null;}
    elementFromPoint(x, y) { return wrap(call('dom.hitTest', x, y, true)); }
    elementsFromPoint(x, y) { return call('dom.hitTest', x, y).map(wrap); }
  }
  class DOMParser {
    parseFromString(source,type){if(type!=='text/html')throw new DOMException('Only HTML parsing is supported','NotSupportedError');const doc=wrap(call('dom.createSpecial','#document',''));source=String(source);if(!/<html[\s>]/i.test(source))source=/<body[\s>]/i.test(source)?'<html><head></head>'+source+'</html>':'<html><head></head><body>'+source+'</body></html>';call('dom.html',doc._id,source);return doc;}
  }
  const comparePoints=(a,start,b,end)=>{if(a===b)return Math.sign(start-end);if(a.contains(b)){let child=b;while(child.parentNode!==a)child=child.parentNode;return start<=a.childNodes.indexOf(child)?-1:1;}if(b.contains(a))return -comparePoints(b,end,a,start);return a.compareDocumentPosition(b)&Node.DOCUMENT_POSITION_FOLLOWING?-1:1;};
  const rangeNodes=range=>call('dom.rangeNodes',range.startContainer._id,range.startOffset,range.endContainer._id,range.endOffset).map(([id,start,end])=>({node:wrap(id),start,end}));
  const textPoint=(root,offset)=>{const [id,index]=call('dom.textPoint',root._id,offset);return [wrap(id),index];};
  const textOffset=(root,node,offset)=>call('dom.textOffset',root._id,node?._id,offset);
  class Range {
    constructor() { this.startContainer = this.endContainer = document; this.startOffset = this.endOffset = 0; }
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; }
    get commonAncestorContainer() { return path(this.startContainer).find(node => node.contains(this.endContainer)) ?? document; }
    setStart(node, offset) {
      offset = Number(offset) >>> 0;
      if (offset > (node.nodeType === 3 || node.nodeType === 8 ? node.length : node.childNodes.length)) throw new DOMException('Invalid range offset', 'IndexSizeError');
      if (node.getRootNode() !== this.endContainer.getRootNode() || comparePoints(node, offset, this.endContainer, this.endOffset) > 0) { this.endContainer = node; this.endOffset = offset; }
      this.startContainer = node; this.startOffset = offset;
      if (selection?.ranges[0] === this) selection._setRange(this, selection.direction, true);
    }
    setEnd(node, offset) {
      offset = Number(offset) >>> 0;
      if (offset > (node.nodeType === 3 || node.nodeType === 8 ? node.length : node.childNodes.length)) throw new DOMException('Invalid range offset', 'IndexSizeError');
      if (node.getRootNode() !== this.startContainer.getRootNode() || comparePoints(node, offset, this.startContainer, this.startOffset) < 0) { this.startContainer = node; this.startOffset = offset; }
      this.endContainer = node; this.endOffset = offset;
      if (selection?.ranges[0] === this) selection._setRange(this, selection.direction, true);
    }
    setStartBefore(node) { this.setStart(node.parentNode, node.parentNode.childNodes.indexOf(node)); } setStartAfter(node) { this.setStart(node.parentNode, node.parentNode.childNodes.indexOf(node) + 1); }
    setEndBefore(node) { this.setEnd(node.parentNode, node.parentNode.childNodes.indexOf(node)); } setEndAfter(node) { this.setEnd(node.parentNode, node.parentNode.childNodes.indexOf(node) + 1); }
    selectNode(node) { this.setStartBefore(node); this.setEndAfter(node); }
    selectNodeContents(node) { this.setStart(node, 0); this.setEnd(node, node.nodeType === 3 ? node.length : node.childNodes.length); }
    collapse(start = false) { if (start) this.setEnd(this.startContainer, this.startOffset); else this.setStart(this.endContainer, this.endOffset); }
    cloneRange() { const r = new Range(); Object.assign(r, this); return r; }
    toString() { if (this.startContainer === this.endContainer && this.startContainer.nodeType === 3) return this.startContainer.data.slice(this.startOffset, this.endOffset); return this.cloneContents().textContent; }
    cloneContents() {const fragment=document.createDocumentFragment(),range=this;const copy=(node,parent)=>{if(node.nodeType===3){const start=node===range.startContainer?range.startOffset:0,end=node===range.endContainer?range.endOffset:node.length;if(comparePoints(node,end,range.startContainer,range.startOffset)>0&&comparePoints(node,start,range.endContainer,range.endOffset)<0)parent.append(document.createTextNode(node.data.slice(start,end)));return;}for(const child of node.childNodes){const index=node.childNodes.indexOf(child);if(comparePoints(node,index,range.startContainer,range.startOffset)>=0&&comparePoints(node,index+1,range.endContainer,range.endOffset)<=0)parent.append(child.cloneNode(true));else if(comparePoints(node,index+1,range.startContainer,range.startOffset)>0&&comparePoints(node,index,range.endContainer,range.endOffset)<0){if(child.nodeType===3)copy(child,parent);else{const clone=child.cloneNode(false);copy(child,clone);parent.append(clone);}}}};if(!this.collapsed)copy(this.commonAncestorContainer,fragment);return fragment;}
    deleteContents() {
      if (this.collapsed) return;
      const root = this.commonAncestorContainer, whole = [], stack = [root];
      let container = this.startContainer, offset = this.startOffset;
      if (!container.contains(this.endContainer)) {
        let reference = container;
        while (reference.parentNode && !reference.parentNode.contains(this.endContainer)) reference = reference.parentNode;
        container = reference.parentNode; offset = container.childNodes.indexOf(reference) + 1;
      }
      while (stack.length) {
        const node = stack.pop();
        if (node === root || !node.parentNode) { stack.push(...node.childNodes); continue; }
        const parent = node.parentNode, index = parent.childNodes.indexOf(node);
        if (comparePoints(parent,index,this.startContainer,this.startOffset) >= 0 && comparePoints(parent,index+1,this.endContainer,this.endOffset) <= 0) whole.push(node);
        else stack.push(...node.childNodes);
      }
      for (const {node,start,end} of rangeNodes(this)) if (!whole.some(parent => parent.contains(node))) node.deleteData(start,end-start);
      for (const node of whole) { if (node.parentNode === container && container.childNodes.indexOf(node) < offset) offset--; node.remove(); }
      this.setStart(container,offset); this.collapse(true);
    }
    extractContents() { const result = this.cloneContents(); this.deleteContents(); return result; }
    insertNode(node) { const start = this.startContainer; if (start.nodeType === 3) start.parentNode.insertBefore(node, start.splitText(this.startOffset)); else start.insertBefore(node, start.childNodes[this.startOffset] ?? null); }
    surroundContents(node) { const contents = this.extractContents(); node.append(contents); this.insertNode(node); this.selectNode(node); }
    createContextualFragment(html) { const div = document.createElement('div'); div.innerHTML = html; const fragment = document.createDocumentFragment(); fragment.append(...div.childNodes); return fragment; }
    getBoundingClientRect() {const rects=this.getClientRects();if(!rects.length)return new DOMRect();const left=Math.min(...rects.map(rect=>rect.left)),top=Math.min(...rects.map(rect=>rect.top));return new DOMRect(left,top,Math.max(...rects.map(rect=>rect.right))-left,Math.max(...rects.map(rect=>rect.bottom))-top);}
    getClientRects() {return rangeNodes(this).flatMap(({node,start,end})=>call('dom.textRect',node._id,start,end).map(rect=>new DOMRect(...rect)));}
    compareBoundaryPoints(how,range){return how===0?comparePoints(this.startContainer,this.startOffset,range.startContainer,range.startOffset):how===1?comparePoints(this.endContainer,this.endOffset,range.startContainer,range.startOffset):how===2?comparePoints(this.endContainer,this.endOffset,range.endContainer,range.endOffset):comparePoints(this.startContainer,this.startOffset,range.endContainer,range.endOffset);}
    isPointInRange(node,offset){return comparePoints(node,offset,this.startContainer,this.startOffset)>=0&&comparePoints(node,offset,this.endContainer,this.endOffset)<=0;}
    comparePoint(node,offset){return comparePoints(node,offset,this.startContainer,this.startOffset)<0?-1:comparePoints(node,offset,this.endContainer,this.endOffset)>0?1:0;}
    intersectsNode(node){if(!node.parentNode)return true;const parent=node.parentNode,index=parent.childNodes.indexOf(node);return comparePoints(parent,index+1,this.startContainer,this.startOffset)>0&&comparePoints(parent,index,this.endContainer,this.endOffset)<0;}
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
      call('dom.selection', 0, rangeNodes(range).map(({node,start,end}) => [node._id,start,end]), range.collapsed && !!active?.isContentEditable && active.contains(range.startContainer));
      selectionChanged();
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
          offset = node.childNodes.indexOf(host) + (side === 'end' ? 1 : 0);
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
    containsNode(node, partial = false) { if (!this.rangeCount || node.getRootNode() !== this.anchorNode.getRootNode()) return false; const range = this.ranges[0], end = node.nodeType === 3 ? node.length : node.childNodes.length; return partial ? range.intersectsNode(node) : comparePoints(range.startContainer, range.startOffset, node, 0) <= 0 && comparePoints(range.endContainer, range.endOffset, node, end) >= 0; }
    deleteFromDocument() { for (const range of this.ranges) range.deleteContents(); }
    toString() { return call('dom.selectionText', 0, this.ranges.flatMap(range => rangeNodes(range).map(({node,start,end}) => [node._id,start,end]))); }
  }
  class MutationObserver { constructor(callback) { this.callback = callback; this.targets = new Map(); this.records = []; } observe(target, options) { if (!options) throw new TypeError('Observer options required'); this.targets.set(target, { ...options, attributes: options.attributes ?? !!(options.attributeOldValue || options.attributeFilter), characterData: options.characterData ?? !!options.characterDataOldValue }); observers.add(this); } disconnect() { this.targets.clear(); this.records = []; observers.delete(this); } takeRecords() { return this.records.splice(0); } }
  const intersectionObservers=new Set();
  class IntersectionObserver {
    constructor(callback,options={}){this.callback=callback;this.root=options.root??null;this.rootMargin=options.rootMargin??'0px';this.thresholds=[...new Set(Array.isArray(options.threshold)?options.threshold:[options.threshold??0])].sort((a,b)=>a-b);if(this.thresholds.some(v=>v<0||v>1))throw new RangeError('Invalid intersection threshold');this.targets=new Map();this.records=[];}
    observe(target){this.targets.set(target,null);intersectionObservers.add(this);}unobserve(target){this.targets.delete(target);if(!this.targets.size)intersectionObservers.delete(this);}disconnect(){this.targets.clear();this.records=[];intersectionObservers.delete(this);}takeRecords(){return this.records.splice(0);}
  }
  const resizeObservers = new Set();
  class ResizeObserver { constructor(callback) { this.callback = callback; this.targets = new Map(); } observe(target) { this.targets.set(target, null); resizeObservers.add(this); } unobserve(target) { this.targets.delete(target); if (!this.targets.size) resizeObservers.delete(this); } disconnect() { this.targets.clear(); resizeObservers.delete(this); } }
  const wrap = id => {
    if (id == null) return null;
    const chain = [];
    for (let next = id; next != null && !nodes.get(next)?.deref();) { const data = call('dom.node', next); chain.push([next, data.tag, data.parent]); next = data.parent; }
    for (const [id, tag, parent] of chain.reverse()) {
      const above = nodes.get(parent)?.deref(), svg = tag === 'svg' || above instanceof SVGElement && above.localName !== 'foreignObject';
      const Type = tag === '#document' ? Document : tag === '#fragment' ? DocumentFragment : tag.startsWith('#') ? Node : svg ? (tag === 'svg' ? SVGSVGElement : SVGElement) : tag === 'canvas' ? HTMLCanvasElement : tag === 'img' ? HTMLImageElement : tag === 'slot' ? HTMLSlotElement : ['input', 'textarea'].includes(tag) ? HTMLInputElement : HTMLElement;
      const node = new Type(id, tag); node._parent = above ?? null; above?._children.add(node);
    }
    return nodes.get(id)?.deref() ?? null;
  };
  let sweepAt = 0;
  globalThis.__appsDomSweep = (force = false) => {
    if (!force && Date.now() < sweepAt) return;
    sweepAt = Date.now() + 250;
    const dead = [];
    for (const [id, ref] of nodes) if (!ref.deref()) { nodes.delete(id); dead.push(id); }
    call('dom.release', dead, registered.splice(0));
  };

  class CanvasRenderingContext2D {
    constructor(canvas) { this.canvas = canvas; this._state = { fillStyle: '#000000', strokeStyle: '#000000', font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', globalAlpha: 1, lineWidth: 1, transform: [1,0,0,1,0,0], clip: [] }; this._stack = []; this._path = []; this._pending = []; }
    _draw(op, data) { this._pending.push({ op, ...this._state, ...data }); if (this._pending.length === 1) queueMicrotask(() => { if (this._pending.length) call('dom.canvas', this.canvas._id, this._pending.splice(0)); }); }
    _resize() { this._pending = []; call('dom.canvas', this.canvas._id, null); this._state.transform = [1,0,0,1,0,0]; this._state.clip = []; }
    save() { this._stack.push({ ...this._state, transform: [...this._state.transform], clip: this._state.clip ? [...this._state.clip] : null }); } restore() { if (this._stack.length) this._state = this._stack.pop(); }
    setTransform(a, b, c, d, e, f) { this._state.transform = typeof a === 'object' ? [a.a,a.b,a.c,a.d,a.e,a.f] : [a,b,c,d,e,f]; }
    resetTransform() { this.setTransform(1,0,0,1,0,0); }
    transform(a,b,c,d,e,f) { const [u,v,w,x,y,z] = this._state.transform; this.setTransform(u*a+w*b,v*a+x*b,u*c+w*d,v*c+x*d,u*e+w*f+y,v*e+x*f+z); }
    scale(x,y) { this.transform(x,0,0,y,0,0); } translate(x,y) { this.transform(1,0,0,1,x,y); } rotate(a) { this.transform(Math.cos(a),Math.sin(a),-Math.sin(a),Math.cos(a),0,0); }
    fillRect(x,y,width,height) { this._draw('fillRect', { x,y,width,height,reset: x === 0 && y === 0 && width >= this.canvas.width && height >= this.canvas.height && !this._state.clip && this.globalAlpha === 1 }); }
    clearRect(x,y,width,height) { this._draw('clearRect',{x,y,width,height}); }
    strokeRect(x,y,width,height) { this._draw('strokeRect', { x,y,width,height }); }
    fillText(text,x,y,maxWidth) { this._draw('fillText', { text: String(text),x,y,maxWidth }); }
    measureText(text) { return call('dom.measureText', this.canvas._id, String(text), this.font); }
    _flush() { if (this._pending.length) call('dom.canvas',this.canvas._id,this._pending.splice(0)); }
    createImageData(width,height) { if (width instanceof ImageData) return new ImageData(width.width,width.height); return new ImageData(width,height); }
    getImageData(x,y,width,height) { this._flush(); if (!width || !height) throw new DOMException('Image dimensions must be nonzero','IndexSizeError'); return new ImageData(new Uint8ClampedArray(call('dom.canvasPixels',this.canvas._id,x,y,Math.abs(width),Math.abs(height))),Math.abs(width),Math.abs(height)); }
    putImageData(data,x,y) { if (!(data instanceof ImageData)) throw new TypeError('Expected ImageData'); this._draw('putImageData',{data:data.data,width:data.width,height:data.height,x,y}); }
    drawImage(image,...args) { if (!(image instanceof HTMLImageElement) && !(image instanceof HTMLCanvasElement)) throw new TypeError('Expected an image or canvas'); image._context?._flush?.(); const iw=image.naturalWidth ?? image.width,ih=image.naturalHeight ?? image.height; if (!iw||!ih) throw new DOMException('Image is not loaded','InvalidStateError'); let sx=0,sy=0,sw=iw,sh=ih,dx,dy,dw=iw,dh=ih; if(args.length===2) [dx,dy]=args; else if(args.length===4) [dx,dy,dw,dh]=args; else if(args.length===8) [sx,sy,sw,sh,dx,dy,dw,dh]=args; else throw new TypeError('Invalid drawImage arguments'); this._draw('image',{url:call('dom.canvasURL',image._id),iw,ih,sx,sy,sw,sh,dx,dy,dw,dh}); }

    beginPath() { this._path = []; } closePath() { this._path.push(['close']); } moveTo(x,y) { this._path.push(['move',x,y]); } lineTo(x,y) { this._path.push(['line',x,y]); }
    rect(x,y,width,height) { this._path.push(['move',x,y],['line',x+width,y],['line',x+width,y+height],['line',x,y+height],['close']); }
    quadraticCurveTo(x1,y1,x,y) { this._path.push(['quadratic',x1,y1,x,y]); } bezierCurveTo(x1,y1,x2,y2,x,y) { this._path.push(['bezier',x1,y1,x2,y2,x,y]); }
    fill() { this._draw('fill', { path: this._path.map(p => [...p]) }); } stroke() { this._draw('stroke', { path: this._path.map(p => [...p]) }); }
    clip(rule='nonzero') { if (!['nonzero','evenodd'].includes(rule)) throw new TypeError('Invalid fill rule'); this._state.clip=[...this._state.clip,{path:this._path.map(p=>[...p]),transform:[...this._state.transform],rule}]; }
  }
  for (const name of ['fillStyle','strokeStyle','font','textAlign','textBaseline','globalAlpha','lineWidth']) Object.defineProperty(CanvasRenderingContext2D.prototype, name, { get() { return this._state[name]; }, set(value) { this._state[name] = value; } });
  class ImageData {
    constructor(data,width,height) { if (typeof data==='number') {height=width;width=data;data=new Uint8ClampedArray(width*height*4);} else {if (!(data instanceof Uint8ClampedArray)) throw new TypeError('Expected Uint8ClampedArray');height??=data.length/(width*4);} if(!Number.isInteger(width)||!Number.isInteger(height)||width<=0||height<=0||data.length!==width*height*4) throw new DOMException('Invalid image dimensions','IndexSizeError');Object.assign(this,{data,width,height,colorSpace:'srgb'}); }
  }
  class HTMLCanvasElement extends HTMLElement {
    get clientWidth(){const width=this.getBoundingClientRect().width;if(width)return width;const value=getComputedStyle(this).width;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.width;}
    get clientHeight(){const height=this.getBoundingClientRect().height;if(height)return height;const value=getComputedStyle(this).height;return /^\d+(?:\.\d+)?px$/.test(value)?parseFloat(value):this.height;}
    get width() { const v = this.getAttribute('width'); return v !== null && /^\d+$/.test(v) ? Number(v) : 300; } set width(v) { this.setAttribute('width', Number(v) >>> 0); }
    get height() { const v = this.getAttribute('height'); return v !== null && /^\d+$/.test(v) ? Number(v) : 150; } set height(v) { this.setAttribute('height', Number(v) >>> 0); }
    setAttribute(name, value) { super.setAttribute(name, value); if (name === 'width' || name === 'height') this._context?._resize(); }
    toDataURL(type='image/png') { if (type!=='image/png') throw new DOMException('Only PNG encoding is supported','NotSupportedError');this._context?._flush?.();return call('dom.canvasURL',this._id); }
    toBlob(callback,type='image/png') { const data=this.toDataURL(type).split(',')[1];queueMicrotask(()=>callback(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}))); }
    getContext(type) { if (['webgl','webgl2','experimental-webgl','experimental-webgl2'].includes(type))throw new DOMException('WebGL is not supported; use WebGPU or Canvas 2D','NotSupportedError'); if (this._contextType && this._contextType !== type) return null; if (!['webgpu','2d'].includes(type)) return null; this._contextType = type; return this._context ??= type === '2d' ? new CanvasRenderingContext2D(this) : new GPUCanvasContext(this, this._id); }
  }
  const document = globalThis.document = wrap(0);
  globalThis.window = globalThis; globalThis.self = globalThis;
  selection = new Selection();
  Object.assign(globalThis, { Node, Element, HTMLElement, SVGElement, SVGSVGElement, HTMLSlotElement, HTMLInputElement, HTMLTextAreaElement: HTMLInputElement, HTMLImageElement, HTMLCanvasElement, HTMLIFrameElement: class extends Element {}, Document, DocumentFragment, Text: Node, Comment: Node, Range, Selection, DOMRect, DOMRectReadOnly: DOMRect, MutationObserver, ResizeObserver, IntersectionObserver, UIEvent, MouseEvent, PointerEvent, KeyboardEvent, FocusEvent, InputEvent, CompositionEvent, CanvasRenderingContext2D, ImageData, DataTransfer, ClipboardEvent, DragEvent });
  Object.assign(globalThis,{customElements,ShadowRoot,CSSStyleSheet,DOMParser});
  globalThis.Image = function Image(width,height) { const image=document.createElement('img'); if(width!==undefined) image.width=width; if(height!==undefined) image.height=height; return image; };
  class FontFace {
    constructor(family,source,descriptors={}) { Object.assign(this,{family:String(family),style:'normal',weight:'normal',stretch:'normal',status:'unloaded'},descriptors); this._source=source;this.loaded=new Promise((resolve,reject)=>{this._resolve=resolve;this._reject=reject;});this.loaded.catch(()=>{}); }
    load() { if (this.status!=='unloaded') return this.loaded; this.status='loading'; (async()=>{try {let bytes=this._source;if(typeof bytes==='string'){const local=/^local\(["']?([^"')]+)["']?\)$/.exec(bytes);if(local){if(!call('dom.fontCheck',0,local[1])) throw new DOMException('Local font unavailable','NetworkError');} else {const url=/url\(["']?([^"')]+)["']?\)/.exec(bytes);if(!url) throw new DOMException('Unsupported font source','SyntaxError');const response=await fetch(url[1]);if(!response.ok)throw new DOMException('Font request failed','NetworkError');bytes=await response.arrayBuffer();call('dom.font',0,this.family,new Uint8Array(bytes));}}else call('dom.font',0,this.family,ArrayBuffer.isView(bytes)?new Uint8Array(bytes.buffer,bytes.byteOffset,bytes.byteLength):new Uint8Array(bytes));this.status='loaded';this._resolve(this);}catch(error){this.status='error';this._reject(error);}})();return this.loaded; }
  }
  class FontFaceSet extends EventTarget {
    constructor() {super();this._faces=new Set();this.status='loaded';this.ready=Promise.resolve(this);}
    add(face) {if(!(face instanceof FontFace))throw new TypeError('Expected FontFace');this._faces.add(face);return this;} delete(face){return this._faces.delete(face);} clear(){this._faces.clear();} has(face){return this._faces.has(face);} get size(){return this._faces.size;} values(){return this._faces.values();} keys(){return this.values();} entries(){return this._faces.entries();} forEach(fn,thisArg){this._faces.forEach(face=>fn.call(thisArg,face,face,this));} [Symbol.iterator](){return this.values();}
    load(font,text=' ') {const families=String(font).replace(/^.*?\d+(?:\.\d+)?px\s+/,'').split(',').map(v=>v.trim().replace(/^['"]|['"]$/g,'')),faces=[...this._faces].filter(face=>families.includes(face.family));this.status='loading';this.dispatchEvent(new Event('loading'));const result=Promise.all(faces.map(face=>face.load()));this.ready=result.then(()=>{this.status='loaded';this.dispatchEvent(new Event('loadingdone'));return this;},error=>{this.status='loaded';this.dispatchEvent(new Event('loadingerror'));throw error;});this.ready.catch(()=>{});return result;}
    check(font,text=' ') {const families=String(font).replace(/^.*?\d+(?:\.\d+)?px\s+/,'').split(',').map(v=>v.trim().replace(/^['"]|['"]$/g,''));return families.some(family=>{const faces=[...this._faces].filter(face=>face.family===family);return faces.length?faces.every(face=>face.status==='loaded'):call('dom.fontCheck',0,family);});}
  }
  Object.assign(globalThis,{FontFace,FontFaceSet});document.fonts=new FontFaceSet();
  globalThis.NodeFilter = { SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_TEXT: 4, SHOW_COMMENT: 128, FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3 };
  Object.defineProperties(globalThis, { innerWidth: { get: () => call('dom.viewport')[0] }, innerHeight: { get: () => call('dom.viewport')[1] }, devicePixelRatio: { get: () => call('dom.viewport')[2] } });
  globalThis.getSelection = () => selection;
  globalThis.getComputedStyle = node => { const values = call('dom.style', node._id); return styleObject(() => Object.entries(values).map(([k,v]) => k+':'+v).join(';'), () => { throw new DOMException('Computed styles are read-only', 'NoModificationAllowedError'); }); };
  const animationFrames = new Map(); let animationFrameId = 0, animationTimer;
  globalThis.__appsAnimationFrame = () => {
    animationTimer = undefined;
    const now = performance.now(), callbacks = [...animationFrames];
    for (const [id, callback] of callbacks) {
      if (!animationFrames.delete(id)) continue;
      try { callback(now); } catch (error) { console.error(error); }
    }
  };
  globalThis.requestAnimationFrame = fn => {
    if (typeof fn !== 'function') throw new TypeError('Expected an animation callback');
    const id = ++animationFrameId; animationFrames.set(id, fn);
    if (!call('dom.requestFrame') && animationTimer === undefined) animationTimer = setTimeout(__appsAnimationFrame, 16);
    return id;
  };
  globalThis.cancelAnimationFrame = id => animationFrames.delete(id);
  const mediaQueries = new Set();
  let mediaViewport = '';
  class MediaQueryListEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.media = String(options.media ?? ''); this.matches = !!options.matches; }
  }
  class MediaQueryList extends EventTarget {
    constructor(media) { super(); this._media = String(media); this._matches = this.matches; this.onchange = null; mediaQueries.add(new WeakRef(this)); }
    get media() { return this._media; }
    get matches() { return call('dom.mediaMatches', this.media); }
    addListener(callback) { this.addEventListener('change', callback); }
    removeListener(callback) { this.removeEventListener('change', callback); }
  }
  Object.assign(globalThis, { MediaQueryList, MediaQueryListEvent, matchMedia: query => new MediaQueryList(query) });
  globalThis.__appsLayout = () => { for (const observer of resizeObservers) { const entries = []; for (const [target, previous] of observer.targets) { const rect = target.getBoundingClientRect(), value = rect.width + ':' + rect.height; if (value !== previous) { observer.targets.set(target, value); entries.push({ target, contentRect: new DOMRect(0,0,rect.width,rect.height), borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }], contentBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }], devicePixelContentBoxSize: [{ inlineSize: rect.width * devicePixelRatio, blockSize: rect.height * devicePixelRatio }] }); } } if (entries.length) observer.callback(entries, observer); } }; 
  const resizeLayout=globalThis.__appsLayout;
  globalThis.__appsMediaChanged=()=>{
    if (!mediaQueries.size) return;
    const viewport = call('dom.viewport').join(':');
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
  globalThis.__appsLayout=()=>{for(const id of call('dom.scrollEvents'))wrap(id)?.dispatchEvent(new Event('scroll'));resizeLayout();__appsMediaChanged();};
  const mediaLayout=globalThis.__appsLayout;
  globalThis.__appsLayout=()=>{mediaLayout();const clips=new Map();for(const observer of intersectionObservers){const root=observer.root?.getBoundingClientRect()??new DOMRect(0,0,innerWidth,innerHeight);const margins=observer.rootMargin.trim().split(/\s+/).map(value=>value.endsWith('%')?parseFloat(value)*root.width/100:parseFloat(value));if(margins.some(v=>!Number.isFinite(v)))throw new DOMException('Invalid rootMargin','SyntaxError');const [top,right=top,bottom=top,left=right]=margins;const rootBounds=new DOMRect(root.x-left,root.y-top,root.width+left+right,root.height+top+bottom);for(const [target,previous]of observer.targets){const bounds=target.getBoundingClientRect();let x=Math.max(bounds.left,rootBounds.left),y=Math.max(bounds.top,rootBounds.top),r=Math.min(bounds.right,rootBounds.right),b=Math.min(bounds.bottom,rootBounds.bottom);for(let parent=target.parentElement;parent&&parent!==observer.root;parent=parent.parentElement){if(!clips.has(parent))clips.set(parent,Object.values(call('dom.style',parent._id,['overflow','overflow-x','overflow-y'])).some(value=>['hidden','clip','auto','scroll'].includes(value)));if(clips.get(parent)){const clip=parent.getBoundingClientRect();x=Math.max(x,clip.left);y=Math.max(y,clip.top);r=Math.min(r,clip.right);b=Math.min(b,clip.bottom);}}const isIntersecting=target.isConnected&&r>=x&&b>=y;const intersectionRect=new DOMRect(x,y,Math.max(0,r-x),Math.max(0,b-y));const area=bounds.width*bounds.height,ratio=isIntersecting?(area?intersectionRect.width*intersectionRect.height/area:1):0;const next={ratio,isIntersecting};if(previous===null||previous.isIntersecting!==isIntersecting||observer.thresholds.some(t=>(previous.ratio<t)!==(ratio<t))){observer.records.push({time:performance.now(),target,rootBounds,boundingClientRect:bounds,intersectionRect,isIntersecting,intersectionRatio:ratio});}observer.targets.set(target,next);}const entries=observer.takeRecords();if(entries.length)observer.callback(entries,observer);}};

  globalThis.__appsEditing = () => active instanceof HTMLInputElement ? { id: active._id, text: active.value, start: active.selectionDirection === 'backward' ? active.selectionEnd : active.selectionStart, end: active.selectionDirection === 'backward' ? active.selectionStart : active.selectionEnd, marked: composing } : active?.isContentEditable ? { id: active._id, text: active.textContent, start: textOffset(active,selection.anchorNode,selection.anchorOffset), end: textOffset(active,selection.focusNode,selection.focusOffset), marked: composing } : null;
  globalThis.__appsInput = (text, start, end, mark = false, selected = null, inputType, dataTransfer = null) => {
    const target = active;
    if (!target || target.disabled || target.readOnly) return;
    const state = __appsEditing(); if (!state) return;
    text = String(text);
    start ??= composing?.[0] ?? Math.min(state.start,state.end); end ??= composing?.[1] ?? Math.max(state.start,state.end);
    start = Math.max(0, Math.min(start, state.text.length)); end = Math.max(start, Math.min(end, state.text.length));
    const isComposing = mark || composing !== null, control = target instanceof HTMLInputElement;
    inputType ??= isComposing ? 'insertCompositionText' : text ? 'insertText' : 'deleteContentBackward';
    if (mark && !composing) {
      composing = [start,end];
      dispatch(target, new CompositionEvent('compositionstart', { bubbles: true, composed: true, data: '' }));
    }
    if (isComposing) dispatch(target, new CompositionEvent('compositionupdate', { bubbles: true, composed: true, data: text }));
    const targetRanges = [];
    if (!control) {
      const [startContainer,startOffset] = textPoint(target,start), [endContainer,endOffset] = textPoint(target,end);
      targetRanges.push(new StaticRange({startContainer,startOffset,endContainer,endOffset}));
    }
    const data = inputType.startsWith('delete') || ['insertParagraph','insertLineBreak','historyUndo','historyRedo'].includes(inputType) || (inputType === 'insertFromPaste' && !control) ? null : text;
    const options = { bubbles: true, composed: true, data, inputType, isComposing, dataTransfer: control ? null : dataTransfer };
    if (!dispatch(target, new InputEvent('beforeinput', { ...options, cancelable: !isComposing, targetRanges }))) return;
    if (active !== target || !target.isConnected) { if (isComposing) composing = null; return; }
    if (control) {
      target.value = state.text.slice(0,start) + text + state.text.slice(end);
      target.setSelectionRange(start + (selected?.[0] ?? text.length), start + (selected?.[1] ?? text.length));
    } else {
      const range = document.createRange(); range.setStart(...textPoint(target,start)); range.setEnd(...textPoint(target,end));
      range.deleteContents();
      if (text) range.insertNode(document.createTextNode(text));
      selection.setBaseAndExtent(...textPoint(target,start + (selected?.[0] ?? text.length)), ...textPoint(target,start + (selected?.[1] ?? text.length)));
    }
    if (isComposing) composing = [start,start+text.length];
    dispatch(target, new InputEvent('input', options));
    if (isComposing && !mark) {
      composing = null;
      dispatch(target, new CompositionEvent('compositionend', { bubbles: true, composed: true, data: text }));
    }
  };
  globalThis.__appsFrameEvent=(id,event)=>{const target=wrap(id);if(event.type==='did-finish-load')dispatch(target,new Event('load'));else if(event.type==='did-fail-load')dispatch(target,new Event('error'));else if(event.type==='message')dispatch(target,new MessageEvent('message',{data:event.data,origin:'null'}));};
  globalThis.__appsClipboard = (type, text = '') => {
    const target = active ?? document.body, data = new DataTransfer();
    if (type === 'paste') data.setData('text/plain', text);
    if (dispatch(target, new ClipboardEvent(type, { bubbles: true, cancelable: true, composed: true, clipboardData: data }))) {
      const state = __appsEditing();
      if (type === 'paste') { if (state) __appsInput(text, undefined, undefined, false, null, 'insertFromPaste', data); }
      else {
        data.setData('text/plain', state ? state.text.slice(Math.min(state.start,state.end), Math.max(state.start,state.end)) : selection.toString());
        if (type === 'cut' && state && state.start !== state.end) __appsInput('', undefined, undefined, false, null, 'deleteByCut');
      }
    }
    return data.getData('text/plain');
  };
  globalThis.__appsDrop=(files,x,y)=>{const data=new DataTransfer();for(const value of files){const file=new File([new Uint8Array(value.bytes)],value.name,{lastModified:value.lastModified,type:({'png':'image/png','jpg':'image/jpeg','jpeg':'image/jpeg','webp':'image/webp','gif':'image/gif','svg':'image/svg+xml','txt':'text/plain','md':'text/markdown','json':'application/json','pdf':'application/pdf'})[value.name.split('.').at(-1).toLowerCase()]??'application/octet-stream'});file.path=value.path;data.items.add(file);}const target=document.elementFromPoint(x,y)??document.body;dispatch(target,new DragEvent('drop',{bubbles:true,cancelable:true,composed:true,clientX:x,clientY:y,dataTransfer:data}));};
  const nativeKeys = { backspace:'Backspace', delete:'Delete', enter:'Enter', escape:'Escape', tab:'Tab', left:'ArrowLeft', right:'ArrowRight', up:'ArrowUp', down:'ArrowDown', home:'Home', end:'End', pageup:'PageUp', pagedown:'PageDown', insert:'Insert', space:' ', shift:'Shift', control:'Control', ctrl:'Control', alt:'Alt', command:'Meta', super:'Meta' };
  const keyCodes = { Backspace:8, Tab:9, Enter:13, Shift:16, Control:17, Alt:18, Escape:27, ' ':32, PageUp:33, PageDown:34, End:35, Home:36, ArrowLeft:37, ArrowUp:38, ArrowRight:39, ArrowDown:40, Insert:45, Delete:46, Meta:91 };
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
      const fire = (node, type, ancestors, relatedTarget, bubbles) => dispatch(node, new Type(kind + type, { ...data, bubbles, cancelable: bubbles, composed: bubbles, relatedTarget }), ancestors);
      if (previous) fire(previous, 'out', from.slice(), target, true);
      from.forEach((node, index) => { if (node.nodeType !== 11 && !to.includes(node)) fire(node, 'leave', from.slice(index), target, false); });
      if (target) fire(target, 'over', to.slice(), previous, true);
      for (let index = to.length; index--;) if (to[index].nodeType !== 11 && !from.includes(to[index])) fire(to[index], 'enter', to.slice(index), previous, false);
    }
  };
  globalThis.__appsDispatch = (name, ids, data = {}) => {
    if (name === 'pointerexit') { hover(null, data); return true; }
    if (name.startsWith('window:')) return dispatchEvent(new Event(name.slice(7)));
    if (name === 'modifiers') {
      const previous = modifiers; modifiers = data;
      for (const [field,key] of Object.entries({ shiftKey:'Shift', ctrlKey:'Control', altKey:'Alt', metaKey:'Meta' })) {
        if (!!previous[field] !== !!data[field]) __appsDispatch(data[field] ? 'keydown' : 'keyup', [], { ...data, key, code: key + 'Left', keyCode: keyCodes[key] });
      }
      return true;
    }
    if (name === 'keydown' || name === 'keyup') {
      const target = active ?? document.body; if (!target) return false;
      if (data.nativeKey !== undefined) {
        const raw = data.nativeKey, upper = raw.toUpperCase(), named = nativeKeys[raw] ?? (/^f\d{1,2}$/.test(raw) ? upper : null);
        const code = /^[a-z]$/i.test(raw) ? 'Key' + upper : /^\d$/.test(raw) ? 'Digit' + raw : ({ ' ':'Space', space:'Space', '-':'Minus', '=':'Equal', '[':'BracketLeft', ']':'BracketRight', '\\':'Backslash', ';':'Semicolon', "'":'Quote', ',':'Comma', '.':'Period', '/':'Slash', '`':'Backquote' })[raw] ?? named ?? '';
        const key = named ?? data.keyChar ?? (data.shiftKey ? upper : raw);
        data = { ...data, key, code, keyCode: keyCodes[key] ?? (/^[a-z0-9]$/i.test(raw) ? upper.charCodeAt(0) : /^F\d+$/.test(code) ? 111 + Number(code.slice(1)) : 0) };
      }
      const event = new KeyboardEvent(name, { bubbles: true, cancelable: true, composed: true, isComposing: composing !== null, ...data });
      if (!dispatch(target, event)) return false;
      if (name === 'keyup' || event.isComposing || event.keyCode === 229) return true;
      const state = __appsEditing(), mac = navigator.platform.includes('Mac'), primary = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (state) {
        const start = Math.min(state.start,state.end), end = Math.max(state.start,state.end);
        if (primary && !event.altKey && event.key.toLowerCase() === 'a') {
          if (target instanceof HTMLInputElement) target.select(); else selection.selectAllChildren(target);
          return false;
        }
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
        if (event.key === 'Enter' && !event.metaKey && !event.ctrlKey && !event.altKey && (target.localName === 'textarea' || target.isContentEditable)) {
          __appsInput('\n', start, end, false, null, target.localName === 'textarea' || event.shiftKey ? 'insertLineBreak' : 'insertParagraph');
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
          if (target instanceof HTMLInputElement) target.setSelectionRange(Math.min(anchor,next), Math.max(anchor,next), next < anchor ? 'backward' : 'forward');
          else selection.setBaseAndExtent(...textPoint(target,anchor), ...textPoint(target,next));
          return false;
        }
      }
      if (!state && primary && !event.altKey && event.key.toLowerCase() === 'a') {
        const root = selection.anchorNode?.getRootNode();
        selection.selectAllChildren(root?.host ? root : document.body);
        return false;
      }
      if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey) {
        const focusable = [], stack = [document];
        while (stack.length) { const node = stack.pop(); if (node.nodeType === 1 && !node.disabled && !node.hidden && (node.tabIndex >= 0 || node.isContentEditable && !node.parentElement?.isContentEditable)) focusable.push(node); stack.push(...(node._shadow ?? node).childNodes.reverse()); }
        focusable.sort((a,b) => (a.tabIndex > 0 ? a.tabIndex : Infinity) - (b.tabIndex > 0 ? b.tabIndex : Infinity));
        const index = focusable.indexOf(active), next = (index + (event.shiftKey ? -1 : 1) + focusable.length) % focusable.length;
        focusable[next]?.focus();
        return false;
      }
      return true;
    }
    const hit = captures.get(data.pointerId) ?? wrap(ids[0]) ?? (Number.isFinite(data.clientX) && Number.isFinite(data.clientY) ? document.elementFromPoint(data.clientX,data.clientY) : null);
    if (name === 'pointermove' || name === 'pointerdown' || name === 'pointerup') hover(hit?.nodeType === 3 ? hit.parentElement : hit, data);
    const target = hit ?? (selectionDrag?.id === data.pointerId ? selectionDrag.host : null);
    if (name === 'pointerdown' && target && (data.button ?? 0) === 0) call('dom.active', path(target).map(node => node._id));
    if (name === 'pointerup' || name === 'pointercancel') call('dom.active', []);
    if (!target) return false;
    if (name === 'pointerup' && selectionDrag?.id === data.pointerId && Number.isFinite(data.clientX) && Number.isFinite(data.clientY)) __appsDispatch('pointermove', ids, { ...data, buttons: 1 });
    if (name === 'pointerdown') pointers.add(data.pointerId);
    const event = new (name.startsWith('pointer') ? PointerEvent : MouseEvent)(name, { bubbles: true, cancelable: true, composed: true, ...data });
    const result = dispatch(target,event);
    if (name === 'pointerdown' && result && (data.button ?? 0) === 0) {
      selectionDrag = null;
      const focusable = path(target).find(n => n.nodeType === 1 && (n.tabIndex >= 0 || n.isContentEditable && !n.parentElement?.isContentEditable));
      focusable?.focus();
      if (focusable instanceof HTMLInputElement) {
        const offset = call('dom.textIndex',focusable._id,data.clientX,data.clientY), anchor = data.shiftKey ? (__appsEditing()?.start ?? offset) : offset;
        const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
        const [start,end] = granularity > 1 ? selectionExtent(focusable.value,offset,granularity) : [anchor,offset];
        focusable.setSelectionRange(Math.min(start,end),Math.max(start,end),end < start ? 'backward' : 'forward');
        selectionDrag = { id: data.pointerId, host: focusable, offset: start, endOffset: end, granularity, x: data.clientX, y: data.clientY };
      } else if (call('dom.userSelect',target._id)[0] !== 'none') {
        const host = focusable?.isContentEditable ? focusable : target.getRootNode();
        const point = document.caretRangeFromPoint(data.clientX,data.clientY);
        if (point && host.contains(point.startContainer)) {
          const [mode,boundary] = call('dom.userSelect',point.startContainer._id);
          if (mode !== 'none' && dispatch(target, new Event('selectstart', { bubbles: true, cancelable: true }))) {
            if (!focusable) active?.blur();
            const granularity = data.shiftKey ? 1 : Math.min(3,data.detail ?? 1);
            const range = selectionRange(point.startContainer,point.startOffset,granularity,mode,boundary);
            if (data.shiftKey && host.contains(selection.anchorNode)) selection.extend(range.endContainer,range.endOffset);
            else selection.setBaseAndExtent(range.startContainer,range.startOffset,range.endContainer,range.endOffset);
            selectionDrag = { id:data.pointerId, host, node:selection.anchorNode, offset:selection.anchorOffset, endNode:selection.focusNode, endOffset:selection.focusOffset, granularity, x:data.clientX, y:data.clientY };
          }
        }
      }
    }
    if (name === 'pointermove' && result && selectionDrag?.id === data.pointerId && (selectionDrag.x !== data.clientX || selectionDrag.y !== data.clientY)) {
      const { host, node, offset, endNode, endOffset, granularity } = selectionDrag;
      selectionDrag.x = data.clientX; selectionDrag.y = data.clientY;
      if (host instanceof HTMLInputElement) {
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
    if (name === 'pointerup' || name === 'pointercancel') { if (selectionDrag?.id === data.pointerId) selectionDrag = null; pointers.delete(data.pointerId); captures.get(data.pointerId)?.releasePointerCapture(data.pointerId); }
    if (name === 'click' && result) {
      const label = path(target).find(node => node.localName === 'label');
      const control = target.localName === 'input' ? target : label && (label.getAttribute('for') ? document.getElementById(label.getAttribute('for')) : label.querySelector('input'));
      const kind = control?.localName === 'input' ? String(control.getAttribute('type')).toLowerCase() : '';
      if ((kind === 'checkbox' || kind === 'radio') && !control.disabled && !(kind === 'radio' && control.checked)) {
        if (kind === 'radio' && control.getAttribute('name')) for (const other of control.getRootNode().querySelectorAll('input[type=radio]')) if (other.getAttribute('name') === control.getAttribute('name')) other.checked = false;
        control.checked = kind === 'radio' || !control.checked;
        dispatch(control, new Event('input', { bubbles: true, composed: true }));
        dispatch(control, new Event('change', { bubbles: true }));
      }
    }
    if (name === 'click' && data.detail === 2) dispatch(target, new MouseEvent('dblclick', { bubbles: true, cancelable: true, composed: true, ...data }));
    return result;
  };
})();
