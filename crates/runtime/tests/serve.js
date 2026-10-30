globalThis.testResult = 'pending';
const assert = (condition, message) => { if (!condition) throw Error(message); };
(async () => {
  const { serve } = __apps;
  for (const options of [undefined, {}, { fetch: 1 }, ...[-1, 65536, 1.5, NaN, '8080'].map(port => ({ fetch() {}, port }))]) {
    let error;
    try { serve(options); } catch (caught) { error = caught; }
    assert(error instanceof TypeError || error instanceof RangeError, 'Invalid options');
  }
  let headCanceled = false, holdCanceled = false, holdAborted = false;
  const server = serve({ hostname: '127.0.0.1', port: 0, fetch(request) {
    assert(request instanceof Request, 'Request instance');
    const url = new URL(request.url);
    if (url.pathname === '/echo') return request.bytes().then(body => new Response(body, {
      status: 201, statusText: 'Created Here',
      headers: [['x-method', request.method], ['x-query', url.search], ['x-input', request.headers.get('x-input')], ['set-cookie', 'a=1'], ['set-cookie', 'b=2']],
    }));
    if (url.pathname === '/slow') return scheduler.wait(50).then(() => new Response('slow'));
    if (url.pathname === '/empty') return new Response(null, { status: 204 });
    if (url.pathname === '/throw') throw Error('handler failure');
    if (url.pathname === '/reject') return Promise.reject(Error('handler rejection'));
    if (url.pathname === '/invalid') return 'invalid';
    if (url.pathname === '/used') {
      const response = new Response('used');
      return response.text().then(() => response);
    }
    if (url.pathname === '/head') return new Response(new ReadableStream({ cancel() { headCanceled = true; } }), { headers: { 'content-length': '12' } });
    if (url.pathname === '/hold') { request.signal.addEventListener('abort', () => { holdAborted = true; }); return new Response(new ReadableStream({ start(c) { c.enqueue(new Uint8Array([1])); }, cancel() { holdCanceled = true; } })); }
    if (url.pathname === '/stream') return new Response(ReadableStream.from([new Uint8Array([0, 255]), new Uint8Array([1, 128])]));
    return new Response('hello world', {});
  } });
  const second = serve({ hostname: '127.0.0.1', port: 0, fetch: () => Response.json({ answer: 42 }) });
  try {
    assert(server.port > 0 && server.hostname === '127.0.0.1', 'Server address');
    assert(__appsDelay() < 0 && __appsAlive() && !__appsPending(), 'Servers wake the loop without polling or blocking startup checks');
    let occupied = false;
    try { serve({ hostname: server.hostname, port: server.port, fetch() {} }); } catch { occupied = true; }
    assert(occupied, 'Bind failure is synchronous');
    const base = `http://127.0.0.1:${server.port}`;
    const hello = await fetch(base);
    assert(hello.status === 200 && await hello.text() === 'hello world', 'Synchronous handler');
    const body = new Uint8Array([0, 1, 127, 128, 255]);
    const echo = await fetch(base + '/echo?q=hello', { method: 'POST', body, headers: { 'x-input': 'value' } });
    assert(echo.status === 201 && echo.headers.get('x-method') === 'POST' && echo.headers.get('x-query') === '?q=hello' && echo.headers.get('x-input') === 'value', 'Request metadata and response status');
    assert(echo.headers.getSetCookie().join(';') === 'a=1;b=2', 'Repeated response headers');
    assert(String(await echo.bytes()) === String(body), 'Binary body');
    const chunked = await fetch(base + '/echo', { method: 'POST', body: ReadableStream.from([body, body]), duplex: 'half' });
    assert(String(await chunked.bytes()) === String(new Uint8Array([...body, ...body])), 'Chunked upload');
    assert(String(await (await fetch(base + '/stream')).bytes()) === '0,255,1,128', 'Stream response body');
    let slowDone = false;
    const slow = fetch(base + '/slow').then(response => response.text()).then(text => { slowDone = true; return text; });
    assert(await (await fetch(base)).text() === 'hello world' && !slowDone, 'Concurrent handlers');
    assert(await slow === 'slow', 'Async handler');
    const empty = await fetch(base + '/empty');
    assert(empty.status === 204 && await empty.text() === '', 'Empty response');
    const head = await fetch(base + '/head', { method: 'HEAD' });
    assert(head.status === 200 && head.headers.get('content-length') === '12' && await head.text() === '' && headCanceled, 'HEAD cancels body');
    const hold = (await fetch(base + '/hold')).body.getReader(); await hold.read(); await hold.cancel();
    for (let i = 0; i < 400 && !(holdCanceled && holdAborted); i++) await scheduler.wait(5);
    assert(holdCanceled && holdAborted, 'Client disconnect cancels streamed responses');
    for (const path of ['/throw', '/reject', '/invalid', '/used']) {
      const response = await fetch(base + path);
      assert(response.status === 500 && await response.text() === 'Internal Server Error', path + ' returns 500');
    }
    server.stop(); server.stop();
    assert((await (await fetch(`http://127.0.0.1:${second.port}`)).json()).answer === 42, 'Independent servers');
  } finally {
    server.stop(); second.stop();
  }
  assert(__appsDelay() === -1, 'Stop removes server polling');
})().then(() => testResult = 'ok', error => testResult = String(error) + '\n' + (error.stack || ''));
