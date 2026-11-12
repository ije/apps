globalThis.__appsServe = call => {
  const servers = new Map(), requests = new WeakMap(), uploads = new Map(), signals = new Map(), drains = new Map(), { decode, memory } = globalThis.__appsBytes;
  return {
    serve(options) {
      if (!options || typeof options.fetch !== 'function') throw new TypeError('Expected a fetch handler');
      const { fetch: handler, port = 3000, hostname = '0.0.0.0' } = options;
      if (!Number.isInteger(port) || port < 0 || port > 65535) throw new RangeError('Invalid port');
      if (typeof hostname !== 'string' || !hostname) throw new TypeError('Invalid hostname');
      const address = call('web.serve.start', { port, hostname });
      let close;
      const closed = new Promise(resolve => close = resolve);
      const server = Object.freeze({
        port: address.port, hostname,
        requestIP(request) { const pending = requests.get(request); return pending?.server === address.id ? { ...pending.peer } : null; },
        upgrade(request, settings = {}) {
          const pending = requests.get(request);
          if (!pending || pending.server !== address.id || pending.upgraded) throw new Error('Request is not pending on this server');
          if (request.method !== 'GET' || request.headers.get('upgrade')?.toLowerCase() !== 'websocket' || request.headers.get('sec-websocket-version') !== '13' || !request.headers.get('connection')?.toLowerCase().split(',').some(v => v.trim() === 'upgrade')) throw new TypeError('Invalid WebSocket upgrade');
          const protocol = settings.protocol ?? '';
          if (protocol && !request.headers.get('sec-websocket-protocol')?.split(',').some(v => v.trim() === protocol)) throw new TypeError('Unrequested WebSocket protocol');
          const socket = __appsAcceptSocket(call('web.serve.upgrade', address.id, pending.id, request.headers.get('sec-websocket-key'), protocol));
          pending.upgraded = true; socket.data = settings.data;
          socket.addEventListener('open', () => options.websocket?.open?.(socket));
          socket.addEventListener('message', event => options.websocket?.message?.(socket, event.data));
          socket.addEventListener('close', event => options.websocket?.close?.(socket, event.code, event.reason));
          socket.addEventListener('error', event => options.websocket?.error?.(socket, event));
          socket.binaryType = 'arraybuffer';
          return socket;
        },
        stop(force = false) { call('web.serve.stop', address.id, !!force); if (force && servers.delete(address.id)) close(); return closed; },
      });
      servers.set(address.id, { handler, server, close });
      return server;
    },
    poll() {
      for (const resolve of drains.values()) resolve();
      drains.clear();
      if (!servers.size) return;
      for (const event of call('web.serve.poll')) {
        const state = servers.get(event.server);
        if (!state) continue;
        if (event.kind === 'closed') { servers.delete(event.server); state.close(); continue; }
        const key=event.server+':'+event.id;
        if(event.kind==='body'||event.kind==='cancel'){
          if(event.kind==='cancel'||event.error)signals.get(key)?.abort(new TypeError(event.error??'Client disconnected'));
          const upload=uploads.get(key);if(!upload)continue;
          if(event.kind==='cancel'||event.error){const error=new TypeError(event.error??'Client disconnected');upload.signal.abort(error);upload.controller?.error(error);uploads.delete(key);}
          else if(event.done){upload.controller.close();uploads.delete(key);}
          else upload.controller.enqueue(decode(event.bytes));
          upload.resolve?.();upload.resolve=null;continue;
        }
        (async () => {
          let response, upload;
          try {
            const signal=new AbortController();
            signals.set(key,signal);
            const body=event.body&&!['GET','HEAD'].includes(event.method)?new ReadableStream({
              start(controller){upload={controller,signal,resolve:null};uploads.set(key,upload);},
              pull(){call('web.serve.pull',event.server,event.id);return new Promise(resolve=>upload.resolve=resolve);},
              cancel(){uploads.delete(key);upload.resolve?.();call('web.serve.cancel',event.server,event.id);},
            }):null;
            const request = new Request(event.url, {
              method: event.method, headers: event.headers, body, signal:signal.signal,
            });
            const pending = { server: event.server, id: event.id, upgraded: false, peer: event.peer };
            requests.set(request, pending);
            response = await state.handler(request, state.server);
            requests.delete(request);
            if (pending.upgraded) return;
            if (!(response instanceof Response) || response.status === 0) throw new TypeError('Fetch handler must return a Response');
            if(response.bodyUsed||response.body?.locked)throw new TypeError('Response body is already used');
            const data = response.body && memory(response.body), stream = !data && event.method !== 'HEAD' && response.body !== null;
            if (event.method === 'HEAD') await response.body?.cancel();
            call('web.serve.respond', event.server, event.id, {
              status: response.status, statusText: response.statusText, headers: [...response.headers], body: event.method === 'HEAD' || !data ? [] : data, stream,
            });
            if (stream) {
              const reader = response.body.getReader();
              signal.signal.addEventListener('abort', () => reader.cancel(signal.signal.reason).catch(() => {}), { once: true });
              try {
                while (true) {
                  const { value, done } = await reader.read();
                  if (signal.signal.aborted) break;
                  if (!done && !(value instanceof Uint8Array)) throw new TypeError('Response chunks must be Uint8Array');
                  const chunk = done ? null : value;
                  while (!signal.signal.aborted && !call('web.serve.write', event.server, event.id, chunk)) await new Promise(resolve => drains.set(key, resolve));
                  if (done) break;
                }
              } catch (error) { await reader.cancel(error); throw error; }
              finally { reader.releaseLock(); }
            }
          } catch (error) {
            console.error(String(error)+'\n'+(error?.stack??''));
            call('web.serve.respond', event.server, event.id, { status: 500, headers: [], body: new TextEncoder().encode('Internal Server Error') });
          } finally { signals.delete(key);if(uploads.has(key)){uploads.delete(key);upload.resolve?.();upload.controller.error(new TypeError('Request ended'));}try{call('web.serve.cancel',event.server,event.id);}catch{} }
        })().catch(error => console.error(error?.stack ?? String(error)));
      }
    },
    pending: () => servers.size > 0,
  };
};
