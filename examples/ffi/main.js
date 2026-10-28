import { dlopen, suffix } from 'node:ffi';

const { lib, functions } = dlopen(`./examples/ffi/math.${suffix}`, {
  add: { arguments: ['int32', 'int32'], return: 'int32' },
  apply: { arguments: ['function', 'int32'], return: 'int32' },
});

try {
  console.log(functions.add(20, 22));
  const double = lib.registerCallback(
    { arguments: ['int32'], return: 'int32' },
    value => value * 2,
  );
  console.log(functions.apply(double, 42));
  lib.unregisterCallback(double);
} finally {
  lib.close();
}
