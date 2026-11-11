use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{JavaScriptCore, JsContext},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn run(source: &str) {
    let engine =
        JsContext::new(JavaScriptCore::load().unwrap(), Rc::default(), Role::Main).unwrap();
    engine.eval(&format!("globalThis.result='pending';(async()=>{{{source}}})().then(()=>result='ok',e=>result=String(e)+'\\n'+e.stack);"),"compatibility").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        engine.tick().unwrap();
        let result = engine.eval("result", "test").unwrap();
        if result != "pending" {
            assert_eq!(result, "ok");
            break;
        }
        assert!(Instant::now() < deadline, "Test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn buffer_streams_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = serde_json::to_string(&dir.path().join("file.txt")).unwrap();
    run(&format!(
        r#"
        const assert=(c,m)=>{{if(!c)throw Error(m)}};
        const b=Buffer.from('Hello 世界');assert(Buffer.from(b.toString('base64'),'base64').equals(b),'base64');
        const n=Buffer.alloc(8);n.writeBigUInt64LE(12345678901234n);assert(n.readBigUInt64LE()===12345678901234n,'bigint');
        const fs=__apps.fs;await fs.promises.writeFile({path},b);assert((await fs.promises.readFile({path})).equals(b),'file');
        const handle=await fs.promises.open({path},'r');const small=Buffer.alloc(5);await handle.read(small,0,5,0);assert(small.toString()==='Hello','handle');const first=Buffer.alloc(1);await handle.read(first,0,1,null);assert(first.toString()==='H','positioned read preserves offset');await handle.close();
        const chunks=[];for await(const chunk of fs.createReadStream({path},{{highWaterMark:4}}))chunks.push(chunk);assert(Buffer.concat(chunks).equals(b),'read stream');
        const upper=new __apps.stream.Transform({{transform(chunk,encoding,callback){{callback(null,Buffer.from(chunk.toString().toUpperCase()));}}}});
        const out=[];const sink=new __apps.stream.Writable({{write(chunk,encoding,callback){{out.push(chunk.toString());callback();}}}});
        await __apps.stream.promises.pipeline(__apps.stream.Readable.from(['a','b']),upper,sink);assert(out.join('')==='AB','pipeline');
        await fs.promises.rename({path},{path}+'.moved');assert(!fs.existsSync({path}),'rename');await fs.promises.rm({path}+'.moved');
    "#
    ));
}

#[test]
fn node_buffer_matches_node() {
    run(r##"
        const {assert,buffer:{Buffer,transcode}}=__apps;
        const code=(fn)=>{try{fn();return 'none';}catch(error){return error.code+' '+error.message;}};
        assert.strictEqual(Buffer.from('aGk=aGk=','base64').toString('hex'),'6869');
        assert.strictEqual(Buffer.from('a!GV s\tbG8','base64').toString(),'hello');
        assert.strictEqual(Buffer.from('-_+/','base64').toString('hex'),'fbffbf');
        assert.strictEqual(Buffer.from('hello').toString('utf8',-2,100),'hello');
        assert.strictEqual(Buffer.from('hello').toString('utf8',3,1),'');
        assert.strictEqual(Buffer.from('ab','utf-16le').toString('hex'),'61006200');
        assert.strictEqual(Buffer.from('ab','UCS-2').toString('ucs2'),'ab');
        assert.strictEqual(Buffer.from([0,0x61,0,0x62,0]).subarray(1).toString('utf16le'),'ab');
        assert.strictEqual(Buffer.from(new String('hi')).toString(),'hi');
        assert.strictEqual(Buffer.from({[Symbol.toPrimitive](){return 'xy';}}).toString(),'xy');
        assert.strictEqual(Buffer.from(new ArrayBuffer(4),'x',2).length,2);
        assert.deepStrictEqual([Buffer.isEncoding('UTF8'),Buffer.isEncoding(''),Buffer.isEncoding('utf-16-le'),Buffer.isEncoding('Binary')],[true,false,false,true]);
        assert.deepStrictEqual([Buffer.byteLength('aGVs bG8=','base64'),Buffer.byteLength('abc','hex'),Buffer.byteLength('\ud800'),Buffer.byteLength('héllo','foo')],[6,1,3,6]);
        const write=(size,...args)=>{const b=Buffer.alloc(size);return [b.write(...args),b.toString('hex')];};
        assert.deepStrictEqual([write(3,'é€'),write(3,'abc','utf16le'),write(2,'aGVsbG8=','base64'),write(4,'abcd',1,'latin1')],[[2,'c3a900'],[2,'610000'],[2,'6865'],[3,'00616263']]);
        const copy=Buffer.alloc(3);assert.deepStrictEqual([Buffer.from('abc').copy(copy,1.5),copy.toString('hex'),Buffer.from('ab').copy(Buffer.alloc(3),5)],[2,'006162',0]);
        assert.deepStrictEqual([Buffer.from('abcabc').indexOf('b',-2),Buffer.from('abcabc').lastIndexOf('b',-2),Buffer.from('abc').lastIndexOf(''),Buffer.from('abc').indexOf(97+256)],[4,4,3,0]);
        assert.strictEqual(Buffer.alloc(5).fill('ab',1,4).toString('hex'),'0061626100');
        assert.deepStrictEqual([Buffer.from('ffffffffff7f','hex').readIntBE(0,6),Buffer.from('0100000000ff','hex').readIntLE(0,6)],[-129,-1099511627775]);
        assert.deepStrictEqual([
          code(()=>Buffer.from('hi','foo')),code(()=>Buffer.from('hi').toString(null)),code(()=>Buffer.alloc('5')),code(()=>Buffer.alloc(2).fill('zz','hex')),
          code(()=>Buffer.alloc(2).readUInt8(2)),code(()=>Buffer.alloc(8).readUInt32LE(1.5)),code(()=>Buffer.alloc(2).writeUInt8(300)),code(()=>Buffer.alloc(8).writeUIntLE(2**49,0,6)),
          code(()=>Buffer.alloc(8).writeBigInt64LE(2n**64n)),code(()=>Buffer.alloc(8).readUIntBE(0,7)),code(()=>Buffer.alloc(0).readUInt8(0)),code(()=>Buffer.from(new ArrayBuffer(4),5)),
          code(()=>Buffer.concat(['a'])),code(()=>Buffer.alloc(2).write('a',5)),code(()=>Buffer.from(5)),code(()=>Buffer.from('abc').indexOf({})),
        ],[
          'ERR_UNKNOWN_ENCODING Unknown encoding: foo','ERR_UNKNOWN_ENCODING Unknown encoding: null',`ERR_INVALID_ARG_TYPE The "size" argument must be of type number. Received type string ('5')`,"ERR_INVALID_ARG_VALUE The argument 'value' is invalid. Received 'zz'",
          'ERR_OUT_OF_RANGE The value of "offset" is out of range. It must be >= 0 and <= 1. Received 2','ERR_OUT_OF_RANGE The value of "offset" is out of range. It must be an integer. Received 1.5','ERR_OUT_OF_RANGE The value of "value" is out of range. It must be >= 0 and <= 255. Received 300','ERR_OUT_OF_RANGE The value of "value" is out of range. It must be >= 0 and < 2 ** 48. Received 562_949_953_421_312',
          'ERR_OUT_OF_RANGE The value of "value" is out of range. It must be >= -(2n ** 63n) and < 2n ** 63n. Received 18_446_744_073_709_551_616n','ERR_OUT_OF_RANGE The value of "byteLength" is out of range. It must be >= 1 and <= 6. Received 7','ERR_BUFFER_OUT_OF_BOUNDS Attempt to access memory outside buffer bounds','ERR_BUFFER_OUT_OF_BOUNDS "offset" is outside of buffer bounds',
          `ERR_INVALID_ARG_TYPE The "list[0]" argument must be an instance of Buffer or Uint8Array. Received type string ('a')`,'ERR_OUT_OF_RANGE The value of "offset" is out of range. It must be >= 0 && <= 2. Received 5','ERR_INVALID_ARG_TYPE The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object. Received type number (5)','ERR_INVALID_ARG_TYPE The "value" argument must be one of type number or string or an instance of Buffer or Uint8Array. Received an instance of Object',
        ]);
        assert.deepStrictEqual([transcode(Buffer.from('a€é'),'utf8','ascii').toString('hex'),transcode(Buffer.from('a€é'),'utf8','latin1').toString('hex'),transcode(Buffer.from('a€é'),'utf8','ucs2').toString('hex'),transcode(Buffer.from([0xe9,0x41]),'ascii','utf8').toString('hex')],['613f3f','613fe9','6100ac20e900','efbfbd41']);
        assert.strictEqual(code(()=>transcode(Buffer.from('a'),'utf8','hex')),'U_ILLEGAL_ARGUMENT_ERROR Unable to transcode Buffer [U_ILLEGAL_ARGUMENT_ERROR]');
        assert.deepStrictEqual(Buffer.copyBytesFrom(new Uint16Array([1,2,3]),1,1),Buffer.from([2,0]));
        const statics=[];for(const key in Buffer)statics.push(key);assert.ok(['from','alloc','isBuffer','byteLength','concat','of','copyBytesFrom'].every(key=>statics.includes(key)),statics.join());
        assert.deepStrictEqual([Buffer.alloc(0),Buffer.alloc(51)].map(b=>__apps.util.inspect(b).slice(-20)),['<Buffer >',' 00 ... 1 more byte>']);
    "##);
}

#[test]
fn node_fast_paths_match_node() {
    run(r##"
        const {assert,buffer:{Buffer},crypto,fs,util,os}=__apps;
        for (const [name, size, min, max] of [['UInt8', 1, 0, 255], ['Int8', 1, -128, 127], ['UInt16', 2, 0, 65535], ['Int16', 2, -32768, 32767], ['UInt32', 4, 0, 4294967295], ['Int32', 4, -2147483648, 2147483647]])
        for (const end of size === 1 ? [''] : ['LE', 'BE']) for (const value of [min, max, 0, min + 1, max - 1, ((min + max) / 3) | 0]) {
        const b = Buffer.alloc(size + 2);
        assert.strictEqual(b[`write${name}${end}`](value, 1), size + 1);
        assert.strictEqual(new DataView(b.buffer, b.byteOffset + 1)[`get${name.replace('UInt', 'Uint')}`](0, end === 'LE'), value, name + end);
        assert.strictEqual(b[`read${name}${end}`](1), value);
        }
        assert.throws(() => Buffer.alloc(4).readUInt32LE('0'), { code: 'ERR_INVALID_ARG_TYPE' });
        assert.throws(() => Buffer.alloc(4).readInt16BE(3), { code: 'ERR_OUT_OF_RANGE' });
        assert.throws(() => Buffer.alloc(4).writeInt32LE(2 ** 31), { code: 'ERR_OUT_OF_RANGE' });
        assert.deepStrictEqual([Buffer.from('abz', 'hex'), Buffer.from('abc', 'hex'), Buffer.from('zz00', 'hex')].map(b => b.toString('hex')), ['ab', 'ab', '']);
        assert.deepStrictEqual(['aGVsbG8', 'aGVs\nbG8=', 'A', 'aGVsbG8=aGk='].map(text => Buffer.from(text, 'base64').toString()), ['hello', 'hello', '', 'hello']);
        assert.strictEqual(Buffer.from('aGVsbG8', 'base64url').toString(), 'hello');
        assert.deepStrictEqual([Buffer.from('héllo'), Buffer.from('ab\0c'), Buffer.from('ÿ', 'latin1'), Buffer.from('é', 'ascii')].map(b => b.toString('hex')), ['68c3a96c6c6f', '61620063', 'ff', 'e9']);
        const write = (size, ...args) => { const b = Buffer.alloc(size); return [b.write(...args), b.toString('hex')]; };
        assert.deepStrictEqual([write(4, 'abcdef', 1), write(4, 'aé'), write(4, 'ab', 1, 1), write(4, 'abc', 'latin1')], [[3, '00616263'], [3, '61c3a900'], [1, '00610000'], [3, '61626300']]);
        assert.deepStrictEqual([Buffer.compare(Buffer.from('ab'), Buffer.from('abc')), Buffer.compare(Buffer.from('b'), Buffer.from('abc')), Buffer.from('x'.repeat(40)).compare(Buffer.from('x'.repeat(40))), Buffer.from('ab').equals(Buffer.from('ab'))], [-1, 1, 0, true]);
        const hello = Buffer.from('hello');
        assert.deepStrictEqual([hello.subarray(-3, -1).toString(), hello.subarray(4, 2).toString(), hello.slice(1).toString(), hello.subarray(1) instanceof Buffer, hello.subarray(NaN, Infinity).length], ['ll', '', 'ello', true, 5]);
        assert.strictEqual(crypto.createHash('sha256').update('héllo').update(Buffer.from('x')).digest('hex'), crypto.createHash('sha256').update(Buffer.from('héllox')).digest('hex'));
        assert.strictEqual(crypto.createHash('md5').update('a').digest('hex'), '0cc175b9c0f1b6a831c399e269772661');
        assert.strictEqual(crypto.createHmac('sha256', 'k').update('a').digest('base64'), 'eNqRUR5nVYf1ud94vt669VYNoqu4gWLuh13N90SVHZ4=');
        assert.deepStrictEqual([crypto.timingSafeEqual(Buffer.from('ab'), Buffer.from('ab')), crypto.timingSafeEqual(Buffer.from('ab'), Buffer.from('ac'))], [true, false]);
        assert.match(crypto.randomUUID(), /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
        const random = crypto.getRandomValues(new Uint32Array(80)); assert.ok(random.some(Boolean));
        assert.strictEqual(new URL('https://me@x.y/').password, '');
        assert.strictEqual(new URLSearchParams('a=%zz&b=%FF&c=+x').toString(), 'a=%25zz&b=%EF%BF%BD&c=+x');
        assert.strictEqual(new URLSearchParams({ q: "a b!'()~*" }).toString(), 'q=a+b%21%27%28%29%7E*');
        assert.strictEqual(util.inspect({ set: new Set([1]), map: new Map([[1, 2]]), date: new Date(0), re: /x/g, nested: { a: [1] } }), "{\n  set: Set(1) { 1 },\n  map: Map(1) { 1 => 2 },\n  date: 1970-01-01T00:00:00.000Z,\n  re: /x/g,\n  nested: { a: [ 1 ] }\n}");
        const order = [];
        await new Promise(resolve => {
        setImmediate(() => { order.push('i1'); Promise.resolve().then(() => order.push('m1')); process.nextTick(() => order.push('t1')); });
        const second = setImmediate(() => order.push('i2'));
        setImmediate(() => { clearImmediate(fourth); order.push('i3'); });
        const fourth = setImmediate(() => order.push('i4'));
        setTimeout(() => { order.push('t'); queueMicrotask(() => order.push('m2')); }, 0);
        setImmediate(resolve);
        clearImmediate(second);
        });
        assert.deepStrictEqual(order.filter(item => item !== 't' && item !== 'm2'), ['i1', 't1', 'm1', 'i3']);
        const path = os.tmpdir() + '/h2o-fast-' + process.pid + '.txt', expected = Array.from({ length: 100 }, (_, i) => String(i).padStart(3, '0')).join('');
        const stream = fs.createWriteStream(path); for (let i = 0; i < 100; i++) stream.write(String(i).padStart(3, '0'));
        await new Promise(resolve => stream.end(resolve));
        assert.strictEqual(fs.readFileSync(path, 'utf8'), expected); assert.strictEqual(stream.bytesWritten, 300);
        assert.deepStrictEqual([fs.existsSync(path), fs.existsSync(path + '.missing'), fs.statSync(path).size, fs.statSync(path).isFile()], [true, false, 300, true]);
    "##);
}

#[test]
fn node_path_matches_node_vectors() {
    run(r##"
        const {posix:P,win32:W}=__apps.path;
        const t=f=>{try{return f();}catch(error){return 'THROW '+error.code+' '+error.message;}};
        const cases=[
        [t(()=>[P.normalize('./'),P.normalize('//a//b/'),P.normalize('a/../../b'),P.normalize(''),P.join('/a','/b','../c'),P.join('',''),P.join()]),"[\"./\",\"/a/b/\",\"../b\",\".\",\"/a/c\",\".\",\".\"]"],
        [t(()=>[P.relative('/a/b','/'),P.relative('/foo/bar','/foo/barbaz'),P.relative('/a/b/c','/a/d'),P.relative('/','/a'),P.relative('/a','/a')]),"[\"../..\",\"../barbaz\",\"../../d\",\"a\",\"\"]"],
        [t(()=>[P.dirname(''),P.dirname('//a'),P.dirname('a//b//'),P.dirname('/'),P.dirname('a')]),"[\".\",\"//\",\"a/\",\"/\",\".\"]"],
        [t(()=>[P.basename('b.txt','b.txt'),P.basename('aaa','a'),P.basename('/a/b.txt','txt'),P.basename('a//'),P.basename('/'),P.basename('a/xt/','txt')]),"[\"\",\"aa\",\"b.\",\"a\",\"\",\"xt/\"]"],
        [t(()=>[P.extname('.a.b'),P.extname('..'),P.extname('a.'),P.extname('.bashrc'),P.extname('a.b/')]),"[\".b\",\"\",\".\",\"\",\".b\"]"],
        [t(()=>[P.parse('./a.b'),P.parse('/..'),P.parse('a/b/'),P.parse('/home/u/f.txt')]),"[{\"root\":\"\",\"dir\":\".\",\"base\":\"a.b\",\"ext\":\".b\",\"name\":\"a\"},{\"root\":\"/\",\"dir\":\"/\",\"base\":\"..\",\"ext\":\".\",\"name\":\".\"},{\"root\":\"\",\"dir\":\"a\",\"base\":\"b\",\"ext\":\"\",\"name\":\"b\"},{\"root\":\"/\",\"dir\":\"/home/u\",\"base\":\"f.txt\",\"ext\":\".txt\",\"name\":\"f\"}]"],
        [t(()=>[P.format({root:'/r/',base:'b'}),P.format({dir:'d',name:'n',ext:'e'}),P.isAbsolute(''),P.toNamespacedPath('/a')]),"[\"/r/b\",\"d/n.e\",false,\"/a\"]"],
        [t(()=>[W.normalize('//a//b/'),W.normalize('C:'),W.normalize('C:a\\b'),W.normalize('C:\\a\\..\\..\\b'),W.normalize('\\\\server\\share\\a\\..'),W.normalize('CON:'),W.normalize('ab:'),W.normalize('c:/x/y'),W.normalize('\\\\?\\C:\\a'),W.normalize('./')]),"[\"\\\\\\\\a\\\\b\\\\\",\"C:.\",\"C:a\\\\b\",\"C:\\\\b\",\"\\\\\\\\server\\\\share\\\\\",\".\\\\CON:.\",\".\\\\ab:\",\"c:\\\\x\\\\y\",\"\\\\\\\\?\\\\C:\\\\a\",\".\\\\\"]"],
        [t(()=>[W.resolve('C:\\a','\\b'),W.resolve('C:\\a','D:\\b','c'),W.resolve('\\\\?\\C:\\a','//'),W.resolve('\\\\server\\share','..\\x')]),"[\"C:\\\\b\",\"D:\\\\b\\\\c\",\"\\\\\\\\?\\\\\",\"\\\\\\\\server\\\\share\\\\x\"]"],
        [t(()=>[W.join('C:','a'),W.join('\\\\server\\share','x'),W.join('CON:','a/b/'),W.join('a\\','\\b'),W.join('//server','share')]),"[\"C:\\\\a\",\"\\\\\\\\server\\\\share\\\\x\",\"CON:\\\\a\\\\b\\\\\",\"a\\\\b\",\"\\\\\\\\server\\\\share\\\\\"]"],
        [t(()=>[W.dirname('C:a'),W.dirname('C:'),W.dirname('\\\\server\\share'),W.dirname('\\\\server\\share\\x'),W.dirname('C:\\')]),"[\"C:\",\"C:\",\"\\\\\\\\server\\\\share\",\"\\\\\\\\server\\\\share\\\\\",\"C:\\\\\"]"],
        [t(()=>[W.basename('C:'),W.basename('C:a'),W.basename('C:\\a\\b.txt','.txt'),W.extname('C:.txt'),W.isAbsolute('C:a'),W.isAbsolute('//server')]),"[\"\",\"a\",\"b\",\"\",false,true]"],
        [t(()=>[W.relative('C:\\a\\B','c:\\a\\b\\c'),W.relative('C:\\a','D:\\b'),W.relative('\\\\s\\h\\a','\\\\s\\h\\b'),W.toNamespacedPath('C:\\a'),W.toNamespacedPath('\\\\server\\share\\a')]),"[\"c\",\"D:\\\\b\",\"..\\\\b\",\"\\\\\\\\?\\\\C:\\\\a\",\"\\\\\\\\?\\\\UNC\\\\server\\\\share\\\\a\"]"],
        [t(()=>[W.parse('C:'),W.parse('\\\\server\\share\\x.y'),W.parse('C:\\path\\dir\\file.txt')]),"[{\"root\":\"C:\",\"dir\":\"C:\",\"base\":\"\",\"ext\":\"\",\"name\":\"\"},{\"root\":\"\\\\\\\\server\\\\share\\\\\",\"dir\":\"\\\\\\\\server\\\\share\\\\\",\"base\":\"x.y\",\"ext\":\".y\",\"name\":\"x\"},{\"root\":\"C:\\\\\",\"dir\":\"C:\\\\path\\\\dir\",\"base\":\"file.txt\",\"ext\":\".txt\",\"name\":\"file\"}]"],
        [t(()=>P.join('a',1)),"\"THROW ERR_INVALID_ARG_TYPE The \\\"path\\\" argument must be of type string. Received type number (1)\""],
        [t(()=>P.basename(null)),"\"THROW ERR_INVALID_ARG_TYPE The \\\"path\\\" argument must be of type string. Received null\""],
        [t(()=>W.resolve('a',{})),"\"THROW ERR_INVALID_ARG_TYPE The \\\"paths[1]\\\" argument must be of type string. Received an instance of Object\""],
        [t(()=>P.format(null)),"\"THROW ERR_INVALID_ARG_TYPE The \\\"pathObject\\\" argument must be of type object. Received null\""]
        
        ];
        cases.forEach(([actual,expected],index)=>{if(JSON.stringify(actual)!==expected)throw Error(`case ${index}: ${JSON.stringify(actual)} !== ${expected}`);});
    "##);
}

#[test]
fn node_string_decoder_url_querystring_and_util_match_node() {
    run(r##"
        const {string_decoder:{StringDecoder},util,url,querystring:qs,buffer:{Buffer}}=__apps;
        const show=(d,parts)=>[...parts.map(p=>p===null?d.end():d.write(Buffer.from(p))),d.lastNeed,d.lastTotal,d.lastChar.toString('hex')];
        const t=f=>{try{return f();}catch(error){return 'THROW '+error.code+' '+error.message;}};
        const cases=[
        [t(()=>show(new StringDecoder('utf8'),[[0xe2],[0x82]])),"[\"\",\"\",1,3,\"e2820000\"]"],
        [t(()=>show(new StringDecoder('utf8'),[[0xff,0xe2],null])),"[\"�\",\"�\",0,0,\"e2000000\"]"],
        [t(()=>show(new StringDecoder('utf8'),[[0xf0,0x9f,0x98,0x80,0xf0],[0x9f,0x98],[0x81,0x61]])),"[\"😀\",\"\",\"😁a\",0,0,\"f09f9881\"]"],
        [t(()=>show(new StringDecoder('utf8'),[[0x80,0x80],[0xc3],[0x28]])),"[\"��\",\"\",\"�(\",0,0,\"c3000000\"]"],
        [t(()=>show(new StringDecoder('utf16le'),[[0x3d],[0xd8,0x00],[0xde],null])),"[\"\",\"\\ud83d\",\"\\ude00\",\"\",0,0,\"00de0000\"]"],
        [t(()=>show(new StringDecoder('utf16le'),[[0x3d,0xd8],null])),"[\"\",\"\\ud83d\",0,0,\"3dd80000\"]"],
        [t(()=>show(new StringDecoder('ucs2'),[[0x3d,0xd8,0x00],[0xde,0x61],[0x00]])),"[\"\\ud83d\",\"\\ude00\",\"a\",0,0,\"61000000\"]"],
        [t(()=>show(new StringDecoder('base64'),[[1],[2,3,4],null])),"[\"\",\"AQID\",\"BA==\",0,0,\"04020300\"]"],
        [t(()=>show(new StringDecoder('base64url'),[[0xfb,0xff],null])),"[\"\",\"-_8\",0,0,\"fbff0000\"]"],
        [t(()=>show(new StringDecoder('hex'),[[1],[0xab]])),"[\"01\",\"ab\",0,0,\"00000000\"]"],
        [t(()=>show(new StringDecoder('latin1'),[[0xe9]])),"[\"é\",0,0,\"00000000\"]"],
        [t(()=>[new StringDecoder(null).encoding,new StringDecoder('UCS-2').encoding,new StringDecoder('binary').encoding,Object.keys(new StringDecoder())]),"[\"utf8\",\"utf16le\",\"latin1\",[\"encoding\"]]"],
        [t(()=>new StringDecoder('foo')),"\"THROW ERR_UNKNOWN_ENCODING Unknown encoding: foo\""],
        [t(()=>new StringDecoder().write(5)),"\"THROW ERR_INVALID_ARG_TYPE The \\\"buf\\\" argument must be an instance of Buffer, TypedArray, or DataView. Received type number (5)\""],
        [t(()=>qs.parse('a=1&b=2&c=3',null,null,{maxKeys:2})),"{\"a\":\"1\",\"b\":\"2\"}"],
        [t(()=>[qs.parse('a:1;b:2',';',':'),qs.parse('a=%E9&b=%zz'),qs.unescapeBuffer('a%20b+c%zz',true).toString(),qs.stringify({a:[1,2],b:'x y'},null,null),qs.escape("a b!'()*~")]),"[{\"a\":\"1\",\"b\":\"2\"},{\"a\":\"�\",\"b\":\"%zz\"},\"a b c%zz\",\"a=1&a=2&b=x%20y\",\"a%20b!'()*~\"]"],
        [t(()=>qs.escape('\ud800')),"\"THROW ERR_INVALID_URI URI malformed\""],
        [t(()=>[url.format({protocol:'https',hostname:'a.com',port:8080,pathname:'x y#?',query:{a:1,b:'c d'},hash:'h'}),url.format({protocol:'mailto',pathname:'a@b.com'}),url.format({protocol:'file',pathname:'/tmp'}),url.format({host:'a.com',auth:'u:p@x',slashes:true}),url.format({hostname:'::1',protocol:'http:'})]),"[\"https://a.com:8080/x y%23%3F?a=1&b=c%20d#h\",\"mailto:a@b.com\",\"file:///tmp\",\"//u:p%40x@a.com\",\"http://[::1]\"]"],
        [t(()=>[url.format(new URL('http://u:p@a.com/x?y#z'),{auth:false,search:false,fragment:false}),url.format(new URL('http://xn--espaol-zwa.com/a'),{unicode:true}),url.format('http://a.com/x')]),"[\"http://a.com/x\",\"http://español.com/a\",\"http://a.com/x\"]"],
        [t(()=>url.format(5)),"\"THROW ERR_INVALID_ARG_TYPE The \\\"urlObject\\\" argument must be one of type object or string. Received type number (5)\""],
        [t(()=>[url.domainToASCII('español.com'),url.domainToASCII('xn--iñvalid.com'),url.domainToUnicode('xn--espaol-zwa.com'),url.domainToUnicode('xn--iñvalid.com'),url.domainToASCII(''),url.domainToUnicode('example.COM')]),"[\"xn--espaol-zwa.com\",\"\",\"español.com\",\"\",\"\",\"example.com\"]"],
        [t(()=>(()=>{const m=new util.MIMEType('Text/HTML; Charset="utf-8"; x=y');m.params.set('a','b c');return [m.type,m.subtype,m.essence,m.params.get('charset'),m.params.get('none'),String(m),JSON.stringify(m),[...m.params.keys()]];})()),"[\"text\",\"html\",\"text/html\",\"utf-8\",null,\"text/html;charset=utf-8;x=y;a=\\\"b c\\\"\",\"\\\"text/html;charset=utf-8;x=y;a=\\\\\\\"b c\\\\\\\"\\\"\",[\"charset\",\"x\",\"a\"]]"],
        [t(()=>new util.MIMEType('text')),"\"THROW ERR_INVALID_MIME_SYNTAX The MIME syntax for a type in \\\"text\\\" is invalid\""],
        [t(()=>new util.MIMEParams().set('a b','c')),"\"THROW ERR_INVALID_MIME_SYNTAX The MIME syntax for a parameter name in \\\"a b\\\" is invalid at 1\""],
        [t(()=>[util.getSystemErrorMap().get(-2),util.getSystemErrorMessage(-13),util.getSystemErrorName(-2),util.getSystemErrorName(-99999),util.toUSVString('a\ud800b')]),"[[\"ENOENT\",\"no such file or directory\"],\"permission denied\",\"ENOENT\",\"Unknown system error -99999\",\"a�b\"]"],
        [t(()=>util.getSystemErrorName(2)),"\"THROW ERR_OUT_OF_RANGE The value of \\\"err\\\" is out of range. It must be a negative integer. Received 2\""],
        [t(()=>util.transferableAbortSignal({})),"\"THROW ERR_INVALID_ARG_TYPE The \\\"signal\\\" argument must be an instance of AbortSignal. Received an instance of Object\""],
        [t(()=>[util.types.isNativeError(Object.create(Error.prototype)),util.types.isNativeError(new TypeError()),util.types.isDate(Object.create(Date.prototype)),util.types.isMap(new (class extends Map {})()),util.types.isArgumentsObject((function(){return arguments;})()),util.types.isFloat16Array(new Float16Array(1)),util.types.isBoxedPrimitive(Object(1n)),util.types.isRegExp(RegExp.prototype),util.types.isSet(Set.prototype),util.types.isNumberObject({[Symbol.toStringTag]:'Number'})]),"[false,true,false,true,true,true,true,false,false,false]"],
        [t(()=>['isArgumentsObject','isBigInt64Array','isBigUint64Array','isExternal','isFloat16Array','isFloat32Array','isInt8Array','isKeyObject','isMapIterator','isProxy','isSetIterator','isSymbolObject','isUint8ClampedArray','isCryptoKey'].map(name=>typeof util.types[name])),"[\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\",\"function\"]"],
        [t(()=>[()=>__apps.assert.fail(),()=>__apps.assert.fail('x'),()=>__apps.assert.ok(0),()=>__apps.assert.strictEqual(1,2),()=>__apps.assert.throws(()=>{}),()=>__apps.assert.ifError('e')].map(f=>{try{f();}catch(error){return error.generatedMessage;}})),"[true,false,true,true,false,false]"],
        [t(()=>(({getColorDepth,hasColors})=>[typeof __apps.tty.ReadStream,getColorDepth({TERM:'xterm-256color'}),getColorDepth({}),getColorDepth({FORCE_COLOR:'3'}),getColorDepth({COLORTERM:'truecolor'}),getColorDepth({TERM:'screen'}),getColorDepth({NO_COLOR:'1',TERM:'xterm'}),getColorDepth({TERM_PROGRAM:'iTerm.app',TERM_PROGRAM_VERSION:'3.4'}),hasColors(256,{TERM:'xterm-256color'}),hasColors({TERM:'dumb'})])(__apps.tty.WriteStream.prototype)),"[\"function\",8,1,24,24,4,1,24,true,false]"]
        
        ];
        cases.forEach(([actual,expected],index)=>{if(JSON.stringify(actual)!==expected)throw Error(`case ${index}: ${JSON.stringify(actual)} !== ${expected}`);});
    "##);
}

#[test]
fn node_util_inspect_matches_node() {
    run(r##"
        const {util}=__apps,i=util.inspect;
        const t=f=>{try{return f();}catch(error){return 'THROW '+error.code+' '+error.message;}};
        const cases=[
        [t(()=>i(Array.from({length:30},(_,k)=>k*7))),"\"[\\n    0,   7,  14,  21,  28,  35,  42,  49,\\n   56,  63,  70,  77,  84,  91,  98, 105,\\n  112, 119, 126, 133, 140, 147, 154, 161,\\n  168, 175, 182, 189, 196, 203\\n]\""],
        [t(()=>i(Array.from({length:12},(_,k)=>'item'+k))),"\"[\\n  'item0',  'item1',\\n  'item2',  'item3',\\n  'item4',  'item5',\\n  'item6',  'item7',\\n  'item8',  'item9',\\n  'item10', 'item11'\\n]\""],
        [t(()=>i(Array.from({length:120},(_,k)=>k))),"\"[\\n   0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11,\\n  12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,\\n  24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,\\n  36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,\\n  48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59,\\n  60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,\\n  72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83,\\n  84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,\\n  96, 97, 98, 99,\\n  ... 20 more items\\n]\""],
        [t(()=>i(new Uint8Array(40))),"\"Uint8Array(40) [\\n  0, 0, 0, 0, 0, 0, 0, 0, 0, 0,\\n  0, 0, 0, 0, 0, 0, 0, 0, 0, 0,\\n  0, 0, 0, 0, 0, 0, 0, 0, 0, 0,\\n  0, 0, 0, 0, 0, 0, 0, 0, 0, 0\\n]\""],
        [t(()=>i((()=>{const a=[];a[5]=1;a[1e6]=2;return a;})())),"\"[ <5 empty items>, 1, <999994 empty items>, 2 ]\""],
        [t(()=>i([1,,3])),"\"[ 1, <1 empty item>, 3 ]\""],
        [t(()=>i((()=>{const o={a:1};o.self=o;return o;})())),"\"<ref *1> { a: 1, self: [Circular *1] }\""],
        [t(()=>i((()=>{const a={x:{}};a.x.y=a;const b=[a];b.push(b);return b;})())),"\"<ref *2> [ <ref *1> { x: { y: [Circular *1] } }, [Circular *2] ]\""],
        [t(()=>i(new ArrayBuffer(2))),"\"ArrayBuffer { [Uint8Contents]: <00 00>, [byteLength]: 2 }\""],
        [t(()=>i(new DataView(new ArrayBuffer(2)))),"\"DataView {\\n  [byteLength]: 2,\\n  [byteOffset]: 0,\\n  [buffer]: ArrayBuffer { [Uint8Contents]: <00 00>, [byteLength]: 2 }\\n}\""],
        [t(()=>i({d:new DataView(new ArrayBuffer(3),1)})),"\"{\\n  d: DataView {\\n    [byteLength]: 2,\\n    [byteOffset]: 1,\\n    [buffer]: ArrayBuffer { [Uint8Contents]: <00 00 00>, [byteLength]: 3 }\\n  }\\n}\""],
        [t(()=>i(new URL('http://a.com/b'))),"\"URL {\\n  href: 'http://a.com/b',\\n  origin: 'http://a.com',\\n  protocol: 'http:',\\n  username: '',\\n  password: '',\\n  host: 'a.com',\\n  hostname: 'a.com',\\n  port: '',\\n  pathname: '/b',\\n  search: '',\\n  searchParams: URLSearchParams {},\\n  hash: ''\\n}\""],
        [t(()=>i(new URLSearchParams('a=1&b=2'))),"\"URLSearchParams { 'a' => '1', 'b' => '2' }\""],
        [t(()=>i({a:{b:{c:{d:{e:1}}}}})),"\"{ a: { b: { c: [Object] } } }\""],
        [t(()=>i({a:1},{compact:false})),"\"{\\n  a: 1\\n}\""],
        [t(()=>i({a:{b:{c:1}},d:[1,2]},{compact:1})),"\"{\\n  a: {\\n    b: { c: 1 }\\n  },\\n  d: [ 1, 2 ]\\n}\""],
        [t(()=>i({a:'x'.repeat(30),b:'y'.repeat(30),c:{d:'z'.repeat(30)}},{compact:true})),"\"{ a: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx',\\n  b: 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy',\\n  c: { d: 'zzzzzzzzzzzzzzzzzzzzzzzzzzzzzz' } }\""],
        [t(()=>i({b:1,a:2},{sorted:true})),"\"{ a: 2, b: 1 }\""],
        [t(()=>i({a:'x'.repeat(50),b:'y'.repeat(50)},{breakLength:Infinity})),"\"{ a: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx', b: 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy' }\""],
        [t(()=>i({a:1,b:2,c:3,d:4},{breakLength:20})),"\"{\\n  a: 1,\\n  b: 2,\\n  c: 3,\\n  d: 4\\n}\""],
        [t(()=>i({[Symbol('s')]:3,'b-c':2,'$x':3,0:1})),"\"{ '0': 1, 'b-c': 2, '$x': 3, Symbol(s): 3 }\""],
        [t(()=>i((function(){return arguments;})(1,2))),"\"[Arguments] { '0': 1, '1': 2 }\""],
        [t(()=>i(new (class MyErr extends Error {})('m')).split('\n')[0]),"\"MyErr [Error]: m\""],
        [t(()=>i(Object.assign(new Error('withprops'),{code:'E1'})).split('\n').filter(l=>!l.includes(' at ')).join('|')),"\"Error: withprops|  code: 'E1'|}\""],
        [t(()=>i({err:new TypeError('tt')}).split('\n')[0]),"\"{\""],
        [t(()=>i((()=>{const e=new Error('m');e.stack=undefined;return e;})())),"\"[Error: m]\""],
        [t(()=>i([class A {},class B extends Map {},async function af(){},function*g(){},()=>{},function f(){}.bind(null)])),"\"[\\n  [class A],\\n  [class B extends Map],\\n  [AsyncFunction: af],\\n  [GeneratorFunction: g],\\n  [Function (anonymous)],\\n  [Function: bound f]\\n]\""],
        [t(()=>i([new String('ab'),Object(Symbol('s')),Object(5n),new Number(-0),Object.assign(new Date(0),{x:1}),Object.assign(/x/g,{y:1})])),"\"[\\n  [String: 'ab'],\\n  [Symbol: Symbol(s)],\\n  [BigInt: 5n],\\n  [Number: -0],\\n  1970-01-01T00:00:00.000Z { x: 1 },\\n  /x/g { y: 1 }\\n]\""],
        [t(()=>i([new Map([[{a:1},[1,2]]]),new Set([1,'x']),new WeakMap(),Object.create(null),Object.assign(Object.create(null),{a:1}),{[Symbol.toStringTag]:'X'}])),"\"[\\n  Map(1) { { a: 1 } => [ 1, 2 ] },\\n  Set(2) { 1, 'x' },\\n  WeakMap { <items unknown> },\\n  [Object: null prototype] {},\\n  [Object: null prototype] { a: 1 },\\n  { Symbol(Symbol.toStringTag): 'X' }\\n]\""],
        [t(()=>i((()=>{'use strict';return function named(){};})(),{showHidden:true})),"\"<ref *1> [Function: named] {\\n  [length]: 0,\\n  [name]: 'named',\\n  [prototype]: { [constructor]: [Circular *1] }\\n}\""],
        [t(()=>i({get a(){return 1;},get b(){return {c:2};}},{getters:true})),"\"{ a: [Getter: 1], b: [Getter] { c: 2 } }\""],
        [t(()=>i([1234567,1234.5678,12345678901234567890n],{numericSeparator:true})),"\"[ 1_234_567, 1_234.567_8, 12_345_678_901_234_567_890n ]\""],
        [t(()=>i('abc\n'.repeat(30))),"\"'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n' +\\n  'abc\\\\n'\""],
        [t(()=>i(["it's",'say "hi"','both \'"','\x00\x1b\x7f\u0085','\ud800','tab\t'])),"\"[\\n  \\\"it's\\\",\\n  'say \\\"hi\\\"',\\n  `both '\\\"`,\\n  '\\\\x00\\\\x1B\\\\x7F\\\\x85',\\n  '\\\\ud800',\\n  'tab\\\\t'\\n]\""],
        [t(()=>i({a:{[util.inspect.custom](depth,options){return `depth=${depth} opt=${options.depth} ${typeof options.stylize}`;}}})),"\"{ a: depth=1 opt=2 function }\""],
        [t(()=>i({a:1,b:'x',c:null,d:undefined},{colors:true})),"\"{ a: \\u001b[33m1\\u001b[39m, b: \\u001b[32m'x'\\u001b[39m, c: \\u001b[1mnull\\u001b[22m, d: \\u001b[90mundefined\\u001b[39m }\""],
        [t(()=>i([[[[[1]]]]])),"\"[ [ [ [Array] ] ] ]\""],
        [t(()=>i(new (class Foo{constructor(){this.x=1;}})())),"\"Foo { x: 1 }\""],
        [t(()=>util.format('%s %d %i %f %j %o %O %%','a',1.5,2.7,'3.5',{x:1},[1],{y:2})),"\"a 1.5 2 3.5 {\\\"x\\\":1} [ 1, [length]: 1 ] { y: 2 } %\""],
        [t(()=>[typeof util.inspect.colors,typeof util.inspect.styles,Object.keys(util.inspect).includes('defaultOptions'),util.inspect.defaultOptions.breakLength]),"[\"object\",\"object\",false,80]"]
        
        ];
        cases.forEach(([actual,expected],index)=>{if(JSON.stringify(actual)!==expected)throw Error(`case ${index}: ${JSON.stringify(actual)} !== ${expected}`);});
    "##);
}

#[test]
fn node_events_match_node() {
    run(r##"
        const {assert,events,util}=__apps,EventEmitter=events,sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
        const seen=[];const capturing=new EventEmitter({captureRejections:true});capturing.on('x',async()=>{throw new Error('boom');});capturing.on('error',error=>seen.push(error.message));capturing.emit('x');
        const custom=new EventEmitter({captureRejections:true});custom[events.captureRejectionSymbol]=(error,name)=>seen.push(`${error.message}:${name}`);custom.on('y',async()=>{throw new Error('s');});custom.emit('y');
        await sleep(5);assert.deepStrictEqual(seen,['boom','s:y']);
        assert.deepStrictEqual([events.captureRejections,typeof events.captureRejectionSymbol],[false,'symbol']);
        assert.throws(()=>new EventEmitter({captureRejections:1}),{code:'ERR_INVALID_ARG_TYPE',message:'The "options.captureRejections" property must be of type boolean. Received type number (1)'});
        assert.deepStrictEqual([events.getMaxListeners(new EventEmitter()),events.getMaxListeners(new EventEmitter().setMaxListeners(3)),events.getMaxListeners(new EventTarget())],[10,3,10]);
        const target=new EventTarget();events.setMaxListeners(5,target);assert.strictEqual(events.getMaxListeners(target),5);
        const controller=new AbortController();let aborted=0;const disposable=events.addAbortListener(controller.signal,()=>aborted++);controller.abort();assert.deepStrictEqual([aborted,typeof disposable[Symbol.dispose]],[1,'function']);
        assert.throws(()=>new EventEmitter().emit('error','boom'),{code:'ERR_UNHANDLED_ERROR',message:"Unhandled error. ('boom')"});
        assert.throws(()=>new EventEmitter().emit('error'),{code:'ERR_UNHANDLED_ERROR',message:'Unhandled error. (undefined)'});
        assert.throws(()=>new EventEmitter().off('a',1),{code:'ERR_INVALID_ARG_TYPE',message:'The "listener" argument must be of type function. Received type number (1)'});
        assert.throws(()=>new EventEmitter().setMaxListeners(-1),{code:'ERR_OUT_OF_RANGE',message:'The value of "setMaxListeners" is out of range. It must be >= 0. Received -1'});
        const app=function(){};for(const key of Object.getOwnPropertyNames(EventEmitter.prototype))Object.defineProperty(app,key,Object.getOwnPropertyDescriptor(EventEmitter.prototype,key));let calls=0;app.on('x',()=>calls++);app.emit('x');assert.deepStrictEqual([calls,app.listenerCount('x')],[1,1]);
        function Legacy(){}util.inherits(Legacy,EventEmitter);const legacy=new Legacy();let once=0;legacy.once('a',()=>once++);legacy.emit('a');legacy.emit('a');assert.deepStrictEqual([once,typeof legacy._events,legacy._eventsCount],[1,'object',0]);
        const shaped=new EventEmitter(),listener=()=>{};shaped.on('a',listener);const single=typeof shaped._events.a;shaped.on('a',listener);assert.deepStrictEqual([single,Array.isArray(shaped._events.a),shaped._eventsCount,Object.keys(new EventEmitter())],['function',true,1,['_events','_eventsCount','_maxListeners']]);
        const removed=[],emitter=new EventEmitter();emitter.on('removeListener',name=>removed.push(name));emitter.on('a',listener);emitter.on('a',listener);emitter.removeListener('a',listener);emitter.removeAllListeners('a');assert.deepStrictEqual([removed,emitter.eventNames()],[['a','a'],['removeListener']]);
        const sequence=[],prepended=new EventEmitter();prepended.on('a',()=>sequence.push(1));prepended.prependOnceListener('a',()=>sequence.push(0));prepended.emit('a');prepended.emit('a');assert.deepStrictEqual(sequence,[0,1,1]);
    "##);
}

#[test]
fn node_stream_helpers_match_node() {
    run(r##"
        const {assert,stream,buffer:{Buffer}}=__apps,{Readable,Writable,Duplex,PassThrough,Transform}=stream,sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
        const order=[],w=new Writable({write(chunk,encoding,callback){callback();}});
        w.on('finish',()=>order.push('finish'));w.end('x',()=>order.push('callback'));await sleep(5);
        let again;w.end(error=>{again=error.code;});await sleep(5);assert.deepStrictEqual([order,again],[['callback','finish'],'ERR_STREAM_ALREADY_FINISHED']);
        let destroyed;const d=new Writable({write(chunk,encoding,callback){setTimeout(callback,5);}});d.write('a');d.end(error=>{destroyed=error?.code;});d.destroy();await sleep(10);assert.strictEqual(destroyed,'ERR_STREAM_DESTROYED');
        const batches=[],v=new Writable({writev(chunks,callback){batches.push(chunks.length);callback();}});v.cork();v.write('a');v.write('b');v.uncork();await sleep(5);assert.deepStrictEqual(batches,[2]);
        assert.deepStrictEqual([stream.getDefaultHighWaterMark(false),stream.getDefaultHighWaterMark(true)],[65536,16]);
        stream.setDefaultHighWaterMark(true,32);assert.strictEqual(new Readable({objectMode:true,read(){}}).readableHighWaterMark,32);stream.setDefaultHighWaterMark(true,16);
        assert.throws(()=>stream.setDefaultHighWaterMark(false,-1),{code:'ERR_OUT_OF_RANGE'});
        const r=new Readable({read(){}});r.on('error',()=>{});assert.deepStrictEqual([stream.isReadable(r),stream.isReadable({}),stream.isErrored(r)],[true,null,false]);
        r.destroy(new Error('x'));await sleep(1);assert.deepStrictEqual([stream.isReadable(r),stream.isErrored(r)],[false,true]);
        const [a,b]=stream.duplexPair(),received=[];b.on('data',chunk=>received.push(chunk.toString()));a.write('hello');a.end();await new Promise(resolve=>b.on('end',resolve));assert.deepStrictEqual(received,['hello']);
        const upper=stream.compose(new Transform({transform(chunk,encoding,callback){callback(null,chunk.toString().toUpperCase());}}),async function*(source){for await(const chunk of source)yield chunk+'!';});
        const composed=[];upper.on('data',chunk=>composed.push(chunk.toString()));upper.write('ab');upper.end('cd');await new Promise(resolve=>upper.on('end',resolve));assert.deepStrictEqual(composed,['AB!','CD!']);
        const lengths=Duplex.from(async function*(source){for await(const chunk of source)yield String(chunk).length;}),sizes=[];lengths.on('data',size=>sizes.push(size));lengths.write('abc');lengths.end('de');await new Promise(resolve=>lengths.on('end',resolve));assert.deepStrictEqual(sizes,[3,2]);
        const collect=async source=>{const items=[];for await(const item of source)items.push(item);return items;};
        assert.deepStrictEqual(await collect(Duplex.from(['x','y'])),['x','y']);
        assert.deepStrictEqual(await collect(Duplex.from(Promise.resolve('p'))),['p']);
        assert.deepStrictEqual(await collect(Duplex.from({readable:Readable.from(['r']),writable:new PassThrough()})),['r']);
        assert.throws(()=>Duplex.from(5),{code:'ERR_INVALID_ARG_TYPE'});
        const utf16=new Readable({read(){}});utf16.setEncoding('utf16le');for(const part of [[0x61,0],[0x3d],[0xd8,0x00,0xde]])utf16.push(Buffer.from(part));utf16.push(null);assert.strictEqual((await collect(utf16)).join(''),'a\u{1F600}');
    "##);
}

#[test]
fn bridge_preload_calls_main_and_publishes_events() {
    let api = JavaScriptCore::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    main.eval("__apps.ipcMain.handle('sum',(_e,a,b)=>a+b);__apps.ipcMain.handle('double',(_e,v)=>v.map(x=>x*2));__apps.ipcMain.on('version',e=>e.returnValue='v1')","test").unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('desktop',{sum:(a,b)=>__apps.ipcRenderer.invoke('sum',a,b),double:v=>__apps.ipcRenderer.invoke('double',v),version:__apps.ipcRenderer.sendSync('version'),subscribe:callback=>{const listener=(_event,data)=>callback(data);__apps.ipcRenderer.on('change',listener);return()=>__apps.ipcRenderer.off('change',listener);}});","test").unwrap();
    let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
    assert_eq!(page.eval("desktop.version", "test").unwrap(), "v1");
    page.eval(
        "globalThis.result='pending';desktop.sum(3,4).then(v=>result=v,e=>result=String(e));globalThis.bytes='pending';desktop.double(new Float32Array([1.5,-2,0.25])).then(v=>bytes=v.constructor.name+':'+[...v],e=>bytes=String(e));",
        "test",
    )
    .unwrap();
    for _ in 0..10 {
        main.tick().unwrap();
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("result", "test").unwrap(), "7");
    assert_eq!(page.eval("bytes", "test").unwrap(), "Float32Array:3,-4,0.5");
    page.eval("globalThis.observed=[];globalThis.unsubscribe=desktop.subscribe(value=>observed.push(value));if(typeof unsubscribe!=='function')throw Error('unsubscribe must be synchronous');", "test").unwrap();
    main.eval("__apps.send(1,'change',{value:42});", "test")
        .unwrap();
    assert_eq!(page.eval("observed.length", "test").unwrap(), "0");
    preload.tick().unwrap();
    assert_eq!(page.eval("observed[0].value", "test").unwrap(), "42");
    page.eval("unsubscribe();", "test").unwrap();
    main.eval("__apps.send(1,'change',{value:43});", "test")
        .unwrap();
    preload.tick().unwrap();
    assert_eq!(page.eval("observed.length", "test").unwrap(), "1");
    assert_eq!(
        page.eval("typeof process+':'+typeof __apps", "test")
            .unwrap(),
        "undefined:undefined"
    );
}

#[test]
fn bridge_clones_files_and_preload_reads_dropped_paths() {
    let api = JavaScriptCore::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    host.borrow_mut()
        .dropped
        .insert(1, vec!["/tmp/first.txt".into(), "/tmp/notes.txt".into()]);
    let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    preload.eval("const {contextBridge,webUtils}=__apps;contextBridge.exposeInMainWorld('desktop',{path:file=>webUtils.getPathForFile(file),describe:file=>[file.constructor.name,file.name,file.type,file.size,file.lastModified],echo:value=>value});", "test").unwrap();
    let page = JsContext::new(api, host, Role::Renderer(1)).unwrap();
    page.eval("document.addEventListener('dragover',event=>event.preventDefault());document.addEventListener('drop',event=>globalThis.dropped=event.dataTransfer.files[0]);__appsDrag('enter',0,0,['notes.txt']);__appsDrop([{name:'notes.txt',file:1,bytes:[104,105],lastModified:7}],0,0);", "test").unwrap();
    assert_eq!(
        page.eval("typeof __appsFiles+':'+('path' in dropped)", "test")
            .unwrap(),
        "undefined:false"
    );
    assert_eq!(
        page.eval("desktop.path(dropped)", "test").unwrap(),
        "/tmp/notes.txt"
    );
    assert_eq!(
        page.eval("desktop.path(new File(['x'],'x.txt'))", "test")
            .unwrap(),
        ""
    );
    assert_eq!(
        page.eval("desktop.describe(dropped).join()", "test")
            .unwrap(),
        "File,notes.txt,text/plain,2,7"
    );
    assert_eq!(
        page.eval(
            "desktop.describe(new Blob(['hello'],{type:'text/plain'})).join()",
            "test"
        )
        .unwrap(),
        "Blob,,text/plain,5,"
    );
    assert_eq!(
        page.eval("desktop.path(desktop.echo(dropped))", "test")
            .unwrap(),
        "/tmp/notes.txt"
    );
    assert!(
        page.eval(
            "(()=>{try{desktop.path({})}catch(e){return e.message}})()",
            "test"
        )
        .unwrap()
        .contains("Expected a File")
    );
    assert_eq!(
        main.eval("__apps.webUtils.getPathForFile(new File([],'a'))", "test")
            .unwrap(),
        ""
    );
}

#[test]
fn bridge_capabilities_failures_and_closed_contexts() {
    let api = JavaScriptCore::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = JsContext::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    main.eval("__apps.ipcMain.handle('wait',()=>new Promise(()=>{}));const receive=__appsBridgeReceive;__appsBridgeReceive=message=>{globalThis.lastMessage=message;return receive(message)};", "test").unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('desktop',{value:()=>7,callback:fn=>fn(),fail:()=>Promise.reject(0),wait:()=>__apps.ipcRenderer.invoke('wait')});", "test").unwrap();
    let token = host.borrow().bridge[&1]["desktop"].value["items"]["value"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        token.len() > 30,
        "Callback capabilities must not use guessable counters"
    );
    let denied=preload.eval(&format!("__appsBridgeReceive({{kind:'call',function:{},sender:{{renderer:2}},args:{{type:'array',id:0,items:[]}}}})",serde_json::to_string(&token).unwrap()),"test").unwrap_err();
    assert!(denied.to_string().contains("unavailable to this context"));
    let page = JsContext::new(api.clone(), host.clone(), Role::Renderer(1)).unwrap();
    assert_eq!(page.eval("desktop.value()", "test").unwrap(), "7");
    assert!(page.eval("(()=>{try{desktop.callback(()=>{throw Error('callback failed')})}catch(e){return e.message}})()","test").unwrap().ends_with("callback failed"));
    page.eval(
        "globalThis.rejected='pending';desktop.fail().then(()=>rejected='wrong',e=>rejected=e)",
        "test",
    )
    .unwrap();
    for _ in 0..3 {
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("rejected", "test").unwrap(), "0");
    page.eval(
        "globalThis.result='pending';desktop.wait().then(v=>result=v,e=>result=e.message)",
        "test",
    )
    .unwrap();
    let request = main.eval("lastMessage.id", "test").unwrap();
    preload.eval(&format!("__appsBridgeReceive({{kind:'reply',id:{},sender:{{renderer:1}},value:{{type:'value',value:'forged'}}}})",serde_json::to_string(&request).unwrap()),"test").unwrap();
    preload.tick().unwrap();
    page.tick().unwrap();
    assert_eq!(
        page.eval("result", "test").unwrap(),
        "pending",
        "Replies must come from the called context"
    );
    drop(main);
    for _ in 0..3 {
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(page.eval("result", "test").unwrap(), "Bridge target closed");
    drop(page);
    let page = JsContext::new(api, host.clone(), Role::Renderer(1)).unwrap();
    assert_eq!(
        page.eval("desktop.value()", "test").unwrap(),
        "7",
        "Published APIs survive page reload"
    );
    drop(preload);
    assert!(
        !host.borrow().bridge.contains_key(&1),
        "Closed preload exposures are removed"
    );
    assert!(
        page.eval("desktop.value()", "test")
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
}

fn contexts() -> (Rc<RefCell<Host>>, JsContext, JsContext) {
    let api = JavaScriptCore::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><p id='x'>before</p><span>a</span><span>b</span></body>"),
            dirty: false,
        },
    );
    let main = JsContext::new(api.clone(), host.clone(), Role::Main).unwrap();
    main.eval(
        "__appsEvent('ready');globalThis.win=new __apps.BrowserWindow();globalThis.log=[];",
        "test",
    )
    .unwrap();
    let preload = JsContext::new(api, host.clone(), Role::Preload(1)).unwrap();
    (host, main, preload)
}

#[test]
fn ipc_is_asynchronous_and_reports_errors_like_electron() {
    let (_host, main, preload) = contexts();
    main.eval(r#"
        const {ipcMain} = __apps;
        process.on('uncaughtException', error => log.push('uncaught:' + error.message));
        ipcMain.on('bad', () => { throw new Error('listener bug'); });
        ipcMain.on('ping', (e, i) => { log.push('ping' + i); if (i < 3000) e.reply('pong', i + 1); });
        ipcMain.on('sender', e => log.push([e.sender === win.webContents, __apps.BrowserWindow.fromWebContents(e.sender) === win, e.sender.id, typeof e.sender.getURL, e.processId === process.pid, e.frameId, typeof e.senderFrame.url].join()));
        ipcMain.on('sync', (e, v) => { e.returnValue = v * 2; });
        ipcMain.handle('throws', () => { throw new TypeError('boom'); });
        ipcMain.handle('rejects', async () => { throw new Error('async boom'); });
        ipcMain.handle('fn', () => () => 1);
    "#, "test").unwrap();
    preload
        .eval(
            r#"
        const {ipcRenderer} = __apps, order = [];
        globalThis.results = order;
        ipcRenderer.on('pong', (e, i) => ipcRenderer.send('ping', i));
        ipcRenderer.send('bad');
        ipcRenderer.send('error', 1);
        ipcRenderer.send('sender');
        ipcRenderer.send('ping', 0);
    "#,
            "test",
        )
        .unwrap();
    assert_eq!(
        main.eval("log.length", "test").unwrap(),
        "0",
        "listeners must not run inside the sender's call"
    );
    preload.eval(r#"
        results.push('after send', ipcRenderer.sendSync('sync', 21));
        for (const channel of ['throws', 'rejects', 'missing', 'fn']) ipcRenderer.invoke(channel).then(() => results.push(channel + ': resolved'), error => results.push(error.message));
        for (const value of [() => 1, Symbol('x'), Promise.resolve(1)]) try { ipcRenderer.send('x', { value }); results.push('sent'); } catch (error) { results.push(error.message); }
    "#, "test").unwrap();
    assert_eq!(
        main.eval("log.length", "test").unwrap(),
        "3",
        "sendSync runs after the messages sent before it"
    );
    for _ in 0..4000 {
        main.tick().unwrap();
        preload.tick().unwrap();
    }
    assert_eq!(
        main.eval("log.slice(0, 3).join('|')", "test").unwrap(),
        "uncaught:listener bug|true,true,1,function,true,1,string|ping0"
    );
    assert_eq!(main.eval("log.at(-1)", "test").unwrap(), "ping3000");
    assert_eq!(
        preload.eval("results.join('|')", "test").unwrap(),
        "after send|42|An object could not be cloned.|An object could not be cloned.|An object could not be cloned.|Error invoking remote method 'throws': TypeError: boom|Error invoking remote method 'missing': Error: No handler registered for 'missing'|Error invoking remote method 'fn': Error: An object could not be cloned.|Error invoking remote method 'rejects': Error: async boom"
    );
}

#[test]
fn ipc_values_follow_structured_clone() {
    let (_host, main, preload) = contexts();
    main.eval("__apps.ipcMain.handle('echo',(e,v)=>v)", "test")
        .unwrap();
    preload.eval(r#"
        globalThis.out = 'pending';
        const sent = [[1,,3], new Array(2), new TypeError('t'), new RangeError('r'), -0, new Date(5), new Map([[1, {a: 2}]]), new Set([3]), new Uint16Array([1, 2]), undefined, 10n, NaN, {nested: {list: [1, 2]}}];
        __apps.ipcRenderer.invoke('echo', sent).then(v => {
            out = [v[0].length, 1 in v[0], v[1].length, 0 in v[1], v[2] instanceof TypeError, v[2].name, v[2].message, v[3] instanceof RangeError, Object.is(v[4], -0), v[5] instanceof Date && v[5].getTime(), v[6].get(1).a, v[7].has(3), v[8] instanceof Uint16Array && v[8][1], v[9] === undefined && 9 in v, typeof v[10], Number.isNaN(v[11]), v[12].nested.list[1]].join();
        }, error => { out = error.message; });
    "#, "test").unwrap();
    for _ in 0..3 {
        main.tick().unwrap();
        preload.tick().unwrap();
    }
    assert_eq!(
        preload.eval("out", "test").unwrap(),
        "3,false,2,false,true,TypeError,t,true,true,5,2,true,2,true,bigint,true,2"
    );
}

#[test]
fn context_bridge_exposes_late_values_promises_and_the_page_dom() {
    let (host, main, preload) = contexts();
    main.eval(
        "__apps.ipcMain.on('dom', (e, text) => log.push(text))",
        "test",
    )
    .unwrap();
    preload.eval(r#"
        const {contextBridge, ipcRenderer} = __apps;
        contextBridge.exposeInMainWorld('early', { ready: Promise.resolve(5), fail: () => { throw new TypeError('preload boom'); }, later: () => Promise.reject(new RangeError('no')) });
        setTimeout(() => contextBridge.exposeInMainWorld('timer', { value: 1 }), 0);
        window.addEventListener('DOMContentLoaded', () => {
            const node = document.getElementById('x');
            node.innerText = process.versions.node;
            node.classList.add('done');
            ipcRenderer.send('dom', [node.textContent, document.querySelectorAll('span').length, node.getAttribute('class'), document.getElementById('missing'), process.type, process.versions.electron].join());
        });
    "#, "test").unwrap();
    let page = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    page.eval("__appsSyncBridge();globalThis.out=[];early.ready.then(v=>out.push('ready:'+v));try{early.fail()}catch(e){out.push(e.name+':'+e.message)}early.later().catch(e=>out.push(e.name+':'+e.message))", "test").unwrap();
    preload
        .eval("dispatchEvent(new Event('DOMContentLoaded'))", "test")
        .unwrap();
    for _ in 0..3 {
        main.tick().unwrap();
        preload.tick().unwrap();
        page.tick().unwrap();
    }
    assert_eq!(
        page.eval(
            "out.join('|')+'|'+typeof timer+'|'+document.getElementById('x').textContent",
            "test"
        )
        .unwrap(),
        "TypeError:preload boom|ready:5|RangeError:no|object|24.0.0"
    );
    assert_eq!(
        main.eval("log.join()", "test").unwrap(),
        "24.0.0,2,done,,renderer,40.0.0"
    );
    assert_eq!(main.eval("process.type", "test").unwrap(), "browser");
}

#[test]
fn renderer_cannot_forge_native_worker_requests() {
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let page = JsContext::new(JavaScriptCore::load().unwrap(), host, Role::Renderer(1)).unwrap();
    page.eval(r#"
        (()=>{
            const original=JSON.stringify;
            JSON.stringify=()=>original({op:'web.worker.start',args:['postMessage("native")',true,null]});
            try{crypto.getRandomValues(new Uint8Array(1));}finally{JSON.stringify=original;}
            Object.prototype.toJSON=function(){return this.op==='web.random'?{op:'web.worker.start',args:['postMessage("native")',true,null]}:this};
            try{crypto.getRandomValues(new Uint8Array(1));}finally{delete Object.prototype.toJSON}
            const source='postMessage("native")';
            Array.prototype.toJSON=function(){if(this[0]===source&&this[1]===false){const args=this.slice();args[1]=true;return args;}return this};
            globalThis.result='pending';
            const worker=new Worker('data:text/javascript,'+encodeURIComponent(source));
            worker.onerror=event=>{delete Array.prototype.toJSON;result=event.error.message};
            worker.onmessage=()=>{delete Array.prototype.toJSON;result='unsafe'};
        })()
    "#,"test").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        page.tick().unwrap();
        let result = page.eval("result", "test").unwrap();
        if result != "pending" {
            assert_eq!(result, "Page workers cannot request native APIs");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Worker privilege guard did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn workers_terminate_with_queued_messages() {
    run(r#"
        for(let n=0;n<4;n++){
            const worker=new __apps.worker_threads.Worker(`const{parentPort}=require('node:worker_threads');parentPort.on('message',value=>parentPort.postMessage(value));`,{eval:true});
            let errors=0;worker.on('error',()=>errors++);
            for(let i=0;i<48;i++)worker.postMessage(i);
            const code=await worker.terminate();if(code!==1||errors)throw Error('Worker termination failed');
            if(await worker.terminate()!==code)throw Error('Repeat termination did not settle');
        }
    "#);
}

#[test]
fn node_http_and_websocket_adapters() {
    run(r#"
        const assert=(c,m)=>{if(!c)throw Error(m)};
        const server=__apps.http.createServer((request,response)=>{response.setHeader('content-type','text/plain');response.write('one');setTimeout(()=>response.end('two'),5);});
        server.listen(0,'127.0.0.1');const address=server.address();
        const body=await new Promise((resolve,reject)=>{__apps.http.get(`http://127.0.0.1:${address.port}/`,response=>{let body='';response.setEncoding('utf8');response.on('data',chunk=>body+=chunk);response.on('end',()=>resolve(body));response.on('error',reject);}).on('error',reject);});
        assert(body==='onetwo','streamed HTTP');
        const wss=new __apps.ws.WebSocketServer({server});wss.on('connection',ws=>ws.on('message',data=>ws.send(data)));
        const ws=new __apps.ws.WebSocket(`ws://127.0.0.1:${address.port}/`);
        const result=await new Promise((resolve,reject)=>{ws.on('open',()=>ws.send('echo'));ws.on('message',data=>resolve(data.toString()));ws.on('error',reject);});
        assert(result==='echo','WebSocket adapter');ws.close();wss.close();server.close();
    "#);
}

#[test]
fn node_http_classes_ports_and_errors() {
    run(r#"
        const {assert,http,https,stream}=__apps,net=__apps.module.createRequire()('net');
        assert.ok(Object.create(http.IncomingMessage.prototype) instanceof stream.Readable&&Object.create(http.ServerResponse.prototype) instanceof http.OutgoingMessage);
        assert.ok(http.createServer() instanceof net.Server&&http.maxHeaderSize===16384&&http.STATUS_CODES[418]==="I'm a Teapot"&&Object.keys(http.STATUS_CODES).length===63&&http.METHODS.length===35);
        assert.throws(()=>http.validateHeaderName('a b'),{code:'ERR_INVALID_HTTP_TOKEN',message:'Header name must be a valid HTTP token ["a b"]'});
        assert.throws(()=>http.validateHeaderValue('a','x\ny'),{code:'ERR_INVALID_CHAR'});assert.throws(()=>http.validateHeaderValue('a',undefined),{code:'ERR_HTTP_INVALID_HEADER_VALUE'});
        assert.throws(()=>https.createServer(),/HTTPS servers are not supported/);
        assert.throws(()=>http.createServer().listen(70000),{code:'ERR_SOCKET_BAD_PORT',message:'options.port should be >= 0 and < 65536. Received type number (70000).'});
        http.ServerResponse.prototype.sendPatched=function(text){this.setHeader('X-Patched','1');this.end(text);};
        const server=http.createServer((req,res)=>{
            Object.setPrototypeOf(res,Object.create(http.ServerResponse.prototype,{extra:{value:'yes'}}));
            assert.ok(req instanceof http.IncomingMessage&&res instanceof http.ServerResponse&&typeof req.socket.remoteAddress==='string'&&!res.headersSent);
            if(req.url==='/patched')return res.sendPatched('patched:'+res.extra);
            if(req.url==='/204'){res.writeHead(204);return res.end();}
            if(req.url==='/stream'){res.writeHead(200,{'X-A':['1','2']});res.write('a');return setTimeout(()=>res.end('b'),5);}
            let body='';req.setEncoding('utf8');req.on('data',chunk=>body+=chunk);req.on('end',()=>{
                res.setHeader('Set-Cookie',['a=1','b=2']);res.setHeader('X-Num',5);
                assert.deepStrictEqual(res.getHeaders(),Object.assign(Object.create(null),{'set-cookie':['a=1','b=2'],'x-num':5}));assert.ok(res.hasHeader('X-NUM'));res.removeHeader('x-num');
                res.statusCode=201;res.end(req.method+' '+req.url+' '+req.headers['x-test']+' '+body);
                assert.ok(res.headersSent);assert.throws(()=>res.setHeader('a','b'),{code:'ERR_HTTP_HEADERS_SENT'});
            });
        });
        await new Promise(resolve=>server.listen(String(0),'127.0.0.1',resolve));
        const port=server.address().port;
        const fetch=(options,body)=>new Promise((resolve,reject)=>{const req=http.request({host:'127.0.0.1',port,...options},res=>{let data='';res.setEncoding('utf8');res.on('data',chunk=>data+=chunk);res.on('end',()=>resolve({status:res.statusCode,message:res.statusMessage,headers:res.headers,data}));});assert.ok(req instanceof http.ClientRequest);req.on('error',reject);req.end(body);});
        const posted=await fetch({method:'POST',path:'/p?q=1',headers:{'X-Test':'yes'}},'hello');
        assert.deepStrictEqual([posted.status,posted.message,posted.data,posted.headers['set-cookie'],posted.headers['x-num']],[201,'Created','POST /p?q=1 yes hello',['a=1','b=2'],undefined]);
        const streamed=await fetch({path:'/stream'});assert.deepStrictEqual([streamed.data,streamed.headers['x-a']],['ab','1, 2']);
        assert.strictEqual((await fetch({path:'/patched'})).data,'patched:yes');
        assert.deepStrictEqual(await fetch({path:'/204'}).then(r=>[r.status,r.data]),[204,'']);
        const busy=http.createServer().listen({port:String(port),host:'127.0.0.1'}),inUse=await new Promise(resolve=>busy.on('error',resolve));
        assert.strictEqual(inUse.message,`listen EADDRINUSE: address already in use 127.0.0.1:${port}`);assert.ok(inUse.code==='EADDRINUSE'&&inUse.syscall==='listen'&&inUse.port===port);
        await new Promise(resolve=>server.close(resolve));
        assert.strictEqual(await new Promise(resolve=>server.close(resolve)).then(error=>error.code),'ERR_SERVER_NOT_RUNNING');
        const refused=await new Promise(resolve=>http.get(`http://127.0.0.1:${port}/`).on('error',resolve));
        assert.ok(refused.code==='ECONNREFUSED'&&refused.syscall==='connect'&&refused.message===`connect ECONNREFUSED 127.0.0.1:${port}`);
        const tcp=net.createServer(socket=>socket.end('bye'));await new Promise(resolve=>tcp.listen(String(port),'127.0.0.1',resolve));
        assert.deepStrictEqual(tcp.address(),{address:'127.0.0.1',family:'IPv4',port});
        const reply=await new Promise((resolve,reject)=>{let data='';const socket=net.connect(String(port),'127.0.0.1');socket.setEncoding('utf8');socket.on('data',chunk=>data+=chunk);socket.on('error',reject);socket.on('close',hadError=>resolve([data,hadError]));});
        assert.deepStrictEqual(reply,['bye',false]);
        await new Promise(resolve=>tcp.close(resolve));
        const failed=await new Promise(resolve=>{const socket=net.connect({port,host:'127.0.0.1'}),events=[];socket.on('error',error=>events.push(error));socket.on('close',hadError=>resolve([...events,hadError]));});
        assert.ok(failed[0].code==='ECONNREFUSED'&&failed[0].errno<0&&failed[0].address==='127.0.0.1'&&failed[0].port===port&&failed[1]===true);
        assert.deepStrictEqual([net.isIP('fe80::1%lo0'),net.isIP('1.2.3.4%x')],[6,0]);
    "#);
}

#[test]
fn node_http_close_finishes_requests_and_streams_large_bodies() {
    run(r#"
        const {assert,http}=__apps,listen=server=>new Promise(resolve=>server.listen(0,'127.0.0.1',()=>resolve(`http://127.0.0.1:${server.address().port}`)));
        const loopback=http.createServer((req,res)=>{res.end('<html>done</html>');loopback.close();}),events=[];
        loopback.on('close',()=>events.push('close'));
        assert.strictEqual(await fetch(await listen(loopback)+'/callback').then(r=>r.text()),'<html>done</html>');
        while(!events.length)await new Promise(resolve=>setTimeout(resolve,5));assert.ok(!loopback.listening);
        let release;const slow=http.createServer((req,res)=>{release=()=>res.end('late');});const url=await listen(slow);
        const pending=fetch(url).then(r=>r.text());while(!release)await new Promise(resolve=>setTimeout(resolve,5));
        let closed=false;slow.close(()=>closed=true);await new Promise(resolve=>setTimeout(resolve,20));
        assert.ok(!closed,'close waits for requests in flight');release();
        assert.strictEqual(await pending,'late');while(!closed)await new Promise(resolve=>setTimeout(resolve,5));
        const upload=http.createServer((req,res)=>{let size=0;req.on('data',chunk=>size+=chunk.length);req.on('end',()=>res.end(String(size)));});
        assert.strictEqual(await fetch(await listen(upload),{method:'POST',body:new Uint8Array(20<<20)}).then(r=>r.text()),String(20<<20));
        upload.close();
    "#);
}

#[test]
fn electron_net_requests_fetch_and_online_state() {
    run(r#"
        const {assert,http,net}=__apps;
        const server=http.createServer((req,res)=>{
            if(req.url==='/moved'){res.writeHead(302,{Location:'/echo'});return res.end();}
            let body='';req.setEncoding('utf8');req.on('data',chunk=>body+=chunk);req.on('end',()=>res.end(req.method+' '+req.url+' '+req.headers['x-test']+' '+body));
        });
        await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
        const port=server.address().port,base=`http://127.0.0.1:${port}`;
        const send=(options,body)=>new Promise((resolve,reject)=>{const request=net.request(options),events=[];request.on('response',response=>{let data='';response.on('data',chunk=>data+=chunk);response.on('end',()=>resolve([response.statusCode,data,events]));});request.on('finish',()=>events.push('finish'));request.on('error',reject);request.setHeader('X-Test','yes');assert.strictEqual(request.getHeader('x-test'),'yes');assert.strictEqual(request.end(body),request);});
        assert.deepStrictEqual(await send({method:'POST',url:base+'/echo'},'hello'),[200,'POST /echo yes hello',['finish']]);
        assert.deepStrictEqual(await send({hostname:'127.0.0.1',port,path:'/moved'}),[200,'GET /echo yes ',['finish']]);
        const failed=await new Promise(resolve=>{const request=net.request({url:base+'/moved',redirect:'error'});request.on('error',resolve);request.on('response',()=>resolve(null));request.end();});
        assert.ok(failed instanceof Error);
        assert.throws(()=>net.request({url:base,redirect:'manual'}),/Unsupported redirect mode/);
        assert.strictEqual(await (await net.fetch(base+'/echo',{method:'PUT',body:'x',headers:{'X-Test':'f'}})).text(),'PUT /echo f x');
        assert.strictEqual(typeof net.isOnline(),'boolean');assert.strictEqual(net.online,net.isOnline());
        await new Promise(resolve=>server.close(resolve));
    "#);
}

#[test]
fn node_net_reads_while_writing_and_reuses_socket_paths() {
    run(r#"
        const {assert,os,path,fs}=__apps,net=__apps.module.createRequire()('net'),size=16<<20;
        const echo=net.createServer(socket=>socket.pipe(socket));await new Promise(resolve=>echo.listen(0,'127.0.0.1',resolve));
        const client=net.connect(echo.address().port,'127.0.0.1');let received=0;
        client.on('data',chunk=>{received+=chunk.length;if(received===size)client.end();});client.write(Buffer.alloc(size,7));
        await new Promise(resolve=>client.on('close',resolve));assert.strictEqual(received,size);echo.close();
        if(process.platform==='win32')return;
        const http=__apps.http.createServer(),wss=new __apps.ws.WebSocketServer({server:http});wss.on('connection',ws=>ws.on('message',data=>ws.send(data)));
        await new Promise(resolve=>http.listen(0,'127.0.0.1',resolve));
        const ws=new __apps.ws.WebSocket(`ws://127.0.0.1:${http.address().port}/`);let echoed=0;
        await new Promise((resolve,reject)=>{ws.on('open',()=>{for(let i=0;i<16;i++)ws.send(new Uint8Array(1<<20));});ws.on('message',()=>{if(++echoed===16)resolve();});ws.on('error',reject);});
        ws.close();wss.close();http.close();
        const file=path.join(os.tmpdir(),`apps-${process.pid}-${Date.now()}.sock`),server=net.createServer(socket=>socket.end('hi'));
        await new Promise(resolve=>server.listen(file,resolve));server.close();assert.ok(!fs.existsSync(file),'close removes the socket file');
        await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(file,resolve);});
        const text=await new Promise((resolve,reject)=>{let data='';const socket=net.connect(file);socket.setEncoding('utf8');socket.on('data',chunk=>data+=chunk);socket.on('end',()=>resolve(data));socket.on('error',reject);});
        assert.strictEqual(text,'hi');server.close();assert.ok(!fs.existsSync(file));
    "#);
}

#[cfg(unix)]
#[test]
fn terminal_has_real_tty_and_can_resize() {
    run(r#"
        const terminal=__apps.pty.spawn('/bin/sh',['-c','stty size; test -t 0 && printf tty'],{cols:91,rows:32});
        let output='';terminal.onData(data=>output+=data);
        await new Promise(resolve=>terminal.onExit(resolve));
        if(!output.includes('32 91')||!output.includes('tty'))throw Error('Not a PTY: '+output);
    "#);
}

#[cfg(unix)]
#[test]
fn synchronous_child_processes_and_errors() {
    run(r#"
        const child=__apps.child_process;
        const result=child.spawnSync('/bin/sh',['-c','cat; printf failure >&2; exit 7'],{input:'hello',encoding:'utf8'});
        if(result.status!==7||result.stdout!=='hello'||result.stderr!=='failure')throw Error(JSON.stringify(result));
        const shell=child.spawnSync('printf shell',{shell:true,encoding:'utf8'});if(shell.stdout!=='shell')throw Error('shell option');
        const output=child.execFileSync('/bin/echo',['ok'],{encoding:'utf8'});
        if(output!=='ok\n')throw Error('execFileSync output');
        const timeout=child.spawnSync('/bin/sleep',['10'],{timeout:20});
        if(timeout.error?.code!=='ETIMEDOUT')throw Error('missing timeout error');
        let missing=false;try{__apps.fs.readFileSync('/does/not/exist/apps-test')}catch(e){missing=e.code==='ENOENT'}
        if(!missing)throw Error('synchronous file error code');
    "#);
}

#[test]
fn node_core_modules() {
    run(r#"
        const {assert,util,path,os,url,querystring,events,crypto,zlib,string_decoder:{StringDecoder}}=__apps;
        assert.deepStrictEqual(Buffer.concat([Buffer.alloc(0),Buffer.from('ab')]),Buffer.from('ab'));
        assert.strictEqual(Buffer.alloc(5).fill('ab').toString(),'ababa');
        assert.strictEqual(Buffer.from('abcabc').lastIndexOf('b'),4);
        const word=Buffer.alloc(6);word.writeUIntBE(0x123456789a,0,5);assert.strictEqual(word.readUIntBE(0,5),0x123456789a);
        assert.strictEqual(util.format('%s=%d %j','a',5,{x:1}),'a=5 {"x":1}');
        assert.strictEqual(util.inspect({a:[1,{b:new Map([[1,2]])}],c:'x'}),"{ a: [ 1, { b: [Map] } ], c: 'x' }");
        assert.strictEqual(util.inspect(Buffer.from('hi')),'<Buffer 68 69>');
        assert.throws(()=>assert.deepStrictEqual({a:1},{a:'1'}),{code:'ERR_ASSERTION'});
        await assert.rejects(Promise.reject(new TypeError('x')),TypeError);
        assert.deepStrictEqual(util.parseArgs({args:['-v','--name=x','pos'],options:{verbose:{type:'boolean',short:'v'},name:{type:'string'}},allowPositionals:true}).positionals,['pos']);
        assert.deepStrictEqual(path.parse('/home/u/f.txt'),{root:'/',dir:'/home/u',base:'f.txt',ext:'.txt',name:'f'});
        assert.strictEqual(path.win32.format({dir:'C:\\a',name:'b',ext:'.c'}),'C:\\a\\b.c');
        assert.strictEqual(url.fileURLToPath(url.pathToFileURL('/tmp/a b#c')),'/tmp/a b#c');
        assert.ok(os.totalmem()>0&&os.cpus().length>0&&!os.tmpdir().endsWith('/')&&typeof os.userInfo().username==='string');
        assert.deepStrictEqual(querystring.parse('a=1&a=2&b=x+y'),Object.assign(Object.create(null),{a:['1','2'],b:'x y'}));
        const decoder=new StringDecoder('utf8'),euro=Buffer.from('€');assert.strictEqual(decoder.write(euro.subarray(0,1))+decoder.end(euro.subarray(1)),'€');
        const emitter=new events.EventEmitter();let added;emitter.on('newListener',name=>added=name);emitter.on('x',()=>{});assert.strictEqual(added,'x');
        setTimeout(()=>{emitter.emit('tick',1);emitter.emit('tick',2);},1);const ticks=[];for await(const [value] of events.on(emitter,'tick')){ticks.push(value);if(ticks.length===2)break;}assert.deepStrictEqual(ticks,[1,2]);
        assert.strictEqual(crypto.createHash('md5').update('a').digest('hex'),'0cc175b9c0f1b6a831c399e269772661');
        const big=crypto.createHash('sha256');for(let i=0;i<40;i++)big.update(Buffer.alloc(4096,i));const copy=big.copy();assert.strictEqual(big.digest('hex'),copy.digest('hex'));
        const key=crypto.randomBytes(32),iv=crypto.randomBytes(12),cipher=crypto.createCipheriv('aes-256-gcm',key,iv);
        const sealed=Buffer.concat([cipher.update('secret','utf8'),cipher.final()]),decipher=crypto.createDecipheriv('aes-256-gcm',key,iv);decipher.setAuthTag(cipher.getAuthTag());
        assert.strictEqual(Buffer.concat([decipher.update(sealed),decipher.final()]).toString(),'secret');
        assert.strictEqual((await util.promisify(crypto.pbkdf2)('pw','salt',1000,16,'sha256')).toString('hex'),crypto.pbkdf2Sync('pw','salt',1000,16,'sha256').toString('hex'));
        assert.strictEqual(zlib.gunzipSync(zlib.gzipSync('hello')).toString(),'hello');
        assert.strictEqual((await util.promisify(zlib.inflate)(zlib.deflateSync('async'))).toString(),'async');
        assert.throws(()=>zlib.gunzipSync(Buffer.from('nope')),{code:'Z_DATA_ERROR'});
        assert.strictEqual(typeof setTimeout(()=>{},0).unref().hasRef,'function');
        assert.strictEqual(await util.promisify(setTimeout)(1,'done'),'done');
        assert.ok(process.memoryUsage().rss>0&&process.cpuUsage().user>=0&&process.versions.node);
        assert.ok(__apps.module.isBuiltin('node:readline/promises'));
    "#);
}

#[cfg(unix)]
#[test]
fn node_files_streams_and_processes() {
    let dir = tempfile::tempdir().unwrap();
    let root = serde_json::to_string(dir.path()).unwrap();
    run(&format!(
        r#"
        const {{assert,fs,path,stream,readline,zlib,crypto,util,child_process}}=__apps,root={root};
        const file=path.join(root,'a','lines.txt');
        fs.mkdirSync(path.dirname(file),{{recursive:true}});fs.writeFileSync(file,'one\r\ntwo\nthree');
        try{{fs.readFileSync(path.join(root,'missing'))}}catch(error){{assert.strictEqual(error.message,`ENOENT: no such file or directory, open '${{path.join(root,'missing')}}'`);assert.strictEqual(error.syscall,'open');}}
        assert.strictEqual(fs.statSync(path.join(root,'missing'),{{throwIfNoEntry:false}}),undefined);
        const stats=fs.statSync(file);assert.ok(stats.isFile()&&stats.atime instanceof Date&&stats.ctimeMs>0&&stats.nlink===1);
        assert.deepStrictEqual(fs.readdirSync(root,{{recursive:true}}).sort(),['a',path.join('a','lines.txt')]);
        const entry=fs.readdirSync(root,{{recursive:true,withFileTypes:true}}).find(item=>item.isFile());assert.strictEqual(entry.name,'lines.txt');assert.strictEqual(entry.parentPath,path.join(root,'a'));
        fs.utimesSync(file,1000,2000);assert.strictEqual(fs.statSync(file).mtimeMs,2000000);
        fs.cpSync(path.join(root,'a'),path.join(root,'b'),{{recursive:true}});assert.strictEqual(fs.readFileSync(path.join(root,'b','lines.txt'),'utf8'),'one\r\ntwo\nthree');
        fs.copyFileSync(file,path.join(root,'b','copy.txt'));assert.throws(()=>fs.copyFileSync(file,path.join(root,'b','copy.txt'),fs.constants.COPYFILE_EXCL),{{code:'EEXIST'}});
        fs.truncateSync(path.join(root,'b','copy.txt'),3);assert.strictEqual(fs.readFileSync(path.join(root,'b','copy.txt'),'utf8'),'one');
        const lines=[];for await(const line of readline.createInterface({{input:fs.createReadStream(file,{{highWaterMark:2}})}}))lines.push(line);assert.deepStrictEqual(lines,['one','two','three']);
        class Upper extends stream.Transform{{_transform(chunk,encoding,callback){{callback(null,chunk.toString().toUpperCase());}}}}
        const output=path.join(root,'out.gz');
        await stream.promises.pipeline(fs.createReadStream(file),new Upper(),async function*(source){{for await(const chunk of source)yield chunk;}},zlib.createGzip(),fs.createWriteStream(output));
        assert.strictEqual(zlib.gunzipSync(fs.readFileSync(output)).toString(),'ONE\r\nTWO\nTHREE');
        const hash=await new Promise((resolve,reject)=>fs.createReadStream(file).pipe(crypto.createHash('sha1')).setEncoding('hex').on('data',resolve).on('error',reject));
        assert.strictEqual(hash,crypto.createHash('sha1').update(fs.readFileSync(file)).digest('hex'));
        assert.strictEqual(await stream.consumers.text(stream.Readable.from(['a','b'])),'ab');
        const exec=util.promisify(child_process.exec);
        const {{stdout}}=await exec('echo "$0"',{{shell:'/bin/sh'}});assert.strictEqual(stdout.trim(),'/bin/sh');
        await assert.rejects(exec('echo bad >&2; exit 3'),error=>error.code===3&&error.stderr==='bad\n');
        process.env.APPS_ENV_TEST='inherited';assert.strictEqual((await exec('printf "$APPS_ENV_TEST"')).stdout,'inherited');assert.strictEqual(child_process.execSync('printf "$APPS_ENV_TEST"',{{encoding:'utf8'}}),'inherited');
    "#
    ));
}

#[test]
fn node_fs_remove_keeps_files_and_directories_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let root = serde_json::to_string(dir.path()).unwrap();
    run(&format!(
        r#"
        const {{assert,fs,path}}=__apps,root={root},file=path.join(root,'f.txt'),empty=path.join(root,'empty');
        fs.writeFileSync(file,'keep');fs.mkdirSync(empty);
        assert.throws(()=>fs.rmdirSync(file),{{code:'ENOTDIR',syscall:'rmdir'}});
        assert.throws(()=>fs.rmdirSync(file,{{recursive:true}}),{{code:'ENOTDIR'}});
        assert.strictEqual(fs.readFileSync(file,'utf8'),'keep');
        assert.throws(()=>fs.rmSync(empty),{{name:'SystemError',code:'ERR_FS_EISDIR',errno:21,syscall:'rm',message:`Path is a directory: rm returned EISDIR (is a directory) ${{empty}}`}});
        assert.throws(()=>fs.rmSync(empty,{{force:true}}),{{code:'ERR_FS_EISDIR'}});
        await assert.rejects(fs.promises.rm(empty),{{code:'ERR_FS_EISDIR'}});
        assert.ok(fs.existsSync(empty));
        assert.throws(()=>fs.rmSync(path.join(root,'nope')),{{code:'ENOENT',syscall:'lstat'}});
        fs.rmSync(path.join(root,'nope'),{{force:true}});
        fs.symlinkSync(empty,path.join(root,'link'));fs.rmSync(path.join(root,'link'));assert.ok(fs.existsSync(empty));
        fs.rmSync(empty,{{recursive:true}});fs.rmSync(file);assert.deepStrictEqual(fs.readdirSync(root),[]);
    "#
    ));
}

#[cfg(unix)]
#[test]
fn node_fs_descriptors_handles_and_streams() {
    let dir = tempfile::tempdir().unwrap();
    let root = serde_json::to_string(dir.path()).unwrap();
    run(&format!(
        r#"
        const {{assert,fs,path}}=__apps,fsp=fs.promises,root={root},file=path.join(root,'x.txt');
        fs.writeFileSync(file,'hello world');
        const fd=fs.openSync(file,'r+');
        assert.strictEqual(fs.readFileSync(fd,'utf8'),'hello world');
        fs.writeFileSync(fd,'HELLO');fs.appendFileSync(fd,'!');assert.strictEqual(fs.fstatSync(fd).size,17);
        fs.fchmodSync(fd,'600');fs.futimesSync(fd,1,new Date(2000));fs.fdatasyncSync(fd);fs.fchownSync(fd,-1,-1);
        const stats=fs.fstatSync(fd);assert.strictEqual((stats.mode&0o777).toString(8),'600');assert.strictEqual(stats.mtimeMs,2000);
        await new Promise((resolve,reject)=>fs.fchmod(fd,0o644,error=>error?reject(error):fs.fdatasync(fd,error=>error?reject(error):resolve())));
        assert.throws(()=>fs.read(fd,Buffer.alloc(4),0,10,0,()=>{{}}),{{code:'ERR_OUT_OF_RANGE',message:'The value of "length" is out of range. It must be <= 4. Received 10'}});
        assert.throws(()=>fs.readSync(fd,Buffer.alloc(4),-1,1,0),{{code:'ERR_OUT_OF_RANGE',message:'The value of "offset" is out of range. It must be >= 0 && <= 9007199254740991. Received -1'}});
        assert.throws(()=>fs.writeSync(fd,Buffer.alloc(4),5),{{code:'ERR_OUT_OF_RANGE',message:'The value of "offset" is out of range. It must be <= 4. Received 5'}});
        assert.throws(()=>fs.write(fd,Buffer.alloc(4),1,4,()=>{{}}),{{code:'ERR_OUT_OF_RANGE',message:'The value of "length" is out of range. It must be <= 3. Received 4'}});
        assert.strictEqual(fs.readSync(fd,Buffer.alloc(4),{{offset:1,length:3,position:1}}),3);
        assert.strictEqual(fs.readSync(fd,Buffer.alloc(4),0,3,1n),3);
        assert.strictEqual(fs.readSync(fd,Buffer.alloc(65<<20),0,65<<20,0),17);
        fs.closeSync(fd);
        const moded=path.join(root,'moded.txt'),written=path.join(root,'written.txt');
        fs.closeSync(fs.openSync(moded,'w','600'));fs.writeFileSync(written,'x',{{mode:'640'}});
        assert.deepStrictEqual([moded,written].map(file=>(fs.statSync(file).mode&0o777).toString(8)),['600','640']);
        assert.throws(()=>fs.closeSync(fd),{{code:'EBADF',syscall:'close',message:'EBADF: bad file descriptor, close'}});
        assert.throws(()=>fs.watch(path.join(root,'missing')),{{code:'ENOENT'}});
        assert.throws(()=>fs.readSync(fd,Buffer.alloc(1)),{{code:'EBADF',syscall:'read'}});
        const handle=await fsp.open(file,'r+');
        await assert.rejects(handle.read(Buffer.alloc(4),0,10),{{code:'ERR_OUT_OF_RANGE'}});
        assert.strictEqual(await fsp.readFile(handle,'utf8'),'hello worldHELLO!');
        await fsp.writeFile(handle,'+');await handle.chmod(0o640);await handle.utimes(3,4);await handle.datasync();
        assert.strictEqual((await handle.stat()).mtimeMs,4000);await handle.close();assert.strictEqual(handle.fd,-1);
        assert.strictEqual(fs.readFileSync(file,'utf8'),'hello worldHELLO!+');
        await assert.rejects(fsp.readFile(file,{{signal:AbortSignal.abort()}}),{{name:'AbortError',code:'ABORT_ERR'}});
        assert.throws(()=>fs.writeFileSync(file,123),{{code:'ERR_INVALID_ARG_TYPE',message:'The "data" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received type number (123)'}});
        assert.throws(()=>fs.readFileSync('a\0b'),{{code:'ERR_INVALID_ARG_VALUE'}});
        assert.strictEqual(fs.mkdirSync(path.join(root,'a','b','c'),{{recursive:true}}),path.join(root,'a'));
        assert.strictEqual(fs.mkdirSync(path.join(root,'a','b'),{{recursive:true}}),undefined);
        assert.strictEqual(await fsp.mkdir(path.join(root,'a','d','e'),{{recursive:true}}),path.join(root,'a','d'));
        assert.deepStrictEqual(fs.readdirSync(path.join(root,'a'),{{encoding:'buffer'}}),[Buffer.from('b'),Buffer.from('d')]);
        assert.ok(Buffer.isBuffer(fs.readdirSync(path.join(root,'a'),{{encoding:'buffer',withFileTypes:true}})[0].name));
        assert.ok(!Object.keys(fs.statSync(file)).includes('mtime')&&fs.statSync(file).mtime instanceof Date);
        const dir=fs.opendirSync(root);assert.ok(dir instanceof fs.Dir);dir.closeSync();
        const blob=await fs.openAsBlob(file,{{type:'text/plain'}});assert.strictEqual(blob.type,'text/plain');assert.strictEqual(await blob.text(),'hello worldHELLO!+');
        const reader=fs.createReadStream(file,{{start:2,end:6,encoding:'utf8'}}),events=[];
        assert.ok(reader instanceof fs.ReadStream&&reader instanceof __apps.stream.Readable&&reader.pending);
        reader.on('open',fd=>events.push('open:'+typeof fd)).on('ready',()=>events.push('ready'));
        let text='';for await(const chunk of reader)text+=chunk;
        assert.strictEqual(text,'llo w');assert.deepStrictEqual(events,['open:number','ready']);assert.strictEqual(reader.bytesRead,5);
        assert.throws(()=>fs.createReadStream(file,{{start:5,end:2}}),{{code:'ERR_OUT_OF_RANGE'}});
        const opened=await fsp.open(file);let head='';await new Promise(resolve=>opened.createReadStream({{end:4}}).on('data',chunk=>head+=chunk).on('close',resolve));assert.strictEqual(head,'hello');assert.strictEqual(opened.fd,-1);
        const out=path.join(root,'out.txt'),writer=fs.createWriteStream(out);assert.ok(writer instanceof fs.WriteStream);
        writer.write('ab');writer.write(Buffer.from('cd'));await new Promise(resolve=>writer.end('ef',resolve));
        assert.strictEqual(fs.readFileSync(out,'utf8'),'abcdef');assert.strictEqual(writer.bytesWritten,6);
        await new Promise(resolve=>fs.createWriteStream(out,{{flags:'r+',start:2}}).end('XY',resolve));assert.strictEqual(fs.readFileSync(out,'utf8'),'abXYef');
        const failure=await new Promise(resolve=>fs.createWriteStream(path.join(root,'missing','x')).on('error',resolve));assert.strictEqual(failure.code,'ENOENT');
    "#
    ));
}

#[test]
fn node_zlib_streams_crypto_digests_and_os_priority() {
    run(r#"
        const {assert,zlib,crypto,os,readline,stream}=__apps;
        const gzip=zlib.createGzip(),sizes=[];gzip.on('data',chunk=>sizes.push(chunk.length));gzip.write('hello');
        await new Promise(resolve=>gzip.flush(resolve));assert.ok(sizes.length>0&&gzip.bytesWritten===5,'flush emits output');
        const deflate=zlib.createDeflate({level:1}),inflate=zlib.createInflate(),parts=[];inflate.on('data',chunk=>parts.push(chunk));deflate.pipe(inflate);
        deflate.write('aaa');await new Promise(resolve=>deflate.params(9,0,resolve));deflate.end('bbb');
        await new Promise(resolve=>inflate.on('end',resolve));assert.strictEqual(Buffer.concat(parts).toString(),'aaabbb');
        await new Promise(resolve=>zlib.createGzip().close(resolve));
        const truncated=await new Promise(resolve=>{const gunzip=zlib.createGunzip();gunzip.on('error',resolve);gunzip.end(zlib.gzipSync('hello').subarray(0,10));gunzip.resume();});
        assert.strictEqual(truncated.code,'Z_BUF_ERROR');assert.strictEqual(truncated.message,'unexpected end of file');
        assert.throws(()=>zlib.gunzipSync(Buffer.from('notgzip')),{code:'Z_DATA_ERROR',message:'incorrect header check'});
        assert.throws(()=>zlib.gunzipSync(zlib.gzipSync('x'.repeat(100)),{maxOutputLength:10}),{code:'ERR_BUFFER_TOO_LARGE',message:'Cannot create a Buffer larger than 10 bytes'});
        assert.strictEqual(zlib.gunzipSync(Buffer.concat([zlib.gzipSync('a'),zlib.gzipSync('b')])).toString(),'ab');
        assert.strictEqual(zlib.deflateSync('aaaaaaaaaa',{level:0}).toString('hex'),'7801010a00f5ff6161616161616161616114e103cb');
        const unzip=zlib.createUnzip();unzip.end(zlib.deflateSync('unzipped'));assert.strictEqual(await stream.consumers.text(unzip),'unzipped');
        const dictionary=Buffer.from('hello world'),text='hello world hello world',packed=zlib.deflateRawSync(text,{dictionary});
        assert.ok(packed.length<zlib.deflateRawSync(text).length,'one-shot deflate uses the dictionary');
        assert.strictEqual(zlib.inflateRawSync(packed,{dictionary}).toString(),text);
        const streamed=zlib.createInflateRaw({dictionary});streamed.end(packed);assert.strictEqual(await stream.consumers.text(streamed),text);
        assert.strictEqual((await new Promise((resolve,reject)=>zlib.inflateRaw(packed,{dictionary},(error,data)=>error?reject(error):resolve(data)))).toString(),text);
        assert.strictEqual(zlib.inflateRawSync(zlib.deflateRawSync(text,{windowBits:9}),{windowBits:9}).toString(),text);
        const wrapped=zlib.deflateSync(text,{dictionary});assert.strictEqual(zlib.inflateSync(wrapped,{dictionary}).toString(),text);
        const inflating=zlib.createInflate({dictionary});inflating.end(wrapped);assert.strictEqual(await stream.consumers.text(inflating),text);
        assert.throws(()=>zlib.inflateSync(wrapped),{code:'Z_DATA_ERROR'});assert.throws(()=>zlib.inflateSync(wrapped,{dictionary:Buffer.from('other')}),{code:'Z_DATA_ERROR'});
        assert.deepStrictEqual([zlib.crc32('hello'),zlib.crc32(Buffer.from('world'),zlib.crc32('hello ')),zlib.crc32('')],[907060870,222957957,0]);
        const key=Buffer.alloc(32,1),iv=Buffer.alloc(12,2),cipher=crypto.createCipheriv('aes-256-gcm',key,iv);cipher.setAAD(Buffer.from('aad'));
        const first=cipher.update('hello '),second=cipher.update('world','utf8','hex');
        assert.strictEqual(first.toString('hex'),'6fb3a5252577');assert.strictEqual(second,'b692a1a0d8');assert.strictEqual(cipher.final('hex'),'');
        assert.strictEqual(cipher.getAuthTag().toString('hex'),'afe1438751549432bf83d9348ad63a18');
        const decipher=crypto.createDecipheriv('aes-256-gcm',key,iv);decipher.setAAD(Buffer.from('aad'));decipher.setAuthTag(cipher.getAuthTag());
        const sealed=Buffer.concat([first,Buffer.from(second,'hex')]);
        assert.strictEqual(decipher.update(sealed.subarray(0,3),null,'utf8')+decipher.update(sealed.subarray(3),null,'utf8')+decipher.final('utf8'),'hello world');
        const big=crypto.createCipheriv('aes-128-gcm',Buffer.alloc(16,3),Buffer.alloc(12,4)),data=Buffer.alloc(100000,7),chunks=[];
        for(let i=0;i<data.length;i+=3000)chunks.push(big.update(data.subarray(i,i+3000)));chunks.push(big.final());
        assert.strictEqual(crypto.createHash('sha1').update(Buffer.concat(chunks)).digest('hex'),'5a2cad53f9747b0261e4e023a23dbf2ad56944c1');
        assert.strictEqual(big.getAuthTag().toString('hex'),'47c4b202c135de3b2a78bd8926046801');
        const bad=crypto.createDecipheriv('aes-128-gcm',Buffer.alloc(16),Buffer.alloc(12));bad.setAuthTag(Buffer.alloc(16));bad.update('abc');assert.throws(()=>bad.final(),/unable to authenticate/);
        const hashes=['md5','sha1','sha224','sha256','sha384','sha512'];
        assert.deepStrictEqual(hashes.map(name=>crypto.createHmac(name,'k'.repeat(100)).update('data').digest('hex').slice(0,16)),['38a034384b6cbf08','e3dc391a03c12663','a80ba0a171bfdfef','09380ee4b802da23','4de205aedc09121d','f8020ac27b43bef1']);
        assert.strictEqual(crypto.createHmac('sha224','key').update('data').digest('hex'),'19424d4210e50d7a4521b5f0d54b4b0cff3060deddccfd894fda5b3b');
        assert.strictEqual(crypto.createHmac('md5','key').update('data').digest('hex'),'9d5c73ef85594d34ec4438b7c97e51d8');
        assert.deepStrictEqual(hashes.map(name=>crypto.pbkdf2Sync('pw','salt',3,40,name).toString('hex').slice(0,24)),['d684741fdc128b5679ae66b7','b095208e7496b1a485a5dbe9','26ada39be31f195500c2ef78','3c671234e386cb307220d70e','b9a2cf5af8187f5db8f65515','6a7be24f5b46c5c3769d73a6']);
        assert.strictEqual((await __apps.util.promisify(crypto.pbkdf2)('pw','salt',1,16,'md5')).toString('hex'),'9887054ca80d1507d8b6e84a29476d04');
        assert.strictEqual(crypto.constants.RSA_PKCS1_PADDING,1);assert.deepStrictEqual(crypto.getCurves(),['prime256v1','secp384r1']);
        assert.throws(()=>crypto.timingSafeEqual(Buffer.from('a'),Buffer.from('ab')),{code:'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH'});
        if(process.platform!=='win32'){
            const priority=os.getPriority();assert.strictEqual(os.getPriority(process.pid),priority);os.setPriority(priority);
            assert.throws(()=>os.getPriority(999999),{name:'SystemError',code:'ERR_SYSTEM_ERROR',message:'A system error occurred: uv_os_getpriority returned ESRCH (no such process)',errno:-3});
            assert.throws(()=>os.setPriority(30),{code:'ERR_OUT_OF_RANGE'});
            assert.strictEqual(os.constants.dlopen.RTLD_NOW,2);
        }
        const written=[],output=new stream.Writable({write(chunk,encoding,callback){written.push(chunk.toString());callback();}});
        const line=new readline.promises.Readline(output);await line.clearLine(0).cursorTo(2,3).moveCursor(-1,1).clearScreenDown().commit();
        line.cursorTo(1).rollback();await line.commit();
        assert.deepStrictEqual(written,['\x1b[2K\x1b[4;3H\x1b[1D\x1b[1B\x1b[0J','']);
    "#);
}

#[test]
fn node_event_targets_timerify_loop_use_and_net_addresses_match_node() {
    run(r#"
        const {assert,events,util,tty,worker_threads,perf_hooks:{performance,PerformanceObserver,createHistogram}}=__apps,{BlockList,SocketAddress}=__apps.module.createRequire()('net');
        const target=new EventTarget(),listener=()=>{},handler={handleEvent(){}},emitter=new events.EventEmitter(),controller=new AbortController();
        target.addEventListener('foo',listener);target.addEventListener('foo',handler);emitter.on('x',listener);controller.signal.addEventListener('abort',listener);
        assert.deepStrictEqual([events.getEventListeners(target,'foo'),events.getEventListeners(target,'bar'),events.getEventListeners(emitter,'x'),events.getEventListeners(controller.signal,'abort')],[[listener,handler],[],[listener],[listener]]);
        const histogram=createHistogram(),add=function add(a,b){return a+b},timed=performance.timerify(add,{histogram});
        const seen=new Promise(resolve=>new PerformanceObserver((list,observer)=>{observer.disconnect();resolve(list.getEntries().map(entry=>[entry.constructor.name,entry.entryType,entry.name,entry.detail]));}).observe({entryTypes:['function']}));
        assert.deepStrictEqual([timed.name,timed.length,timed(1,2),histogram.count],['timerified add',2,3,1]);
        assert.deepStrictEqual(await seen,[['PerformanceNodeEntry','function','add',[1,2]]]);assert.strictEqual(performance.getEntriesByType('function').length,0);
        class K{constructor(x){this.x=x}}assert.ok(new (performance.timerify(K))(5) instanceof K);assert.strictEqual(await performance.timerify(async()=>7)(),7);
        assert.deepStrictEqual(performance.eventLoopUtilization({idle:5,active:5},{idle:2,active:1}),{idle:3,active:4,utilization:4/7});
        const worker=new worker_threads.Worker(`const{parentPort}=require('node:worker_threads');setTimeout(()=>{const first=performance.eventLoopUtilization(),delta=performance.eventLoopUtilization(first);parentPort.postMessage([first.idle,first.active,first.utilization,delta.idle>=0&&delta.active>=0]);},60);`,{eval:true});
        const [idle,active,utilization,delta]=await new Promise(resolve=>worker.once('message',resolve));
        assert.ok(idle>=40&&active>=0&&utilization<0.5&&delta,`${idle} ${active} ${utilization}`);
        const list=new BlockList();list.addAddress('1.2.3.4');list.addRange('10.0.0.1','10.0.0.9');list.addSubnet('8592:757c:efae:4e45::',64,'ipv6');list.addSubnet(new SocketAddress({address:'192.168.0.0'}),16);
        assert.deepStrictEqual(list.rules,['Subnet: IPv4 192.168.0.0/16','Subnet: IPv6 8592:757c:efae:4e45::/64','Range: IPv4 10.0.0.1-10.0.0.9','Address: IPv4 1.2.3.4']);
        assert.deepStrictEqual([list.check('1.2.3.4'),list.check('::ffff:1.2.3.4','ipv6'),list.check('::ffff:10.0.0.5','ipv6'),list.check('192.168.3.4'),list.check('8592:757c:efae:4e45::f','ipv6'),list.check('10.0.0.10'),list.check('x'),list.check('::1.2.3.4','ipv6')],[true,true,true,true,true,false,false,false]);
        const mapped=new BlockList();mapped.addSubnet('::ffff:0:0',96,'ipv6');assert.ok(mapped.check('1.2.3.4'));
        const copy=new BlockList();copy.fromJSON(JSON.stringify(list));assert.deepStrictEqual(copy.rules,[...list.rules].reverse());
        assert.ok(BlockList.isBlockList(list)&&!BlockList.isBlockList({})&&SocketAddress.isSocketAddress(new SocketAddress())&&!SocketAddress.isSocketAddress({}));
        assert.throws(()=>list.addAddress('x'),{code:'ERR_INVALID_ADDRESS'});assert.throws(()=>list.addSubnet('1.2.3.4',33),{code:'ERR_OUT_OF_RANGE'});assert.throws(()=>list.addRange('1.2.3.9','1.2.3.4'),{code:'ERR_INVALID_ARG_VALUE'});
        assert.strictEqual(util.inspect(mapped),"BlockList { rules: [ 'Subnet: IPv6 ::ffff:0:0/96' ] }");
        assert.deepStrictEqual(new SocketAddress().toJSON(),{address:'127.0.0.1',port:0,family:'ipv4',flowlabel:0});
        assert.deepStrictEqual(SocketAddress.parse('1.2.3.4:5').toJSON(),{address:'1.2.3.4',port:5,family:'ipv4',flowlabel:0});
        assert.deepStrictEqual([SocketAddress.parse('[::1]:80').address,SocketAddress.parse('[::1]:80').port,SocketAddress.parse('x')],['::1',0,undefined]);
        assert.strictEqual(util.inspect(new SocketAddress({address:'0:0::1',family:'IPv6',port:8})),"SocketAddress { address: '::1', port: 8, family: 'ipv6', flowlabel: 0 }");
        assert.throws(()=>new SocketAddress({address:'x'}),{code:'ERR_INVALID_ADDRESS'});assert.throws(()=>new SocketAddress({port:70000}),{code:'ERR_SOCKET_BAD_PORT'});
        assert.strictEqual(process.stdout.isTTY,tty.isatty(1)||undefined);
        if(!process.stdout.isTTY)assert.deepStrictEqual([process.stdout.columns,typeof process.stdout.getWindowSize],[undefined,'undefined']);
    "#);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn node_dns_resolver_reads_answers_like_node() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let label = |text: &str| {
            let mut out: Vec<u8> = text
                .split('.')
                .flat_map(|part| [&[part.len() as u8][..], part.as_bytes()].concat())
                .collect();
            out.push(0);
            out
        };
        let text = |value: &str| [&[value.len() as u8][..], value.as_bytes()].concat();
        let numbers: Vec<u8> = [2024010101u32, 7200, 3600, 1209600, 300]
            .iter()
            .flat_map(|n| n.to_be_bytes())
            .collect();
        let records: Vec<(u16, u32, Vec<u8>)> = vec![
            (1, 300, vec![192, 0, 2, 1]),
            (1, 60, vec![192, 0, 2, 2]),
            (
                28,
                300,
                [0x20, 1, 0x0d, 0xb8]
                    .into_iter()
                    .chain([0; 11])
                    .chain([1])
                    .collect(),
            ),
            (15, 300, [vec![0, 10], label("mail.example.test")].concat()),
            (16, 300, text("v=spf1 -all")),
            (16, 300, [text("a"), text("b")].concat()),
            (
                33,
                300,
                [vec![0, 1, 0, 2, 1, 187], label("svc.example.test")].concat(),
            ),
            (5, 300, label("alias.example.test")),
            (2, 300, label("ns1.example.test")),
            (12, 300, label("host.example.test")),
            (
                6,
                300,
                [
                    label("ns1.example.test"),
                    label("admin.example.test"),
                    numbers,
                ]
                .concat(),
            ),
            (
                257,
                300,
                [&[0, 5][..], b"issue", b"letsencrypt.org"].concat(),
            ),
            (
                257,
                300,
                [&[128, 5][..], b"iodef", b"mailto:x@example.test"].concat(),
            ),
            (
                35,
                300,
                [
                    vec![0, 100, 0, 10],
                    text("S"),
                    text("SIP+D2U"),
                    text(""),
                    label("_sip._udp.example.test"),
                ]
                .concat(),
            ),
            (52, 300, vec![3, 1, 1, 0xde, 0xad, 0xbe, 0xef]),
        ];
        let mut query = [0u8; 512];
        while let Ok((size, peer)) = socket.recv_from(&mut query) {
            let end = 12 + query[12..size].iter().position(|&byte| byte == 0).unwrap() + 5;
            let name = String::from_utf8_lossy(&query[12..end - 5]).into_owned();
            let kind = u16::from_be_bytes([query[end - 4], query[end - 3]]);
            let rcode = if name.contains("missing") {
                3
            } else if name.contains("fail") {
                2
            } else {
                0
            };
            let answers: Vec<_> = records
                .iter()
                .filter(|(ty, ..)| {
                    rcode == 0 && !name.contains("empty") && (kind == 255 || *ty == kind)
                })
                .collect();
            let mut reply = [
                &query[..2],
                &[0x81, 0x80 | rcode, 0, 1],
                &(answers.len() as u16).to_be_bytes()[..],
                &[0, 0, 0, 0],
                &query[12..end],
            ]
            .concat();
            for (ty, ttl, data) in answers {
                reply.extend(
                    [0xc0, 12]
                        .into_iter()
                        .chain(ty.to_be_bytes())
                        .chain([0, 1])
                        .chain(ttl.to_be_bytes())
                        .chain((data.len() as u16).to_be_bytes())
                        .chain(data.iter().copied()),
                );
            }
            socket.send_to(&reply, peer).unwrap();
        }
    });
    run(&r#"
        const {assert,dns}=__apps,resolver=new dns.promises.Resolver({timeout:2000,tries:1}),server='127.0.0.1:PORT',name='example.test';
        resolver.setServers([server]);assert.deepStrictEqual(resolver.getServers(),[server]);
        assert.deepStrictEqual(await resolver.resolve4(name),['192.0.2.1','192.0.2.2']);
        assert.deepStrictEqual(await resolver.resolve4(name,{ttl:true}),[{address:'192.0.2.1',ttl:300},{address:'192.0.2.2',ttl:60}]);
        assert.deepStrictEqual(await resolver.resolve6(name),['2001:db8::1']);
        assert.deepStrictEqual(await resolver.resolveMx(name),[{exchange:'mail.example.test',priority:10}]);
        assert.deepStrictEqual(await resolver.resolveTxt(name),[['v=spf1 -all'],['a','b']]);
        assert.deepStrictEqual(await resolver.resolveSrv(name),[{name:'svc.example.test',port:443,priority:1,weight:2}]);
        assert.deepStrictEqual([await resolver.resolveCname(name),await resolver.resolveNs(name),await resolver.resolvePtr(name),await resolver.reverse('192.0.2.1')],[['alias.example.test'],['ns1.example.test'],['host.example.test'],['host.example.test']]);
        assert.deepStrictEqual(await resolver.resolveSoa(name),{nsname:'ns1.example.test',hostmaster:'admin.example.test',serial:2024010101,refresh:7200,retry:3600,expire:1209600,minttl:300});
        assert.deepStrictEqual(await resolver.resolveCaa(name),[{critical:0,issue:'letsencrypt.org'},{critical:128,iodef:'mailto:x@example.test'}]);
        assert.deepStrictEqual(await resolver.resolveNaptr(name),[{flags:'S',service:'SIP+D2U',regexp:'',replacement:'_sip._udp.example.test',order:100,preference:10}]);
        const [tlsa]=await resolver.resolveTlsa(name);assert.deepStrictEqual([tlsa.certUsage,tlsa.selector,tlsa.match,Buffer.from(tlsa.data).toString('hex'),tlsa.data instanceof ArrayBuffer],[3,1,1,'deadbeef',true]);
        const any=await resolver.resolveAny(name);
        assert.deepStrictEqual(any.map(record=>record.type),['A','A','AAAA','MX','TXT','TXT','SRV','CNAME','NS','PTR','SOA','CAA','CAA','NAPTR','TLSA']);
        assert.deepStrictEqual([any[0],any[3],any[4]],[{address:'192.0.2.1',ttl:300,type:'A'},{exchange:'mail.example.test',priority:10,type:'MX'},{entries:['v=spf1 -all'],type:'TXT'}]);
        for(const [host,code] of [['missing.test','ENOTFOUND'],['fail.test','ESERVFAIL'],['empty.test','ENODATA']])await assert.rejects(resolver.resolve4(host),{code,message:`queryA ${code} ${host}`,syscall:'queryA',hostname:host,errno:undefined});
        assert.deepStrictEqual(await resolver.resolve(name,'MX'),[{exchange:'mail.example.test',priority:10}]);
        assert.throws(()=>resolver.resolve(name,'BOGUS'),{code:'ERR_INVALID_ARG_VALUE'});assert.throws(()=>resolver.setServers(['x']),{code:'ERR_INVALID_IP_ADDRESS'});
        await assert.rejects(resolver.reverse('x'),{code:'EINVAL',syscall:'getHostByAddr'});
        dns.setServers([server]);
        assert.deepStrictEqual(await new Promise((resolve,reject)=>dns.resolveMx(name,(error,value)=>error?reject(error):resolve(value))),[{exchange:'mail.example.test',priority:10}]);
        assert.deepStrictEqual([await dns.promises.resolve6(name),dns.promises.getServers()],[['2001:db8::1'],[server]]);
        const callbacks=new dns.Resolver();callbacks.setServers([server]);
        assert.deepStrictEqual(await new Promise(resolve=>callbacks.reverse('192.0.2.1',(error,names)=>resolve(names))),['host.example.test']);
        const cancelled=resolver.resolve4(name);resolver.cancel();await assert.rejects(cancelled,{code:'ECANCELLED'});
        assert.deepStrictEqual([dns.NOTFOUND,dns.promises.TIMEOUT,dns.getDefaultResultOrder()],['ENOTFOUND','ETIMEOUT','verbatim']);
    "#.replace("PORT", &port.to_string()));
}

#[test]
fn node_fs_vectors_statfs_and_glob_match_node() {
    let dir = tempfile::tempdir().unwrap();
    let root = serde_json::to_string(&dir.path()).unwrap();
    run(&format!(
        r#"
        const {{assert,fs,path}}=__apps,root={root},fold=process.platform!=='linux',sort=list=>[...list].sort();
        for(const file of ['a.js','b.ts','.hidden.js','src/c.js','src/d.JS','src/.e.js','src/deep/f.js','src/deep/g.txt','.dot/h.js','lib/i.js']){{fs.mkdirSync(path.join(root,path.dirname(file)),{{recursive:true}});fs.writeFileSync(path.join(root,file),'');}}
        const glob=(pattern,o)=>sort(fs.globSync(pattern,{{cwd:root,...o}})),d=fold?['src/d.JS']:[];
        assert.deepStrictEqual(glob('*.js'),['a.js']);
        assert.deepStrictEqual(glob('**/*.js'),['a.js','lib/i.js','src/c.js',...d,'src/deep/f.js']);
        assert.deepStrictEqual(glob('**'),['.','a.js','b.ts','lib','lib/i.js','src','src/c.js','src/d.JS','src/deep','src/deep/f.js','src/deep/g.txt']);
        assert.deepStrictEqual(glob('src/**'),['src','src/c.js','src/d.JS','src/deep','src/deep/f.js','src/deep/g.txt']);
        assert.deepStrictEqual(glob('*/*.{{js,ts}}'),['lib/i.js','src/c.js',...d]);
        assert.deepStrictEqual(glob('src/[cd].js'),['src/c.js',...d]);
        assert.deepStrictEqual(glob('src/[!c].*'),['src/d.JS']);
        assert.deepStrictEqual(glob('**/*.js',{{exclude:['src/**']}}),['a.js','lib/i.js']);
        assert.deepStrictEqual(glob('**/*.js',{{exclude:p=>p.includes('deep')}}),['a.js','lib/i.js','src/c.js',...d]);
        assert.deepStrictEqual(glob(['a.js','nope.js','src']),['a.js','src']);
        assert.deepStrictEqual([glob('./src/*.js'),glob('src/../a.js'),glob('.dot/*')],[['src/c.js',...d],['a.js'],['.dot/h.js']]);
        assert.deepStrictEqual(fs.globSync('*.js',{{cwd:root,withFileTypes:true}}).map(entry=>[entry.name,entry.parentPath===root,entry.isFile()]),[['a.js',true,true]]);
        assert.deepStrictEqual(fs.globSync(path.join(root,'src/*.js')),[path.join(root,'src/c.js'),...d.map(file=>path.join(root,file))]);
        assert.deepStrictEqual(await new Promise((resolve,reject)=>fs.glob('*.js',{{cwd:root}},(error,list)=>error?reject(error):resolve(list))),['a.js']);
        assert.deepStrictEqual(sort(await Array.fromAsync(fs.promises.glob('src/*',{{cwd:root}}))),['src/c.js','src/d.JS','src/deep']);
        const stats=fs.statfsSync(root),big=await fs.promises.statfs(root,{{bigint:true}});
        assert.deepStrictEqual(Object.keys(stats),['type','bsize','frsize','blocks','bfree','bavail','files','ffree']);
        assert.ok(stats.bsize>0&&stats.blocks>=stats.bfree&&big.bsize===BigInt(stats.bsize));
        assert.throws(()=>fs.statfsSync(path.join(root,'missing')),{{code:'ENOENT'}});
        const fd=fs.openSync(path.join(root,'v.bin'),'w+');
        assert.strictEqual(fs.writevSync(fd,[Buffer.from('abc'),new Uint8Array([100,101])]),5);
        const a=Buffer.alloc(2),b=Buffer.alloc(4);assert.strictEqual(fs.readvSync(fd,[a,b],0),5);assert.deepStrictEqual([a.toString(),b.toString('latin1')],['ab','cde\0']);
        const [written]=await new Promise(resolve=>fs.writev(fd,[Buffer.from('XY')],1,(error,count,buffers)=>resolve([count,buffers])));assert.strictEqual(written,2);
        assert.strictEqual(await new Promise(resolve=>fs.readv(fd,[a],0,(error,count)=>resolve(count+a.toString()))),'2aX');
        fs.closeSync(fd);
        const handle=await fs.promises.open(path.join(root,'v.bin'),'r+');
        assert.strictEqual((await handle.writev([Buffer.from('12')],3)).bytesWritten,2);
        const c=Buffer.alloc(5);assert.deepStrictEqual([(await handle.readv([c],0)).bytesRead,c.toString()],[5,'aXY12']);
        await handle.close();
        assert.throws(()=>fs.readvSync(fd,'x'),{{code:'ERR_INVALID_ARG_TYPE'}});
    "#
    ));
}

#[test]
fn node_crypto_keys_signatures_and_kdfs_match_node() {
    run(r#"
        const {assert,crypto,util}=__apps,msg=Buffer.from('hello h2o'),hash=value=>crypto.createHash('sha256').update(value).digest('hex');
        const rsaDer='MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQC1g8caNlHfV5pKM5XtVKZYhyipR8ftOOdvAKj8Nd1/AoU9cF42yGHO5PZxkCbIRYISOfLcGqyRqOp6kIPisVHvA/ixKUdGX0xEoCVhbHWMTTWiXlXXe+vnVcFbRMyMWA9sf6fDzlV1a1N75AnPzHfpbWsrLoS3gvZnsWE0TYHQeRRH5iz/VsMc62IJIhnPEhy1AiFCtKd4guycGbi5oT+HTfIy4Sw4xUrfjIB7H7Xn54mVeDR4dDEXtv/PQYtP1Wuq9NgPy0gFZ8fu7ENwyDBZY0QQ6/kzVgMua6C9DVxnFA2hTgnLO+AQLLaRN5iKWzfjQ4iz4harp0Nmze/Va7v7AgMBAAECggEAGWakr6yHU4pg+zOcg6xTZP3GY74O98mn9Wwxua2uCTk957r4VuaUGA20K8Xg734ZStn/fEFirIrnWUSCTZsZMf9WcjBVGuojdFJhjhsimKzbWGqmzPNjWqmxAnKDxLteroUaDI2CKbQzEsTMLLSv2SN976U1tFqS3IWW6pNSxcIBg4fSub9PgVwoo4SoOcr6njftKrH/U3D65TsoYshy8wEkbpIL7zOiaW3TuLceQMx9U6rEtEGza92mVKOsRTqNluOvXJr/nmQcrNxpShf9A3sXfz7TgHkAbUFwWKT8UYY0oIiFvMhirOulx14mbSOKqcUOx8QqTMxlHVuzXnVBpQKBgQD7RM2ALY7niUqjpb+/jUsfh+iexOXmdg9Cks0beiI3ZlRD4BCYkLTS55PtRqLi7vKY/nFlyPwvOVS3G1UFNiHQJJSqeu1hLR8X7vYwX3ZD/TuipAn8XaWSLu/xzZnLpEqne887QQuiVjqP3XhpUcQVIQChPFj94WaAciGfAQi5pwKBgQC47rzs4j+qPnWCoS+Vvf3bBxgVrlJM+q7ss7XLQYK26mNFPKboBeJzLzD6v83VtwlqYbynZqndgmgfKy4NrxrGyIJde34cLUPXt4qtbyhdxN6fjOP+wwBA9fpIUW7IswwlOz/yerPI9JHAMZs/MFjzKrP+nenfD6eVZy4CglANjQKBgQDmblSfxigWAnxm06yjuKo6QmN8p0nHLmIURZjAKCOU9s00ahoOMDcEa8fnhC4BO+1TjZTlKYrMrCEaw4AXBWvA/KsyJQXpZn3rbwRrnEBTXbsBDrrxq9gZ1d3KvSjb7tFgA1UrHB3VRIr0QMEapRfT4K45L6TVP47HIOPgdAxSZQKBgQCsSQXK7hB/ywZwtzd8mm6oIZjHggp8WsKiE94WSKa9P+lRtGqOGAVaZ6mKwSvCoW3tJLa7gcJYccgM2Y5jAzG/5UbGTmkzWNwuWHiikL8bdHAuVfSGACb5Xe9YdjzuTxfOi79MaBGb8nTih5RpLuJ/T9Y2K+mYfJ4UavaLMdngdQKBgQCn537D4+XAgxJKf0Z6fjSz1vUi39Tcmsn+oV7+nf7U8RfehFl69HarNpem5rfjmaUnghOSgd6gJVYPzlCI6AcyQfGdYUjeRxfl5/mWMtzSGX7rEM4wkhJVlmPAP3qth3i5zSfQ8/Mp8jX/+gn/FglHTOMOJDlncL8zJTi+FTEfuQ==';
        const rsa=crypto.createPrivateKey({key:Buffer.from(rsaDer,'base64'),format:'der',type:'pkcs8'}),rsaPublic=crypto.createPublicKey(rsa);
        const pkcs1=rsa.export({type:'pkcs1',format:'pem'});assert.strictEqual(hash(pkcs1),'d786c238780bf710c299c17e04599711a16629e2fb295f9ea39d34c2ba1ed4c2');
        assert.ok(crypto.createPrivateKey(pkcs1).equals(rsa));assert.ok(!rsa.equals(rsaPublic));
        assert.strictEqual(hash(rsaPublic.export({type:'spki',format:'der'})),'1dd714d2993e56cce33ceb3a100bac5b0e6238f3418d19c0a23d3ff5629ba6fe');
        assert.strictEqual(hash(rsaPublic.export({type:'pkcs1',format:'der'})),'73cb3b74d10306fa7f03641e02320bce5788648d73f2d0e7212502736868c4d9');
        assert.strictEqual(hash(JSON.stringify(rsa.export({format:'jwk'}))),'cc2cb7ddb9c0196c74cf1c44e46e21f4583678f0a46dbc62f930a7b2cfc4d108');
        assert.ok(crypto.createPrivateKey({key:rsa.export({format:'jwk'}),format:'jwk'}).equals(rsa));
        assert.deepStrictEqual([rsa.type,rsa.asymmetricKeyType,rsa.asymmetricKeyDetails,rsaPublic.type],['private','rsa',{modulusLength:2048,publicExponent:65537n},'public']);
        assert.strictEqual(crypto.sign('sha256',msg,rsa).toString('hex').slice(0,64),'6e6d4069d6ca270c300d20ff6e3e3a063ff09f762d53298eb08555b790443fe9');
        const signer=crypto.createSign('RSA-SHA256');signer.update('hello ');signer.update('h2o');assert.strictEqual(signer.sign(pkcs1,'hex').slice(0,64),'6e6d4069d6ca270c300d20ff6e3e3a063ff09f762d53298eb08555b790443fe9');
        const pss='sz3NYpZrXeggOHhz3kCAIIUyoTDefQ3BfzeYwzGsERIsHRHvCP+wpz7d1ZQNJi8/9TFOCfv76lABkLuw20rwWGKDAqA/XfCN03w8xTfQkNUvk9BEe8SvCF0p6GMvZKQTGx8yGO69BDJFDmheo0O044B4yHfvko7UvXmO5KYnOuV7Nmo2gUZxpfoSE2Ey7PJu6WMNa6QeXwoMwIbIRLz/wtrjiQqoSo8dfdTs0yEL1ezgFJtBvsrz/VBt5V4HCGHad39JQpXzAmBkM34teFzJN9oP8iwkfjuj+bBBmYRKaTaoDJ3BZYpBF1TKlF0bvoQS1tysi+2blLEmg/Sgg5wkRA==';
        const padding=crypto.constants.RSA_PKCS1_PSS_PADDING;
        assert.ok(crypto.verify('sha384',msg,{key:rsaPublic,padding},Buffer.from(pss,'base64')));
        assert.ok(crypto.verify('sha512',msg,{key:rsa,padding},crypto.sign('sha512',msg,{key:rsa,padding,saltLength:64})));
        assert.ok(!crypto.verify('sha256',msg,rsaPublic,Buffer.alloc(256)));
        assert.throws(()=>crypto.sign('sha256',msg,{key:rsa,padding,saltLength:10}),{code:'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM'});
        const ec=crypto.createPrivateKey('-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIIG+s5J1c36/gJS8B3T4WZgsm/mVPw9h0Imbgyj/1GpzoAoGCCqGSM49\nAwEHoUQDQgAElI3PLMevVdcqqsX4mEbJnHxcvDwuu6/D/LilFYQZ8ezzk1D9eER9\nkZBPBpBv/kAEJEuuNxYJ1eDm9HcFChC1mw==\n-----END EC PRIVATE KEY-----\n');
        const ecPem='-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAElI3PLMevVdcqqsX4mEbJnHxcvDwu\nu6/D/LilFYQZ8ezzk1D9eER9kZBPBpBv/kAEJEuuNxYJ1eDm9HcFChC1mw==\n-----END PUBLIC KEY-----\n';
        assert.deepStrictEqual(ec.export({format:'jwk'}),{kty:'EC',x:'lI3PLMevVdcqqsX4mEbJnHxcvDwuu6_D_LilFYQZ8ew',y:'85NQ_XhEfZGQTwaQb_5ABCRLrjcWCdXg5vR3BQoQtZs',crv:'P-256',d:'gb6zknVzfr-AlLwHdPhZmCyb-ZU_D2HQiZuDKP_UanM'});
        assert.strictEqual(ec.export({type:'pkcs8',format:'der'}).toString('base64'),'MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQggb6zknVzfr+AlLwHdPhZmCyb+ZU/D2HQiZuDKP/UanOhRANCAASUjc8sx69V1yqqxfiYRsmcfFy8PC67r8P8uKUVhBnx7POTUP14RH2RkE8GkG/+QAQkS643FgnV4Ob0dwUKELWb');
        assert.strictEqual(crypto.createPublicKey(ec).export({type:'spki',format:'pem'}),ecPem);assert.deepStrictEqual(ec.asymmetricKeyDetails,{namedCurve:'prime256v1'});
        assert.ok(crypto.verify('sha256',msg,ecPem,Buffer.from('MEUCIQCZSBGRm9oo/Y6idzvsJxXFy3LZvZvQ4suw0KeOgGZp0AIgd0ZaM7YXh4KR9PFedOAd85uaHUI4+6TYZWRvJPNlC0I=','base64')));
        assert.ok(crypto.verify('sha256',msg,{key:ecPem,dsaEncoding:'ieee-p1363'},Buffer.from('SHfQsOpqTWQGOP8Gnb4B1IO2JCUtO9LD4ihJBVU1Hw2yr0aVAdlypRqzwcIMAD8P4nUK93HOL8rnVTd88w+fhg==','base64')));
        const p1363=crypto.sign(null,msg,{key:ec,dsaEncoding:'ieee-p1363'});assert.strictEqual(p1363.length,64);assert.ok(crypto.verify(null,msg,{key:ec,dsaEncoding:'ieee-p1363'},p1363));
        const verifier=crypto.createVerify('sha256');verifier.update(msg);assert.ok(verifier.verify(ecPem,crypto.sign('sha256',msg,ec)));
        assert.throws(()=>crypto.sign('sha512',msg,ec),/P-256 keys with SHA-256/);
        const certificates=['-----BEGIN CERTIFICATE-----\nMIIBDDCBsgIJAJ0+4NzYE8qJMAoGCCqGSM49BAMCMA4xDDAKBgNVBAMMA2gybzAe\nFw0yNjExMDIwNzMzMjVaFw0zNjEwMzAwNzMzMjVaMA4xDDAKBgNVBAMMA2gybzBZ\nMBMGByqGSM49AgEGCCqGSM49AwEHA0IABJhLmqF04CXNo2cwI3eUzBqspDNeZZcs\nY4hdET7UOD1eIBE0xc6gTDAOhK6vX98fjKeWgXUIKQOdETq9Wgcs44swCgYIKoZI\nzj0EAwIDSQAwRgIhAIjj9M6qgebs/3QmbVW6YaHBG/MbIPJj/V0PKfW+IDp7AiEA\ntzF4aVQYhNX4YqXPbHKtosLXAoa4OMfxbtqHSmp09dE=\n-----END CERTIFICATE-----\n','-----BEGIN CERTIFICATE-----\nMIIBIzCBy6ADAgECAgkArnKlMWQNvl4wCgYIKoZIzj0EAwIwDjEMMAoGA1UEAwwD\naDJvMB4XDTI2MTEwMjA3MzM0MFoXDTM2MTAzMDA3MzM0MFowDjEMMAoGA1UEAwwD\naDJvMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEmEuaoXTgJc2jZzAjd5TMGqyk\nM15llyxjiF0RPtQ4PV4gETTFzqBMMA6Erq9f3x+Mp5aBdQgpA50ROr1aByzji6MS\nMBAwDgYDVR0RBAcwBYIDaDJvMAoGCCqGSM49BAMCA0cAMEQCIDhR+XN/BuK/LZGr\nLqCrh0ZlxYTQD5g/aMn2N4umdiN/AiA98y2nwnjq5OX3NV6mUyWhoLUYdQXBfieI\nV2WA6cE2hg==\n-----END CERTIFICATE-----\n'];
        for(const pem of certificates)assert.strictEqual(crypto.createPublicKey(pem).export({type:'spki',format:'der'}).toString('base64'),'MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEmEuaoXTgJc2jZzAjd5TMGqykM15llyxjiF0RPtQ4PV4gETTFzqBMMA6Erq9f3x+Mp5aBdQgpA50ROr1aByzjiw==');
        const ec384=crypto.createPrivateKey('-----BEGIN PRIVATE KEY-----\nMIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDBBGaOiYcvVt0Ti9NSA\nF/mVr6BFt95DB0/It+CuaspprPSc9zXk4bvJJM82VtnQZtahZANiAAQdZkCvhdUI\nYAQZtAa2L7QoQaSF3nye8v3u6TO6s/epQ17osHv/M2mMghG4miLZjo4oeRbLhhaN\ncDHea2SoXj+W97mOqeBAL3THY5NDx4NeSwQKX7ZdB84Za1FUWNlpEJs=\n-----END PRIVATE KEY-----\n');
        assert.ok(crypto.verify('sha384',msg,ec384,Buffer.from('MGUCMQCLDkOAs1e4uxnhaW7rZntDHBUu6mf0Vf3RONP2zCNOKxCahEaNFYzu+UVYimuAOwoCMGiBbtVlaKns10HHSIi0K2vXPUHn04VFl1+iuZtfk2uIHM5zotjC1iFheYflXYlcPQ==','base64')));
        assert.ok(crypto.verify('sha384',msg,ec384,crypto.sign('sha384',msg,ec384)));
        const ed=crypto.createPrivateKey('-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEICL95NdoRvNMwGUzfxxO354E/RxQrcT3ky9upJg3AGl2\n-----END PRIVATE KEY-----\n');
        assert.strictEqual(crypto.createPublicKey(ed).export({type:'spki',format:'der'}).toString('hex'),'302a300506032b657003210068646bbae5e0ceaf8564a58545c400d6735345a7103f3fbabd3d45603b4f0dea');
        assert.strictEqual(crypto.sign(undefined,msg,ed).toString('hex'),'d3780a92c554bd37eda3bf3b71ff5ae3ebfe31f8b717309374b07727e8fe75bb61b00c7b7628ed29590fabaf0b89da719d0e84f6b910ff91e13753b3ab5f310f');
        assert.deepStrictEqual(ed.export({format:'jwk'}),{crv:'Ed25519',d:'Iv3k12hG80zAZTN_HE7fngT9HFCtxPeTL26kmDcAaXY',x:'aGRruuXgzq-FZKWFRcQA1nNTRacQPz-6vT1FYDtPDeo',kty:'OKP'});
        assert.deepStrictEqual([ed.asymmetricKeyType,ed.asymmetricKeyDetails],['ed25519',{}]);
        for(const [type,o,digest] of [['ec',{namedCurve:'P-384'},'sha384'],['ec',{namedCurve:'prime256v1'}],['ed25519',{}]]){
            const {publicKey,privateKey}=await util.promisify(crypto.generateKeyPair)(type,o),data=Buffer.from('data');
            assert.ok(crypto.verify(digest,data,publicKey,crypto.sign(digest,data,privateKey)),type);
            const pem=crypto.generateKeyPairSync(type,{...o,privateKeyEncoding:{type:'pkcs8',format:'pem'}}).privateKey;assert.ok(crypto.createPrivateKey(pem).export({type:'pkcs8',format:'pem'})===pem);
        }
        assert.strictEqual(await new Promise((resolve,reject)=>crypto.sign(null,msg,ed,(error,value)=>error?reject(error):resolve(value.length))),64);
        assert.throws(()=>crypto.generateKeyPairSync('rsa',{modulusLength:2048}),{code:'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM'});
        assert.throws(()=>crypto.createPrivateKey('garbage'),{code:'ERR_OSSL_UNSUPPORTED'});
        assert.throws(()=>crypto.createPrivateKey(ecPem),{code:'ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE'});
        assert.throws(()=>ec.export({type:'pkcs1',format:'pem'}),{code:'ERR_CRYPTO_INCOMPATIBLE_KEY_OPTIONS'});
        assert.throws(()=>new crypto.KeyObject(),{code:'ERR_INVALID_ARG_VALUE'});
        const secret=crypto.createSecretKey('abc','utf8');
        assert.deepStrictEqual([secret.type,secret.symmetricKeySize,secret.asymmetricKeyType,secret.export().toString(),secret.export({format:'jwk'})],['secret',3,undefined,'abc',{kty:'oct',k:'YWJj'}]);
        assert.ok(secret.equals(crypto.createSecretKey(Buffer.from('abc'))));assert.ok(util.types.isKeyObject(secret)&&!util.types.isKeyObject({}));
        assert.strictEqual(crypto.createHmac('sha256',secret).update('x').digest('hex'),crypto.createHmac('sha256','abc').update('x').digest('hex'));
        assert.strictEqual(crypto.generateKeySync('aes',{length:256}).symmetricKeySize,32);
        assert.strictEqual(crypto.scryptSync('password','salt',32,{N:1024,r:8,p:2}).toString('hex'),'1765486cf4c5597fb5308d16b99794d7d3a0bd2e9464b9acb1ca32031a85dfed');
        assert.strictEqual((await util.promisify(crypto.scrypt)('','',64,{cost:16,blockSize:1,parallelization:1})).toString('hex'),'77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906');
        assert.throws(()=>crypto.scryptSync('p','s',16,{N:3}),{code:'ERR_CRYPTO_INVALID_SCRYPT_PARAMS'});
        assert.throws(()=>crypto.scryptSync('p','s',16,{N:1024,maxmem:1024}),{code:'ERR_CRYPTO_INVALID_SCRYPT_PARAMS'});
        assert.deepStrictEqual(['sha256','sha512','md5','sha224','sha1'].map(name=>Buffer.from(crypto.hkdfSync(name,'key','salt','info',42)).toString('hex').slice(0,20)),['9ca0d662557439e3b833','24156e2c35525baaf3d0','edd053d0030729baf813','0ff75af37d8684eb5b7a','a0bc90fe7a3fef57087f']);
        const derived=await util.promisify(crypto.hkdf)('sha256',Buffer.alloc(0),'','',16);assert.ok(derived instanceof ArrayBuffer);assert.strictEqual(Buffer.from(derived).toString('hex'),'eb70f01dede9afafa449eee1b1286504');
        assert.throws(()=>crypto.hkdfSync('sha256','k','s','i',255*32+1),{code:'ERR_CRYPTO_INVALID_KEYLEN'});
    "#);
}

#[test]
fn data_urls_load_binary_wasm_without_a_server() {
    run(r#"
        const raw=await fetch('data:application/octet-stream,%00%FF%23#ignored');
        if(new Uint8Array(await raw.arrayBuffer()).join(',')!=='0,255,35')throw Error('data bytes');
        const result=await WebAssembly.instantiateStreaming(fetch('data:application/wasm;base64,AGFzbQEAAAA='));
        if(!(result.instance instanceof WebAssembly.Instance))throw Error('Wasm data URL');
    "#);
}

#[test]
fn node_ticks_timers_and_immediates_run_in_node_order() {
    run(r#"
        const {assert,timers}=__apps;
        const order=[];
        await new Promise(resolve=>setTimeout(()=>{
            Promise.resolve().then(()=>order.push('promise'));process.nextTick(()=>order.push('tick'));queueMicrotask(()=>order.push('micro'));
            setTimeout(()=>{order.push('B');resolve();},1);setImmediate(()=>order.push('immediate'));order.push('A');
        },1));
        assert.deepStrictEqual(order,['A','tick','promise','micro','immediate','B']);
        const seen=[];
        setTimeout(()=>{seen.push('A');Promise.resolve().then(()=>seen.push('A-micro'));process.nextTick(()=>seen.push('A-tick'));},2);
        setTimeout(()=>seen.push('B'),2);
        await new Promise(resolve=>setTimeout(resolve,20));
        assert.deepStrictEqual(seen,['A','A-tick','A-micro','B']);
        const nested=[];
        process.nextTick(()=>{nested.push('t1');Promise.resolve().then(()=>nested.push('p-in-t1'));process.nextTick(()=>nested.push('t2'));});
        Promise.resolve().then(()=>{nested.push('p1');process.nextTick(()=>nested.push('t-in-p1'));});
        await new Promise(resolve=>setTimeout(resolve,5));
        assert.deepStrictEqual(nested,['p1','t1','t-in-p1','t2','p-in-t1']);
        const immediate=setImmediate(()=>{});
        assert.strictEqual(immediate.constructor.name,'Immediate');assert.strictEqual(immediate.hasRef(),true);clearImmediate(immediate);
        assert.strictEqual(typeof setTimeout(()=>{},1)[Symbol.dispose],'function');
        assert.throws(()=>process.nextTick(1),{code:'ERR_INVALID_ARG_TYPE'});
        const controller=new AbortController(),waiting=timers.promises.setTimeout(1000,'x',{signal:controller.signal});controller.abort();
        await assert.rejects(waiting,{name:'AbortError',code:'ABORT_ERR'});
        assert.strictEqual(await timers.promises.setImmediate('v'),'v');
    "#);
}

#[test]
fn node_process_env_constants_and_worker_exit() {
    run(r#"
        const {assert,module,util,worker_threads}=__apps;
        process.env.APPS_COERCE=42;assert.strictEqual(process.env.APPS_COERCE,'42');
        process.env.APPS_COERCE=undefined;assert.strictEqual(process.env.APPS_COERCE,'undefined');
        delete process.env.APPS_COERCE;assert.ok(!('APPS_COERCE' in process.env));
        assert.throws(()=>Object.defineProperty(process.env,'APPS_GETTER',{get(){}}),{code:'ERR_INVALID_OBJECT_DEFINE_PROPERTY'});
        const require=module.createRequire();
        assert.strictEqual(typeof require('constants').O_RDONLY,'number');assert.strictEqual(require('sys'),util);
        const prepare=Error.prepareStackTrace,holder={};Error.prepareStackTrace=(_,stack)=>stack;(function named(){Error.captureStackTrace(holder);})();Error.prepareStackTrace=prepare;
        assert.ok(holder.stack[0].getFunctionName()==='named'&&typeof holder.stack[0].getFileName()==='string'&&holder.stack[0].getLineNumber()>0&&holder.stack[0].isNative()===false);
        const worker=new worker_threads.Worker(`const{parentPort}=require('node:worker_threads');process.on('exit',code=>parentPort.postMessage('exit '+code));process.exit(7);parentPort.postMessage('after');`,{eval:true});
        const messages=[];worker.on('message',message=>messages.push(message));
        assert.strictEqual(await new Promise(resolve=>worker.on('exit',resolve)),7);
        assert.deepStrictEqual(messages,['exit 7']);
        const closing=new worker_threads.Worker(`require('node:worker_threads').parentPort.close();`,{eval:true});
        assert.strictEqual(await new Promise(resolve=>closing.on('exit',resolve)),0);
    "#);
}

#[test]
fn node_perf_hooks_and_worker_thread_helpers() {
    run(r#"
        const {assert,perf_hooks:{performance,PerformanceObserver,createHistogram,monitorEventLoopDelay},worker_threads}=__apps;
        const observed=new Promise(resolve=>new PerformanceObserver((list,observer)=>{observer.disconnect();resolve(list.getEntries().map(entry=>entry.entryType+':'+entry.name));}).observe({entryTypes:['mark','measure']}));
        performance.mark('a');performance.mark('b',{detail:1});const measure=performance.measure('ab','a','b');
        assert.ok(measure.entryType==='measure'&&measure.duration>=0);
        assert.deepStrictEqual(performance.getEntriesByType('mark').map(entry=>entry.name),['a','b']);
        assert.strictEqual(performance.getEntriesByName('b')[0].detail,1);
        assert.deepStrictEqual(await observed,['mark:a','mark:b','measure:ab']);
        performance.clearMarks();assert.strictEqual(performance.getEntriesByType('mark').length,0);
        assert.throws(()=>performance.measure('x','missing'),{name:'SyntaxError'});
        const histogram=createHistogram();for(let i=1;i<=100;i++)histogram.record(i);
        assert.deepStrictEqual([histogram.count,histogram.min,histogram.max,histogram.mean,histogram.percentile(50),histogram.percentile(99)],[100,1,100,50.5,50,99]);
        assert.deepStrictEqual([...histogram.percentiles.keys()],[0,50,75,87.5,93.75,96.875,98.4375,99.21875,100]);
        assert.throws(()=>histogram.record(0),{code:'ERR_OUT_OF_RANGE'});
        const delay=monitorEventLoopDelay({resolution:1});delay.enable();await new Promise(resolve=>setTimeout(resolve,20));delay.disable();assert.ok(delay.count>0&&delay.max>0);
        const {port1,port2}=new worker_threads.MessageChannel();port1.postMessage({x:1});
        assert.deepStrictEqual(worker_threads.receiveMessageOnPort(port2),{message:{x:1}});assert.strictEqual(worker_threads.receiveMessageOnPort(port2),undefined);
        const a=new worker_threads.BroadcastChannel('apps'),b=new worker_threads.BroadcastChannel('apps');
        assert.strictEqual(await new Promise(resolve=>{b.onmessage=event=>resolve(event.data);a.postMessage('hi');}),'hi');a.close();b.close();
        const main=new worker_threads.BroadcastChannel('cross');
        const worker=new worker_threads.Worker(`const{BroadcastChannel,parentPort}=require('node:worker_threads');const channel=new BroadcastChannel('cross');channel.onmessage=event=>{channel.postMessage({pong:event.data});channel.close();};parentPort.postMessage('ready');`,{eval:true});
        const exited=new Promise(resolve=>worker.on('exit',resolve));await new Promise(resolve=>worker.once('message',resolve));
        const reply=new Promise(resolve=>{main.onmessage=event=>resolve(event.data);});main.postMessage(new Uint8Array([7]));
        assert.deepStrictEqual(await reply,{pong:new Uint8Array([7])});main.close();
        assert.strictEqual(await exited,0);
        const flood=new worker_threads.BroadcastChannel('flood');
        const busy=new worker_threads.Worker(`const{BroadcastChannel,parentPort}=require('node:worker_threads');const channel=new BroadcastChannel('flood');let count=0;channel.onmessage=event=>{count++;if(event.data==='last'){parentPort.postMessage(count);channel.close();}};parentPort.postMessage('ready');const end=Date.now()+300;while(Date.now()<end);`,{eval:true});
        await new Promise(resolve=>busy.once('message',resolve));
        for(let n=0;n<5000;n++)flood.postMessage('x');
        await new Promise(resolve=>setTimeout(resolve,500));
        const counted=new Promise(resolve=>busy.once('message',resolve));flood.postMessage('last');
        const received=await counted;assert.ok(received>=4097&&received<=5001,'a reader that fell behind still gets later messages: '+received);flood.close();
        assert.strictEqual(typeof worker_threads.SHARE_ENV,'symbol');
    "#);
}
