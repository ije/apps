const assert = (value, message) => { if (!value) throw Error(message); };
const addon = __appsLoadAddon('fixture/addon.node');
assert(addon === __appsLoadAddon('fixture/addon.node'), 'addon cache');
assert(addon.add(20, 21) === 42, 'numbers and callback data');
assert(addon.text('你好') === '你', 'UTF-8 truncation');
assert(addon.text('a\0b') === 'a\0b', 'embedded null');
const bytes = new Uint8Array([1,2,3,4]);
const copy = addon.mutate(bytes.subarray(1, 3));
assert(bytes.join() === '1,3,4,4' && copy.join() === '3,4', 'buffer offsets and copying');
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
