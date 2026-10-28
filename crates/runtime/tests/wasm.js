globalThis.testResult = 'pending';
globalThis.testStep = 'wasm';
(async () => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  const rejects = async (promise, type) => {
    assert(promise instanceof Promise, 'Wasm APIs return promises');
    try { await promise; } catch (error) { assert(error instanceof type, `Expected ${type.name}, got ${error}`); return; }
    throw Error(`Expected ${type.name}`);
  };
  const bytes = new Uint8Array([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 7, 1, 96, 2, 127, 127, 1, 127,
    3, 2, 1, 0,
    7, 7, 1, 3, 97, 100, 100, 0, 0,
    10, 9, 1, 7, 0, 32, 0, 32, 1, 106, 11,
  ]);
  assert(WebAssembly.validate(bytes) && !WebAssembly.validate(new Uint8Array([0])), 'native validation');
  const module = new WebAssembly.Module(bytes);
  assert(new WebAssembly.Instance(module).exports.add(20, 22) === 42, 'native execution');
  const exports = WebAssembly.Module.exports(module);
  assert(exports.length === 1 && exports[0].name === 'add' && exports[0].kind === 'function', 'native exports');
  assert(WebAssembly.Module.imports(module).length === 0, 'native imports');
  for (const name of ['compile', 'instantiate', 'compileStreaming', 'instantiateStreaming']) {
    const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, name);
    assert(descriptor.enumerable && descriptor.writable && descriptor.configurable, `${name} descriptor`);
  }

  testStep = 'wasm promises';
  const padded = new Uint8Array(bytes.length + 8); padded.set(bytes, 4);
  for (const source of [bytes.buffer, padded.subarray(4, -4), new DataView(padded.buffer, 4, bytes.length)]) {
    const compiled = await WebAssembly.compile(source);
    assert(compiled instanceof WebAssembly.Module, 'compile result');
    const instance = await WebAssembly.instantiate(compiled);
    assert(instance instanceof WebAssembly.Instance && instance.exports.add(19, 23) === 42, 'module overload');
    const result = await WebAssembly.instantiate(source);
    assert(result.module instanceof WebAssembly.Module && result.instance.exports.add(18, 24) === 42, 'bytes overload');
  }
  const copy = bytes.slice(), pending = WebAssembly.compile(copy);
  copy.fill(0);
  assert((await WebAssembly.instantiate(await pending)).exports.add(21, 21) === 42, 'compile snapshots bytes');
  copy.set(bytes);
  const pendingInstance = WebAssembly.instantiate(copy); copy.fill(0);
  assert((await pendingInstance).instance.exports.add(1, 41) === 42, 'instantiate snapshots bytes');
  await rejects(WebAssembly.compile(new Uint8Array([0])), WebAssembly.CompileError);
  await rejects(WebAssembly.instantiate(new Uint8Array([0])), WebAssembly.CompileError);
  await rejects(WebAssembly.compile(), TypeError);
  await rejects(WebAssembly.instantiate(), TypeError);
  await rejects(WebAssembly.compile([0, 97, 115, 109, 1, 0, 0, 0]), TypeError);
  await rejects(WebAssembly.instantiate(module, null), TypeError);

  testStep = 'wasm imports';
  const imported = new Uint8Array([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 6, 1, 96, 1, 127, 1, 127,
    2, 14, 1, 3, 101, 110, 118, 6, 100, 111, 117, 98, 108, 101, 0, 0,
    3, 2, 1, 0,
    7, 7, 1, 3, 114, 117, 110, 0, 1,
    10, 8, 1, 6, 0, 32, 0, 16, 0, 11,
  ]);
  let accessed = false;
  const importing = WebAssembly.instantiate(imported, { get env() { accessed = true; return { double: value => value * 2 }; } });
  assert(!accessed, 'instantiation defers import access');
  assert((await importing).instance.exports.run(21) === 42 && accessed, 'JS callback import');
  await rejects(WebAssembly.instantiate(imported, { env: { double: 1 } }), WebAssembly.LinkError);
  const failure = Error('import failed');
  try {
    await WebAssembly.instantiate(imported, { get env() { throw failure; } });
    throw Error('Expected import failure');
  } catch (error) { assert(error === failure, 'import exception identity'); }
  const trap = new Uint8Array([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 4, 1, 96, 0, 0, 3, 2, 1, 0, 8, 1, 0, 10, 5, 1, 3, 0, 0, 11,
  ]);
  await rejects(WebAssembly.instantiate(trap), WebAssembly.RuntimeError);
  const memory = new WebAssembly.Memory({ initial: 1, maximum: 2 });
  new Uint8Array(memory.buffer)[0] = 42;
  assert(memory.grow(1) === 1 && memory.buffer.byteLength === 131072 && new Uint8Array(memory.buffer)[0] === 42, 'native memory');
  const table = new WebAssembly.Table({ element: 'anyfunc', initial: 1 });
  table.set(0, new WebAssembly.Instance(module).exports.add);
  assert(table.get(0)(20, 22) === 42, 'native table');
  const global = new WebAssembly.Global({ value: 'i64', mutable: true }, 41n);
  global.value += 1n;
  assert(global.value === 42n, 'native i64 global');

  testStep = 'wasm responses';
  const headers = { 'Content-Type': 'application/wasm' };
  const response = new Response(bytes, { headers });
  assert(await WebAssembly.compileStreaming(response) instanceof WebAssembly.Module && response.bodyUsed, 'compile response');
  await rejects(WebAssembly.compileStreaming(response), TypeError);
  const streamed = await WebAssembly.instantiateStreaming(Promise.resolve(new Response(imported, { headers })), { env: { double: value => value * 2 } });
  assert(streamed.module instanceof WebAssembly.Module && streamed.instance.exports.run(21) === 42, 'instantiate response with imports');
  assert(await WebAssembly.compileStreaming(new Response(bytes, { headers: { 'content-type': ' APPLICATION/WASM\t' } })) instanceof WebAssembly.Module, 'MIME case and whitespace');
  const chunks = new ReadableStream({
    start(controller) {
      controller.enqueue(bytes.slice(0, 15));
      setTimeout(() => { controller.enqueue(bytes.slice(15)); controller.close(); }, 1);
    },
  });
  assert((await WebAssembly.instantiateStreaming(new Response(chunks, { headers }))).instance.exports.add(20, 22) === 42, 'delayed response chunks');
  for (const name of ['compileStreaming', 'instantiateStreaming']) {
    await rejects(WebAssembly[name](), TypeError);
    await rejects(WebAssembly[name]({ headers: new Headers(headers), arrayBuffer: () => bytes.buffer }), TypeError);
    for (const type of ['', 'application/octet-stream', 'application/wasm;', 'application/wasm; charset=utf-8']) {
      await rejects(WebAssembly[name](new Response(bytes, { headers: { 'content-type': type } })), TypeError);
    }
    await rejects(WebAssembly[name](new Response(bytes, { status: 404, headers })), TypeError);
    await rejects(WebAssembly[name](Response.error()), TypeError);
    await rejects(WebAssembly[name](new Response(new Uint8Array([0]), { headers })), WebAssembly.CompileError);
    const locked = new Response(bytes, { headers }), reader = locked.body.getReader();
    await rejects(WebAssembly[name](locked), TypeError); reader.releaseLock();
    const disturbed = new Response(bytes, { headers }); await disturbed.arrayBuffer();
    await rejects(WebAssembly[name](disturbed), TypeError);
    for (const source of [Promise.reject(failure), new Response(new ReadableStream({ start(controller) { controller.error(failure); } }), { headers })]) {
      try { await WebAssembly[name](source); throw Error('Expected response failure'); }
      catch (error) { assert(error === failure, 'response exception identity'); }
    }
  }
  await rejects(WebAssembly.instantiateStreaming(new Response(imported, { headers }), { env: { double: 1 } }), WebAssembly.LinkError);
  await rejects(WebAssembly.instantiateStreaming(new Response(trap, { headers })), WebAssembly.RuntimeError);
  testResult = 'ok';
})().catch(error => { testResult = `${testStep}: ${error}\n${error.stack || ''}`; });
