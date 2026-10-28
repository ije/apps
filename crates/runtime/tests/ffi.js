const ffi = __apps.ffi;
const assert = (value, message) => { if (!value) throw Error(message); };
const throws = (fn, message) => {
  let error;
  try { fn(); } catch (caught) { error = caught; }
  assert(error, message);
  return error;
};
const { lib, functions } = ffi.dlopen(libraryPath, {
  add: { arguments: ['int32', 'int32'], return: 'int32' },
});
assert(lib.path === libraryPath && libraryPath.endsWith(ffi.suffix), 'Library path and suffix');
assert(functions.add(20, 22) === 42, 'Native call');
assert(lib.getFunctions() === lib.functions && lib.functions === functions, 'Function cache');
assert(lib.getFunction('add', { arguments: ['i32', 'i32'], return: 'i32' }) === functions.add, 'Alias cache');
assert(ffi.dlsym(lib, 'add') === functions.add.pointer && lib.getSymbols().add === functions.add.pointer, 'Symbol pointers');
throws(() => lib.getFunction('add', { return: 'double' }), 'Mismatched signature');
throws(() => functions.add(1), 'Missing arguments');
throws(() => functions.add(1, 2, 3), 'Extra arguments');
throws(() => lib.getFunction('missing_symbol'), 'Missing symbol');
throws(() => lib.getFunction('answer', { arguments: ['void'] }), 'Void argument');
throws(() => lib.getFunction('answer', { return: 'struct' }), 'Unsupported type');
throws(() => new ffi.DynamicLibrary('/apps-missing-library'), 'Missing library');
throws(() => ffi.dlopen(libraryPath, { missing_symbol: {} }), 'Failed definition cleanup');
assert(lib.getFunction('answer', { return: 'i32' })() === 42, 'Zero arguments');
assert(lib.getFunction('mixed', { arguments: ['i32', 'double', 'u64', 'float'], return: 'f64' })(-2, 1.5, 4n, 0.25) === 3.75, 'Mixed registers');
assert(lib.getFunction('many', { arguments: Array(10).fill('i32'), return: 'i32' })(1, 2, 3, 4, 5, 6, 7, 8, 9, 10) === 55, 'Stack arguments');

for (const [name, values, invalid] of [
  ['i8', [-128, 127], [-129, 128, 1.5, NaN, Infinity, 1n]],
  ['u8', [0, 255], [-1, 256, true]],
  ['i16', [-32768, 32767], [-32769, 32768]],
  ['u16', [0, 65535], [-1, 65536]],
  ['i32', [-2147483648, 2147483647], [-2147483649, 2147483648]],
  ['u32', [0, 4294967295], [-1, 4294967296]],
  ['i64', [-9223372036854775808n, 9223372036854775807n, 42], [-9223372036854775809n, 9223372036854775808n, 2 ** 53, NaN, 1.5]],
  ['u64', [0n, 18446744073709551615n, 42], [-1n, 18446744073709551616n, -1, Infinity]],
  ['f32', [-1.5, 0, Infinity, NaN], ['1']],
  ['f64', [-1.5, 1e100, -Infinity, NaN], [1n]],
]) {
  const sig = { arguments: [name], return: name };
  const fn = lib.getFunction(name, sig);
  const call = lib.getFunction('call_' + name, { arguments: ['function', name], return: name });
  const callback = lib.registerCallback(sig, value => value);
  for (const value of values) {
    const expected = name.endsWith('64') && name[0] !== 'f' ? BigInt(value) : value;
    assert(Object.is(fn(value), expected), name + ' return ' + value);
    assert(Object.is(call(callback, value), expected), name + ' callback ' + value);
  }
  for (const value of invalid) throws(() => fn(value), name + ' invalid ' + value);
  lib.unregisterCallback(callback);
}
assert(lib.getFunction('character', { arguments: ['char'], return: 'char' })(65) === 65, 'C char');
assert(ffi.types.INT_32 === 'int32' && ffi.types.ARRAY_BUFFER === 'arraybuffer', 'Type constants');
const strlen = lib.getFunction('string_length', { arguments: ['string'], return: 'u64' });
assert(strlen('🌊') === 4n && strlen('hello\0ignored') === 5n, 'UTF-8 and NUL');
const greet = lib.getFunction('greeting', { return: 'str' });
assert(typeof greet() === 'bigint' && ffi.toString(greet()) === 'hello', 'String return pointer');
assert(ffi.toString(0n) === null, 'Null string');
const ptr = lib.getFunction('pointer', { arguments: ['ptr'], return: 'pointer' });
assert(ptr(null) === 0n && ptr(undefined) === 0n && ptr(0n) === 0n, 'Null pointers');
throws(() => ptr(-1n), 'Negative pointer');
throws(() => ptr(1n << 64n), 'Pointer overflow');
throws(() => ptr(42), 'Number pointer');
const storage = new Uint8Array([9, 1, 2, 9]);
const address = ffi.getRawPointer(storage);
assert(ptr(storage) === address && ptr(storage.buffer) === address, 'Backing pointer');
assert(ptr(storage.subarray(1, 3)) === address + 1n, 'Typed array offset');
assert(ptr(new DataView(storage.buffer, 2, 1)) === address + 2n, 'DataView offset');
const mutate = lib.getFunction('mutate', { arguments: ['buffer', 'u32'] });
assert(mutate(storage.subarray(1, 3), 2) === undefined && storage.join(',') === '9,2,3,9', 'Native writes to view');
if (typeof SharedArrayBuffer !== 'undefined') {
  const shared = new SharedArrayBuffer(8);
  const sharedBytes = new Uint8Array(shared);
  mutate(sharedBytes.subarray(1, 3), 2);
  assert(sharedBytes[1] === 1 && ffi.getRawPointer(shared) === ffi.getRawPointer(sharedBytes), 'Shared buffer');
}

