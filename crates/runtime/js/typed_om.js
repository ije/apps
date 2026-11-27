// CSS Typed OM. dom.js makes the globals lazy, so `make` runs only when a page first uses them.
globalThis.__appsTypedOM = {
  names: 'CSSStyleValue CSSNumericValue CSSUnitValue CSSMathValue CSSMathSum CSSMathProduct CSSMathNegate CSSMathInvert CSSMathMin CSSMathMax CSSMathClamp CSSNumericArray CSSKeywordValue CSSUnparsedValue CSSVariableReferenceValue CSSImageValue CSSPositionValue CSSTransformValue CSSTransformComponent CSSTranslate CSSRotate CSSScale CSSSkew CSSSkewX CSSSkewY CSSPerspective CSSMatrixComponent StylePropertyMapReadOnly StylePropertyMap',
  // The unit functions of `CSS`, by base type: number, percent, length, angle, time, frequency, resolution, flex.
  units: 'number|percent|em rem ex rex ch rch ic ric lh rlh cap rcap vw vh vi vb vmin vmax svw svh svi svb svmin svmax lvw lvh lvi lvb lvmin lvmax dvw dvh dvi dvb dvmin dvmax cqw cqh cqi cqb cqmin cqmax cm mm Q in pt pc px|deg grad rad turn|s ms|Hz kHz|dpi dpcm dppx x|fr',
  // `property` gives the name of a property as written in the maps, or null. `parse` gives the specified text of a
  // valid value, or null. `computed` gives the computed value of a property.
  make({ property, parse, supports, longhands, computed, computedNames, layers, known }) {
    const S = Symbol('state'), T = Symbol('text');
    const { DOMMatrix } = globalThis, escape = globalThis.CSS.escape;
    const illegal = () => { throw new TypeError('Illegal constructor'); };
    const fail = () => { throw new TypeError('Invalid CSS value'); };
    const syntax = () => { throw new DOMException('Invalid CSS text', 'SyntaxError'); };
    // Like Chrome's `%.6g`.
    const fmt = n => {
      if (!n) return '0';
      const [mantissa, power] = n.toExponential(5).split('e'), e = +power;
      if (e < -4 || e >= 6) return mantissa.replace(/\.?0+$/, '') + 'e' + (e < 0 ? '-' : '+') + String(Math.abs(e)).padStart(2, '0');
      const fixed = n.toFixed(5 - e);
      return fixed.includes('.') ? fixed.replace(/\.?0+$/, '') : fixed;
    };
    const bases = ['length', 'angle', 'time', 'frequency', 'resolution', 'flex', 'percent'];
    const unitBase = new Map(), groups = this.units.split('|');
    groups.forEach((list, i) => { for (const unit of list.split(' ')) unitBase.set(unit.toLowerCase(), ['', 'percent', 'length', 'angle', 'time', 'frequency', 'resolution', 'flex'][i]); });
    const canonical = { length: 'px', angle: 'deg', time: 's', frequency: 'hz', resolution: 'dppx' };
    const cm = 96 / 2.54, scale = { px: 1, cm, mm: cm / 10, q: cm / 40, in: 96, pt: 96 / 72, pc: 16, deg: 1, grad: 0.9, rad: 180 / Math.PI, turn: 360, s: 1, ms: 0.001, hz: 1, khz: 1000, dppx: 1, x: 1, dpi: 1 / 96, dpcm: 1 / cm };
    const canonOf = unit => unit in scale ? canonical[unitBase.get(unit)] : unit;
    const unitName = unit => { unit = String(unit).toLowerCase(); unit = unit === '%' ? 'percent' : unit; return unitBase.has(unit) ? unit : null; };

    // Types are objects of exponents by base type, with `hint` for the percent hint.
    const typeOf = unit => { const base = unitBase.get(unit); return base ? { [base]: 1 } : {}; };
    const hinted = (type, hint) => { const out = { ...type, hint }; if (out.percent) { out[hint] = (out[hint] ?? 0) + out.percent; delete out.percent; if (!out[hint]) delete out[hint]; } return out; };
    const same = (a, b) => bases.every(base => (a[base] ?? 0) === (b[base] ?? 0));
    const addTypes = (a, b) => {
      if (a.hint && b.hint && a.hint !== b.hint) return null;
      if (a.hint) b = hinted(b, a.hint); else if (b.hint) a = hinted(a, b.hint);
      if (same(a, b)) return a.hint ? a : b;
      if ((a.percent || b.percent) && bases.some(base => base !== 'percent' && (a[base] || b[base]))) for (const hint of bases.slice(0, 6)) { const x = hinted(a, hint); if (same(x, hinted(b, hint))) return x; }
      return null;
    };
    const multiplyTypes = (a, b) => {
      if (a.hint && b.hint && a.hint !== b.hint) return null;
      if (a.hint) b = hinted(b, a.hint); else if (b.hint) a = hinted(a, b.hint);
      const out = {};
      for (const base of bases) if ((a[base] ?? 0) + (b[base] ?? 0)) out[base] = (a[base] ?? 0) + (b[base] ?? 0);
      if (a.hint || b.hint) out.hint = a.hint || b.hint;
      return out;
    };
    const fold = (values, join) => values.map(value => value[S].type).reduce((a, b) => a && join(a, b)) ?? fail();

    // Indexed lists. `check` makes a list writable at indices up to its length.
    const list = (self, items, check) => new Proxy(self, {
      get: (target, key, receiver) => typeof key === 'string' && /^\d+$/.test(key) ? items[key] : Reflect.get(target, key, receiver),
      set: (target, key, value, receiver) => {
        if (typeof key !== 'string' || !/^\d+$/.test(key)) return Reflect.set(target, key, value, receiver);
        if (!check) return false;
        if (+key > items.length) throw new RangeError('Index out of range');
        items[key] = check(value);
        return true;
      },
      has: (target, key) => typeof key === 'string' && /^\d+$/.test(key) ? +key < items.length : key in target,
      ownKeys: target => [...items.keys()].map(String).concat(Reflect.ownKeys(target)),
      getOwnPropertyDescriptor: (target, key) => typeof key === 'string' && /^\d+$/.test(key) ? +key < items.length ? { value: items[key], writable: !!check, enumerable: true, configurable: true } : undefined : Reflect.getOwnPropertyDescriptor(target, key),
    });
    const listMethods = {
      get length() { return this[S].items.length; },
      entries() { return this[S].items.entries(); }, keys() { return this[S].items.keys(); }, values() { return this[S].items.values(); },
      forEach(callback, self) { this[S].items.forEach((value, index) => callback.call(self, value, index, this)); },
    };
    const listy = Type => { Object.defineProperties(Type.prototype, Object.getOwnPropertyDescriptors(listMethods)); Type.prototype[Symbol.iterator] = listMethods.values; };

    class CSSStyleValue {
      constructor() { if (abstract.has(new.target)) illegal(); }
      toString() { return this[T](); }
      [T]() { return this[S].text; }
      static parse(name, text) { return parseAll(name, text)[0]; }
      static parseAll(name, text) { return parseAll(name, text); }
    }
    const unsupported = (name, text) => { const value = Object.create(CSSStyleValue.prototype); value[S] = { name, text }; return value; };

    const rectify = value => typeof value === 'number' ? new CSSUnitValue(value, 'number') : value instanceof CSSNumericValue ? value : fail();
    const unit = (value, name) => new CSSUnitValue(value, name);
    const negate = value => value instanceof CSSUnitValue ? unit(-value.value, value.unit) : value instanceof CSSMathNegate ? value.value : new CSSMathNegate(value);
    const invert = value => {
      if (value instanceof CSSUnitValue && value.unit === 'number') return value.value ? unit(1 / value.value, 'number') : (() => { throw new RangeError('Division by zero'); })();
      return value instanceof CSSMathInvert ? value.value : new CSSMathInvert(value);
    };
    const arith = (self, args, Kind) => {
      const values = [...(self instanceof Kind ? self[S].items : [self]), ...args.map(rectify)];
      if (values.every(value => value instanceof CSSUnitValue)) {
        if (Kind === CSSMathProduct) {
          const units = values.filter(value => value.unit !== 'number');
          if (units.length < 2) return unit(values.reduce((product, value) => product * value.value, 1), units[0]?.unit ?? 'number');
        } else if (values.every(value => value.unit === values[0].unit)) {
          const numbers = values.map(value => value.value);
          return unit(Kind === CSSMathSum ? numbers.reduce((sum, value) => sum + value) : Math[Kind === CSSMathMin ? 'min' : 'max'](...numbers), values[0].unit);
        }
      }
      return new Kind(...values);
    };
    const equal = (a, b) => a.constructor === b.constructor && (a instanceof CSSUnitValue ? a.value === b.value && a.unit === b.unit : a[S].items.length === b[S].items.length && a[S].items.every((value, i) => equal(value, b[S].items[i])));
    // A sum value: terms with a value and their units with powers, as in the spec. Null when it can't be made.
    const sumValue = value => {
      if (value instanceof CSSUnitValue) {
        const name = canonOf(value.unit), units = new Map(value.unit === 'number' ? [] : [[name, 1]]);
        return [{ value: value.value * (scale[value.unit] ?? 1) / (scale[name] ?? 1), units }];
      }
      const { op, items } = value[S], parts = items.map(sumValue);
      if (parts.includes(null)) return null;
      const key = term => [...term.units].sort().join();
      if (op === 'sum') {
        const out = [];
        for (const term of parts.flat()) { const found = out.find(other => key(other) === key(term)); if (found) found.value += term.value; else out.push({ ...term }); }
        return out;
      }
      if (op === 'negate') return parts[0].map(term => ({ ...term, value: -term.value }));
      if (op === 'invert') return parts[0].length === 1 ? [{ value: 1 / parts[0][0].value, units: new Map([...parts[0][0].units].map(([name, power]) => [name, -power])) }] : null;
      if (op === 'product') return parts.reduce((out, part) => out.flatMap(a => part.map(b => {
        const units = new Map(a.units);
        for (const [name, power] of b.units) { const sum = (units.get(name) ?? 0) + power; if (sum) units.set(name, sum); else units.delete(name); }
        return { value: a.value * b.value, units };
      })), [{ value: 1, units: new Map() }]);
      if (parts.some(part => part.length !== 1 || key(part[0]) !== key(parts[0][0]))) return null;
      const numbers = parts.map(part => part[0].value);
      return [{ value: op === 'min' ? Math.min(...numbers) : op === 'max' ? Math.max(...numbers) : Math.max(numbers[0], Math.min(numbers[1], numbers[2])), units: parts[0][0].units }];
    };
    // Converts one term of a sum value to a unit, or gives undefined.
    const convert = (term, name) => {
      const units = [...term.units];
      if (!units.length) return name === 'number' ? term.value : undefined;
      if (units.length === 1 && units[0][1] === 1 && units[0][0] === canonOf(name)) return term.value * (1 / (scale[name] ?? 1));
    };

    class CSSNumericValue extends CSSStyleValue {
      add(...values) { return arith(this, values, CSSMathSum); }
      sub(...values) { return arith(this, values.map(value => negate(rectify(value))), CSSMathSum); }
      mul(...values) { return arith(this, values, CSSMathProduct); }
      div(...values) { return arith(this, values.map(value => invert(rectify(value))), CSSMathProduct); }
      min(...values) { return arith(this, values, CSSMathMin); }
      max(...values) { return arith(this, values, CSSMathMax); }
      equals(...values) { return values.every(value => equal(this, rectify(value))); }
      to(name) {
        const target = unitName(name) ?? syntax(), sum = sumValue(this);
        const value = sum?.length === 1 ? convert(sum[0], target) : undefined;
        return value === undefined ? fail() : unit(value, target);
      }
      toSum(...names) {
        const targets = names.map(name => unitName(name) ?? syntax()), sum = sumValue(this) ?? fail();
        if (sum.some(term => term.units.size > 1 || [...term.units.values()].some(power => power !== 1))) fail();
        if (!targets.length) return new CSSMathSum(...sum.map(term => unit(term.value, term.units.keys().next().value ?? 'number')).sort((a, b) => a.unit < b.unit ? -1 : a.unit > b.unit ? 1 : 0));
        let rest = sum;
        const out = targets.map(target => {
          let total = 0;
          rest = rest.filter(term => { const value = convert(term, target); if (value === undefined) return true; total += value; return false; });
          return unit(total, target);
        });
        return rest.length ? fail() : new CSSMathSum(...out);
      }
      type() { const type = this[S].type, out = {}; for (const base of bases) if (type[base]) out[base] = type[base]; if (type.hint) out.percentHint = type.hint; return out; }
      static parse(text) { return numeric(String(text).trim(), false) ?? syntax(); }
    }
    class CSSUnitValue extends CSSNumericValue {
      constructor(value, name) {
        super();
        name = unitName(name) ?? fail();
        this[S] = { value: finite(value), unit: name, type: typeOf(name) };
      }
      get value() { return this[S].value; } set value(value) { this[S].value = finite(value); }
      get unit() { return this[S].unit; }
      [T]() { const { value, unit } = this[S]; return fmt(value) + (unit === 'number' ? '' : unit === 'percent' ? '%' : unit); }
    }
    const finite = value => { value = Number(value); return Number.isFinite(value) ? value : fail(); };
    class CSSMathValue extends CSSNumericValue {
      get operator() { return this[S].op; }
      [T](nested, bare) {
        const { op, items } = this[S], text = (value, nested, bare) => value[T](nested, bare);
        if (op === 'min' || op === 'max' || op === 'clamp') return op + '(' + items.map(value => text(value, false, true)).join(', ') + ')';
        const [open, close] = bare ? ['', ''] : [nested ? '(' : 'calc(', ')'];
        if (op === 'negate') return open + '-' + text(items[0], true) + close;
        if (op === 'invert') return open + '1 / ' + text(items[0], true) + close;
        const [joint, other, Inverse] = op === 'sum' ? [' + ', ' - ', CSSMathNegate] : [' * ', ' / ', CSSMathInvert];
        return open + items.map((value, i) => !i ? text(value, true) : value instanceof Inverse ? other + text(value[S].items[0], true) : joint + text(value, true)).join('') + close;
      }
    }
    const variadic = (self, op, args, join) => {
      const items = args.map(rectify);
      if (!items.length) syntax();
      self[S] = { op, items, type: fold(items, join) };
    };
    const valuesOf = self => self[S].list ??= list(Object.assign(Object.create(CSSNumericArray.prototype), { [S]: { items: self[S].items } }), self[S].items);
    class CSSMathSum extends CSSMathValue { constructor(...args) { super(); variadic(this, 'sum', args, addTypes); } get values() { return valuesOf(this); } }
    class CSSMathProduct extends CSSMathValue { constructor(...args) { super(); variadic(this, 'product', args, multiplyTypes); } get values() { return valuesOf(this); } }
    class CSSMathMin extends CSSMathValue { constructor(...args) { super(); variadic(this, 'min', args, addTypes); } get values() { return valuesOf(this); } }
    class CSSMathMax extends CSSMathValue { constructor(...args) { super(); variadic(this, 'max', args, addTypes); } get values() { return valuesOf(this); } }
    class CSSMathNegate extends CSSMathValue { constructor(value) { super(); value = rectify(value); this[S] = { op: 'negate', items: [value], type: value[S].type }; } get value() { return this[S].items[0]; } }
    class CSSMathInvert extends CSSMathValue {
      constructor(value) {
        super(); value = rectify(value);
        const type = {}; for (const base of bases) if (value[S].type[base]) type[base] = -value[S].type[base];
        if (value[S].type.hint) type.hint = value[S].type.hint;
        this[S] = { op: 'invert', items: [value], type };
      }
      get value() { return this[S].items[0]; }
    }
    class CSSMathClamp extends CSSMathValue {
      constructor(lower, value, upper) { super(); const items = [lower, value, upper].map(rectify); this[S] = { op: 'clamp', items, type: fold(items, addTypes) }; }
      get lower() { return this[S].items[0]; } get value() { return this[S].items[1]; } get upper() { return this[S].items[2]; }
    }
    class CSSNumericArray { constructor() { illegal(); } }
    listy(CSSNumericArray);

    class CSSKeywordValue extends CSSStyleValue {
      constructor(value) { super(); this.value = value; }
      get value() { return this[S].value; } set value(value) { value = String(value); this[S] = { value: value || fail() }; }
      [T]() { const value = this[S].value; return wide.test(value) || known(value) ? value.toLowerCase() : escape(value); }
    }
    const keyword = value => new CSSKeywordValue(value);
    class CSSVariableReferenceValue {
      constructor(variable, fallback = null) { this.variable = variable; if (fallback !== null && !(fallback instanceof CSSUnparsedValue)) fail(); this[S].fallback = fallback; }
      get variable() { return this[S].variable; }
      set variable(value) { value = String(value); if (!value.startsWith('--')) fail(); (this[S] ??= {}).variable = value; }
      get fallback() { return this[S].fallback; }
    }
    const member = value => value instanceof CSSVariableReferenceValue ? value : typeof value === 'string' ? value : fail();
    class CSSUnparsedValue extends CSSStyleValue {
      constructor(members) { super(); const items = Array.from(members, member); this[S] = { items }; return list(this, items, member); }
      // A comment keeps two pieces from joining into one token, as in Chrome.
      [T]() {
        let out = '';
        for (const item of this[S].items) {
          const text = typeof item === 'string' ? item : `var(${item.variable}${item.fallback ? ',' + item.fallback[T]() : ''})`;
          if (/[\w-]$/.test(out) && /^[\w(-]/.test(text) || /\d$/.test(out) && text[0] === '%' || out.endsWith('/') && text[0] === '*') out += '/**/';
          out += text;
        }
        return out;
      }
    }
    listy(CSSUnparsedValue);
    class CSSImageValue extends CSSStyleValue { constructor() { illegal(); } }
    class CSSPositionValue extends CSSStyleValue {
      constructor(x, y) { super(); this[S] = {}; this.x = x; this.y = y; }
      get x() { return this[S].x; } set x(value) { this[S].x = lengthLike(value, true); }
      get y() { return this[S].y; } set y(value) { this[S].y = lengthLike(value, true); }
      [T]() { return this[S].x + ' ' + this[S].y; }
    }

    // A numeric value of one base type, with or without percentages.
    const kind = (value, base, percent) => {
      const type = value instanceof CSSNumericValue ? value[S].type : fail(), keys = Object.keys(type).filter(key => key !== 'hint');
      const ok = base ? keys.length === 1 && type[base] === 1 && (!type.hint || percent && type.hint === base) || percent && keys.length === 1 && type.percent === 1 && !type.hint : !keys.length;
      return ok ? value : fail();
    };
    const lengthLike = (value, percent) => kind(value, 'length', percent);
    const angle = value => kind(value, 'angle');
    const number = value => kind(rectify(value), '');
    const px = value => value.to('px').value, deg = value => value.to('deg').value, tan = value => Math.tan(deg(value) * Math.PI / 180);
    const fields = 'm11 m12 m13 m14 m21 m22 m23 m24 m31 m32 m33 m34 m41 m42 m43 m44'.split(' ');
    const matrix = (m, is2D) => DOMMatrix.fromMatrix(Object.fromEntries([...Array.from(m.length === 6 ? [m[0], m[1], 0, 0, m[2], m[3], 0, 0, 0, 0, 1, 0, m[4], m[5], 0, 1] : m, (v, i) => [fields[i], v]), ['is2D', is2D]]));
    const flag = (self, value) => { self[S].is2D = !!value; };
    class CSSTransformComponent {
      constructor() { if (new.target === CSSTransformComponent) illegal(); }
      get is2D() { return this[S].is2D; } set is2D(value) { flag(this, value); }
      toMatrix() { return this[S].matrix(); }
      toString() { return this[T](); }
    }
    const define = (Type, names, check) => { for (const name of names) Object.defineProperty(Type.prototype, name, { get() { return this[S][name]; }, set(value) { this[S][name] = check(value); }, configurable: true, enumerable: true }); };
    class CSSTranslate extends CSSTransformComponent {
      constructor(x, y, z) { super(); this[S] = { x: lengthLike(x, true), y: lengthLike(y, true), z: z === undefined ? unit(0, 'px') : lengthLike(z), is2D: z === undefined, matrix: () => this[S].is2D ? matrix([1, 0, 0, 1, px(this[S].x), px(this[S].y)], true) : matrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, px(this[S].x), px(this[S].y), px(this[S].z), 1], false) }; }
      [T]() { const { x, y, z, is2D } = this[S]; return is2D ? `translate(${x}, ${y})` : `translate3d(${x}, ${y}, ${z})`; }
    }
    define(CSSTranslate, ['x', 'y'], value => lengthLike(value, true)); define(CSSTranslate, ['z'], value => lengthLike(value));
    class CSSRotate extends CSSTransformComponent {
      constructor(...args) {
        super();
        const is2D = args.length === 1;
        if (!is2D && args.length !== 4) fail();
        const [x, y, z] = is2D ? [0, 0, 1].map(n => unit(n, 'number')) : args.slice(0, 3).map(number);
        this[S] = { x, y, z, angle: angle(args[is2D ? 0 : 3]), is2D, matrix: () => { const s = this[S]; return s.is2D ? new DOMMatrix().rotateSelf(deg(s.angle)) : matrix(new DOMMatrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]).rotateAxisAngleSelf(s.x.to('number').value, s.y.to('number').value, s.z.to('number').value, deg(s.angle)).toFloat64Array(), false); } };
      }
      [T]() { const { x, y, z, angle, is2D } = this[S]; return is2D ? `rotate(${angle})` : `rotate3d(${x}, ${y}, ${z}, ${angle})`; }
    }
    define(CSSRotate, ['x', 'y', 'z'], number); define(CSSRotate, ['angle'], angle);
    class CSSScale extends CSSTransformComponent {
      constructor(x, y, z) {
        super();
        const n = value => number(value).to('number').value;
        this[S] = { x: number(x), y: number(y), z: z === undefined ? unit(1, 'number') : number(z), is2D: z === undefined, matrix: () => { const { x, y, z, is2D } = this[S]; return is2D ? matrix([n(x), 0, 0, n(y), 0, 0], true) : matrix([n(x), 0, 0, 0, 0, n(y), 0, 0, 0, 0, n(z), 0, 0, 0, 0, 1], false); } };
      }
      [T]() { const { x, y, z, is2D } = this[S]; return is2D ? `scale(${x}, ${y})` : `scale3d(${x}, ${y}, ${z})`; }
    }
    define(CSSScale, ['x', 'y', 'z'], number);
    const skew = (self, ax, ay) => { self[S] = { ax, ay, is2D: true, matrix: () => matrix([1, tan(self[S].ay), tan(self[S].ax), 1, 0, 0], true) }; };
    const fixed2D = Type => Object.defineProperty(Type.prototype, 'is2D', { get() { return true; }, set() {}, configurable: true, enumerable: true });
    class CSSSkew extends CSSTransformComponent {
      constructor(ax, ay) { super(); skew(this, angle(ax), angle(ay)); }
      [T]() { const { ax, ay } = this[S]; return ay instanceof CSSUnitValue && !ay.value ? `skew(${ax})` : `skew(${ax}, ${ay})`; }
    }
    define(CSSSkew, ['ax', 'ay'], angle); fixed2D(CSSSkew);
    class CSSSkewX extends CSSTransformComponent { constructor(ax) { super(); skew(this, angle(ax), unit(0, 'deg')); } [T]() { return `skewX(${this[S].ax})`; } }
    define(CSSSkewX, ['ax'], angle); fixed2D(CSSSkewX);
    class CSSSkewY extends CSSTransformComponent { constructor(ay) { super(); skew(this, unit(0, 'deg'), angle(ay)); } [T]() { return `skewY(${this[S].ay})`; } }
    define(CSSSkewY, ['ay'], angle); fixed2D(CSSSkewY);
    const depth = value => typeof value === 'string' || value instanceof CSSKeywordValue ? String(value) === 'none' ? keyword('none') : fail() : lengthLike(value);
    class CSSPerspective extends CSSTransformComponent {
      constructor(length) {
        super();
        this[S] = { length: depth(length), matrix: () => { const length = this[S].length; return matrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, length instanceof CSSKeywordValue ? 0 : -1 / px(length), 0, 0, 0, 1], true); } };
      }
      get is2D() { return false; } set is2D(value) {}
      [T]() { return `perspective(${this[S].length})`; }
    }
    define(CSSPerspective, ['length'], depth);
    class CSSMatrixComponent extends CSSTransformComponent {
      constructor(value, options) {
        super();
        if (!(value instanceof globalThis.DOMMatrixReadOnly)) fail();
        this[S] = { matrix: () => matrix(this[S].value.toFloat64Array(), this[S].is2D), value: DOMMatrix.fromMatrix(value), is2D: options?.is2D ?? value.is2D };
      }
      get matrix() { return this[S].value; } set matrix(value) { this[S].value = value instanceof globalThis.DOMMatrixReadOnly ? DOMMatrix.fromMatrix(value) : fail(); }
      [T]() { const m = this[S].value; return this[S].is2D ? `matrix(${[m.a, m.b, m.c, m.d, m.e, m.f].map(fmt).join(', ')})` : `matrix3d(${[...m.toFloat64Array()].map(fmt).join(', ')})`; }
    }
    const component = value => value instanceof CSSTransformComponent ? value : fail();
    class CSSTransformValue extends CSSStyleValue {
      constructor(transforms) { super(); const items = Array.from(transforms, component); if (!items.length) fail(); this[S] = { items }; return list(this, items, component); }
      get is2D() { return this[S].items.every(item => item.is2D); }
      toMatrix() { return this[S].items.reduce((m, item) => m.multiplySelf(item.toMatrix()), new DOMMatrix()); }
      [T]() { return this[S].items.join(' '); }
    }
    listy(CSSTransformValue);

    const split = (text, comma) => comma ? layers(text) : layers(text, ' ').filter(Boolean);
    // Math functions, simplified as Chrome does when it parses them. Null when the text is not numeric.
    const numeric = (text, bare) => {
      text = text.replace(/\/\*[\s\S]*?\*\//g, '').trim();
      const tokens = [], re = /\s*(?:([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)|(-?[a-z][\w-]*)\(|(-?[a-z][\w-]*)|(\S))/giy;
      let match;
      while (re.lastIndex < text.length && (match = re.exec(text))) tokens.push(match);
      if (re.lastIndex < text.length || !tokens.length) return null;
      let at = 0;
      const peek = () => tokens[at]?.[5], take = c => peek() === c && ++at;
      const sum = () => {
        const terms = [];
        const add = (value, minus) => {
          if (value instanceof CSSMathSum) { for (const term of value[S].items) add(term, minus); return; }
          if (minus) value = negate(value);
          if (value instanceof CSSMathNegate && value.value instanceof CSSUnitValue) value = negate(value.value);
          const same = value instanceof CSSUnitValue && terms.find(term => term instanceof CSSUnitValue && term.unit === value.unit);
          if (same) terms[terms.indexOf(same)] = unit(same.value + value.value, same.unit); else terms.push(value);
        };
        add(product());
        // As in CSS, `+` and `-` need whitespace on both sides.
        for (let minus; (minus = peek() === '-') || peek() === '+';) { if (!/^\s/.test(tokens[at][0]) || !/^\s/.test(tokens[++at]?.[0])) syntax(); add(product(), minus); }
        return terms.length === 1 ? terms[0] : new CSSMathSum(...terms.map((term, i) => i && term instanceof CSSUnitValue && term.value < 0 ? new CSSMathNegate(negate(term)) : term));
      };
      const product = () => {
        let value = factor();
        for (let divide; (divide = peek() === '/') || peek() === '*';) { at++; const next = factor(); value = divide ? scaled(value, invert(next)) : scaled(value, next); }
        return value;
      };
      // A number scales a unit or each term of a sum.
      const scaled = (a, b) => {
        if (a instanceof CSSUnitValue && a.unit === 'number') [a, b] = [b, a];
        if (b instanceof CSSUnitValue && b.unit === 'number') {
          if (b.value === 1) return a;
          if (a instanceof CSSUnitValue) return unit(a.value * b.value, a.unit);
          if (a instanceof CSSMathSum && a[S].items.every(term => term instanceof CSSUnitValue)) return new CSSMathSum(...a[S].items.map(term => unit(term.value * b.value, term.unit)));
        }
        return a.mul(b);
      };
      const factor = () => {
        const token = tokens[at++] ?? syntax();
        if (token[1] !== undefined) return unit(+token[1], token[2] ? unitName(token[2]) ?? syntax() : 'number');
        if (token[4] && /^(pi|e)$/i.test(token[4])) return unit(token[4].toLowerCase() === 'pi' ? Math.PI : Math.E, 'number');
        if (token[5] === '(') { const value = sum(); take(')') || syntax(); return value; }
        const name = token[3]?.toLowerCase();
        if (name === 'calc' || name === '-webkit-calc') { const value = sum(); take(')') || syntax(); return value; }
        if (name === 'min' || name === 'max' || name === 'clamp') {
          const args = [sum()];
          while (take(',')) args.push(sum());
          take(')') || syntax();
          if (name === 'clamp' && args.length !== 3) syntax();
          if (args.every(value => value instanceof CSSUnitValue && value.unit === args[0].unit)) {
            const numbers = args.map(value => value.value);
            return unit(name === 'clamp' ? Math.max(numbers[0], Math.min(numbers[1], numbers[2])) : Math[name](...numbers), args[0].unit);
          }
          return name === 'clamp' ? new CSSMathClamp(...args) : new (name === 'min' ? CSSMathMin : CSSMathMax)(...args);
        }
        return syntax();
      };
      try {
        const first = tokens[0][3]?.toLowerCase(), value = sum();
        if (at < tokens.length || tokens[0][5]) return null;
        // `calc()` stays a sum, also with one value.
        return /^(-webkit-)?calc$/.test(first) && !bare && !(value instanceof CSSMathValue && value[S].op !== 'sum' && value[S].op !== 'negate') ? value instanceof CSSMathSum ? value : new CSSMathSum(value) : value;
      } catch { return null; }
    };

    const transform = text => {
      const items = [];
      for (const part of split(text)) {
        const open = part.indexOf('('), name = part.slice(0, open).toLowerCase(), args = split(part.slice(open + 1, -1), true).map(arg => arg === 'none' ? keyword('none') : numeric(arg, true) ?? fail());
        const zero = unit(0, 'px'), one = unit(1, 'number'), flat = arg => arg instanceof CSSUnitValue && arg.unit === 'number' && !arg.value;
        const [a, b, c, d] = /^(translate|perspective)/.test(name) ? args.map(arg => flat(arg) ? zero : arg) : /^(rotate|skew)/.test(name) ? args.map((arg, i) => flat(arg) && (name[1] === 'k' || i === args.length - 1) ? unit(0, 'deg') : arg) : args;
        const Make = {
          translate: () => new CSSTranslate(a, b ?? zero), translatex: () => new CSSTranslate(a, zero), translatey: () => new CSSTranslate(zero, a), translatez: () => new CSSTranslate(zero, zero, a), translate3d: () => new CSSTranslate(a, b, c),
          scale: () => new CSSScale(a, b ?? a), scalex: () => new CSSScale(a, one), scaley: () => new CSSScale(one, a), scalez: () => new CSSScale(one, one, a), scale3d: () => new CSSScale(a, b, c),
          rotate: () => new CSSRotate(a), rotatex: () => new CSSRotate(1, 0, 0, a), rotatey: () => new CSSRotate(0, 1, 0, a), rotatez: () => new CSSRotate(0, 0, 1, a), rotate3d: () => new CSSRotate(a, b, c, d),
          skew: () => new CSSSkew(a, b ?? unit(0, 'deg')), skewx: () => new CSSSkewX(a), skewy: () => new CSSSkewY(a), perspective: () => new CSSPerspective(a),
          matrix: () => new CSSMatrixComponent(new DOMMatrix(args.map(arg => arg.value))), matrix3d: () => new CSSMatrixComponent(new DOMMatrix(args.map(arg => arg.value))),
        }[name];
        items.push(Make ? Make() : fail());
      }
      return new CSSTransformValue(items);
    };
    // `left 10px` and the like as a length from the start of the axis.
    const position = text => {
      const parts = split(text), edge = { left: 0, top: 0, center: 50, right: 100, bottom: 100 }, out = [];
      for (let i = 0; i < parts.length; i++) {
        const name = parts[i].toLowerCase(), offset = name in edge && parts[i + 1] && !(parts[i + 1] in edge) ? numeric(parts[++i], true) : null;
        const value = name in edge ? edge[name] === 100 && offset ? new CSSMathSum(unit(100, 'percent'), new CSSMathNegate(offset)) : offset ?? unit(edge[name], 'percent') : numeric(name, true);
        if (!value) return null;
        out.push([/^(top|bottom)$/.test(name) ? 'y' : /^(left|right)$/.test(name) ? 'x' : '', value]);
      }
      if (out.length === 1) out.push(['', unit(50, 'percent')]);
      if (out.length !== 2) return null;
      if (out[0][0] === 'y' || out[1][0] === 'x') out.reverse();
      return new CSSPositionValue(out[0][1], out[1][1]);
    };
    const unparsed = text => {
      const items = [];
      let at = 0, start = 0;
      for (let found; (found = /var\(\s*/gi.exec(text.slice(at))?.index) !== undefined;) {
        const open = at + found, head = /var\(\s*/i.exec(text.slice(open))[0].length;
        let depth = 1, end = open + head, comma = -1;
        while (end < text.length && depth) { const c = text[end++]; if (c === '(') depth++; else if (c === ')') depth--; else if (c === ',' && depth === 1 && comma < 0) comma = end; }
        if (open > start) items.push(text.slice(start, open));
        const name = text.slice(open + head, comma < 0 ? end - 1 : comma - 1).trim();
        items.push(new CSSVariableReferenceValue(name, comma < 0 ? null : unparsed(text.slice(comma, end - 1))));
        at = start = end;
      }
      if (start < text.length) items.push(text.slice(start));
      return new CSSUnparsedValue(items);
    };

    const wide = /^(initial|inherit|unset|revert|revert-layer)$/i;
    // Chrome gives these only as `CSSStyleValue`, like shorthands.
    const opaque = /^(-webkit-text-|-macos-|-terminal-|align-|justify-(content|items|self)$|app-region|appearance|aspect-ratio|background-(position|repeat)|border-image-(outset|repeat|slice|width)|border-spacing|color-scheme|column-rule-|contain-intrinsic-(height|width)|content$|corner-|fill$|font-family|grid-area$|columns$|hyphenate-character|list-style-type|mask-(?!type)|overflow-clip-margin|position-visibility|rotate$|scale$|scrollbar-color|stroke$|text-box|text-wrap$|transform-origin|translate$|white-space$|zoom)/;
    const repeated = /^(transition-(?!property)|animation-|background-(attachment|clip|image|origin|blend-mode|size)$)/;
    const one = (name, text, raw) => {
      if (name === 'transform' && text !== 'none') try { return transform(text); } catch { return null; }
      if (/^(object-position|perspective-origin|offset-anchor|offset-position)$/.test(name) && !/^(auto|normal|none)$/.test(text)) return position(text);
      if (/^url\([^)]*\)$/i.test(text)) { const image = Object.create(CSSImageValue.prototype); image[S] = { text }; return image; }
      if (/^-*[a-z_][\w-]*$/i.test(text)) return /color$/.test(name) && !/^(currentcolor|auto)$/i.test(text) || /^(transition-property:all|will-change:(?!auto$))/.test(name + ':' + text) ? null : keyword(text);
      const value = /^grid-(row|column)-(start|end)$/.test(name) ? null : numeric(/^[\w-]+\(/.test(raw) ? raw : text, false);
      const only = value instanceof CSSMathSum && value[S].items.length === 1 && value[S].items[0];
      return only instanceof CSSUnitValue && supports(name, String(only)) ? only : value;
    };
    const reify = (name, text, raw = text, math = raw) => {
      if (name.startsWith('--') || /var\(/i.test(text.replace(/"(\\.|[^"])*"|'(\\.|[^'])*'/g, ''))) return [unparsed(text)];
      if (wide.test(text)) return [keyword(text.toLowerCase())];
      if (longhands(name) || opaque.test(name) || name === 'transition-property' && text.includes(',')) return [unsupported(name, raw)];
      const items = repeated.test(name) ? split(text, true) : [text];
      const values = items.map(item => one(name, item, items.length > 1 ? item : math));
      return values.includes(null) ? [unsupported(name, raw)] : values;
    };
    const parseAll = (name, text) => {
      name = property(name) ?? fail();
      text = String(text);
      const value = parse(name, text);
      if (!value) fail();
      return reify(name, value, text.trim());
    };

    // The text that sets a value, or a TypeError as in Chrome.
    const textOf = (name, value, many) => {
      if (!(value instanceof CSSStyleValue)) { const text = String(value); return parse(name, text) ? text : fail(); }
      if (value[S].name !== undefined) return value[S].name === name ? value[S].text : fail();
      if (value instanceof CSSUnparsedValue) return String(value);
      if (value instanceof CSSKeywordValue && wide.test(value.value) && !many) return String(value);
      if (longhands(name) || opaque.test(name)) fail();
      if (value instanceof CSSNumericValue) {
        if (value[S].type.hint) fail();
        const text = String(value), math = value instanceof CSSUnitValue ? `calc(${text})` : text;
        return supports(name, math) ? supports(name, text) ? text : math : fail();
      }
      return supports(name, String(value)) ? String(value) : fail();
    };
    const texts = (name, values, append) => {
      if (name.startsWith('--')) {
        const value = values.length === 1 ? values[0] : fail();
        return value instanceof CSSStyleValue && !(value instanceof CSSUnparsedValue) ? fail() : parse(name, String(value)) ?? fail();
      }
      if (!values.length && !append || values.length > 1 && !repeated.test(name)) fail();
      return values.map(value => textOf(name, value, values.length > 1 || append)).join(', ');
    };
    const key = name => property(name) ?? fail();
    class StylePropertyMapReadOnly {
      constructor() { illegal(); }
      get(name) { return this.getAll(name)[0]; }
      getAll(name) { name = key(name); const text = this[S].get(name); return text ? reify(name, text, text, this[S].raw?.(name)) : []; }
      has(name) { return !!this[S].get(key(name)); }
      get size() { return this[S].names().length; }
      *entries() { for (const name of this[S].names()) yield [name, this.getAll(name)]; }
      *keys() { yield* this[S].names(); }
      *values() { for (const [, value] of this.entries()) yield value; }
      forEach(callback, self) { for (const [name, value] of this.entries()) callback.call(self, value, name, this); }
    }
    StylePropertyMapReadOnly.prototype[Symbol.iterator] = StylePropertyMapReadOnly.prototype.entries;
    class StylePropertyMap extends StylePropertyMapReadOnly {
      set(name, ...values) { name = key(name); this[S].style.setProperty(name, texts(name, values, false)); }
      append(name, ...values) {
        name = key(name);
        if (!repeated.test(name)) fail();
        const added = texts(name, values, true), old = this[S].style.getPropertyValue(name);
        if (added) this[S].style.setProperty(name, old ? old + ', ' + added : added);
      }
      delete(name) { this[S].style.removeProperty(key(name)); }
      clear() { this[S].style.cssText = ''; }
    }
    const map = (Type, state) => Object.assign(Object.create(Type.prototype), { [S]: state });

    const classes = { CSSStyleValue, CSSNumericValue, CSSUnitValue, CSSMathValue, CSSMathSum, CSSMathProduct, CSSMathNegate, CSSMathInvert, CSSMathMin, CSSMathMax, CSSMathClamp, CSSNumericArray, CSSKeywordValue, CSSUnparsedValue, CSSVariableReferenceValue, CSSImageValue, CSSPositionValue, CSSTransformValue, CSSTransformComponent, CSSTranslate, CSSRotate, CSSScale, CSSSkew, CSSSkewX, CSSSkewY, CSSPerspective, CSSMatrixComponent, StylePropertyMapReadOnly, StylePropertyMap };
    for (const [name, Type] of Object.entries(classes)) Object.defineProperty(Type.prototype, Symbol.toStringTag, { value: name, configurable: true });
    const abstract = new Set([CSSStyleValue, CSSNumericValue, CSSMathValue]);
    const units = {};
    for (const name of groups.join(' ').split(' ')) units[name] = { [name]: value => unit(value, name) }[name];
    return {
      classes, units, unit,
      map: owner => map(StylePropertyMap, { get style() { return owner.style; }, names() { return [...this.style]; }, get(name) { return this.style.getPropertyValue(name); }, raw(name) { return this.style._list.findLast(entry => entry[0] === name)?.[1]; } }),
      computed: element => map(StylePropertyMapReadOnly, { names: () => computedNames(element), get: name => { const text = computed(element, name); return name.startsWith('--') && !/["']/.test(text) ? text.replace(/\s+/g, ' ') : text; } }),
    };
  },
};
