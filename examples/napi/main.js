import addon, { add } from './build/Release/addon.node';

console.log('Native add:', add(20, 22));
console.log('Default export:', addon.add(1, 2));
