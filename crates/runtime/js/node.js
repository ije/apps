globalThis.__appsNode = (call, EventEmitter) => {
  const bytes = text => { if(typeof text !== 'string')return Buffer.from(text); const length = text.length, out = new Buffer(length); for (let i = 0; i < length; i++) out[i] = text.charCodeAt(i); return out; };
  const binary = data => { let s = ''; for (let i = 0; i < data.length; i += 8192) s += String.fromCharCode.apply(null, data.subarray(i, i + 8192)); return s; };
  const view = (value, encoding) => typeof value === 'string' ? Buffer.from(value, encoding) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : Buffer.from(value);
  const utf8 = /^utf-?8$/i;
  const encode = (value, encoding = 'utf8') => {
    encoding = encoding.toLowerCase().replace('-', '');
    if (encoding === 'hex') { const pairs = String(value).match(/^[\da-f]*/i)[0]; return Uint8Array.from(pairs.match(/../g) ?? [], s => parseInt(s, 16)); }
    if (encoding === 'base64' || encoding === 'base64url') return bytes(atob(String(value).replace(/-/g, '+').replace(/_/g, '/').replace(/\s/g, '')));
    if (encoding === 'ascii' || encoding === 'latin1' || encoding === 'binary') return bytes(String(value));
    if (encoding === 'utf16le' || encoding === 'ucs2') { const s=String(value), bytes=new Uint8Array(s.length*2); for(let i=0;i<s.length;i++){bytes[i*2]=s.charCodeAt(i)&255;bytes[i*2+1]=s.charCodeAt(i)>>>8;} return bytes; }
    if (encoding !== 'utf8') throw new TypeError('Unsupported encoding: ' + encoding);
    return new TextEncoder().encode(String(value));
  };
  class Buffer extends Uint8Array {
    static from(value, encoding, length) {
      if (typeof value === 'string') { const bytes = encode(value, encoding); return new Buffer(bytes.buffer, bytes.byteOffset, bytes.length); }
      if (value instanceof ArrayBuffer) return new Buffer(value, encoding, length);
      if (value?.type === 'Buffer' && Array.isArray(value.data)) value = value.data;
      if (typeof value === 'number' || value == null) throw new TypeError('Expected a string, array, or buffer');
      return new Buffer(value);
    }
    static alloc(size, fill = 0, encoding) { const out=new Buffer(size); if(typeof fill==='string'){const p=encode(fill,encoding);if(p.length)for(let i=0;i<out.length;i++)out[i]=p[i%p.length];}else out.fill(fill);return out; }
    static allocUnsafe(size) { return new Buffer(size); }
    static allocUnsafeSlow(size) { return new Buffer(size); }
    static isBuffer(value) { return value instanceof Buffer; }
    static isEncoding(value) { return /^(utf-?8|hex|base64(?:url)?|ascii|latin1|binary|utf-?16le|ucs-?2)$/i.test(value); }
    static byteLength(value, encoding) { return typeof value==='string'?encode(value,encoding).length:value.byteLength; }
    static concat(values, length=values.reduce((n,v)=>n+v.length,0)) { const out=new Buffer(length);let offset=0;for(const value of values){const n=Math.min(value.length,length-offset);if(n<=0)break;out.set(value.subarray(0,n),offset);offset+=n;}return out; }
    static compare(a,b) { for(let i=0;i<Math.min(a.length,b.length);i++)if(a[i]!==b[i])return a[i]<b[i]?-1:1;return Math.sign(a.length-b.length); }
    toString(encoding='utf8',start=0,end=this.length) {
      const bytes=this.subarray(start,end);encoding=encoding.toLowerCase().replace('-','');
      if(encoding==='hex')return Array.from(bytes,v=>v.toString(16).padStart(2,'0')).join('');
      if(encoding==='base64'||encoding==='base64url'){let out=btoa(binary(bytes));return encoding==='base64url'?out.replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,''):out;}
      if(encoding==='ascii'||encoding==='latin1'||encoding==='binary')return binary(encoding==='ascii'?bytes.map(byte=>byte&127):bytes);
      if(encoding==='utf16le'||encoding==='ucs2'){let s='';for(let i=0;i+1<bytes.length;i+=2)s+=String.fromCharCode(bytes[i]|bytes[i+1]<<8);return s;}
      if(encoding!=='utf8')throw new TypeError('Unsupported encoding: '+encoding);return new TextDecoder('utf-8',{ignoreBOM:true}).decode(bytes);
    }
    equals(other){return Buffer.compare(this,other)===0;}
    compare(other){return Buffer.compare(this,other);}
    slice(start,end){return this.subarray(start,end);}
    copy(target,targetStart=0,start=0,end=this.length){const v=this.subarray(start,Math.min(end,start+target.length-targetStart));target.set(v,targetStart);return v.length;}
    write(value,offset=0,length=this.length-offset,encoding='utf8'){if(typeof offset==='string'){encoding=offset;offset=0;length=this.length;}if(typeof length==='string'){encoding=length;length=this.length-offset;}const b=encode(value,encoding).subarray(0,length);this.set(b,offset);return b.length;}
    toJSON(){return {type:'Buffer',data:Array.from(this)};}
    indexOf(value,offset=0,encoding){if(typeof value==='number')return super.indexOf(value,offset);const needle=typeof value==='string'?encode(value,encoding):value;outer:for(let i=Math.max(0,offset);i<=this.length-needle.length;i++){for(let j=0;j<needle.length;j++)if(this[i+j]!==needle[j])continue outer;return i;}return -1;}
    includes(value,offset,encoding){return this.indexOf(value,offset,encoding)>=0;}
  }
  for(const [suffix,type,size] of [['UInt8','Uint8',1],['Int8','Int8',1],['UInt16','Uint16',2],['Int16','Int16',2],['UInt32','Uint32',4],['Int32','Int32',4],['Float','Float32',4],['Double','Float64',8],['BigInt64','BigInt64',8],['BigUInt64','BigUint64',8]])for(const endian of size===1?['']:['LE','BE']){
    Buffer.prototype['read'+suffix+endian]=function(offset=0){return new DataView(this.buffer,this.byteOffset,this.byteLength)['get'+type](offset,endian==='LE');};
    Buffer.prototype['write'+suffix+endian]=function(value,offset=0){new DataView(this.buffer,this.byteOffset,this.byteLength)['set'+type](offset,value,endian==='LE');return offset+size;};
  }
  globalThis.Buffer=Buffer;
  globalThis.setImmediate=(fn,...args)=>setTimeout(fn,0,...args);globalThis.clearImmediate=clearTimeout;
  const pending=new Map(),watchers=new Map();
  const sync=(op,...args)=>call('node.'+op,...args);
  const asyncCall=(op,...args)=>new Promise((resolve,reject)=>{const id=sync('async',op,args);pending.set(id,{resolve,reject});});
  const path=value=>value instanceof URL?decodeURIComponent(value.pathname):String(value);
  const options=value=>typeof value==='string'?{encoding:value}:value??{};
  const stat=value=>({...value,mtime:new Date(value.mtimeMs),birthtime:new Date(value.birthtimeMs??value.mtimeMs),isFile:()=>value.file,isDirectory:()=>value.directory,isSymbolicLink:()=>value.symlink,isBlockDevice:()=>!!value.block,isCharacterDevice:()=>!!value.character,isFIFO:()=>!!value.fifo,isSocket:()=>!!value.socket});
  const files={constants:sync('constants'),promises:{}};
  const chunk=(buffer,offset,length,position)=>{if(typeof buffer==='string')return[new Uint8Array(Buffer.from(buffer,length)),offset??null];const bytes=view(buffer);offset??=0;return[bytes.subarray(offset,offset+(length??bytes.length-offset)),position??null];};
  const finish=(name,result,args)=>{
    if(name==='readFile'){const encoding=options(args[1]).encoding;return utf8.test(encoding)?result:encoding?bytes(result).toString(encoding):bytes(result);}
    if(name==='stat'||name==='lstat')return stat(result);
    if(name==='readdir')return result.map(v=>options(args[1]).withFileTypes?{...stat(v),name:v.name,parentPath:args[0]}:v.name);
    if(name==='open')return {fd:result,async close(){await asyncCall('close',result);},async read(buffer,offset=0,length=buffer.length,position=null){const data=await asyncCall('read',result,length,position);buffer.set(bytes(data),offset);return{bytesRead:data.length,buffer};},async write(buffer,...args){return{bytesWritten:await asyncCall('write',result,...chunk(buffer,...args)),buffer};},async readFile(o){const chunks=[];for(;;){const b=await asyncCall('read',result,1048576,null);if(!b.length)break;chunks.push(bytes(b));}const b=Buffer.concat(chunks);return options(o).encoding?b.toString(options(o).encoding):b;},async writeFile(data,o){const bytes=view(data,options(o).encoding);let at=0;while(at<bytes.length)at+=await asyncCall('write',result,bytes.subarray(at,at+1048576),null);},async sync(){await asyncCall('fsync',result);},async truncate(size=0){await asyncCall('ftruncate',result,size);}};
    return result??undefined;
  };
  for(const name of ['readFile','writeFile','appendFile','mkdir','readdir','stat','lstat','rename','copyFile','realpath','readlink','symlink','chmod','access','unlink','rm','rmdir','mkdtemp','open','close','fsync','ftruncate']){
    const prepare=args=>{args=[...args];if(typeof args[0]!=='number')args[0]=path(args[0]);if(['rename','copyFile','symlink'].includes(name))args[1]=path(args[1]);if(['readFile','readdir','mkdir','rm','rmdir'].includes(name))args[1]=options(args[1]);if(name==='writeFile'||name==='appendFile'){args[2]=options(args[2]);args[1]=typeof args[1]==='string'&&(!args[2].encoding||utf8.test(args[2].encoding))?args[1]:view(args[1],args[2].encoding);}return args;};
    files[name+'Sync']=(...args)=>{args=prepare(args);const result=sync(name,...args);return name==='open'?result:finish(name,result,args);};
    files.promises[name]=async(...args)=>{args=prepare(args);return finish(name,await asyncCall(name,...args),args);};
    files[name]=(...args)=>{const callback=args.pop();if(typeof callback!=='function')throw new TypeError('Expected callback');files.promises[name](...args).then(result=>callback(null,name==='open'?result.fd:result),callback);};
  }
  files.existsSync=value=>{try{sync('access',path(value));return true;}catch{return false;}};
  files.readSync=(fd,buffer,offset=0,length=buffer.length,position=null)=>{const data=sync('read',fd,length,position);buffer.set(bytes(data),offset);return data.length;};
  files.writeSync=(fd,...args)=>sync('write',fd,...chunk(...args));
  files.watch=(value,o={},listener)=>{if(typeof o==='function'){listener=o;o={};}const id=sync('watch',path(value),o),watcher=new EventEmitter();watcher.close=()=>{sync('unwatch',id);watchers.delete(id);watcher.emit('close');};watcher.ref=()=>{watcher._ref=true;return watcher;};watcher.unref=()=>{watcher._ref=false;return watcher;};watcher._ref=o.persistent!==false;watchers.set(id,watcher);if(listener)watcher.on('change',listener);o.signal?.addEventListener('abort',()=>watcher.close(),{once:true});return watcher;};
  const poll=()=>{if(!pending.size&&!watchers.size)return;for(const event of sync('poll')){if(event.watch){const watcher=watchers.get(event.id);if(!watcher)continue;if(event.error)watcher.emit('error',new Error(event.error));else for(const p of event.paths)watcher.emit('change',event.event,p.split(/[\\/]/).at(-1));}else{const item=pending.get(event.id);pending.delete(event.id);if(item){if(event.error)item.reject(Object.assign(new Error(event.error),{code:event.code}));else item.resolve(event.value);}}}};
  poll.pending=()=>pending.size>0||[...watchers.values()].some(w=>w._ref);
  const promisify=fn=>{if(fn[promisify.custom])return fn[promisify.custom];return function(...args){return new Promise((resolve,reject)=>fn.call(this,...args,(error,...values)=>error?reject(error):resolve(values[0])));};};promisify.custom=Symbol.for('nodejs.util.promisify.custom');
  const inspect=value=>{try{return typeof value==='string'?value:JSON.stringify(value,null,2);}catch{return String(value);}};inspect.custom=Symbol.for('nodejs.util.inspect.custom');
  const util={promisify,callbackify:fn=>function(...args){const callback=args.pop();Promise.resolve(fn.apply(this,args)).then(value=>callback(null,value),callback);},inspect,format:(...args)=>args.map(inspect).join(' '),types:{isAnyArrayBuffer:v=>v instanceof ArrayBuffer,isUint8Array:v=>v instanceof Uint8Array,isTypedArray:ArrayBuffer.isView,isNativeError:v=>v instanceof Error,isDate:v=>v instanceof Date,isPromise:v=>v instanceof Promise},TextEncoder,TextDecoder};
  const url={URL,URLSearchParams,fileURLToPath:value=>{const u=value instanceof URL?value:new URL(value);if(u.protocol!=='file:')throw new TypeError('Expected file URL');return decodeURIComponent(u.pathname);},pathToFileURL:value=>new URL('file://'+path(value).split('/').map(encodeURIComponent).join('/'))};
  class StringDecoder{constructor(encoding='utf8'){this.decoder=new TextDecoder(encoding,{ignoreBOM:true});}write(value){return this.decoder.decode(value,{stream:true});}end(value){return(value?this.write(value):'')+this.decoder.decode();}}
  return {Buffer,fs:files,util,url,string_decoder:{StringDecoder},poll};
};
