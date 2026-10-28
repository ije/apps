globalThis.__appsBridge = (call,role,EventEmitter) => {
  const pending=new Map(),functions=new Map(),functionIds=new WeakMap(),handlers=new Map(),emitter=EventEmitter?new EventEmitter():null;
  const random=crypto.randomUUID.bind(crypto);
  const peer=role=>role==='main'?'main':typeof role==='object'&&role!==null?'preload'in role?'preload:'+role.preload:'renderer'in role?'renderer:'+role.renderer:null:null;
  const send=(target,message)=>call('bridge.dispatch',target,message);
  const encode=(value,target,seen=new Map(),persistent=false)=>{
    if(typeof value==='function'){const recipient=peer(target);if(!recipient)throw new TypeError('Invalid bridge target');let ids=functionIds.get(value);if(!ids){ids=new Map();functionIds.set(value,ids);}let id=ids.get(recipient);if(!functions.has(id)){id=random();functions.set(id,{action:value,recipient,persistent});ids.set(recipient,id);}else if(persistent)functions.get(id).persistent=true;return{type:'function',id};}
    if(value===undefined)return{type:'undefined'};
    if(typeof value==='bigint')return{type:'bigint',value:String(value)};
    if(typeof value==='number'&&!Number.isFinite(value))return{type:'number',value:String(value)};
    if(value===null||typeof value!=='object')return{type:'value',value};
    if(seen.has(value))return{type:'reference',id:seen.get(value)};
    const id=seen.size;seen.set(value,id);
    if(value instanceof Date)return{type:'date',id,value:value.toISOString()};
    if(value instanceof Error)return{type:'error',id,value:value.message,name:value.name,stack:value.stack};
    if(value instanceof ArrayBuffer||ArrayBuffer.isView(value))return{type:'bytes',id,name:value instanceof ArrayBuffer?'ArrayBuffer':value.constructor.name,value:new Uint8Array(value.buffer??value,value.byteOffset??0,value.byteLength)};
    if(value instanceof Map)return{type:'map',id,items:[...value].map(([k,v])=>[encode(k,target,seen,persistent),encode(v,target,seen,persistent)])};
    if(value instanceof Set)return{type:'set',id,items:[...value].map(v=>encode(v,target,seen,persistent))};
    if(Array.isArray(value))return{type:'array',id,items:value.map(v=>encode(v,target,seen,persistent))};
    return{type:'object',id,items:Object.fromEntries(Object.entries(value).map(([k,v])=>[k,encode(v,target,seen,persistent)]))};
  };
  const decode=(value,target,seen=new Map())=>{
    if(value.type==='function')return(...args)=>rpc(target,{kind:'call',function:value.id,args:encode(args,target)});
    if(value.type==='undefined')return undefined;
    if(value.type==='bigint')return BigInt(value.value);
    if(value.type==='number')return Number(value.value);
    if(value.type==='value')return value.value;
    if(value.type==='reference')return seen.get(value.id);
    let result;
    if(value.type==='date')result=new Date(value.value);
    else if(value.type==='error')result=Object.assign(new Error(value.value),{name:value.name,stack:value.stack});
    else if(value.type==='bytes'){const bytes=new Uint8Array(value.value);const constructors={Int8Array,Uint8Array,Uint8ClampedArray,Int16Array,Uint16Array,Int32Array,Uint32Array,Float32Array,Float64Array,BigInt64Array,BigUint64Array,DataView};result=value.name==='ArrayBuffer'?bytes.buffer:new (constructors[value.name]??Uint8Array)(bytes.buffer);}
    else result=value.type==='array'?[]:value.type==='map'?new Map():value.type==='set'?new Set():{};
    seen.set(value.id,result);
    if(value.type==='array')result.push(...value.items.map(v=>decode(v,target,seen)));
    else if(value.type==='map')for(const[k,v]of value.items)result.set(decode(k,target,seen),decode(v,target,seen));
    else if(value.type==='set')for(const v of value.items)result.add(decode(v,target,seen));
    else if(value.type==='object')for(const[k,v]of Object.entries(value.items))Object.defineProperty(result,k,{value:decode(v,target,seen),enumerable:true});
    return result;
  };
  const rpc=(target,message)=>{
    const id=random();let resolve,reject;
    const promise=new Promise((a,b)=>{resolve=a;reject=b;});
    pending.set(id,{resolve,reject,recipient:peer(target)});
    try{const response=send(target,{...message,id});if(response?.immediate){pending.delete(id);return decode(response.immediate,target);}return promise;}catch(error){pending.delete(id);throw error;}
  };
  const sourceId=source=>typeof source==='object'?(source.preload??source.renderer):null;
  const event=source=>({sender:{id:sourceId(source),send:(channel,...args)=>send({preload:sourceId(source)},{kind:'event',channel,args:encode(args,{preload:sourceId(source)})})},senderFrame:{url:'apps://app/'},reply:(channel,...args)=>send(source,{kind:'event',channel,args:encode(args,source)}),returnValue:undefined});
  globalThis.__appsBridgeReceive=message=>{
    if(message.kind==='reply'){const request=pending.get(message.id);if(!request||request.recipient!==peer(message.sender))return null;pending.delete(message.id);try{if(message.error)request.reject(new Error(message.error));else request.resolve(decode(message.value,message.sender));}catch(error){request.reject(error);}return null;}
    if(message.kind==='event'||message.kind==='sync'){const e=event(message.sender);emitter?.emit(message.channel,e,...decode(message.args,message.sender));return encode(e.returnValue,message.sender);}
    const capability=message.kind==='call'?functions.get(message.function):null;
    if(message.kind==='call'&&(!capability||capability.recipient!==peer(message.sender)))throw new Error('Bridge callback is unavailable to this context');
    const action=message.kind==='call'?capability.action:handlers.get(message.channel);
    if(!action)throw new Error('Missing bridge handler');
    const args=decode(message.args,message.sender);
    const value=action(...(message.kind==='invoke'?[event(message.sender),...args]:args));
    if(!value||typeof value.then!=='function')return{immediate:encode(value,message.sender)};
    const settle=(value,error,failed=false)=>{try{send(message.sender,{kind:'reply',id:message.id,...(failed?{error:error?.message??String(error)}:{value:encode(value,message.sender)})});}catch(error){try{send(message.sender,{kind:'reply',id:message.id,error:error?.message??String(error)});}catch{}}};
    Promise.resolve(value).then(value=>settle(value),error=>settle(undefined,error,true));
    return{pending:true};
  };
  globalThis.__appsBridgeClosed=source=>{const recipient=peer(source);for(const[id,request]of pending)if(request.recipient===recipient){pending.delete(id);request.reject(new Error('Bridge target closed'));}for(const[id,capability]of functions)if(capability.recipient===recipient&&!capability.persistent)functions.delete(id);};
  globalThis.__appsSyncBridge=()=>{
    if(typeof role!=='object'||!('renderer'in role))return;
    for(const[name,value]of Object.entries(call('bridge.exposures')))if(!Object.hasOwn(globalThis,name))Object.defineProperty(globalThis,name,{value:decode(value,{preload:role.renderer}),configurable:true});
  };
  const contextBridge={exposeInMainWorld(name,value){if(typeof role!=='object'||!('preload'in role))throw new Error('contextBridge requires a preload context');call('bridge.expose',String(name),encode(value,{renderer:role.preload},new Map(),true));}};
  const ipcMain=emitter;
  if(role==='main')Object.assign(ipcMain,{handle(channel,fn){if(handlers.has(channel))throw new Error('Handler already registered: '+channel);handlers.set(channel,fn);},handleOnce(channel,fn){this.handle(channel,(...args)=>{handlers.delete(channel);return fn(...args);});},removeHandler:channel=>handlers.delete(channel)});
  const ipcRenderer=emitter;
  if(typeof role==='object'&&'preload'in role)Object.assign(ipcRenderer,{invoke:async(channel,...args)=>rpc('main',{kind:'invoke',channel,args:encode(args,'main')}),send:(channel,...args)=>send('main',{kind:'event',channel,args:encode(args,'main')}),sendSync:(channel,...args)=>decode(send('main',{kind:'sync',channel,args:encode(args,'main')}),'main')});
  const poll=()=>{};poll.pending=()=>pending.size>0;
  return{contextBridge,ipcMain,ipcRenderer,send:(id,channel,...args)=>send({preload:id},{kind:'event',channel,args:encode(args,{preload:id})}),poll};
};
