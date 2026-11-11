const assert = (value, message) => { if (!value) throw Error(message); };
const addon = __appsLoadAddon('fixture/addon.node');
assert(addon === __appsLoadAddon('fixture/addon.node'), 'addon cache');
assert(addon.add(20, 21) === 42, 'numbers and callback data');
assert(addon.text('你好') === '你', 'UTF-8 truncation');
assert(addon.text('a\0b') === 'a\0b', 'embedded null');
const bytes = new Uint8Array([1,2,3,4]);
const copy = addon.mutate(bytes.subarray(1, 3));
assert(bytes.join() === '1,3,4,4' && copy.join() === '3,4', 'buffer offsets and copying');
assert(Buffer.isBuffer(copy) && copy.toString('base64') === 'AwQ=', 'addon buffers are Node buffers');
const viewBytes = new Uint8Array([1, 2, 3, 4]);
assert(addon.view(new DataView(viewBytes.buffer, 1, 2)) === 2 && viewBytes.join() === '1,3,4,4', 'dataview info');
for (const fake of [{ __proto__: DataView.prototype, buffer: 1 }, { __proto__: DataView.prototype, buffer: viewBytes.buffer, byteOffset: 2, byteLength: 1e9 }]) {
  let rejected = false;
  try { addon.view(fake); } catch (error) { rejected = error.code === 'ERR_FIXTURE'; }
  assert(rejected, 'spoofed dataview');
}
assert(addon.int64(-42.9) === -42 && addon.int64(Infinity) === 0 && addon.int64(NaN) === 0, 'int64 conversion');
assert(addon.call(value => addon.add(value, 2), 3) === 6, 'reentrant callbacks');
const error = new Error('caught');
assert(addon.caught(() => { throw error; }) === error, 'pending exceptions');
assert(addon.scoped() === 'escaped', 'escaped handle');
const object = {};
assert(addon.reference(object) === object, 'reference identity');
assert(addon.bigint(-9223372036854775808n) === -9223372036854775808n, 'signed bigint');
let failed = false;
try { addon.add('wrong', 1); } catch(error) { failed = error instanceof TypeError && error.code === 'ERR_FIXTURE'; }
assert(failed, 'native exceptions');
const counter = new addon.Counter();
assert(counter instanceof addon.Counter && counter.increment() === 1 && counter.increment() === 2, 'wrapped class');
assert(addon.state() === 1, 'context instance data');
globalThis.napiPromise = false;
addon.promise(42).then(value => { globalThis.napiPromise = value === 42; });
const node8 = addon.node8({ b: 1, 2: 2, [Symbol('s')]: 3, __proto__: { inherited: 1 } }, function (name) { return this.b + ':' + name; });
assert(node8.major === 24 && node8.noLoop === 1 && node8.detached === 1 && node8.mismatch === 1 && node8.memory === 1, 'Node-API 8 helpers');
assert(node8.keys.length === 2 && node8.keys[0] === 2 && node8.keys[1] === 'b', 'all property names');
assert(node8.words === -(2n ** 64n + 1n), 'bigint words');
assert(node8.needed === 1, 'a short buffer still reports the words a bigint needs');
assert(node8.called === '1:fixture', 'make callback');
assert(node8.status === 6 && node8.message === 'A number was expected', 'last error info');
globalThis.threadsafeValues = [];
globalThis.threadsafeStart = addon.threadsafeCount();
addon.threadsafe(value => threadsafeValues.push(value));
addon.work();
