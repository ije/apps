globalThis.__appsInspect = (() => {
  const custom = Symbol.for('nodejs.util.inspect.custom');
  const tag = value => Object.prototype.toString.call(value).slice(8, -1);
  const boxed = new Set(['Number', 'String', 'Boolean', 'BigInt', 'Symbol']);
  const escapes = { '\\': '\\\\', '\n': '\\n', '\t': '\\t', '\r': '\\r', '\b': '\\b', '\f': '\\f', '\v': '\\v' };
  const quote = text => {
    const mark = !text.includes("'") ? "'" : !text.includes('"') ? '"' : !text.includes('`') && !text.includes('${') ? '`' : "'";
    return mark + text.replace(/[\\\n\t\r\b\f\v]/g, c => escapes[c]).replaceAll(mark, '\\' + mark).replace(/[\x00-\x1f\x7f]/g, c => '\\x' + c.charCodeAt(0).toString(16).padStart(2, '0')) + mark;
  };
  const key = name => typeof name === 'symbol' ? `[${String(name)}]` : /^[A-Za-z_$][\w$]*$/.test(name) ? name : quote(name);
  const inspect = (value, options) => {
    if (typeof options !== 'object' || options === null) options = {};
    const depth = options.depth === null ? Infinity : options.depth ?? 2, breakLength = options.breakLength ?? 80;
    const maxArray = options.maxArrayLength === null ? Infinity : options.maxArrayLength ?? 100, maxString = options.maxStringLength === null ? Infinity : options.maxStringLength ?? 10000;
    const seen = [];
    const wrap = (head, open, close, entries, indent) => {
      if (!entries.length) return (head ? head + ' ' : '') + open + close;
      head = head ? head + ' ' : '';
      let size = entries.length * 2 + indent.length + head.length + open.length + 10;
      for (const entry of entries) size += entry.length;
      if (size <= breakLength && !head.includes('\n') && !entries.some(entry => entry.includes('\n'))) return `${head}${open} ${entries.join(', ')} ${close}`;
      return `${head}${open}\n${indent}  ${entries.join(`,\n${indent}  `)}\n${indent}${close}`;
    };
    const format = (value, level, indent) => {
      switch (typeof value) {
        case 'string': return quote(value.length > maxString ? value.slice(0, maxString) : value) + (value.length > maxString ? `... ${value.length - maxString} more characters` : '');
        case 'number': return Object.is(value, -0) ? '-0' : String(value);
        case 'bigint': return value + 'n';
        case 'symbol': case 'boolean': case 'undefined': return String(value);
      }
      if (value === null) return 'null';
      if (seen.includes(value)) return '[Circular *1]';
      try {
        if (typeof value[custom] === 'function' && value[custom] !== inspect) {
          const result = value[custom](depth - level, { ...options, depth: depth - level }, inspect);
          if (result !== value) return typeof result === 'string' ? result : format(result, level, indent);
        }
      } catch {}
      const kind = tag(value), proto = Object.getPrototypeOf(value);
      let name = '';
      try { name = proto === null ? '[Object: null prototype]' : proto.constructor?.name ?? ''; } catch {}
      let head = name === 'Object' ? '' : name, open = '{', close = '}', entries = [], skip = null;
      const own = () => {
        if (ArrayBuffer.isView(value) && !options.showHidden) return [];
        const list = options.showHidden ? Reflect.ownKeys(value) : [...Object.keys(value), ...Object.getOwnPropertySymbols(value).filter(symbol => Object.prototype.propertyIsEnumerable.call(value, symbol))];
        return skip ? list.filter(name => !skip(name)) : list;
      };
      if (typeof value === 'function') {
        const base = /^class\b/.test(Function.prototype.toString.call(value)) ? `[class ${value.name || '(anonymous)'}]` : `[${kind === 'Object' ? 'Function' : kind}${value.name ? ': ' + value.name : ' (anonymous)'}]`;
        skip = name => ['length', 'name', 'prototype', 'arguments', 'caller'].includes(name);
        if (level > depth || !own().length) return base;
        head = base;
      } else if (value instanceof Error || kind === 'Error') {
        const stack = typeof value.stack === 'string' && value.stack ? '\n' + value.stack.split('\n').map(line => indent + '    ' + line).join('\n') : '';
        head = String(value) + stack;
        skip = name => name === 'stack' || name === 'message';
        if (value.cause !== undefined && !Object.keys(value).includes('cause')) entries.push('[cause]: ' + format(value.cause, level + 1, indent + '  '));
        if (!own().length && !entries.length) return head;
      } else if (kind === 'Date') return Number.isNaN(value.getTime()) ? 'Invalid Date' : value.toISOString();
      else if (kind === 'RegExp') return String(value);
      else if (boxed.has(kind) && typeof value === 'object') return `[${kind}: ${format(value.valueOf(), level, indent)}]`;
      else if (kind === 'WeakMap' || kind === 'WeakSet') return `${kind} { <items unknown> }`;
      else if (kind === 'Promise') head = 'Promise';
      if (level > depth) return `[${name || (Array.isArray(value) ? 'Array' : 'Object')}]`;
      seen.push(value);
      try {
        if (Array.isArray(value) || (ArrayBuffer.isView(value) && !(value instanceof DataView))) {
          const length = value.length;
          head = Array.isArray(value) ? (name === 'Array' ? '' : name) : `${name}(${length})`;
          open = '['; close = ']'; skip = name => typeof name === 'string' && /^(?:0|[1-9]\d*)$/.test(name) && +name < length;
          let holes = 0;
          for (let i = 0; i < Math.min(length, maxArray); i++) {
            if (!(i in value)) { holes++; continue; }
            if (holes) { entries.push(`<${holes} empty item${holes > 1 ? 's' : ''}>`); holes = 0; }
            entries.push(format(value[i], level + 1, indent + '  '));
          }
          if (holes) entries.push(`<${holes} empty item${holes > 1 ? 's' : ''}>`);
          if (length > maxArray) entries.push(`... ${length - maxArray} more item${length - maxArray > 1 ? 's' : ''}`);
        } else if (kind === 'Map') {
          head = `${name}(${value.size})`;
          for (const [k, v] of value) { if (entries.length >= maxArray) { entries.push(`... ${value.size - maxArray} more items`); break; } entries.push(`${format(k, level + 1, indent + '  ')} => ${format(v, level + 1, indent + '  ')}`); }
        } else if (kind === 'Set') {
          head = `${name}(${value.size})`;
          for (const v of value) { if (entries.length >= maxArray) { entries.push(`... ${value.size - maxArray} more items`); break; } entries.push(format(v, level + 1, indent + '  ')); }
        } else if (kind === 'ArrayBuffer' || kind === 'SharedArrayBuffer') entries.push(`byteLength: ${value.byteLength}`);
        for (const name of own()) {
          const descriptor = Object.getOwnPropertyDescriptor(value, name);
          const text = descriptor.get ? (descriptor.set ? '[Getter/Setter]' : '[Getter]') : descriptor.set ? '[Setter]' : format(descriptor.value, level + 1, indent + '  ');
          entries.push(`${descriptor.enumerable ? key(name) : `[${key(name)}]`}: ${text}`);
        }
      } finally { seen.pop(); }
      return wrap(head, open, close, entries, indent);
    };
    return format(value, 0, '');
  };
  inspect.custom = custom;
  inspect.defaultOptions = { depth: 2, breakLength: 80, maxArrayLength: 100, maxStringLength: 10000 };
  const builtin = new Set([Object.prototype.toString, Array.prototype.toString, Error.prototype.toString, Date.prototype.toString, Function.prototype.toString]);
  const formatWithOptions = (options, ...args) => {
    const show = value => typeof value === 'string' ? value : inspect(value, options);
    if (typeof args[0] !== 'string') return args.map(show).join(' ');
    let index = 1;
    let text = args[0].replace(/%([sdifjoOc%])/g, (match, type) => {
      if (type === '%') return '%';
      if (index >= args.length) return match;
      const value = args[index++];
      switch (type) {
        case 's': return typeof value === 'bigint' ? value + 'n' : value !== null && typeof value === 'object' && (typeof value.toString !== 'function' || builtin.has(value.toString)) ? inspect(value, { ...options, depth: 0 }) : typeof value === 'symbol' ? String(value) : Object.is(value, -0) ? '-0' : String(value);
        case 'd': return typeof value === 'bigint' ? value + 'n' : typeof value === 'symbol' ? 'NaN' : String(Number(value));
        case 'i': return typeof value === 'bigint' ? value + 'n' : typeof value === 'symbol' ? 'NaN' : String(parseInt(value));
        case 'f': return typeof value === 'symbol' ? 'NaN' : String(parseFloat(value));
        case 'j': try { return JSON.stringify(value); } catch { return '[Circular]'; }
        case 'o': return inspect(value, { ...options, showHidden: true, depth: 4 });
        case 'O': return inspect(value, options);
        default: return '';
      }
    });
    for (; index < args.length; index++) text += ' ' + show(args[index]);
    return text;
  };
  const deepEqual = (a, b, strict, memo) => {
    if (strict ? Object.is(a, b) : a == b || (a !== a && b !== b)) return true;
    if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
    if (strict && Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
    const kind = tag(a);
    if (kind !== tag(b) || Array.isArray(a) !== Array.isArray(b)) return false;
    if (kind === 'Date' && a.getTime() !== b.getTime()) return false;
    if (kind === 'RegExp' && (String(a) !== String(b) || a.lastIndex !== b.lastIndex)) return false;
    if (a instanceof Error && (a.message !== b.message || a.name !== b.name)) return false;
    if (boxed.has(kind) && !Object.is(a.valueOf(), b.valueOf())) return false;
    if (ArrayBuffer.isView(a) || kind === 'ArrayBuffer') {
      if (a.byteLength !== b.byteLength) return false;
      if (!strict && (kind === 'Float32Array' || kind === 'Float64Array')) { for (let i = 0; i < a.length; i++) if (a[i] != b[i]) return false; }
      else {
        const x = new Uint8Array(a.buffer ?? a, a.byteOffset ?? 0, a.byteLength), y = new Uint8Array(b.buffer ?? b, b.byteOffset ?? 0, b.byteLength);
        for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return false;
      }
      return true;
    }
    memo ??= new Map();
    if (memo.get(a) === b) return true;
    memo.set(a, b);
    const find = (list, match) => { for (const other of list) if (typeof other === 'object' && other !== null && match(other)) return true; return false; };
    if (kind === 'Map') {
      if (a.size !== b.size) return false;
      for (const [k, v] of a) {
        if (b.has(k)) { if (!deepEqual(v, b.get(k), strict, memo)) return false; }
        else if (typeof k !== 'object' || k === null || !find(b.keys(), other => deepEqual(k, other, strict, memo) && deepEqual(v, b.get(other), strict, memo))) return false;
      }
    }
    if (kind === 'Set') {
      if (a.size !== b.size) return false;
      for (const v of a) if (!b.has(v) && (typeof v !== 'object' || v === null || !find(b, other => deepEqual(v, other, strict, memo)))) return false;
    }
    const keys = value => strict ? [...Object.keys(value), ...Object.getOwnPropertySymbols(value).filter(symbol => Object.prototype.propertyIsEnumerable.call(value, symbol))] : Object.keys(value);
    const left = keys(a), right = keys(b);
    if (left.length !== right.length) return false;
    for (const name of left) if (!Object.prototype.propertyIsEnumerable.call(b, name) || !deepEqual(a[name], b[name], strict, memo)) return false;
    return true;
  };
  return { inspect, format: (...args) => formatWithOptions({}, ...args), formatWithOptions, deepEqual };
})();
