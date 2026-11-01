import { dlopen, suffix } from 'node:ffi';
import { join } from 'node:path';
import addon from './build/Release/addon.node';

const { lib, functions } = dlopen(join(import.meta.dirname, `build/Release/math.${suffix}`), {
  add: { arguments: ['int32', 'int32'], return: 'int32' },
  apply: { arguments: ['function', 'int32'], return: 'int32' },
});
const double = lib.registerCallback({ arguments: ['int32'], return: 'int32' }, value => value * 2);

export const napiAdd = addon.add;
export const ffiAdd = functions.add;
export const ffiDouble = value => functions.apply(double, value);