const memory = new Uint8Array(64);
const base = ffi.getRawPointer(memory);
for (const [name, value] of [['Int8', -123], ['Uint8', 254], ['Int16', -12345], ['Uint16', 54321], ['Int32', -123456789], ['Uint32', 4000000000], ['Int64', -9007199254740993n], ['Uint64', 18446744073709551615n], ['Float32', 1.25], ['Float64', Math.PI]]) {
  ffi['set' + name](base, 1, value);
  assert(ffi['get' + name](base, 1) === value, name + ' unaligned memory');
}
ffi.setInt32(base, 0, 42);
assert(ffi.getInt32(base) === 42, 'Default offset');
ffi.setInt64(base, 0, 42);
assert(ffi.getInt64(base) === 42n, 'Safe integer setter');
throws(() => ffi.setInt64(base, 0, 2 ** 53), 'Unsafe integer setter');
throws(() => ffi.setUint8(base, 0, 256), 'Invalid memory value');
throws(() => ffi.setUint8(base, 1), 'Missing explicit value');
throws(() => ffi.getInt32(base, -1), 'Negative offset');
throws(() => ffi.getInt32(0n, 1), 'Null pointer with offset');
throws(() => ffi.setInt32(0n, 1, 42), 'Null destination with offset');
throws(() => ffi.toArrayBuffer(base, -1), 'Negative length');
throws(() => ffi.toArrayBuffer(0n, 1), 'Null memory');
assert(ffi.toArrayBuffer(0n, 0).byteLength === 0, 'Empty copy');
ffi.exportString('hello', base, memory.length);
assert(ffi.toString(base) === 'hello' && memory[5] === 0, 'Export string terminator');
throws(() => ffi.exportString('hello', base, 5), 'String capacity');
const copied = ffi.toBuffer(base, 5);
const borrowed = ffi.toBuffer(base, 5, false);
borrowed[0] = 72;
assert(copied.toString() === 'hello' && borrowed.toString() === 'Hello' && memory[0] === 72, 'Copied and borrowed buffers');
const array = new Uint8Array(ffi.toArrayBuffer(base, 5, false));
array[0] = 104;
assert(memory[0] === 104, 'Borrowed array buffer');
const arrayCopy = new Uint8Array(ffi.toArrayBuffer(base, 5));
arrayCopy[0] = 72;
assert(memory[0] === 104, 'Copied array buffer');
ffi.exportString('🌊', base, 6, 'utf16le');
assert(new DataView(memory.buffer).getUint16(0, true) === 0xd83c && memory[4] === 0 && memory[5] === 0, 'UTF-16 export');
ffi.exportBuffer(storage.subarray(1, 3), base, 2);
assert(memory[0] === 2 && memory[1] === 3, 'Export subarray');
ffi.exportArrayBufferView(new DataView(storage.buffer, 2, 1), base, 1);
assert(memory[0] === 3, 'Export DataView');
ffi.exportArrayBuffer(storage.buffer, base, 4);
assert(memory.subarray(0, 4).join(',') === '9,2,3,9', 'Export array buffer');
ffi.exportBuffer(memory.subarray(0, 3), base + 1n, 3);
assert(memory.subarray(0, 4).join(',') === '9,9,2,3', 'Overlapping copy');
throws(() => ffi.exportBuffer(storage, base, 3), 'Export capacity');
const number = lib.getSymbol('number');
assert(ffi.getInt32(number) === 123, 'Data symbol');
ffi.setInt32(number, 0, 456);
assert(ffi.getInt32(number) === 456, 'Writable data symbol');
ffi.setInt32(number, 0, 123);

