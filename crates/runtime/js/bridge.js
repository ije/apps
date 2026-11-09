globalThis.__appsBridge = (call,role,EventEmitter,hooks={}) => {
  const pending=new Map(),functions=new Map(),functionIds=new WeakMap(),handlers=new Map(),queue=[],refs=new Map(),refIds=new WeakMap(),facades=new Map(),emitter=EventEmitter?new EventEmitter():null;
  const random=crypto.randomUUID.bind(crypto),errors={Error,EvalError,RangeError,ReferenceError,SyntaxError,TypeError,URIError};
  // WARNING: a dropped File carries the index of its path in Rust, never the path. A page can forge an index, but it only names files that were dropped on that page.
  const files=hooks.files=new WeakMap();
  const peer=role=>role==='main'?'main':typeof role==='object'&&role!==null?'preload'in role?'preload:'+role.preload:'renderer'in role?'renderer:'+role.renderer:null:null;
  const send=(target,message)=>call('bridge.dispatch',target,message);
  const uncloneable=()=>{throw new Error('An object could not be cloned.');};
  const jsonEncoder=new TextEncoder(),jsonDecoder=new TextDecoder();
  const capability=(action,target,once)=>{const id=random();functions.set(id,{action,recipient:peer(target),persistent:!once,once});return id;};
  // WARNING: only data that JSON keeps exactly may cross as JSON: finite numbers but -0, no undefined, no holes, plain
  // prototypes and no object twice.
  let floats=false;
  const plain=(value,seen)=>{
    if(value===null||typeof value==='string'||typeof value==='boolean')return true;
    if(typeof value==='number'){if(!Number.isSafeInteger(value))floats=true;return Number.isFinite(value)&&!Object.is(value,-0);}
    if(typeof value!=='object'||seen.has(value))return false;
    seen.add(value);
    if(Array.isArray(value)){if(Object.getPrototypeOf(value)!==Array.prototype)return false;for(let i=0;i<value.length;i++)if(!(i in value)||!plain(value[i],seen))return false;return true;}
    const proto=Object.getPrototypeOf(value);
    if(proto!==Object.prototype&&proto!==null)return false;
    for(const key in value)if(!plain(value[key],seen))return false;
    return true;
  };
  const encode=(value,target,strict=true,persistent=false,seen=new Map())=>{
    // WARNING: Rust forwards bytes without parsing them, and its JSON parser can change the last bit of a float, so
    // plain data with floats or of more than 4 KB goes as UTF-8 JSON, and other numbers that are not safe integers go as text.
    if(!seen.size&&(typeof value==='object'&&value!==null||typeof value==='string')){
      floats=false;
      if(plain(value,new Set())){const text=JSON.stringify(value);return floats||text.length>4096?{type:'json',value:jsonEncoder.encode(text)}:{type:'json',value};}
    }
    const next=v=>encode(v,target,strict,persistent,seen);
    if(typeof value==='function'){if(strict)uncloneable();const recipient=peer(target);if(!recipient)throw new TypeError('Invalid bridge target');let ids=functionIds.get(value);if(!ids){ids=new Map();functionIds.set(value,ids);}let id=ids.get(recipient);if(!functions.has(id)){id=random();functions.set(id,{action:value,recipient,persistent});ids.set(recipient,id);}else if(persistent)functions.get(id).persistent=true;return{type:'function',id};}
    if(value===undefined)return{type:'undefined'};
    if(typeof value==='bigint')return{type:'bigint',value:String(value)};
    if(typeof value==='number'&&!Number.isSafeInteger(value)||Object.is(value,-0))return{type:'number',value:Object.is(value,-0)?'-0':String(value)};
    if(typeof value==='symbol')uncloneable();
    if(value===null||typeof value!=='object')return{type:'value',value};
    if(seen.has(value))return{type:'reference',id:seen.get(value)};
    const id=seen.size;seen.set(value,id);
    if(value instanceof Date)return{type:'date',id,value:String(value.getTime())};
    if(value instanceof Error)return{type:'error',id,value:String(value.message),name:String(value.name),stack:value.stack};
    const blob=value instanceof Blob?hooks.blob?.(value):null;
    if(blob)return{type:'blob',id,value:blob,mime:value.type,...(value instanceof File?{name:value.name,lastModified:value.lastModified,file:files.get(value)}:{})};
    if(value instanceof ArrayBuffer||ArrayBuffer.isView(value))return{type:'bytes',id,name:value instanceof ArrayBuffer?'ArrayBuffer':value.constructor.name,value:new Uint8Array(value.buffer??value,value.byteOffset??0,value.byteLength)};
    if(value instanceof Promise){if(strict)uncloneable();return{type:'promise',id,then:capability(settle=>{value.then(v=>settle(true,v),e=>settle(false,e));},target,!persistent)};}
    if(value instanceof Map)return{type:'map',id,items:[...value].map(([k,v])=>[next(k),next(v)])};
    if(value instanceof Set)return{type:'set',id,items:[...value].map(next)};
    if(Array.isArray(value)){const items=[];for(let i=0;i<value.length;i++)items.push(i in value?next(value[i]):null);return{type:'array',id,items};}
    return{type:'object',id,items:Object.fromEntries(Object.entries(value).map(([k,v])=>[k,next(v)]))};
  };
  const decode=(value,target,seen=new Map())=>{
    if(value.type==='function')return(...args)=>rpc(target,{kind:'call',function:value.id,args:encode(args,target,false)});
    if(value.type==='undefined')return undefined;
    if(value.type==='bigint')return BigInt(value.value);
    if(value.type==='number')return Number(value.value);
    if(value.type==='json')return value.value instanceof Uint8Array?JSON.parse(jsonDecoder.decode(value.value)):value.value;
    if(value.type==='value')return value.value;
    if(value.type==='reference')return seen.get(value.id);
    let result;
    if(value.type==='promise')result=new Promise((resolve,reject)=>rpc(target,{kind:'call',function:value.then,args:{type:'array',id:0,items:[{type:'function',id:capability((ok,v)=>ok?resolve(v):reject(v),target,true)}]}}));
    else if(value.type==='date')result=new Date(Number(value.value));
    else if(value.type==='error'){result=new(Object.hasOwn(errors,value.name)?errors[value.name]:Error)(value.value);if(result.name!==value.name)result.name=value.name;result.stack=value.stack;}
    else if(value.type==='blob'){result=value.name===undefined?new Blob([value.value],{type:value.mime}):new File([value.value],value.name,{type:value.mime,lastModified:value.lastModified});if(value.file!==undefined)files.set(result,value.file);}
    else if(value.type==='bytes'){const bytes=new Uint8Array(value.value);const constructors={Int8Array,Uint8Array,Uint8ClampedArray,Int16Array,Uint16Array,Int32Array,Uint32Array,Float32Array,Float64Array,BigInt64Array,BigUint64Array,DataView};result=value.name==='ArrayBuffer'?bytes.buffer:new (constructors[value.name]??Uint8Array)(bytes.buffer);}
    else result=value.type==='array'?new Array(value.items.length):value.type==='map'?new Map():value.type==='set'?new Set():{};
    seen.set(value.id,result);
    if(value.type==='array')value.items.forEach((v,i)=>{if(v)result[i]=decode(v,target,seen);});
    else if(value.type==='map')for(const[k,v]of value.items)result.set(decode(k,target,seen),decode(v,target,seen));
    else if(value.type==='set')for(const v of value.items)result.add(decode(v,target,seen));
    else if(value.type==='object')for(const[k,v]of Object.entries(value.items))Object.defineProperty(result,k,{value:decode(v,target,seen),enumerable:true,writable:true,configurable:true});
    return result;
  };
  const answer=(response,target)=>{if(response?.thrown)throw decode(response.thrown,target);return decode(response.immediate,target);};
  const rpc=(target,message)=>{
    const id=random();let resolve,reject;
    const promise=new Promise((a,b)=>{resolve=a;reject=b;});
    pending.set(id,{resolve,reject,recipient:peer(target)});
    let response;
    try{response=send(target,{...message,id});}catch(error){pending.delete(id);throw error;}
    if(response?.pending)return promise;
    pending.delete(id);
    return answer(response,target);
  };
  const sourceId=source=>typeof source==='object'?(source.preload??source.renderer):null;
  const event=source=>{
    if(role!=='main')return{sender:emitter,senderId:0,ports:[]};
    const id=sourceId(source),sender=hooks.contents?.(id)??null,processId=globalThis.process?.pid??0;
    return{sender,senderFrame:{url:sender?.getURL?.()??'',processId,routingId:1,send:(channel,...args)=>sender?.send(channel,...args)},processId,frameId:1,ports:[],reply:(channel,...args)=>send(source,{kind:'event',channel,args:encode(args,source)}),returnValue:undefined};
  };
  const emit=(channel,...args)=>emitter.listenerCount(channel)>0&&emitter.emit(channel,...args);
  const report=error=>(hooks.report??console.error)(error);
  // WARNING: events, invoke handlers and replies of one peer keep their send order, as they do over one Electron IPC channel.
  const drain=()=>{for(const message of queue.splice(0)){try{if(typeof message==='function')message();else emit(message.channel,event(message.sender),...decode(message.args,message.sender));}catch(error){report(error);}}};
  const enqueue=task=>{queue.push(task);};
  globalThis.__appsBridgeDrain=()=>{if(!queue.length)return false;drain();return true;};
  const settle=(message,request=pending.get(message.id))=>{if(!request||request.recipient!==peer(message.sender))return;pending.delete(message.id);try{if(message.error)request.reject(decode(message.error,message.sender));else request.resolve(decode(message.value,message.sender));}catch(error){request.reject(error);}};
  const invoke=message=>{
    const fail=(error,log=true)=>{if(log)console.error(`Error occurred in handler for '${message.channel}':`,error);return{thrown:encode(new Error(`Error invoking remote method '${message.channel}': ${error}`),message.sender)};};
    const action=handlers.get(message.channel);
    if(!action)return fail(new Error(`No handler registered for '${message.channel}'`),false);
    let value;
    try{value=action(event(message.sender),...decode(message.args,message.sender));if(!value||typeof value.then!=='function')return{immediate:encode(value,message.sender)};}catch(error){return fail(error);}
    Promise.resolve(value).then(value=>{let encoded;try{encoded=encode(value,message.sender);}catch(error){return reply(message,fail(error));}reply(message,{immediate:encoded});},error=>reply(message,fail(error)));
    return{pending:true};
  };
  const reply=(message,result)=>{try{send(message.sender,{kind:'reply',id:message.id,...(result.thrown?{error:result.thrown}:{value:result.immediate})});}catch{}};
  let lastRef=0;
  const out=value=>{
    if(typeof value==='function')return{$method:1};
    if(Array.isArray(value))return value.map(out);
    if(value===null||typeof value!=='object'||Object.getPrototypeOf(value)===Object.prototype)return value;
    let ref=refIds.get(value);
    if(ref===undefined){ref=++lastRef;refs.set(ref,value);refIds.set(value,ref);}
    return{$ref:ref};
  };
  globalThis.__appsBridgeReceive=message=>{
    if(message.kind==='reply'){if(queue.length)enqueue(()=>settle(message));else settle(message);return null;}
    if(message.kind==='event'){if(emitter)enqueue(message);return null;}
    if(message.kind==='sync'){drain();const eventObject=event(message.sender);try{emit(message.channel,eventObject,...decode(message.args,message.sender));}catch(error){report(error);}return encode(eventObject.returnValue,message.sender);}
    if(message.kind==='expose'){__appsSyncBridge();return null;}
    if(message.kind==='dom'){
      if(typeof role!=='object'||!('renderer'in role)||message.sender?.preload!==role.renderer)throw new Error('DOM access is only available to the preload');
      try{const[op,ref,key,value]=decode(message.args,message.sender),object=ref?refs.get(ref):globalThis.document;if(op==='release'){for(const id of value){refIds.delete(refs.get(id));refs.delete(id);}return{immediate:encode(undefined,message.sender)};}if(!object)throw new Error('Unknown DOM object');return{immediate:encode(op==='set'?void(object[key]=value):out(op==='call'?object[key](...value):object[key]),message.sender)};}
      catch(error){return{thrown:encode(error,message.sender)};}
    }
    if(message.kind==='call'){
      const capability=functions.get(message.function);
      if(!capability||capability.recipient!==peer(message.sender))throw new Error('Bridge callback is unavailable to this context');
      if(capability.once)functions.delete(message.function);
      try{return{immediate:encode(capability.action(...decode(message.args,message.sender)),message.sender,false)};}catch(error){return{thrown:encode(error,message.sender,false)};}
    }
    if(message.kind!=='invoke'||role!=='main')throw new Error('Unknown bridge message');
    enqueue(()=>{const result=invoke(message);if(!result.pending)reply(message,result);});
    return{pending:true};
  };
  globalThis.__appsBridgeClosed=source=>{const recipient=peer(source);for(const[id,request]of pending)if(request.recipient===recipient){pending.delete(id);request.reject(new Error('Bridge target closed'));}for(const[id,capability]of functions)if(capability.recipient===recipient&&!capability.persistent)functions.delete(id);};
  let preloadExports,preloadReady;
  globalThis.__appsImportPreload=()=>preloadExports??(preloadExports=new Promise(resolve=>{preloadReady=resolve;}));
  globalThis.__appsSyncBridge=()=>{
    if(typeof role!=='object'||!('renderer'in role))return;
    for(const[name,value]of Object.entries(call('bridge.exposures'))){
      if(name==='__appsPreload'){if(!preloadExports||preloadReady){preloadExports=decode(value,{preload:role.renderer});preloadReady?.(preloadExports);preloadReady=undefined;}continue;}
      if(!Object.hasOwn(globalThis,name))Object.defineProperty(globalThis,name,{value:decode(value,{preload:role.renderer}),configurable:true});
    }
  };
  const preload=typeof role==='object'&&'preload'in role,page={renderer:role?.preload};
  if(preload)globalThis.__appsPublishPreload=value=>{call('bridge.preload',encode(value,page,false,true));try{send(page,{kind:'expose'});}catch{}};
  const contextBridge={exposeInMainWorld(name,value){if(!preload)throw new Error('contextBridge requires a preload context');call('bridge.expose',String(name),encode(value,page,false,true));try{send(page,{kind:'expose'});}catch{}}};
  const ipcMain=emitter;
  if(role==='main')Object.assign(ipcMain,{handle(channel,fn){if(handlers.has(channel))throw new Error(`Attempted to register a second handler for '${channel}'`);handlers.set(channel,fn);},handleOnce(channel,fn){this.handle(channel,(...args)=>{handlers.delete(channel);return fn(...args);});},removeHandler:channel=>handlers.delete(channel)});
  const ipcRenderer=emitter;
  if(preload){
    Object.assign(ipcRenderer,{invoke:(channel,...args)=>{const result=new Promise(resolve=>resolve(rpc('main',{kind:'invoke',channel,args:encode(args,'main')})));return queue.length?new Promise(resolve=>enqueue(()=>resolve(result))):result;},send:(channel,...args)=>send('main',{kind:'event',channel,args:encode(args,'main')}),sendSync:(channel,...args)=>decode(send('main',{kind:'sync',channel,args:encode(args,'main')}),'main')});
    // WARNING: page objects never enter the preload realm: the preload `document` is a proxy, and the page answers with clones and object handles.
    const remote=(...args)=>answer(send(page,{kind:'dom',args:encode(args,page)}),page);
    const read=value=>Array.isArray(value)?value.map(read):value&&typeof value==='object'&&'$ref'in value?facade(value.$ref):value;
    const released=new Set(),flush=()=>{if(!released.size)return;const ids=[...released];released.clear();try{remote('release',0,null,ids);}catch{}};
    const handles=new __appsFinalizers(ref=>{if(facades.get(ref)?.deref())return;facades.delete(ref);if(released.add(ref).size===1)queueMicrotask(flush);});
    const facade=ref=>{let proxy=facades.get(ref)?.deref();if(!proxy){proxy=new Proxy({},{get:(_,key)=>{if(typeof key!=='string'||key==='then')return undefined;const value=remote('get',ref,key);return value?.$method?(...args)=>read(remote('call',ref,key,args)):read(value);},set:(_,key,value)=>{if(typeof key!=='string')return false;remote('set',ref,key,value);return true;}});facades.set(ref,new WeakRef(proxy));handles.register(proxy,ref);released.delete(ref);}return proxy;};
    globalThis.window??=globalThis;
    Object.defineProperty(globalThis,'document',{get:()=>facade(0),configurable:true});
  }
  const webUtils=Object.freeze({getPathForFile(file){if(!(file instanceof File))throw new TypeError('Expected a File');const index=files.get(file);return index===undefined?'':call('bridge.file',index);}});
  const poll=()=>{if(queue.length)drain();};poll.pending=()=>pending.size>0||queue.length>0;poll.queued=()=>queue.length>0;
  return{contextBridge,ipcMain,ipcRenderer,webUtils,rpc,send:(id,channel,...args)=>send({preload:id},{kind:'event',channel,args:encode(args,{preload:id})}),poll};
};
