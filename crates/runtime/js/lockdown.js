(() => {
  // WARNING: pages cannot turn strings into code; every constructor of a function kind is replaced.
  const kinds = [function () {}, async function () {}, function* () {}, async function* () {}];
  for (const kind of kinds) {
    const prototype = Object.getPrototypeOf(kind), name = prototype.constructor.name;
    const blocked = function () { throw new EvalError(`${name} is disabled in pages`); };
    Object.defineProperties(blocked, { name: { value: name }, prototype: { value: prototype } });
    Object.defineProperty(prototype, 'constructor', { value: blocked });
  }
  Object.defineProperty(globalThis, 'Function', { value: Function.prototype.constructor, writable: true, configurable: true });
  Object.defineProperty(globalThis, 'eval', { value: function eval() { throw new EvalError('eval is disabled in pages'); }, writable: true, configurable: true });
})();
