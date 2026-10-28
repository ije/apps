globalThis.__appsCrypto = (call, encodeBytes, decodeBytes, base) => {
  const keys = new WeakMap();
  const bytes = value => value instanceof ArrayBuffer ? new Uint8Array(value) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : (() => { throw new TypeError('Expected bytes'); })();
  const encode = value => btoa(String.fromCharCode(...value)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
  const decode = value => Uint8Array.from(atob(value.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0));
  const name = algorithm => typeof algorithm === 'string' ? algorithm : algorithm?.name;
  const fail = (message, type = 'NotSupportedError') => { throw new DOMException(message, type); };
  class CryptoKey {
    constructor(token, data) {
      if (token !== keys) throw new TypeError('Illegal constructor');
      keys.set(this, data);
      for (const field of ['type', 'extractable', 'algorithm', 'usages']) Object.defineProperty(this, field, { enumerable: true, get: () => field === 'algorithm' ? structuredClone(data[field]) : field === 'usages' ? [...data[field]] : data[field] });
    }
  }
  const create = data => new CryptoKey(keys, data);
  const access = (key, operation, algorithm) => {
    const data = keys.get(key);
    if (!data || data.algorithm.name !== name(algorithm) || !data.usages.includes(operation)) fail('Key does not permit this operation', 'InvalidAccessError');
    return data;
  };
  const subtle = {
    digest: base.subtle.digest,
    async generateKey(algorithm, extractable, usages) {
      const kind = name(algorithm); usages = Array.from(usages);
      if (kind === 'ECDSA') {
        if (algorithm.namedCurve !== 'P-256') fail('Only P-256 is supported');
        if (usages.some(u => !['sign', 'verify'].includes(u)) || !usages.includes('sign')) fail('Invalid key usages', 'SyntaxError');
        const material = call('web.crypto.generate');
        const data = { ...material, algorithm: { name: kind, namedCurve: 'P-256' } };
        return { privateKey: create({ ...data, type: 'private', extractable: !!extractable, usages: usages.filter(u => u === 'sign') }), publicKey: create({ public: material.public, type: 'public', extractable: true, algorithm: data.algorithm, usages: usages.filter(u => u === 'verify') }) };
      }
      if (!['HMAC', 'AES-GCM'].includes(kind)) fail('Unsupported key algorithm');
      const hash = name(algorithm.hash ?? 'SHA-256');
      const length = algorithm.length ?? (kind === 'HMAC' ? hash === 'SHA-384' || hash === 'SHA-512' ? 1024 : 512 : 256);
      if (!Number.isInteger(length) || length <= 0 || length % 8 || (kind === 'AES-GCM' && ![128, 256].includes(length))) fail('Invalid key length', 'OperationError');
      const secret = new Uint8Array(length / 8); base.getRandomValues(secret);
      return subtle.importKey('raw', secret, algorithm, extractable, usages);
    },
    async importKey(format, input, algorithm, extractable, usages) {
      const kind = name(algorithm); usages = Array.from(usages);
      let data;
      if (kind === 'ECDSA') {
        if (algorithm.namedCurve !== 'P-256') fail('Only P-256 is supported');
        if (format === 'jwk') {
          if (input.kty !== 'EC' || input.crv !== 'P-256') fail('Invalid EC key', 'DataError');
          const x = decode(input.x), y = decode(input.y), secret = input.d ? decode(input.d) : null;
          if (x.length !== 32 || y.length !== 32 || (secret && secret.length !== 32)) fail('Invalid EC key length', 'DataError');
          data = { public: new Uint8Array([4, ...x, ...y]), private: secret ?? undefined, type: secret ? 'private' : 'public' };
        } else if (format === 'raw') {
          const publicKey = bytes(input); if (publicKey.length !== 65 || publicKey[0] !== 4) fail('Invalid public key', 'DataError');
          data = { public: publicKey.slice(), type: 'public' };
        } else fail('EC import supports raw and JWK');
        data.algorithm = { name: kind, namedCurve: 'P-256' };
        if (usages.some(u => u !== (data.type === 'private' ? 'sign' : 'verify'))) fail('Invalid key usages', 'SyntaxError');
      } else if (kind === 'HMAC' || kind === 'AES-GCM') {
        const secret = format === 'raw' ? bytes(input) : format === 'jwk' && input.kty === 'oct' ? decode(input.k) : fail('Unsupported secret key format');
        if (!secret.length || (kind === 'AES-GCM' && ![16, 32].includes(secret.length))) fail('Invalid key length', 'DataError');
        const allowed = kind === 'HMAC' ? ['sign', 'verify'] : ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'];
        if (!usages.length || usages.some(u => !allowed.includes(u))) fail('Invalid key usages', 'SyntaxError');
        data = { type: 'secret', private: secret.slice(), algorithm: { name: kind, length: secret.length * 8, ...(kind === 'HMAC' ? { hash: { name: name(algorithm.hash ?? 'SHA-256') } } : {}) } };
      } else fail('Unsupported key algorithm');
      if (format === 'jwk' && ((input.ext === false && extractable) || (input.key_ops && usages.some(u => !input.key_ops.includes(u))))) fail('JWK usage mismatch', 'DataError');
      return create({ ...data, extractable: !!extractable, usages });
    },
    async exportKey(format, key) {
      const data = keys.get(key);
      if (!data?.extractable) fail('Key is not extractable', 'InvalidAccessError');
      if (format === 'raw' && data.type !== 'private') return new Uint8Array(data.public ?? data.private).buffer;
      if (format === 'pkcs8' && data.pkcs8) return new Uint8Array(data.pkcs8).buffer;
      if (format !== 'jwk') fail('Unsupported key export format');
      const common = { key_ops: [...data.usages], ext: true };
      if (data.algorithm.name === 'ECDSA') return { ...common, kty: 'EC', crv: 'P-256', x: encode(data.public.slice(1, 33)), y: encode(data.public.slice(33)), ...(data.type === 'private' ? { d: encode(data.private) } : {}) };
      return { ...common, kty: 'oct', k: encode(data.private), alg: data.algorithm.name === 'HMAC' ? 'HS' + data.algorithm.hash.name.slice(4) : 'A' + data.algorithm.length + 'GCM' };
    },
    async sign(algorithm, key, input) {
      const data = access(key, 'sign', algorithm);
      if (data.algorithm.name === 'ECDSA' && name(algorithm.hash) !== 'SHA-256') fail('P-256 signing requires SHA-256');
      return decodeBytes(call('web.crypto.sign', data.algorithm.name, data.private, data.public ?? [], encodeBytes(bytes(input)), [], data.algorithm.hash?.name)).buffer;
    },
    async verify(algorithm, key, signature, input) {
      const data = access(key, 'verify', algorithm);
      if (data.algorithm.name === 'ECDSA' && name(algorithm.hash) !== 'SHA-256') fail('P-256 verification requires SHA-256');
      return call('web.crypto.verify', data.algorithm.name, data.private ?? [], data.public ?? [], encodeBytes(bytes(input)), encodeBytes(bytes(signature)), data.algorithm.hash?.name);
    },
  };
  for (const operation of ['encrypt', 'decrypt']) subtle[operation] = async (algorithm, key, input) => {
    const data = access(key, operation, algorithm);
    if (data.algorithm.name !== 'AES-GCM' || (algorithm.tagLength !== undefined && algorithm.tagLength !== 128)) fail('Only AES-GCM with a 128 bit tag is supported');
    try { return decodeBytes(call('web.crypto.' + operation, data.private, encodeBytes(bytes(algorithm.iv)), algorithm.additionalData ? encodeBytes(bytes(algorithm.additionalData)) : [], encodeBytes(bytes(input)))).buffer; }
    catch (error) { fail(error.message, 'OperationError'); }
  };
  const hash = (algorithm, secret) => {
    const chunks = []; let size = 0, finished = false;
    return {
      update(value, encoding = 'utf8') {
        if (finished) throw new Error('Digest already called');
        const chunk = typeof value === 'string' ? globalThis.Buffer ? Buffer.from(value, encoding) : new TextEncoder().encode(value) : bytes(value);
        chunks.push(new Uint8Array(chunk)); size += chunk.length; return this;
      },
      digest(encoding) {
        if (finished) throw new Error('Digest already called'); finished = true;
        const input = new Uint8Array(size); let offset = 0;
        for (const chunk of chunks) { input.set(chunk, offset); offset += chunk.length; }
        const normalized = String(algorithm).toUpperCase().replace(/^SHA-?/, 'SHA-');
        const value = secret ? call('web.crypto.sign', 'HMAC', encodeBytes(secret), [], encodeBytes(input), [], normalized) : call('web.digest', normalized, encodeBytes(input));
        const result = globalThis.Buffer ? Buffer.from(value) : new Uint8Array(value);
        return encoding ? result.toString(encoding) : result;
      },
    };
  };
  globalThis.__appsNodeCrypto = {
    webcrypto: null,
    createHash: algorithm => hash(algorithm),
    createHmac: (algorithm, key) => hash(algorithm, typeof key === 'string' ? new TextEncoder().encode(key) : bytes(key)),
    randomUUID: base.randomUUID,
    randomBytes(size, callback) {
      const result = globalThis.Buffer ? Buffer.alloc(size) : new Uint8Array(size);
      for (let offset = 0; offset < size; offset += 65536) base.getRandomValues(result.subarray(offset, offset + 65536));
      if (callback) { queueMicrotask(() => callback(null, result)); return; } return result;
    },
    timingSafeEqual: (a, b) => call('web.crypto.equal', encodeBytes(bytes(a)), encodeBytes(bytes(b))),
  };
  globalThis.CryptoKey = CryptoKey;
  globalThis.__appsCryptoCodec = { encode: key => { const data = {...keys.get(key)}; for(const field of ['private','public','pkcs8'])if(data[field])data[field]=encode(data[field]);return data; }, decode: value => { const data={...value};for(const field of ['private','public','pkcs8'])if(data[field])data[field]=typeof data[field]==='string'?decode(data[field]):new Uint8Array(data[field]);return create(data); } };
  const result = Object.freeze({ ...base, subtle: Object.freeze(subtle) });
  __appsNodeCrypto.webcrypto = result;
  return result;
};
