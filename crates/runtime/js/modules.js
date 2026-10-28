const definitions = new Map();
const define = value => { if (definitions.has(value.id)) throw Error('Duplicate module: ' + value.id); definitions.set(value.id, value); if (value.map) { const sources = globalThis.__appsSources ??= new Map(); value.map.sources.forEach((name, index) => sources.set(name, value.map.sourcesContent[index])); } };
const modules = new Map(), starredBy = new Map();
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
const compile = globalThis.__appsCompile ?? ((source) => (0, eval)(source));
const prepare = () => {
  for (const [id, definition] of definitions) {
    const commonjs = definition.commonjs ? { exports: {} } : null;
    const namespace = commonjs ? new Proxy(Object.create(null), {
      get: (_, name) => name === 'default' ? commonjs.exports : commonjs.exports?.[name],
      has: (_, name) => name === 'default' || Object.hasOwn(Object(commonjs.exports), name),
      ownKeys: () => [...new Set(['default', ...Object.keys(Object(commonjs.exports))])],
      getOwnPropertyDescriptor: (_, name) => name === 'default' || Object.hasOwn(Object(commonjs.exports), name) ? { enumerable: true, configurable: true } : undefined,
    }) : Object.create(null);
    modules.set(id, { namespace, commonjs, scope: Object.create(null), definition, state: 0 });
  }
  for (const module of modules.values()) {
    const { definition, scope, namespace } = module;
    const debug = definition.map ? `\n//# sourceURL=${definition.url}\n//# sourceMappingURL=data:application/json;charset=utf-8,${encodeURIComponent(JSON.stringify(definition.map))}\n` : '';
    scope.$appsImport = id => Promise.resolve().then(() => load(id)).then(() => modules.get(id).namespace);
    if (module.commonjs) {
      module.execute = definition.factory ? new Function(`return(${definition.factory})`)() : compile(`(function(module,exports,$appsRequire,$appsImport){\n${definition.code}\n})${debug}`, definition.url ?? '');
      continue;
    }
    for (const item of definition.imports) {
      const target = modules.get(item.id);
      if (!target) throw Error('Missing module: ' + item.id);
      if (item.local) Object.defineProperty(scope, item.local, { get: () => item.name === '*' ? target.namespace : target.namespace[item.name] });
    }
    let create;
    if (definition.factory) {
      create = new Function('$scope', `with($scope){return(${definition.factory})}`);
      module.async = definition.async;
    } else {
      const exports = definition.exports.map(item => `[${JSON.stringify(item.name)}]:{enumerable:true,get:()=>(${item.value})}`).join(',');
      const body = `(function($scope){with($scope){return(function*($out,$appsBind){'use strict';$appsBind($out,{${exports}});yield;\n${definition.code}\n})}})${debug}`;
      try { create = compile(body, definition.url ?? ''); }
      catch (error) {
        if (!(error instanceof SyntaxError)) throw error;
        try { create = compile(body.replace('function*(', 'async function*('), definition.url ?? ''); } catch (error) { throw new SyntaxError((definition.url ?? 'module '+definition.id)+': '+error.message); }
        module.async = true;
      }
    }
    module.iterator = create(scope)(namespace, Object.defineProperties);
    module.iterator.next();
  }
  for (const module of modules.values()) {
    module.dynamicStars = !!module.commonjs;
    for (const id of module.definition.stars) {
      if (!starredBy.has(id)) starredBy.set(id, []);
      starredBy.get(id).push(module);
    }
  }
  for (let changed = true; changed;) {
    changed = false;
    for (const module of modules.values()) if (!module.dynamicStars && module.definition.stars.some(id => modules.get(id).dynamicStars)) { module.dynamicStars = true; changed = true; }
  }
  for (const id of modules.keys()) refreshStars(id);
  for (const module of modules.values()) {
    for (const item of module.definition.imports) {
      const target = modules.get(item.id);
      if (item.local && item.name !== '*' && !target.dynamicStars && !Object.hasOwn(target.namespace, item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    if (!module.dynamicStars) Object.preventExtensions(module.namespace);
  }
};
const load = (id, ancestors = new Set()) => {
  const module = modules.get(id);
  if (!module) throw Error('Missing module: ' + id);
  if (module.state === 3) throw module.error;
  if (module.state) return ancestors.has(id) ? module.namespace : module.promise ?? module.namespace;
  module.state = 1;
  if (module.commonjs) {
    module.execute.call(module.commonjs.exports, module.commonjs, module.commonjs.exports, target => {
      const result = load(target);
      if (result instanceof Promise) throw Error('Cannot require an asynchronous module');
      const commonjs = modules.get(target).commonjs;
      if (commonjs) return commonjs.exports;
      const names = Object.keys(result);
      return names.length === 1 && names[0] === 'default' ? result.default : result;
    }, module.scope.$appsImport);
    module.state = 2;
    refreshStars(id);
    return module.namespace;
  }
  const visiting = new Set(ancestors).add(id);
  const pending = [];
  for (const dependency of new Set([...module.definition.imports.map(item => item.id), ...module.definition.stars])) {
    const result = load(dependency, visiting);
    if (result instanceof Promise) pending.push(result);
  }
  const execute = () => {
    for (const item of module.definition.imports) {
      if (item.local && item.name !== '*' && !Object.hasOwn(modules.get(item.id).namespace, item.name)) throw Error(`Missing export ${item.name} from ${item.id}`);
    }
    const result = module.iterator.next();
    if (module.async) return result.then(() => { module.state = 2; });
    module.state = 2;
  };
  const result = pending.length ? Promise.all(pending).then(execute) : execute();
  if (result instanceof Promise) module.promise = result.catch(error => { module.state = 3; module.error = error; throw error; });
  return module.promise ?? module.namespace;
};
