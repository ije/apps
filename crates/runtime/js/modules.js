'use strict';
// WARNING: module code runs outside this function; strict mode also keeps `caller` from reaching loader functions.
const devtools = this.__appsDevtools ?? globalThis.__appsCompile !== undefined;
const compile = this.__appsCompile ?? ((source) => (0, eval)(source));
const chunk = this.__appsChunk ?? globalThis.__appsChunk;
const definitions = new Map(), modules = new Map(), starredBy = new Map(), chunks = new Map(), loaded = new Set(), bind = Object.defineProperties;
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
  if (definition.commonjs) source = `(function(module,exports,$appsRequire,$appsImport){\n${definition.code}\n})${debug}`;
  else {
    const exports = definition.exports.map(item => `[${JSON.stringify(item.name)}]:{enumerable:true,get:()=>(${item.value})}`).join(',');
    const params = definition.linked === false ? '' : ',$appsImport' + definition.deps.map(id => ',$appsN' + id).join('');
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
      for (const name of Object.keys(target.namespace)) {
        if (name !== 'default' && !Object.hasOwn(parent.namespace, name)) {
          Object.defineProperty(parent.namespace, name, { enumerable: true, get: () => target.namespace[name] });
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
const $appsImport = id => Promise.resolve().then(() => load(id)).then(() => modules.get(id).namespace);
const $appsRequire = target => {
  const result = load(target);
  if (result instanceof Promise) throw Error('Cannot require an asynchronous module');
  const commonjs = modules.get(target).commonjs;
  if (commonjs) return commonjs.exports;
  const names = Object.keys(result);
  return names.length === 1 && names[0] === 'default' ? result.default : result;
};
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
    const namespace = commonjs ? new Proxy(Object.create(null), {
      get: (_, name) => name === 'default' ? commonjs.exports : commonjs.exports?.[name],
      has: (_, name) => name === 'default' || Object.hasOwn(Object(commonjs.exports), name),
      ownKeys: () => [...new Set(['default', ...Object.keys(Object(commonjs.exports))])],
      getOwnPropertyDescriptor: (_, name) => name === 'default' || Object.hasOwn(Object(commonjs.exports), name) ? { enumerable: true, configurable: true } : undefined,
    }) : Object.create(null);
    const module = { namespace, commonjs, definition, state: 0, dynamicStars: !!commonjs };
    modules.set(id, module);
    fresh.push(module);
    if (!commonjs) for (const dep of definition.deps) if (dep !== null) pending.push(dep);
  }
  for (const module of fresh) {
    const { definition, namespace } = module, factory = definition.factory;
    if (module.commonjs) continue;
    if (definition.linked === false) {
      const scope = Object.create(null);
      scope.$appsImport = $appsImport;
      for (const item of definition.imports) {
        const target = modules.get(item.id);
        if (item.local) Object.defineProperty(scope, item.local, { get: () => item.name === '*' ? target.namespace : target.namespace[item.name] });
      }
      module.iterator = factory(scope)(namespace, bind);
    } else module.iterator = factory(namespace, bind, $appsImport, ...definition.deps.map(id => modules.get(id)?.namespace));
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
      if (item.local && item.name !== '*' && !target.dynamicStars && !Object.hasOwn(target.namespace, item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    if (!module.dynamicStars) Object.preventExtensions(module.namespace);
  }
};
const load = (id, ancestors = new Set()) => {
  if (!modules.has(id)) setup(id);
  const module = modules.get(id);
  if (module.state === 3) throw module.error;
  if (module.state) return ancestors.has(id) ? module.namespace : module.promise ?? module.namespace;
  module.state = 1;
  if (module.commonjs) {
    const factory = module.definition.factory;
    factory.call(module.commonjs.exports, module.commonjs, module.commonjs.exports, $appsRequire, $appsImport);
    module.state = 2;
    refreshStars(id);
    return module.namespace;
  }
  const visiting = new Set(ancestors).add(id);
  const pending = [];
  for (const dependency of module.definition.deps) {
    if (dependency === null) continue;
    const result = load(dependency, visiting);
    if (result instanceof Promise) pending.push(result);
  }
  const execute = () => {
    for (const item of module.definition.imports ?? []) {
      if (item.local && item.name !== '*' && !Object.hasOwn(modules.get(item.id).namespace, item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    const result = module.definition.async === 1 ? run(module.iterator) : module.iterator.next();
    if (module.definition.async) return result.then(() => { module.state = 2; });
    module.state = 2;
  };
  const result = pending.length ? Promise.all(pending).then(execute) : execute();
  if (result instanceof Promise) module.promise = result.catch(error => { module.state = 3; module.error = error; throw error; });
  return module.promise ?? module.namespace;
};
return (body, execute) => {
  const [entries = [0], chunked = {}] = body(define) ?? [];
  for (const name in chunked) for (const id of chunked[name]) chunks.set(id, name);
  if (!execute) {
    for (const id of chunks.keys()) definitionOf(id);
    for (const id of definitions.keys()) if (!modules.has(id)) setup(id);
    return;
  }
  let failed = false, failure;
  for (const id of entries) {
    try {
      const start = () => { if (id === entries[0]) globalThis.__appsStart?.(modules.get(id).namespace.default); };
      const result = load(id);
      if (result instanceof Promise) globalThis.__appsTrackModule(result.then(start));
      else start();
    } catch (error) {
      if (!failed) [failed, failure] = [true, error];
    }
  }
  if (failed) throw failure;
};
