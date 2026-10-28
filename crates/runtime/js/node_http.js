globalThis.__appsHTTP = (serve, stream, EventEmitter) => {
  const STATUS_CODES={200:'OK',201:'Created',204:'No Content',206:'Partial Content',301:'Moved Permanently',302:'Found',304:'Not Modified',400:'Bad Request',401:'Unauthorized',403:'Forbidden',404:'Not Found',405:'Method Not Allowed',409:'Conflict',413:'Payload Too Large',426:'Upgrade Required',500:'Internal Server Error',503:'Service Unavailable'};
  class Server extends EventEmitter {
    constructor(options,listener){super();if(typeof options==='function')listener=options;if(listener)this.on('request',listener);this.listening=false;this._active=new Set();}
    listen(port=0,hostname='0.0.0.0',callback){if(typeof port==='object'){callback=typeof hostname==='function'?hostname:callback;hostname=port.host??'0.0.0.0';port=port.port??0;}if(typeof hostname==='function'){callback=hostname;hostname='0.0.0.0';}if(callback)this.once('listening',callback);
      this._server=serve({port,hostname,fetch:(request,server)=>new Promise(resolve=>{
        const incoming=stream.Readable.fromWeb(request.body??new ReadableStream({start(c){c.close();}}));const u=new URL(request.url);
        Object.assign(incoming,{method:request.method,url:u.pathname+u.search,headers:Object.fromEntries(request.headers),rawHeaders:[...request.headers].flat(),httpVersion:'1.1',httpVersionMajor:1,httpVersionMinor:1,complete:false,aborted:false});
        const socket=new EventEmitter();const peer=server.requestIP(request);socket.remoteAddress=peer?.address;socket.remotePort=peer?.port;socket.remoteFamily=peer?.family;socket.encrypted=false;socket.destroyed=false;socket.destroy=error=>{if(socket.destroyed)return;socket.destroyed=true;incoming.destroy(error);socket.emit('close');};socket.setTimeout=(ms,fn)=>{clearTimeout(socket._timer);if(ms)socket._timer=setTimeout(()=>{fn?.();socket.emit('timeout');},ms);return socket;};incoming.socket=incoming.connection=socket;
        let controller,drain,started=false;const headers=new Headers();const body=new ReadableStream({start(c){controller=c;},pull(){const next=drain;drain=null;next?.();},cancel(){incoming.aborted=true;incoming.emit('aborted');response.emit('close');}});
        const start=()=>{if(started)return;started=true;response.headersSent=true;resolve(new Response([204,304].includes(response.statusCode)?null:body,{status:response.statusCode,statusText:response.statusMessage??STATUS_CODES[response.statusCode]??'',headers}));};
        const response=new stream.Writable({write(chunk,_encoding,done){try{start();controller.enqueue(new Uint8Array(chunk));if(controller.desiredSize>0)done();else drain=done;}catch(error){done(error);}},final(done){try{start();controller.close();done();response.emit('close');}catch(error){done(error);}},destroy(error,done){try{controller.error(error);}catch{}done(error);}});
        Object.assign(response,{statusCode:200,headersSent:false,req:incoming,socket,connection:socket,setHeader(name,value){if(this.headersSent)throw new Error('Headers already sent');headers.delete(name);for(const part of Array.isArray(value)?value:[value])headers.append(name,String(part));return this;},getHeader:name=>headers.get(name)??undefined,getHeaders:()=>Object.fromEntries(headers),getHeaderNames:()=>[...headers.keys()],hasHeader:name=>headers.has(name),removeHeader:name=>headers.delete(name),writeHead(code,status,values){this.statusCode=code;if(typeof status==='string')this.statusMessage=status;else values=status;if(values)for(const[k,v]of Object.entries(values))this.setHeader(k,v);return this;},flushHeaders:start});
        this._active.add(socket);response.once('close',()=>{this._active.delete(socket);clearTimeout(socket._timer);});incoming.once('end',()=>{incoming.complete=true;});
        socket._upgrade=options=>{const ws=server.upgrade(request,options);resolve(undefined);return ws;};
        if(request.headers.get('upgrade')?.toLowerCase()==='websocket'&&this.listenerCount('upgrade'))this.emit('upgrade',incoming,socket,Buffer.alloc(0));else this.emit('request',incoming,response);
      })});this.listening=true;queueMicrotask(()=>this.emit('listening'));return this;
    }
    address(){return this.listening?{address:this._server.hostname,port:this._server.port,family:'IPv4'}:null;}
    close(callback){this._server?.stop();this.listening=false;queueMicrotask(()=>{this.emit('close');callback?.();});return this;}
    closeAllConnections(){for(const socket of this._active)socket.destroy();this._active.clear();}
    setTimeout(ms,callback){this.timeout=ms;if(callback)this.on('timeout',callback);return this;}
  }
  const request=(input,options,callback)=>{
    if(typeof options==='function'){callback=options;options={};}options??={};
    let url;if(typeof input==='string'||input instanceof URL)url=new URL(input);else{options={...input,...options};url=new URL(`${options.protocol??'http:'}//${options.hostname??options.host??'localhost'}${options.port?':'+options.port:''}${options.path??'/'}`);}
    const controller=new AbortController(),headers=new Headers(options.headers),chunks=[];
    const client=new stream.Writable({write(chunk,_encoding,done){chunks.push(Buffer.from(chunk));done();},final(done){fetch(url,{method:options.method??'GET',headers,body:chunks.length?Buffer.concat(chunks):undefined,signal:controller.signal}).then(response=>{const incoming=stream.Readable.fromWeb(response.body??new ReadableStream({start(c){c.close();}}));Object.assign(incoming,{statusCode:response.status,statusMessage:response.statusText,headers:Object.fromEntries(response.headers),rawHeaders:[...response.headers].flat(),httpVersion:'1.1'});callback?.(incoming);client.emit('response',incoming);done();},done);}});
    Object.assign(client,{setHeader(name,value){headers.set(name,String(value));return this;},getHeader:name=>headers.get(name),removeHeader:name=>headers.delete(name),abort(){controller.abort();client.emit('abort');},setTimeout(ms,listener){if(listener)client.once('timeout',listener);setTimeout(()=>client.emit('timeout'),ms);return client;}});
    return client;
  };
  class WebSocket extends EventEmitter {
    constructor(url,protocols,options){super();this._socket=url instanceof globalThis.WebSocket?url:new globalThis.WebSocket(url,protocols);this._socket.binaryType='arraybuffer';for(const type of ['open','error','close','message'])this._socket.addEventListener(type,event=>{if(type==='message')this.emit(type,typeof event.data==='string'?Buffer.from(event.data):Buffer.from(event.data),typeof event.data!=='string');else if(type==='close')this.emit(type,event.code,Buffer.from(event.reason));else this.emit(type,event);});}
    get readyState(){return this._socket.readyState;}get bufferedAmount(){return this._socket.bufferedAmount;}get protocol(){return this._socket.protocol;}
    send(data,options,callback){if(typeof options==='function')callback=options;try{this._socket.send(data);callback?.();}catch(error){if(callback)callback(error);else throw error;}}
    close(code,reason){this._socket.close(code,reason);}terminate(){this._socket.close();}
    addEventListener(...args){return this._socket.addEventListener(...args);}removeEventListener(...args){return this._socket.removeEventListener(...args);}
  }
  Object.assign(WebSocket,{CONNECTING:0,OPEN:1,CLOSING:2,CLOSED:3});Object.assign(WebSocket.prototype,{CONNECTING:0,OPEN:1,CLOSING:2,CLOSED:3});
  class WebSocketServer extends EventEmitter {
    constructor(options={},callback){super();this.clients=new Set();if(options.server)options.server.on('upgrade',(req,socket,head)=>this.handleUpgrade(req,socket,head,ws=>this.emit('connection',ws,req)));if(callback)this.on('connection',callback);}
    handleUpgrade(request,socket,_head,callback){if(!socket._upgrade)throw new Error('WebSocket upgrade requires an apps HTTP server');const protocols=new Set((request.headers['sec-websocket-protocol']??'').split(',').map(v=>v.trim()).filter(Boolean));const ws=new WebSocket(socket._upgrade({protocol:protocols.values().next().value??''}));this.clients.add(ws);ws.once('close',()=>this.clients.delete(ws));callback(ws,request);}
    close(callback){for(const ws of this.clients)ws.close();queueMicrotask(()=>{this.emit('close');callback?.();});}
  }
  const secureRequest = (input, options, callback) => {
    if (typeof input === 'object' && !(input instanceof URL)) input = { ...input, protocol: 'https:' };
    if (typeof input === 'string' || input instanceof URL) { if (new URL(input).protocol !== 'https:') throw new TypeError('HTTPS requires an https: URL'); }
    return request(input, options, callback);
  };
  return {https:{request:secureRequest,get:(...args)=>{const req=secureRequest(...args);req.end();return req;}},http:{Server,createServer:(...args)=>new Server(...args),request,get:(...args)=>{const req=request(...args);req.end();return req;},STATUS_CODES,METHODS:['GET','HEAD','POST','PUT','PATCH','DELETE','OPTIONS','CONNECT','TRACE']},ws:Object.assign(WebSocket,{WebSocket,WebSocketServer,Server:WebSocketServer})};
};
