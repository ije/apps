(() => {
  if (typeof WebAssembly === 'undefined') return;
  const { Module, Instance } = WebAssembly;
  const responseBytes = async source => {
    const response = await source;
    if (!(response instanceof Response)) throw new TypeError('Expected a Response');
    if (!/^[\t ]*application\/wasm[\t ]*$/i.test(response.headers.get('content-type'))) throw new TypeError('Expected application/wasm');
    if (!response.ok || !['default', 'basic', 'cors'].includes(response.type)) throw new TypeError('Invalid WebAssembly response');
    return response.arrayBuffer();
  };
  // WARNING: JSC's native async Wasm APIs need a platform run loop that apps does not drive.
  const methods = {
    async compile(source, options = undefined) { return new Module(source, options); },
    async instantiate(source, imports = undefined, options = undefined) {
      if (imports !== undefined && (imports === null || (typeof imports !== 'object' && typeof imports !== 'function'))) throw new TypeError('Expected an import object');
      const compiled = source instanceof Module;
      const module = compiled ? source : new Module(source, options);
      await undefined;
      const instance = new Instance(module, imports);
      return compiled ? instance : { module, instance };
    },
    async compileStreaming(source, options = undefined) {
      return new Module(await responseBytes(source), options);
    },
    async instantiateStreaming(source, imports = undefined, options = undefined) {
      const module = await methods.compileStreaming(source, options);
      return { module, instance: new Instance(module, imports) };
    },
  };
  Object.assign(WebAssembly, methods);
})();