const call = lib.functions.call_i32;
let recursive;
recursive = lib.registerCallback({ arguments: ['i32'], return: 'i32' }, value => value ? call(recursive, value - 1) + value : 0);
assert(call(recursive, 4) === 10, 'Reentrant native calls');
lib.unrefCallback(recursive);
lib.refCallback(recursive);
assert(call(recursive, 3) === 6, 'Callback refs');
lib.unregisterCallback(recursive);
throws(() => lib.refCallback(recursive), 'Unknown callback');
const failure = lib.registerCallback({ arguments: ['i32'], return: 'i32' }, () => { throw Error('callback failure'); });
assert(throws(() => call(failure, 1), 'Callback exception').message.includes('callback failure'), 'Callback error propagated');
assert(lib.functions.add(1, 2) === 3, 'Recovery after callback error');
lib.unregisterCallback(failure);
const promise = lib.registerCallback({ arguments: ['i32'], return: 'i32' }, () => Promise.resolve(1));
throws(() => call(promise, 1), 'Promise callback');
lib.unregisterCallback(promise);
let active;
active = lib.registerCallback({ arguments: ['i32'], return: 'i32' }, value => {
  throws(() => lib.close(), 'Close during callback');
  throws(() => lib.unregisterCallback(active), 'Unregister during callback');
  return value;
});
assert(call(active, 7) === 7, 'Active callback survives invalid cleanup');
lib.unregisterCallback(active);
let notified = 0;
const notify = lib.registerCallback(() => { notified++; });
lib.getFunction('call_void', { arguments: ['function'] })(notify);
assert(notified === 1, 'Default void callback');
throws(() => lib.getFunction('call_thread', { arguments: ['function'] })(notify), 'Foreign thread callback');
assert(notified === 1, 'Foreign thread does not enter JSC');
lib.unregisterCallback(notify);
const pointerCallback = lib.registerCallback({ arguments: ['ptr'], return: 'ptr' }, value => value);
assert(lib.getFunction('call_pointer', { arguments: ['function', 'ptr'], return: 'ptr' })(pointerCallback, base) === base, 'Pointer callback');
lib.unregisterCallback(pointerCallback);
const current = new ffi.DynamicLibrary(null);
assert(current.path === null, 'Current process library');
current.close();
throws(() => ffi.getCurrentEventLoop(), 'No libuv loop');
ffi.dlclose(lib);
lib.close();
throws(() => functions.add(1, 2), 'Closed function');
throws(() => lib.getSymbol('number'), 'Closed library');
throws(() => lib.registerCallback(() => {}), 'Closed callback registration');
const retained = new ffi.DynamicLibrary(libraryPath);
retained.getFunction('answer', { return: 'i32' });
retained.registerCallback(() => {});
