globalThis.__appsHTTP = (serve, stream, EventEmitter, net) => {
  const STATUS_CODES={100:'Continue',101:'Switching Protocols',102:'Processing',103:'Early Hints',200:'OK',201:'Created',202:'Accepted',203:'Non-Authoritative Information',204:'No Content',205:'Reset Content',206:'Partial Content',207:'Multi-Status',208:'Already Reported',226:'IM Used',300:'Multiple Choices',301:'Moved Permanently',302:'Found',303:'See Other',304:'Not Modified',305:'Use Proxy',307:'Temporary Redirect',308:'Permanent Redirect',400:'Bad Request',401:'Unauthorized',402:'Payment Required',403:'Forbidden',404:'Not Found',405:'Method Not Allowed',406:'Not Acceptable',407:'Proxy Authentication Required',408:'Request Timeout',409:'Conflict',410:'Gone',411:'Length Required',412:'Precondition Failed',413:'Payload Too Large',414:'URI Too Long',415:'Unsupported Media Type',416:'Range Not Satisfiable',417:'Expectation Failed',418:"I'm a Teapot",421:'Misdirected Request',422:'Unprocessable Entity',423:'Locked',424:'Failed Dependency',425:'Too Early',426:'Upgrade Required',428:'Precondition Required',429:'Too Many Requests',431:'Request Header Fields Too Large',451:'Unavailable For Legal Reasons',500:'Internal Server Error',501:'Not Implemented',502:'Bad Gateway',503:'Service Unavailable',504:'Gateway Timeout',505:'HTTP Version Not Supported',506:'Variant Also Negotiates',507:'Insufficient Storage',508:'Loop Detected',509:'Bandwidth Limit Exceeded',510:'Not Extended',511:'Network Authentication Required'};
  const METHODS=['ACL','BIND','CHECKOUT','CONNECT','COPY','DELETE','GET','HEAD','LINK','LOCK','M-SEARCH','MERGE','MKACTIVITY','MKCALENDAR','MKCOL','MOVE','NOTIFY','OPTIONS','PATCH','POST','PROPFIND','PROPPATCH','PURGE','PUT','QUERY','REBIND','REPORT','SEARCH','SOURCE','SUBSCRIBE','TRACE','UNBIND','UNLINK','UNLOCK','UNSUBSCRIBE'];
  const fail=(Type,code,message)=>Object.assign(new Type(message),{code});
  const validateHeaderName=(name,label='Header name')=>{if(typeof name!=='string'||!/^[\^_`a-zA-Z\-0-9!#$%&'*+.|~]+$/.test(name))throw fail(TypeError,'ERR_INVALID_HTTP_TOKEN',`${label} must be a valid HTTP token ["${name}"]`);};
  const validateHeaderValue=(name,value)=>{if(value===undefined)throw fail(TypeError,'ERR_HTTP_INVALID_HEADER_VALUE',`Invalid value "${value}" for header "${name}"`);if((Array.isArray(value)?value:[value]).some(part=>/[^\t\x20-\x7e\x80-\xff]/.test(String(part))))throw fail(TypeError,'ERR_INVALID_CHAR',`Invalid character in header content ["${name}"]`);};
  const headersOf=(source,message)=>{const headers={},raw=[];for(const[name,value]of source){raw.push(name,value);if(name==='set-cookie')(headers[name]??=[]).push(value);else headers[name]=name in headers?headers[name]+', '+value:value;}message.headers=headers;message.rawHeaders=raw;return message;};
  const kHeaders=Symbol('kOutHeaders');
  class RequestSocket extends EventEmitter {
    constructor(peer,local){super();Object.assign(this,{remoteAddress:peer?.address,remotePort:peer?.port,remoteFamily:peer?.family,localAddress:local?.address,localPort:local?.port,encrypted:false,readable:true,writable:true,destroyed:false});}
    destroy(error){if(this.destroyed)return this;this.destroyed=true;this.readable=this.writable=false;clearTimeout(this._timer);if(error)this.emit('error',error);this.emit('close',!!error);return this;}
    end(){return this.destroy();}
    setTimeout(ms,callback){clearTimeout(this._timer);if(callback)this.once('timeout',callback);if(ms>0)this._timer=setTimeout(()=>this.emit('timeout'),ms);return this;}
    setNoDelay(){return this;}setKeepAlive(){return this;}ref(){return this;}unref(){return this;}
    address(){return this.localAddress?{address:this.localAddress,family:this.localAddress.includes(':')?'IPv6':'IPv4',port:this.localPort}:{};}
  }
  class IncomingMessage extends stream.Readable {
    constructor(socket){super();Object.assign(this,{socket,httpVersionMajor:1,httpVersionMinor:1,httpVersion:'1.1',complete:false,headers:{},rawHeaders:[],trailers:{},rawTrailers:[],upgrade:false,url:'',method:null,statusCode:null,statusMessage:null,_body:null,_reader:null});}
    get connection(){return this.socket;}
    setTimeout(ms,callback){this.socket?.setTimeout(ms,callback);return this;}
    _read(){if(!this._body){this.complete=true;this.push(null);return;}this._reader??=this._body.getReader();this._reader.read().then(({value,done})=>{if(done){this.complete=true;this.push(null);}else this.push(value);},error=>this.destroy(error?.message==='Request ended'?undefined:error));}
    _destroy(error,callback){this._reader?.cancel(error).catch(()=>{});callback(error);}
  }
  class OutgoingMessage extends stream.Writable {
    constructor(options){super(options);Object.assign(this,{[kHeaders]:null,headersSent:false,sendDate:false,socket:null});}
    get connection(){return this.socket;}
    setHeader(name,value){if(this.headersSent)throw fail(Error,'ERR_HTTP_HEADERS_SENT','Cannot set headers after they are sent to the client');validateHeaderName(name);validateHeaderValue(name,value);(this[kHeaders]??={__proto__:null})[name.toLowerCase()]=[name,value];return this;}
    appendHeader(name,value){const previous=this.getHeader(name);return this.setHeader(name,previous===undefined?value:[...[previous].flat(),...[value].flat()]);}
    getHeader(name){return this[kHeaders]?.[String(name).toLowerCase()]?.[1];}
    getHeaders(){const headers={__proto__:null};for(const key in this[kHeaders])headers[key]=this[kHeaders][key][1];return headers;}
    getHeaderNames(){return Object.keys(this[kHeaders]??{});}
    getRawHeaderNames(){return Object.values(this[kHeaders]??{}).map(([name])=>name);}
    hasHeader(name){return !!this[kHeaders]?.[String(name).toLowerCase()];}
    removeHeader(name){if(this.headersSent)throw fail(Error,'ERR_HTTP_HEADERS_SENT','Cannot remove headers after they are sent to the client');if(this[kHeaders])delete this[kHeaders][String(name).toLowerCase()];}
    setTimeout(ms,callback){this.socket?.setTimeout(ms,callback);return this;}
    _webHeaders(){const headers=new Headers();for(const key in this[kHeaders]){const[name,value]=this[kHeaders][key];for(const part of Array.isArray(value)?value:[value])headers.append(name,String(part));}return headers;}
  }
  class ServerResponse extends OutgoingMessage {
    constructor(req){super();Object.assign(this,{req,socket:req?.socket??null,statusCode:200,statusMessage:undefined,sendDate:true,_started:false,_body:undefined,_controller:null,_drain:null,_resolve:null});}
    writeHead(statusCode,statusMessage,headers){
      if(this.headersSent)throw fail(Error,'ERR_HTTP_HEADERS_SENT','Cannot write headers after they are sent to the client');
      if(typeof statusMessage!=='string'){headers??=statusMessage;statusMessage=undefined;}
      statusCode|=0;if(statusCode<100||statusCode>999)throw fail(RangeError,'ERR_HTTP_INVALID_STATUS_CODE',`Invalid status code: ${statusCode}`);
      this.statusCode=statusCode;if(statusMessage!==undefined)this.statusMessage=statusMessage;
      if(Array.isArray(headers)){if(Array.isArray(headers[0]))for(const[name,value]of headers)this.appendHeader(name,value);else for(let i=0;i+1<headers.length;i+=2)this.appendHeader(headers[i],headers[i+1]);}
      else if(headers)for(const name of Object.keys(headers))this.setHeader(name,headers[name]);
      this.headersSent=true;return this;
    }
    flushHeaders(){this._stream();}
    _empty(){return this.req?.method==='HEAD'||this.statusCode===204||this.statusCode===304;}
    _respond(body){if(this._started)return;this._started=this.headersSent=true;this._resolve?.(new Response(this._empty()?null:body,{status:this.statusCode,statusText:this.statusMessage??STATUS_CODES[this.statusCode]??'',headers:this._webHeaders()}));}
    _stream(){if(this._started)return;this._respond(this._empty()?null:new ReadableStream({start:controller=>{this._controller=controller;},pull:()=>{const next=this._drain;this._drain=null;next?.();},cancel:()=>{this._controller=null;this.destroy();}}));}
    end(chunk,encoding,callback){
      if(typeof chunk==='function'){callback=chunk;chunk=null;}else if(typeof encoding==='function'){callback=encoding;encoding=null;}
      if(chunk!=null&&!this._started&&!this.writableLength&&!this.writableEnded){this._body=typeof chunk==='string'?Buffer.from(chunk,encoding||'utf8'):chunk;chunk=null;}
      super.end(chunk,encoding,callback);this.headersSent=true;return this;
    }
    _write(chunk,_encoding,done){try{this._stream();if(!this._controller)return done();this._controller.enqueue(new Uint8Array(chunk.buffer,chunk.byteOffset,chunk.byteLength));if(this._controller.desiredSize>0)done();else this._drain=done;}catch(error){done(error);}}
    _final(done){try{if(this._started)this._controller?.close();else this._respond(this._body??null);done();}catch(error){done(error);}}
    _destroy(error,done){try{this._controller?.error(error);}catch{}if(!this._started){this._started=true;this._resolve?.(new Response(null,{status:500}));}this.socket?.destroy();done(error);}
  }
  class Server extends net.api.Server {
    constructor(options,listener){super();if(typeof options==='function'){listener=options;options={};}Object.assign(this,{timeout:0,keepAliveTimeout:5000,headersTimeout:60000,requestTimeout:300000,maxHeadersCount:null,maxRequestsPerSocket:0,_active:new Set()});if(listener)this.on('request',listener);}
    listen(...args){
      if(this.listening)throw fail(Error,'ERR_SERVER_ALREADY_LISTEN','Listen method has been called more than once without closing.');
      const[options,callback]=net.normalize(args);
      if(options.path!==undefined)throw fail(Error,'ERR_INVALID_ARG_VALUE','HTTP servers can only listen on a TCP port');
      const port=options.port==null?0:net.port(options.port,'options.port'),hostname=options.host||'0.0.0.0';
      if(callback)this.once('listening',callback);
      try{this._server=serve({port,hostname,fetch:(request,server)=>this._handle(request,server)});}
      catch(error){queueMicrotask(()=>this.emit('error',net.failure({code:error.code??'EIO',syscall:'listen',description:error.message.replace(/ \(os error \d+\)$/,'').replace(/^./,c=>c.toLowerCase()),address:hostname,port})));return this;}
      this.listening=true;queueMicrotask(()=>this.emit('listening'));return this;
    }
    _handle(request,server){return new Promise(resolve=>{
      const socket=new RequestSocket(server.requestIP(request),this.address()),req=new IncomingMessage(socket),url=new URL(request.url);
      Object.assign(headersOf(request.headers,req),{method:request.method,url:url.pathname+url.search,_body:request.body});
      const res=new ServerResponse(req);res._resolve=resolve;
      this._active.add(socket);socket.once('close',()=>this._active.delete(socket));res.once('close',()=>socket.destroy());
      if(this.timeout)socket.setTimeout(this.timeout,()=>{if(!this.emit('timeout',socket))socket.destroy();});
      socket._upgrade=options=>{const ws=server.upgrade(request,options);res._started=true;resolve(undefined);return ws;};
      if(request.headers.get('upgrade')?.toLowerCase()==='websocket'&&this.listenerCount('upgrade')){req.upgrade=true;this.emit('upgrade',req,socket,Buffer.alloc(0));}else this.emit('request',req,res);
    });}
    address(){return this.listening?{address:this._server.hostname,family:this._server.hostname.includes(':')?'IPv6':'IPv4',port:this._server.port}:null;}
    close(callback){if(!this.listening){if(callback)queueMicrotask(()=>callback(fail(Error,'ERR_SERVER_NOT_RUNNING','Server is not running.')));return this;}this.listening=false;this._server.stop().then(()=>{this.emit('close');callback?.();});return this;}
    closeAllConnections(){for(const socket of this._active)socket.destroy();}
    closeIdleConnections(){}
    setTimeout(ms,callback){this.timeout=ms;if(callback)this.on('timeout',callback);return this;}
  }
  class Agent extends EventEmitter {
    constructor(options={}){super();Object.assign(this,{options:{path:null,...options},keepAlive:!!options.keepAlive,keepAliveMsecs:options.keepAliveMsecs??1000,maxSockets:options.maxSockets??Infinity,maxFreeSockets:options.maxFreeSockets??256,maxTotalSockets:options.maxTotalSockets??Infinity,defaultPort:80,protocol:'http:',requests:{},sockets:{},freeSockets:{}});}
    destroy(){}
  }
  const failure=(error,url)=>{
    const message=String(error?.message??error),number=/os error (\d+)/.exec(message)?.[1],code=number&&Object.keys(globalThis.__apps?.os.constants.errno??{}).find(name=>globalThis.__apps.os.constants.errno[name]===+number);
    if(code)return net.failure({code,errno:-number,syscall:/connect/i.test(message)?'connect':'read',address:url.hostname.replace(/^\[|\]$/g,''),port:+url.port||(url.protocol==='https:'?443:80)});
    if(/dns error|lookup/i.test(message))return net.failure({code:'ENOTFOUND',errno:-3008,syscall:'getaddrinfo',hostname:url.hostname});
    if(/closed before message completed|connection reset/i.test(message))return fail(Error,'ECONNRESET','socket hang up');
    return error instanceof Error?error:new Error(message);
  };
  class ClientRequest extends OutgoingMessage {
    constructor(input,options,callback){
      super({autoDestroy:false});
      if(typeof options==='function'){callback=options;options={};}
      if(typeof input==='string'||input instanceof URL){const url=new URL(input);options={protocol:url.protocol,hostname:url.hostname.replace(/^\[|\]$/g,''),port:url.port,path:url.pathname+url.search,...(url.username||url.password?{auth:`${decodeURIComponent(url.username)}:${decodeURIComponent(url.password)}`}:{}),...options};}else options={...input,...options};
      const protocol=options.protocol??'http:',host=options.hostname||options.host||'localhost',port=options.port??options.defaultPort;
      this._url=new URL(`${protocol}//${host.includes(':')?`[${host}]`:host}${port?':'+port:''}${options.path??'/'}`);
      Object.assign(this,{method:(options.method??'GET').toUpperCase(),path:options.path??'/',host,protocol,agent:options.agent,reusedSocket:false,maxHeadersCount:null,aborted:false,res:null,socket:new RequestSocket(null,null),_chunks:[],_abort:new AbortController()});
      validateHeaderName(this.method,'Method');
      const headers=options.headers;
      if(Array.isArray(headers))for(let i=0;i+1<headers.length;i+=2)this.appendHeader(headers[i],headers[i+1]);else if(headers)for(const name of Object.keys(headers))this.setHeader(name,headers[name]);
      if(options.auth&&!this.hasHeader('authorization'))this.setHeader('Authorization','Basic '+Buffer.from(options.auth).toString('base64'));
      if(callback)this.once('response',callback);
      if(options.timeout)this.setTimeout(options.timeout);
      if(options.signal){const abort=()=>this.destroy(Object.assign(new Error('The operation was aborted',{cause:options.signal.reason}),{name:'AbortError',code:'ABORT_ERR'}));if(options.signal.aborted)queueMicrotask(abort);else{options.signal.addEventListener('abort',abort,{once:true});this.once('close',()=>options.signal.removeEventListener('abort',abort));}}
      queueMicrotask(()=>{if(!this.destroyed)this.emit('socket',this.socket);});
    }
    _write(chunk,_encoding,done){this._chunks.push(Buffer.from(chunk));done();}
    _final(done){
      this.headersSent=true;
      const body=this._chunks.length&&this.method!=='GET'&&this.method!=='HEAD'?Buffer.concat(this._chunks):undefined;
      fetch(this._url,{method:this.method,headers:this._webHeaders(),body,signal:this._abort.signal,redirect:this._redirect}).then(response=>{
        if(this.destroyed)return response.body?.cancel();
        const res=headersOf(response.headers,new IncomingMessage(this.socket));
        Object.assign(res,{statusCode:response.status,statusMessage:response.statusText||STATUS_CODES[response.status]||'',req:this,_body:response.body});
        this.res=res;res.once('end',()=>queueMicrotask(()=>this.destroy()));res.once('error',error=>this.destroy(error));
        if(!this.emit('response',res))res.resume();
      },error=>{if(!this.destroyed)this.destroy(failure(error,this._url));});
      done();
    }
    _destroy(error,done){clearTimeout(this._timer);this._abort.abort();this.socket.destroyed=true;done(error);}
    abort(){if(this.aborted)return;this.aborted=true;queueMicrotask(()=>this.emit('abort'));this.destroy();}
    setTimeout(ms,callback){clearTimeout(this._timer);if(callback)this.once('timeout',callback);if(ms>0)this._timer=setTimeout(()=>this.emit('timeout'),ms);return this;}
    setNoDelay(){}setSocketKeepAlive(){}
    flushHeaders(){}
  }
  const request=(input,options,callback)=>new ClientRequest(input,options,callback);
  const get=(...args)=>{const req=request(...args);req.end();return req;};
  class WebSocket extends EventEmitter {
    constructor(url,protocols,options){super();this._socket=url instanceof globalThis.WebSocket?url:new globalThis.WebSocket(url,protocols);this._socket.binaryType='arraybuffer';for(const type of ['open','error','close','message'])this._socket.addEventListener(type,event=>{if(type==='message')this.emit(type,Buffer.from(event.data),typeof event.data!=='string');else if(type==='close')this.emit(type,event.code,Buffer.from(event.reason));else this.emit(type,event);});}
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
  const secure=(input,options)=>{if(typeof input==='string'||input instanceof URL){const{protocol}=new URL(input);if(protocol!=='https:')throw fail(TypeError,'ERR_INVALID_PROTOCOL',`Protocol "${protocol}" not supported. Expected "https:"`);return[input,options];}return[{...input,protocol:'https:'},options];};
  const secureRequest=(input,options,callback)=>{if(typeof options==='function'){callback=options;options=undefined;}return request(...secure(input,options),callback);};
  const tls=()=>{throw fail(Error,'ERR_NOT_SUPPORTED','HTTPS servers are not supported. Serve HTTP and put TLS in front of it');};
  class SecureServer extends Server { constructor(){tls();super();} }
  class SecureAgent extends Agent { constructor(options){super(options);Object.assign(this,{defaultPort:443,protocol:'https:'});} }
  return {https:{Agent:SecureAgent,globalAgent:new SecureAgent(),Server:SecureServer,createServer:tls,request:secureRequest,get:(...args)=>{const req=secureRequest(...args);req.end();return req;}},http:{Agent,globalAgent:new Agent(),ClientRequest,IncomingMessage,OutgoingMessage,Server,ServerResponse,createServer:(...args)=>new Server(...args),request,get,validateHeaderName,validateHeaderValue,maxHeaderSize:16384,STATUS_CODES,METHODS},ws:Object.assign(WebSocket,{WebSocket,WebSocketServer,Server:WebSocketServer})};
};
