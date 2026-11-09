globalThis.__appsInspect = (() => {
  const custom = Symbol.for('nodejs.util.inspect.custom');
  const tag = value => Object.prototype.toString.call(value).slice(8, -1);
  const boxed = new Set(['Number', 'String', 'Boolean', 'BigInt', 'Symbol']);
  const meta = { '\b': '\\b', '\t': '\\t', '\n': '\\n', '\f': '\\f', '\r': '\\r', "'": "\\'", '\\': '\\\\' };
  const escapes = /[\x00-\x1f\x27\x5c\x7f-\x9f]|[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
  const quote = text => {
    const mark = !text.includes("'") ? "'" : !text.includes('"') ? '"' : !text.includes('`') && !text.includes('${') ? '`' : "'";
    return mark + text.replace(escapes, c => c === "'" && mark !== "'" ? c : meta[c] ?? (c.charCodeAt(0) < 0xd800 ? '\\x' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0') : '\\u' + c.charCodeAt(0).toString(16))) + mark;
  };
  const colors = { __proto__: null, reset: [0, 0], bold: [1, 22], dim: [2, 22], italic: [3, 23], underline: [4, 24], blink: [5, 25], inverse: [7, 27], hidden: [8, 28], strikethrough: [9, 29], doubleunderline: [21, 24], black: [30, 39], red: [31, 39], green: [32, 39], yellow: [33, 39], blue: [34, 39], magenta: [35, 39], cyan: [36, 39], white: [37, 39], bgBlack: [40, 49], bgRed: [41, 49], bgGreen: [42, 49], bgYellow: [43, 49], bgBlue: [44, 49], bgMagenta: [45, 49], bgCyan: [46, 49], bgWhite: [47, 49], framed: [51, 54], overlined: [53, 55], gray: [90, 39], redBright: [91, 39], greenBright: [92, 39], yellowBright: [93, 39], blueBright: [94, 39], magentaBright: [95, 39], cyanBright: [96, 39], whiteBright: [97, 39], bgGray: [100, 49], bgRedBright: [101, 49], bgGreenBright: [102, 49], bgYellowBright: [103, 49], bgBlueBright: [104, 49], bgMagentaBright: [105, 49], bgCyanBright: [106, 49], bgWhiteBright: [107, 49] };
  for (const [alias, target] of [['grey', 'gray'], ['blackBright', 'gray'], ['bgGrey', 'bgGray'], ['bgBlackBright', 'bgGray'], ['faint', 'dim'], ['crossedout', 'strikethrough'], ['strikeThrough', 'strikethrough'], ['crossedOut', 'strikethrough']]) Object.defineProperty(colors, alias, { get: () => colors[target], set(value) { colors[target] = value; } });
  const styles = { __proto__: null, special: 'cyan', number: 'yellow', bigint: 'yellow', boolean: 'yellow', undefined: 'grey', null: 'bold', string: 'green', symbol: 'green', date: 'magenta', regexp: 'red', module: 'underline' };
  const plain = text => text, paint = (text, style) => { const color = colors[styles[style]]; return color ? `\u001b[${color[0]}m${text}\u001b[${color[1]}m` : text; };
  const width = (ctx, text) => ctx.colors ? text.replace(/\u001b\[\d\d?m/g, '').length : text.length;
  const separate = text => text.replace(/\B(?=(\d{3})+$)/g, '_');
  const number = (ctx, value) => {
    if (Object.is(value, -0)) return ctx.stylize('-0', 'number');
    const text = String(value);
    if (!ctx.numericSeparator || !Number.isFinite(value) || text.includes('e')) return ctx.stylize(text, 'number');
    const [whole, fraction] = text.split('.');
    return ctx.stylize(separate(whole) + (fraction ? '.' + fraction.replace(/(\d{3})(?=\d)/g, '$1_') : ''), 'number');
  };
  const primitive = (ctx, value) => {
    if (typeof value === 'string') {
      let trailer = '';
      if (value.length > ctx.maxStringLength) { const remaining = value.length - ctx.maxStringLength; value = value.slice(0, ctx.maxStringLength); trailer = `... ${remaining} more character${remaining > 1 ? 's' : ''}`; }
      if (ctx.compact !== true && value.length > 16 && value.length > ctx.breakLength - ctx.indentationLvl - 4) return value.split(/(?<=\n)/).map(line => ctx.stylize(quote(line), 'string')).join(` +\n${' '.repeat(ctx.indentationLvl + 2)}`) + trailer;
      return ctx.stylize(quote(value), 'string') + trailer;
    }
    if (typeof value === 'number') return number(ctx, value);
    if (typeof value === 'bigint') return ctx.stylize((ctx.numericSeparator ? separate(String(value)) : String(value)) + 'n', 'bigint');
    if (typeof value === 'boolean') return ctx.stylize(String(value), 'boolean');
    if (value === undefined) return ctx.stylize('undefined', 'undefined');
    return ctx.stylize(String(value), 'symbol');
  };
  const constructorOf = value => {
    for (let object = value; object; object = Object.getPrototypeOf(object)) {
      const descriptor = Object.getOwnPropertyDescriptor(object, 'constructor');
      try { if (typeof descriptor?.value === 'function' && descriptor.value.name !== '' && value instanceof descriptor.value) return String(descriptor.value.name); } catch {}
    }
    return Object.getPrototypeOf(value) === null ? null : 'Object';
  };
  const prefix = (constructor, name, fallback, size = '') => {
    if (constructor === null) return name !== '' && fallback !== name ? `[${fallback}${size}: null prototype] [${name}] ` : `[${fallback}${size}: null prototype] `;
    const position = constructor.indexOf(name), end = position + name.length;
    return `${constructor}${size} ` + (name !== '' && (position === -1 || (end !== constructor.length && constructor[end] === constructor[end].toLowerCase())) ? `[${name}] ` : '');
  };
  const keysOf = (value, hidden) => [...(hidden ? Object.getOwnPropertyNames(value) : Object.keys(value)), ...Object.getOwnPropertySymbols(value).filter(symbol => hidden || Object.prototype.propertyIsEnumerable.call(value, symbol))];
  // WARNING: a brand check throws for other objects, and a throw costs about 1 µs, so `like` first asks the tag or the prototype chain.
  const indexKey = /^(?:0|[1-9]\d*)$/, is = (method, value) => { try { method.call(value); return true; } catch { return false; } };
  const like = (kind, type, name, method, value) => (kind === name || value instanceof type) && is(method, value);
  const setSize = Object.getOwnPropertyDescriptor(Set.prototype, 'size').get, mapSize = Object.getOwnPropertyDescriptor(Map.prototype, 'size').get, source = Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get;
  const extra = (ctx, value, depth, key, typed) => { ctx.indentationLvl += 2; const text = format(ctx, value[key], depth, typed); ctx.indentationLvl -= 2; return `${ctx.stylize(`[${key}]`, 'string')}: ${text}`; };
  const property = (ctx, value, depth, key, array) => {
    const descriptor = Object.getOwnPropertyDescriptor(value, key) ?? { value: value[key], enumerable: true };
    let text, gap = ' ';
    if (descriptor.value !== undefined) {
      const diff = ctx.compact !== true || array ? 2 : 3;
      ctx.indentationLvl += diff;
      text = format(ctx, descriptor.value, depth);
      if (diff === 3 && ctx.breakLength < width(ctx, text)) gap = `\n${' '.repeat(ctx.indentationLvl)}`;
      ctx.indentationLvl -= diff;
    } else if (descriptor.get) {
      const label = descriptor.set ? 'Getter/Setter' : 'Getter';
      if (ctx.getters && (ctx.getters === true || (ctx.getters === 'get' && !descriptor.set) || (ctx.getters === 'set' && descriptor.set))) {
        ctx.indentationLvl += 2;
        try { const result = descriptor.get.call(value); text = result !== null && typeof result === 'object' ? `${ctx.stylize(`[${label}]`, 'special')} ${format(ctx, result, depth)}` : `${ctx.stylize(`[${label}:`, 'special')} ${result === null ? ctx.stylize('null', 'null') : primitive(ctx, result)}${ctx.stylize(']', 'special')}`; }
        catch (error) { text = `${ctx.stylize(`[${label}:`, 'special')} <Inspection threw (${format(ctx, error, depth)})>${ctx.stylize(']', 'special')}`; }
        ctx.indentationLvl -= 2;
      } else text = ctx.stylize(`[${label}]`, 'special');
    } else text = descriptor.set ? ctx.stylize('[Setter]', 'special') : ctx.stylize('undefined', 'undefined');
    if (array === 1) return text;
    let name = typeof key === 'symbol' ? ctx.stylize(String(key).replace(escapes, c => meta[c] ?? '\\x' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0')), 'symbol') : /^[a-zA-Z_][a-zA-Z_0-9]*$/.test(key) ? (key === '__proto__' ? "['__proto__']" : key) : ctx.stylize(quote(key), 'string');
    if (descriptor.enumerable === false) name = `[${name}]`;
    return `${name}:${gap}${text}`;
  };
  const group = (ctx, output, value) => {
    let total = 0, max = 0, length = output.length;
    if (ctx.maxArrayLength < output.length) length--;
    const sizes = new Array(length);
    for (let i = 0; i < length; i++) { const size = width(ctx, output[i]); sizes[i] = size; total += size + 2; if (max < size) max = size; }
    const actual = max + 2;
    if (actual * 3 + ctx.indentationLvl < ctx.breakLength && (total / actual > 5 || max <= 6)) {
      const biased = Math.max(actual - 3 - Math.sqrt(actual - total / output.length), 1);
      const columns = Math.min(Math.round(Math.sqrt(2.5 * biased * length) / biased), Math.floor((ctx.breakLength - ctx.indentationLvl) / actual), ctx.compact * 4, 15);
      if (columns <= 1) return output;
      const widths = [], grouped = [];
      for (let i = 0; i < columns; i++) { let line = 0; for (let j = i; j < output.length; j += columns) if (sizes[j] > line) line = sizes[j]; widths.push(line + 2); }
      const start = value !== undefined && [...output.keys()].every(i => typeof value[i] === 'number' || typeof value[i] === 'bigint');
      for (let i = 0; i < length; i += columns) {
        const end = Math.min(i + columns, length);
        let line = '', j = i;
        for (; j < end - 1; j++) { const pad = widths[j - i] + output[j].length - sizes[j]; line += start ? `${output[j]}, `.padStart(pad) : `${output[j]}, `.padEnd(pad); }
        line += start ? output[j].padStart(widths[j - i] + output[j].length - sizes[j] - 2) : output[j];
        grouped.push(line);
      }
      if (ctx.maxArrayLength < output.length) grouped.push(output[length]);
      output = grouped;
    }
    return output;
  };
  const below = (ctx, output, start, base) => {
    let total = output.length + start;
    if (total + output.length > ctx.breakLength) return false;
    for (const entry of output) if ((total += width(ctx, entry)) > ctx.breakLength) return false;
    return base === '' || !base.includes('\n');
  };
  const reduce = (ctx, output, base, braces, array, depth, value) => {
    if (ctx.compact !== true) {
      if (typeof ctx.compact === 'number' && ctx.compact >= 1) {
        const entries = output.length;
        if (array && entries > 6) output = group(ctx, output, value);
        if (ctx.currentDepth - depth < ctx.compact && entries === output.length && below(ctx, output, output.length + ctx.indentationLvl + braces[0].length + base.length + 10, base)) {
          const joined = output.join(', ');
          if (!joined.includes('\n')) return `${base ? `${base} ` : ''}${braces[0]} ${joined} ${braces[1]}`;
        }
      }
      const indentation = `\n${' '.repeat(ctx.indentationLvl)}`;
      return `${base ? `${base} ` : ''}${braces[0]}${indentation}  ${output.join(`,${indentation}  `)}${indentation}${braces[1]}`;
    }
    if (below(ctx, output, 0, base)) return `${braces[0]}${base ? ` ${base}` : ''} ${output.join(', ')} ${braces[1]}`;
    const indentation = ' '.repeat(ctx.indentationLvl);
    return `${braces[0]}${base === '' && braces[0].length === 1 ? ' ' : `${base ? ` ${base}` : ''}\n${indentation}  `}${output.join(`,\n${indentation}  `)} ${braces[1]}`;
  };
  const more = remaining => `... ${remaining} more item${remaining > 1 ? 's' : ''}`;
  // WARNING: JSC stacks hold only frames (`name@file:line:column`), so they are shown in Node's `    at` form after the error line.
  const stackOf = error => {
    let stack;
    try { stack = error.stack; } catch {}
    const head = Error.prototype.toString.call(error);
    if (typeof stack !== 'string' || !stack) return head;
    if (stack.startsWith(head) || /^\s*at /m.test(stack)) return stack;
    return head + '\n' + stack.split('\n').filter(Boolean).map(line => { const at = line.lastIndexOf('@'); return at <= 0 ? `    at ${line.slice(at + 1)}` : `    at ${line.slice(0, at)} (${line.slice(at + 1)})`; }).join('\n');
  };
  const formatError = (ctx, error, constructor, name, keys) => {
    let message, errorName, stack = stackOf(error);
    try { message = error.message; } catch {}
    try { errorName = error.name; } catch {}
    if (!ctx.showHidden) for (const [key, value] of [['stack'], ['message', message], ['name', errorName]]) { const at = keys.indexOf(key); if (at !== -1 && (key === 'stack' || typeof value !== 'string' || stack.includes(value))) keys.splice(at, 1); }
    errorName ??= 'Error';
    if (Object.hasOwn(error, 'cause') && !keys.includes('cause')) keys.push('cause');
    try { if (Array.isArray(error.errors) && Object.hasOwn(error, 'errors') && !keys.includes('errors')) keys.push('errors'); } catch {}
    const length = String(errorName).length;
    if (constructor === null || (String(errorName).endsWith('Error') && stack.startsWith(errorName) && (stack.length === length || stack[length] === ':' || stack[length] === '\n'))) {
      const fallback = constructor === null ? (/^([A-Z][a-z_ A-Z0-9[\]()-]+)(?::|\n {4}at)/.exec(stack) ?? /^([a-z_A-Z0-9-]*Error)$/.exec(stack))?.[1] || 'Error' : 'Error', start = prefix(constructor, name, fallback).slice(0, -1);
      if (errorName !== start) stack = start.includes(errorName) ? start + stack.slice(length) : `${start} [${errorName}]${stack.slice(length)}`;
    }
    let position = (message && stack.indexOf(message)) || -1;
    if (position !== -1) position += message.length;
    const frames = stack.indexOf('\n    at', position);
    if (frames === -1) stack = `[${stack}]`;
    return ctx.indentationLvl ? stack.replaceAll('\n', `\n${' '.repeat(ctx.indentationLvl)}`) : stack;
  };
  const functionBase = (value, constructor, name) => {
    const source = Function.prototype.toString.call(value);
    if (source.startsWith('class') && source.endsWith('}')) {
      let base = `class ${(Object.hasOwn(value, 'name') && value.name) || '(anonymous)'}`;
      if (constructor !== 'Function' && constructor !== null) base += ` [${constructor}]`;
      if (name !== '' && constructor !== name) base += ` [${name}]`;
      if (constructor === null) base += ' extends [null prototype]'; else { const parent = Object.getPrototypeOf(value).name; if (parent) base += ` extends ${parent}`; }
      return `[${base}]`;
    }
    const kind = tag(value), type = kind.endsWith('Function') ? kind : 'Function';
    let base = `[${type}${constructor === null ? ' (null prototype)' : ''}${value.name === '' ? ' (anonymous)' : `: ${String(value.name)}`}]`;
    if (constructor !== type && constructor !== null) base += ` ${constructor}`;
    if (name !== '' && constructor !== name) base += ` [${name}]`;
    return base;
  };
  const format = (ctx, value, depth, typed) => {
    if (typeof value !== 'object' && typeof value !== 'function') return primitive(ctx, value);
    if (value === null) return ctx.stylize('null', 'null');
    if (ctx.customInspect) {
      let method;
      try { method = value[custom]; } catch {}
      if (typeof method === 'function' && method !== inspect && Object.getOwnPropertyDescriptor(value, 'constructor')?.value?.prototype !== value) {
        const result = method.call(value, ctx.depth === null ? null : ctx.depth - depth, { stylize: ctx.stylize, ...Object.fromEntries(Object.keys(defaults).map(key => [key, ctx[key]])), ...ctx.userOptions }, inspect);
        if (result !== value) return typeof result !== 'string' ? format(ctx, result, depth) : result.replaceAll('\n', `\n${' '.repeat(ctx.indentationLvl)}`);
      }
    }
    if (ctx.seen.includes(value)) {
      ctx.circular ??= new Map();
      if (!ctx.circular.has(value)) ctx.circular.set(value, ctx.circular.size + 1);
      return ctx.stylize(`[Circular *${ctx.circular.get(value)}]`, 'special');
    }
    return raw(ctx, value, depth, typed);
  };
  const raw = (ctx, value, depth, typed) => {
    const constructor = constructorOf(value), kind = tag(value);
    let name = '', keys, base = '', braces = ['{', '}'], formatter = () => [], array = false, extras;
    try { name = value[Symbol.toStringTag]; } catch {}
    if (typeof name !== 'string' || (name !== '' && (ctx.showHidden ? Object.hasOwn : (object, key) => Object.prototype.propertyIsEnumerable.call(object, key))(value, Symbol.toStringTag))) name = '';
    if (Array.isArray(value)) {
      keys = keysOf(value, ctx.showHidden).filter(key => typeof key !== 'string' || !indexKey.test(key));
      braces = [`${constructor !== 'Array' || name !== '' ? prefix(constructor, name, 'Array', `(${value.length})`) : ''}[`, ']'];
      if (!value.length && !keys.length) return `${braces[0]}]`;
      array = true;
      formatter = () => {
        const output = [], length = Math.min(Math.max(0, ctx.maxArrayLength), value.length), empty = count => ctx.stylize(`<${count} empty item${count > 1 ? 's' : ''}>`, 'undefined');
        let i = 0;
        for (; i < length && Object.hasOwn(value, i); i++) output.push(property(ctx, value, depth, i, 1));
        if (i === length) { if (value.length > length) output.push(more(value.length - length)); return output; }
        const names = Object.keys(value);
        let next = i;
        for (; i < names.length && output.length < length; i++) {
          const key = names[i];
          if (+key > 2 ** 32 - 2) break;
          if (`${next}` !== key) { if (!indexKey.test(key)) break; output.push(empty(key - next)); next = +key; if (output.length === length) break; }
          output.push(property(ctx, value, depth, key, 1));
          next++;
        }
        const remaining = value.length - next;
        if (remaining > 0) output.push(output.length !== length ? empty(remaining) : more(remaining));
        return output;
      };
    } else if (like(kind, Set, 'Set', setSize, value)) {
      keys = keysOf(value, ctx.showHidden);
      const start = prefix(constructor, name, 'Set', `(${value.size})`);
      if (!value.size && !keys.length) return `${start}{}`;
      braces = [`${start}{`, '}'];
      formatter = () => { const output = []; ctx.indentationLvl += 2; for (const item of value) { if (output.length >= ctx.maxArrayLength) { output.push(more(value.size - output.length)); break; } output.push(format(ctx, item, depth)); } ctx.indentationLvl -= 2; return output; };
    } else if (like(kind, Map, 'Map', mapSize, value)) {
      keys = keysOf(value, ctx.showHidden);
      const start = prefix(constructor, name, 'Map', `(${value.size})`);
      if (!value.size && !keys.length) return `${start}{}`;
      braces = [`${start}{`, '}'];
      formatter = () => { const output = []; ctx.indentationLvl += 2; for (const [key, item] of value) { if (output.length >= ctx.maxArrayLength) { output.push(more(value.size - output.length)); break; } output.push(`${format(ctx, key, depth)} => ${format(ctx, item, depth)}`); } ctx.indentationLvl -= 2; return output; };
    } else if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
      keys = ctx.showHidden ? keysOf(value, true).filter(key => typeof key !== 'string' || !indexKey.test(key)) : [];
      braces = [`${prefix(constructor, name, kind, `(${value.length})`)}[`, ']'];
      if (!value.length && !keys.length && !ctx.showHidden) return `${braces[0]}]`;
      array = true;
      formatter = () => { const length = Math.min(Math.max(0, ctx.maxArrayLength), value.length), output = []; for (let i = 0; i < length; i++) output.push(primitive(ctx, value[i])); if (value.length > length) output.push(more(value.length - length)); return output; };
      if (ctx.showHidden) { extras = ['BYTES_PER_ELEMENT', 'length', 'byteLength', 'byteOffset', 'buffer']; typed = true; }
    } else if (kind === 'Map Iterator' || kind === 'Set Iterator') {
      keys = keysOf(value, ctx.showHidden);
      braces = [`[${name !== kind ? (name ? name + '] [' : '') + kind : kind}] {`, '}'];
      formatter = () => [ctx.stylize('<items unknown>', 'special')];
    } else {
      keys = keysOf(value, ctx.showHidden);
      if (typeof value === 'function') {
        base = functionBase(value, constructor, name);
        if (!keys.length) return ctx.stylize(base, 'special');
      } else if (constructor === 'Object') {
        if (kind === 'Arguments') braces[0] = '[Arguments] {'; else if (name !== '') braces[0] = `${prefix(constructor, name, 'Object')}{`;
        if (!keys.length) return `${braces[0]}}`;
      } else if (value !== RegExp.prototype && like(kind, RegExp, 'RegExp', source, value)) {
        base = RegExp.prototype.toString.call(value);
        const start = prefix(constructor, name, 'RegExp');
        if (start !== 'RegExp ') base = start + base;
        if (!keys.length || (ctx.depth !== null && depth > ctx.depth)) return ctx.stylize(base, 'regexp');
      } else if (like(kind, Date, 'Date', Date.prototype.getTime, value)) {
        base = Number.isNaN(value.getTime()) ? Date.prototype.toString.call(value) : value.toISOString();
        const start = prefix(constructor, name, 'Date');
        if (start !== 'Date ') base = start + base;
        if (!keys.length) return ctx.stylize(base, 'date');
      } else if (Error.isError(value) || value instanceof Error) {
        base = formatError(ctx, value, constructor, name, keys);
        if (!keys.length) return base;
      } else if (kind === 'ArrayBuffer' || kind === 'SharedArrayBuffer') {
        const start = prefix(constructor, name, kind);
        if (typed === undefined) formatter = () => { const bytes = new Uint8Array(value), more = bytes.length - ctx.maxArrayLength; return [`${ctx.stylize('[Uint8Contents]', 'special')}: <${bytes.subarray(0, ctx.maxArrayLength).toHex().replace(/(..)(?!$)/g, '$1 ')}${more > 0 ? ` ... ${more} more byte${more > 1 ? 's' : ''}` : ''}>`]; };
        else if (!keys.length) return `${start}{ [byteLength]: ${number(ctx, value.byteLength)} }`;
        braces[0] = `${start}{`;
        extras = ['byteLength'];
      } else if (kind === 'DataView') {
        braces[0] = `${prefix(constructor, name, 'DataView')}{`;
        extras = ['byteLength', 'byteOffset', 'buffer'];
      } else if (kind === 'Promise' && value instanceof Promise) { braces[0] = `${prefix(constructor, name, 'Promise')}{`; formatter = () => [ctx.stylize('<unknown>', 'special')]; }
      else if (kind === 'WeakSet' || kind === 'WeakMap') { braces[0] = `${prefix(constructor, name, kind)}{`; formatter = () => [ctx.stylize('<items unknown>', 'special')]; }
      else if (kind === 'Module') {
        const names = keys;
        keys = []; braces[0] = `${prefix(constructor, name, 'Module')}{`;
        formatter = () => names.map(key => { let text; ctx.indentationLvl += 2; try { text = format(ctx, value[key], depth); } catch { text = ctx.stylize('<uninitialized>', 'special'); } ctx.indentationLvl -= 2; return `${typeof key === 'symbol' ? String(key) : /^[a-zA-Z_][a-zA-Z_0-9]*$/.test(key) ? key : quote(key)}: ${text}`; });
      } else if (boxed.has(kind) && is(globalThis[kind].prototype.valueOf, value)) {
        const inner = globalThis[kind].prototype.valueOf.call(value);
        if (kind === 'String') keys.splice(0, value.length);
        base = `[${kind}${kind !== constructor ? (constructor === null ? ' (null prototype)' : ` (${constructor})`) : ''}: ${primitive({ ...ctx, stylize: plain }, inner)}]${name !== '' && name !== constructor ? ` [${name}]` : ''}`;
        if (!keys.length) return ctx.stylize === plain ? base : ctx.stylize(base, kind.toLowerCase());
      } else if (typeof URL === 'function' && value instanceof URL && (ctx.depth === null || depth <= ctx.depth)) {
        const fields = {};
        for (const key of ['href', 'origin', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'searchParams', 'hash']) fields[key] = value[key];
        return `${constructor ?? 'URL'} ${format(ctx, fields, depth)}`;
      } else if (typeof URLSearchParams === 'function' && value instanceof URLSearchParams) {
        const output = [...value].map(([key, item]) => `${format(ctx, key, depth + 1)} => ${format(ctx, item, depth + 1)}`), length = output.reduce((sum, entry) => sum + width(ctx, entry) + 2, -2);
        return `${constructor} {${!output.length ? '}' : length > ctx.breakLength ? `\n  ${output.join(',\n  ')} }` : ` ${output.join(', ')} }`}`;
      } else {
        const start = prefix(constructor, name, constructor === null ? (kind === name ? 'Object' : kind) : '');
        if (!keys.length) return `${start}{}`;
        braces[0] = `${start}{`;
      }
    }
    if (ctx.depth !== null && depth > ctx.depth) {
      const label = prefix(constructor, name, constructor === null ? (kind === name ? 'Object' : kind) : '').slice(0, -1);
      return ctx.stylize(constructor !== null ? `[${label}]` : label, 'special');
    }
    depth++;
    ctx.seen.push(value);
    ctx.currentDepth = depth;
    let output;
    const level = ctx.indentationLvl;
    try {
      output = formatter();
      for (const key of extras ?? []) { let text; try { text = extra(ctx, value, depth, key, typed); } catch { text = extra(ctx, { [key]: value.buffer[key] }, depth, key); } output.push(text); }
      for (const key of keys) output.push(property(ctx, value, depth, key, array ? 2 : 0));
    } catch (error) {
      if (!(error instanceof RangeError) || !/call stack/i.test(error.message)) throw error;
      ctx.seen.pop(); ctx.indentationLvl = level;
      return ctx.stylize(`[${prefix(constructor, name, '').slice(0, -1)}: Inspection interrupted prematurely. Maximum call stack size exceeded.]`, 'special');
    }
    const reference = ctx.circular?.get(value);
    if (reference !== undefined) { const label = ctx.stylize(`<ref *${reference}>`, 'special'); if (ctx.compact !== true) base = base === '' ? label : `${label} ${base}`; else braces[0] = `${label} ${braces[0]}`; }
    ctx.seen.pop();
    if (ctx.sorted) {
      const compare = ctx.sorted === true ? undefined : ctx.sorted;
      if (!array) output.sort(compare);
      else if (keys.length > 1) output.splice(output.length - keys.length, keys.length, ...output.slice(output.length - keys.length).sort(compare));
    }
    return reduce(ctx, output, base, braces, array, depth, value);
  };
  const inspect = (value, options) => {
    const ctx = { ...inspect.defaultOptions, indentationLvl: 0, seen: [], currentDepth: 0, stylize: plain };
    if (typeof options === 'boolean') ctx.showHidden = options;
    else if (options) for (const key of Object.keys(options)) { if (Object.hasOwn(inspect.defaultOptions, key) || key === 'stylize') ctx[key] = options[key]; else ctx.userOptions ??= options; }
    if (ctx.colors && !options?.stylize) ctx.stylize = paint;
    if (ctx.maxArrayLength === null) ctx.maxArrayLength = Infinity;
    if (ctx.maxStringLength === null) ctx.maxStringLength = Infinity;
    return format(ctx, value, 0);
  };
  inspect.custom = custom;
  inspect.colors = colors;
  inspect.styles = styles;
  const defaults = Object.seal({ showHidden: false, depth: 2, colors: false, customInspect: true, showProxy: false, maxArrayLength: 100, maxStringLength: 10000, breakLength: 80, compact: 3, sorted: false, getters: false, numericSeparator: false });
  Object.defineProperty(inspect, 'defaultOptions', { get: () => defaults, set: options => { if (options === null || typeof options !== 'object') throw Object.assign(new TypeError('The "options" argument must be of type object'), { code: 'ERR_INVALID_ARG_TYPE' }); Object.assign(defaults, options); } });
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
