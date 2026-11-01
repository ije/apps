use apps_runtime::{
    dom::Document,
    host::{Host, Page, Role},
    jsc::{Api, Engine},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

fn run(source: &str) {
    let engine = Engine::new(Api::load().unwrap(), Rc::default(), Role::Main).unwrap();
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
    let api = Api::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
    main.eval("__apps.ipcMain.handle('sum',(_e,a,b)=>a+b);__apps.ipcMain.handle('double',(_e,v)=>v.map(x=>x*2));__apps.ipcMain.on('version',e=>e.returnValue='v1')","test").unwrap();
    preload.eval("__apps.contextBridge.exposeInMainWorld('desktop',{sum:(a,b)=>__apps.ipcRenderer.invoke('sum',a,b),double:v=>__apps.ipcRenderer.invoke('double',v),version:__apps.ipcRenderer.sendSync('version'),subscribe:callback=>{const listener=(_event,data)=>callback(data);__apps.ipcRenderer.on('change',listener);return()=>__apps.ipcRenderer.off('change',listener);}});","test").unwrap();
    let page = Engine::new(api, host, Role::Renderer(1)).unwrap();
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
fn bridge_capabilities_failures_and_closed_contexts() {
    let api = Api::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body></body>"),
            dirty: false,
        },
    );
    let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
    let preload = Engine::new(api.clone(), host.clone(), Role::Preload(1)).unwrap();
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
    let page = Engine::new(api.clone(), host.clone(), Role::Renderer(1)).unwrap();
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
    let page = Engine::new(api, host.clone(), Role::Renderer(1)).unwrap();
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

fn contexts() -> (Rc<RefCell<Host>>, Engine, Engine) {
    let api = Api::load().unwrap();
    let host = Rc::new(RefCell::new(Host::default()));
    host.borrow_mut().pages.insert(
        1,
        Page {
            doc: Document::parse("<body><p id='x'>before</p><span>a</span><span>b</span></body>"),
            dirty: false,
        },
    );
    let main = Engine::new(api.clone(), host.clone(), Role::Main).unwrap();
    main.eval(
        "__appsEvent('ready');globalThis.win=new __apps.BrowserWindow();globalThis.log=[];",
        "test",
    )
    .unwrap();
    let preload = Engine::new(api, host.clone(), Role::Preload(1)).unwrap();
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
    let page = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
    let page = Engine::new(Api::load().unwrap(), host, Role::Renderer(1)).unwrap();
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
        const {assert,http,https,net,stream}=__apps;
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
fn node_net_reads_while_writing_and_reuses_socket_paths() {
    run(r#"
        const {assert,net,os,path,fs}=__apps,size=16<<20;
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
        fs.closeSync(fd);
        assert.throws(()=>fs.closeSync(fd),{{code:'EBADF',syscall:'close',message:'EBADF: bad file descriptor, close'}});
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
        assert.strictEqual(crypto.constants.RSA_PKCS1_PADDING,1);assert.deepStrictEqual(crypto.getCurves(),['prime256v1']);
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
        assert.strictEqual(typeof worker_threads.SHARE_ENV,'symbol');
    "#);
}
