globalThis.testResult = 'pending';
globalThis.testStep = 'start';
(async () => {
  const assert = (value, message) => { if (!value) throw Error(message); };
  const rejects = async (fn, name) => { try { await fn(); } catch (error) { assert(!name || error.name === name, `Expected ${name}, got ${error}`); return; } throw Error('Expected rejection'); };
  testStep = 'JSON';
  const json = await fetch(base + '/json'); assert(json.status === 200 && json.ok && (await json.json()).answer === 42, 'JSON response');
  await rejects(() => json.headers.set('x', 'y'), 'TypeError');
  testStep = 'POST';
  const body = new Uint8Array([0, 1, 255, 0]);
  const echo = await fetch(base + '/echo', { method: 'POST', body });
  assert((await echo.bytes()).join() === body.join(), 'binary POST');
  testStep = 'WebAssembly fetch';
  const wasm = new Uint8Array([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 7, 1, 96, 2, 127, 127, 1, 127,
    3, 2, 1, 0,
    7, 7, 1, 3, 97, 100, 100, 0, 0,
    10, 9, 1, 7, 0, 32, 0, 32, 1, 106, 11,
  ]);
  const compiled = await WebAssembly.compileStreaming(fetch(base + '/wasm', { method: 'POST', body: wasm }));
  assert((await WebAssembly.instantiate(compiled)).exports.add(20, 22) === 42, 'compile fetched Wasm');
  const instantiated = await WebAssembly.instantiateStreaming(fetch(base + '/wasm', { method: 'POST', body: wasm }));
  assert(instantiated.instance.exports.add(20, 22) === 42, 'instantiate fetched Wasm');
  await rejects(() => WebAssembly.compileStreaming(fetch(base + '/json')), 'TypeError');
  testStep = 'POST stream';
  const streamed = await fetch(base + '/echo', { method: 'POST', body: ReadableStream.from([new TextEncoder().encode('one'), new TextEncoder().encode('two')]), duplex: 'half' });
  assert(await streamed.text() === 'onetwo', 'streamed request');
  testStep = 'upload streaming';
  let uploadCanceled = false, uploadChunks = 0;
  const early = await fetch(base + '/early', { method: 'POST', body: new ReadableStream({
    pull(c) { if (!uploadChunks++) c.enqueue(new TextEncoder().encode('first')); },
    cancel() { uploadCanceled = true; },
  }, { highWaterMark: 0 }), duplex: 'half' });
  assert(await early.text() === 'ok' && uploadCanceled, 'upload must reach server before stream closes');
  let abortUploadCanceled = false;
  const uploadAbort = new AbortController();
  const uploading = fetch(base + '/echo', { method: 'POST', body: new ReadableStream({ cancel() { abortUploadCanceled = true; } }), signal: uploadAbort.signal, duplex: 'half' });
  setTimeout(() => uploadAbort.abort(), 5); await rejects(() => uploading, 'AbortError');
  assert(abortUploadCanceled, 'abort must cancel upload source');
  await rejects(() => fetch(base + '/echo', { method: 'POST', body: ReadableStream.from(['bad']), duplex: 'half' }), 'TypeError');
  testStep = 'concurrency';
  assert((await Promise.all(Array.from({ length: 80 }, () => fetch(base + '/json').then(response => response.json())))).every(value => value.answer === 42), 'fetches beyond the native limit are queued');
  testStep = 'status';
  const missing = await fetch(base + '/missing'); assert(missing.status === 404 && !missing.ok, 'HTTP errors resolve'); await missing.body.cancel();
  const empty = await fetch(base + '/empty'); assert(empty.body === null && await empty.text() === '', 'null body status');
  const head = await fetch(base + '/json', { method: 'HEAD' }); assert(head.body === null, 'HEAD null body');
  testStep = 'redirect';
  const redirected = await fetch(base + '/redirect'); assert(redirected.redirected && redirected.url === base + '/json' && (await redirected.json()).answer === 42, 'follow redirect');
  const manual = await fetch(base + '/redirect', { redirect: 'manual' }); assert(manual.type === 'opaqueredirect' && manual.status === 0 && manual.body === null && [...manual.headers].length === 0, 'manual redirect');
  await rejects(() => fetch(base + '/redirect', { redirect: 'error' }), 'TypeError');
  const changedMethod = await fetch(base + '/post-redirect', { method: 'POST', body: 'data' }); assert(await changedMethod.text() === 'GET', '303 method rewrite');
  testStep = 'stream';
  const start = Date.now(), response = await fetch(base + '/stream'), reader = response.body.getReader();
  assert(new TextDecoder().decode((await reader.read()).value) === 'one' && Date.now() - start < 130, 'headers and first chunk precede EOF');
  let timerFired = false; setTimeout(() => { timerFired = true; }, 1);
  assert(new TextDecoder().decode((await reader.read()).value) === 'two' && timerFired, 'network must not block timers');
  assert((await reader.read()).done && response.bodyUsed, 'stream EOF');
  testStep = 'abort';
  await rejects(() => fetch(base, { signal: AbortSignal.abort() }), 'AbortError');
  const controller = new AbortController(), waiting = fetch(base + '/slow', { signal: controller.signal });
  setTimeout(() => controller.abort(), 10); await rejects(() => waiting, 'AbortError');
  const bodyController = new AbortController(), abortedResponse = await fetch(base + '/abort', { signal: bodyController.signal });
  const abortReader = abortedResponse.body.getReader(); await abortReader.read(); const next = abortReader.read(); bodyController.abort(); await rejects(() => next, 'AbortError');
  const timeout = AbortSignal.timeout(5); await rejects(() => fetch(base + '/slow', { signal: timeout }), 'TimeoutError');
  testStep = 'errors';
  await rejects(() => fetch('file:///etc/passwd'), 'TypeError');
  await rejects(() => fetch('http://name:password@localhost/'), 'TypeError');
  await rejects(() => fetch('http://127.0.0.1:1/'), 'TypeError');
  testStep = 'beacon';
  await rejects(() => navigator.sendBeacon('file:///a'), 'TypeError');
  await rejects(() => navigator.sendBeacon(base + '/beacon', new ReadableStream()), 'TypeError');
  assert(!navigator.sendBeacon(base + '/beacon', new Uint8Array(65537)), 'beacon size limit');
  assert(navigator.sendBeacon(base + '/beacon', 'hello 水'), 'text beacon accepted');
  assert(navigator.sendBeacon(base + '/beacon', new Blob([new Uint8Array([0, 255])], { type: 'application/octet-stream' })), 'blob beacon accepted');
  assert(navigator.sendBeacon(base + '/beacon', new URLSearchParams({ q: 'a b' })), 'form beacon accepted');
  let beacons;
  for (let i = 0; i < 100; i++) { beacons = await (await fetch(base + '/beacons')).json(); if (beacons.length === 3) break; await scheduler.wait(2); }
  assert(beacons.length === 3 && beacons.every(item => item.header.startsWith('POST /beacon ')), 'beacon POST delivery');
  assert(beacons.some(item => new TextDecoder().decode(new Uint8Array(item.body)) === 'hello 水'), 'beacon UTF-8 body');
  assert(beacons.some(item => item.body.join() === '0,255' && item.header.toLowerCase().includes('content-type: application/octet-stream')), 'beacon binary data and MIME type');
  assert(beacons.some(item => new TextDecoder().decode(new Uint8Array(item.body)) === 'q=a+b'), 'beacon form data');
  testStep = 'idle';
  const idle = await fetch(base + '/json'); assert(__appsDelay() === -1, 'unread responses should not keep polling'); await idle.body.cancel();
  testResult = 'ok';
})().catch(error => { testResult = testStep + ': ' + error + '\n' + error.stack; });
