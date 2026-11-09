'use strict';
// WARNING: module code runs outside this function; strict mode also keeps `caller` from reaching loader functions.
const devtools = this.__appsDevtools ?? globalThis.__appsCompile !== undefined;
const compile = this.__appsCompile ?? ((source) => (0, eval)(source));
const chunk = this.__appsChunk ?? globalThis.__appsChunk;
const definitions = new Map(), modules = new Map(), starredBy = new Map(), chunks = new Map(), loaded = new Set(), bind = Object.defineProperties, slots = [];
const namespaceOf = module => {
  if (module.namespace) return module.namespace;
  const namespace = Object.defineProperty(Object.create(null), Symbol.toStringTag, { value: 'Module' }), table = module.definition.slots;
  for (const name in table) {
    const slot = table[name];
    bind(namespace, { [name]: { enumerable: true, get: typeof slot !== 'number' ? () => namespaceOf(modules.get(slot[0]))[slot[1]] : slot < 0 ? () => slots[~slot]() : () => slots[slot] } });
  }
  return module.namespace = Object.preventExtensions(namespace);
};
const define = definition => {
  if (definitions.has(definition.id)) throw Error('Duplicate module: ' + definition.id);
  definitions.set(definition.id, definition);
  definition.deps ??= [];
  definition.stars ??= [];
  if (definition.source !== undefined) { definition.factory = compile(`(${definition.source})`, ''); delete definition.source; }
  if (definition.code === undefined) return;
  if (definition.map && devtools) { const sources = globalThis.__appsSources ??= new Map(); definition.map.sources.forEach((name, index) => sources.set(name, definition.map.sourcesContent[index])); }
  // WARNING: JSC ignores a sourceMappingURL that contains a quote, and encodeURIComponent keeps `'`.
  const debug = definition.map && devtools ? `\n//# sourceURL=${definition.url}\n//# sourceMappingURL=data:application/json;charset=utf-8,${encodeURIComponent(JSON.stringify(definition.map)).replaceAll("'", '%27')}\n` : '';
  // WARNING: pages compile only while their own script runs, so development modules compile here, before any of them runs.
  let source;
  if (definition.commonjs) source = `(function(exports,require,module,__filename,__dirname,$appsRequire,$appsImport){\n${definition.code}\n})${debug}`;
  else {
    const exports = definition.exports.map(item => `[${JSON.stringify(item.name)}]:{enumerable:true,get:()=>(${item.value})}`).join(',');
    const params = definition.linked === false ? '' : ',$appsImport,$appsRequire' + definition.deps.map(id => ',$appsN' + id).join('');
    const body = `(${definition.async === 2 ? 'async ' : ''}function*($out,$appsBind${params}){'use strict';$appsBind($out,{${exports}});yield;\n${definition.code}\n})`;
    source = definition.linked === false ? `(function($scope){with($scope){return${body}}})${debug}` : body + debug;
  }
  try { definition.factory = compile(source, definition.url ?? ''); } catch (error) { throw error instanceof SyntaxError ? new SyntaxError((definition.url ?? 'module ' + definition.id) + ': ' + error.message) : error; }
  delete definition.code; delete definition.map; delete definition.exports;
};
const refreshStars = id => {
  const pending = [id];
  while (pending.length) {
    const target = modules.get(pending.pop());
    for (const parent of starredBy.get(target.definition.id) ?? []) {
      let changed = false;
      for (const name of Object.keys(namespaceOf(target))) {
        if (name !== 'default' && name !== 'module.exports' && !Object.hasOwn(parent.namespace, name)) {
          Object.defineProperty(parent.namespace, name, { enumerable: true, get: () => namespaceOf(target)[name] });
          changed = true;
        }
      }
      if (changed) pending.push(parent.definition.id);
    }
  }
};
const run = iterator => new Promise((resolve, reject) => {
  const step = (method, value) => {
    let result;
    try { result = iterator[method](value); } catch (error) { return reject(error); }
    if (result.done) resolve();
    else Promise.resolve(result.value).then(value => step('next', value), error => step('throw', error));
  };
  step('next');
});
const $appsImport = id => Promise.resolve().then(() => load(id)).then(() => namespaceOf(modules.get(id)));
const $appsRequire = target => {
  if (load(target) instanceof Promise) throw Error('Cannot require an asynchronous module');
  const commonjs = modules.get(target).commonjs;
  if (commonjs) return commonjs.exports;
  const result = namespaceOf(modules.get(target));
  if (Object.hasOwn(result, 'module.exports')) return result['module.exports'];
  if (!Object.hasOwn(result, 'default')) return result;
  const names = Object.keys(result);
  if (names.length === 1) return result.default;
  // WARNING: like Node, require() of a module with a default export adds __esModule, so transpiled interop picks `default`.
  return modules.get(target).required ??= Object.preventExtensions(Object.defineProperties(Object.create(null), Object.fromEntries([['__esModule', { value: true, enumerable: true }], ...names.map(name => [name, { enumerable: true, get: () => result[name] }])])));
};
const cache = Object.create(null);
const missing = name => Object.assign(new Error(`Cannot find module '${name}'`), { code: 'MODULE_NOT_FOUND' });
const absolute = file => file === undefined || /^(?:[\\/]|[A-Za-z]:)/.test(file) ? file : globalThis.__apps ? __apps.path.join(__apps.app.getAppPath(), file) : '/' + file;
const requireFrom = definition => Object.assign(id => {
  if (typeof id !== 'string') throw Object.assign(new TypeError(`The "id" argument must be of type string. Received ${typeof id}`), { code: 'ERR_INVALID_ARG_TYPE' });
  if (!globalThis.__apps) throw missing(id);
  return __apps.module.createRequire()(id);
}, {
  resolve: id => { if (globalThis.__apps?.module.isBuiltin(id)) return id; const file = definition.resolved?.[id]; if (file === undefined) throw missing(id); return absolute(file); },
  cache, main: undefined,
});
const definitionOf = id => {
  const name = chunks.get(id);
  if (name !== undefined && !loaded.has(name)) {
    loaded.add(name);
    chunk(name)(define);
  }
  return definitions.get(id);
};
const setup = root => {
  const fresh = [], pending = [root];
  while (pending.length) {
    const id = pending.pop();
    if (modules.has(id)) continue;
    const definition = definitionOf(id);
    if (!definition) throw Error('Missing module: ' + id);
    const commonjs = definition.commonjs ? { exports: {} } : null;
    const module = { namespace: null, commonjs, definition, state: 0, dynamicStars: !!commonjs || !!definition.preload };
    const exported = () => module.commonjs.exports;
    module.namespace = commonjs ? new Proxy(Object.create(null), {
      get: (_, name) => name === 'default' || name === 'module.exports' ? exported() : exported()?.[name],
      has: (_, name) => name === 'default' || name === 'module.exports' || Object.hasOwn(Object(exported()), name),
      ownKeys: () => [...new Set(['default', 'module.exports', ...Object.keys(Object(exported()))])],
      getOwnPropertyDescriptor: (_, name) => name === 'default' || name === 'module.exports' || Object.hasOwn(Object(exported()), name) ? { enumerable: true, configurable: true } : undefined,
    }) : definition.flat ? null : Object.defineProperty(Object.create(null), Symbol.toStringTag, { value: 'Module' });
    modules.set(id, module);
    fresh.push(module);
    if (!commonjs) for (const dep of definition.deps) if (dep !== null) pending.push(dep);
  }
  for (const module of fresh) {
    const { definition, namespace } = module, factory = definition.factory;
    if (module.commonjs || definition.preload || definition.flat) continue;
    if (definition.linked === false) {
      const scope = Object.create(null);
      scope.$appsImport = $appsImport;
      for (const item of definition.imports) {
        const target = modules.get(item.id);
        if (item.local) Object.defineProperty(scope, item.local, { get: () => item.name === '*' ? namespaceOf(target) : namespaceOf(target)[item.name] });
      }
      module.iterator = factory(scope)(namespace, bind);
    } else module.iterator = factory(namespace, bind, $appsImport, $appsRequire, ...definition.deps.map(id => id === null ? undefined : namespaceOf(modules.get(id))));
    module.iterator.next();
    for (const id of definition.stars) {
      if (!starredBy.has(id)) starredBy.set(id, []);
      starredBy.get(id).push(module);
    }
  }
  for (let changed = true; changed;) {
    changed = false;
    for (const module of fresh) if (!module.dynamicStars && module.definition.stars.some(id => modules.get(id).dynamicStars)) changed = module.dynamicStars = true;
  }
  for (const module of fresh) for (const id of module.definition.stars) refreshStars(id);
  for (const module of fresh) {
    for (const item of module.definition.imports ?? []) {
      const target = modules.get(item.id);
      if (item.local && item.name !== '*' && !target.dynamicStars && !Object.hasOwn(namespaceOf(target), item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    if (!module.dynamicStars && !module.definition.flat) Object.preventExtensions(module.namespace);
  }
};
const load = (id, ancestors = new Set()) => {
  if (!modules.has(id)) setup(id);
  const module = modules.get(id);
  if (module.state === 3) throw module.error;
  if (module.state === 2 && module.commonjs?.filename !== undefined && cache[module.commonjs.filename] !== module.commonjs) module.state = 0;
  if (module.state) return ancestors.has(id) ? undefined : module.promise;
  module.state = 1;
  if (module.definition.preload) {
    const publish = api => { for (const name of Object.keys(api)) Object.defineProperty(module.namespace, name, { enumerable: true, value: api[name] }); Object.preventExtensions(module.namespace); module.state = 2; refreshStars(id); };
    const result = globalThis.__appsImportPreload();
    if (result instanceof Promise) module.promise = result.then(publish);
    else publish(result);
    return module.promise;
  }
  if (module.commonjs) {
    const { definition } = module, filename = absolute(definition.file);
    const dirname = filename && (globalThis.__apps?.path.dirname(filename) ?? (filename.replace(/[\\/][^\\/]*$/, '') || '/'));
    const commonjs = module.commonjs = { id: filename, path: dirname, exports: {}, filename, loaded: false, children: [], paths: [], require: requireFrom(definition) };
    if (filename !== undefined) cache[filename] = commonjs;
    try { definition.factory.call(commonjs.exports, commonjs.exports, commonjs.require, commonjs, filename, dirname, $appsRequire, $appsImport); }
    catch (error) { module.state = 0; if (cache[filename] === commonjs) delete cache[filename]; throw error; }
    commonjs.loaded = true;
    module.state = 2;
    refreshStars(id);
    return;
  }
  const visiting = new Set(ancestors).add(id);
  const pending = [];
  const execute = () => {
    for (const item of module.definition.imports ?? []) {
      if (item.local && item.name !== '*' && !Object.hasOwn(namespaceOf(modules.get(item.id)), item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    if (module.definition.flat) {
      const { factory, args } = module.definition;
      module.definition.factory = null;
      if (args) factory(slots, $appsImport, $appsRequire, ...args.map(id => id === null ? undefined : namespaceOf(modules.get(id))));
      else factory(slots, $appsImport, $appsRequire);
      module.state = 2;
      return;
    }
    const result = module.definition.async === 1 ? run(module.iterator) : module.iterator.next();
    if (module.definition.async) return result.then(() => { module.state = 2; });
    module.state = 2;
  };
  const failed = error => { module.state = 3; module.error = error; throw error; };
  let result;
  try {
    for (const dependency of module.definition.deps) {
      if (dependency === null) continue;
      const loading = load(dependency, visiting);
      if (loading instanceof Promise) pending.push(loading);
    }
    result = pending.length ? Promise.all(pending).then(execute) : execute();
  } catch (error) { failed(error); }
  if (result instanceof Promise) module.promise = result.catch(failed);
  return module.promise;
};
return (body, execute) => {
  const [entries = [0], chunked = {}] = body(define) ?? [];
  for (const name in chunked) for (const id of chunked[name]) chunks.set(id, name);
  if (!execute) {
    for (const id of chunks.keys()) definitionOf(id);
    for (const id of definitions.keys()) if (!modules.has(id)) setup(id);
    return;
  }
  const entry = id => {
    const start = () => { if (id === entries[0]) globalThis.__appsStart?.(namespaceOf(modules.get(id)).default); };
    const result = load(id);
    if (result instanceof Promise) globalThis.__appsTrackModule(result.then(start));
    else start();
  };
  // WARNING: pages and page workers run one entry per `__appsNext` call of the runtime, so microtasks run between
  // scripts, and those of a classic script still see it as `document.currentScript`, like in browsers.
  if (this !== globalThis) {
    const pending = entries.slice();
    this.__appsNext = () => { globalThis.__appsScript?.(null); if (!pending.length) return false; entry(pending.shift()); return true; };
    return;
  }
  let failed = false, failure;
  for (const id of entries) {
    try { entry(id); } catch (error) { if (!failed) [failed, failure] = [true, error]; }
  }
  if (failed) throw failure;
};
