globalThis.__appsCrypto = (call, encodeBytes, decodeBytes, base) => {
  const keys = new WeakMap();
  const bytes = value => value instanceof ArrayBuffer ? new Uint8Array(value) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : (() => { throw new TypeError('Expected bytes'); })();
  const encode = value => value.toBase64({ alphabet: 'base64url', omitPadding: true });
  const decode = value => Uint8Array.fromBase64(value.replaceAll('-', '+').replaceAll('_', '/'));
  const names = ['HMAC', 'AES-GCM', 'ECDSA', 'Ed25519', 'RSASSA-PKCS1-v1_5', 'RSA-PSS', 'PBKDF2', 'HKDF', 'SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
  const name = algorithm => { const value = String(typeof algorithm === 'string' ? algorithm : algorithm?.name); return names.find(known => known.toUpperCase() === value.toUpperCase()) ?? value; };
  const fail = (message, type = 'NotSupportedError') => { throw new DOMException(message, type); };
  const sizes = { 'SHA-1': 20, 'SHA-256': 32, 'SHA-384': 48, 'SHA-512': 64 };
  const curves = { 'P-256': ['2a8648ce3d030107', 32], 'P-384': ['2b81040022', 48] };
  const oids = { ECDSA: '2a8648ce3d0201', Ed25519: '2b6570', 'RSASSA-PKCS1-v1_5': '2a864886f70d010101', 'RSA-PSS': '2a864886f70d010101' };
  const der = (input, at = 0) => {
    let length = input[at + 1], start = at + 2;
    if (length > 127) { const count = length & 127; length = 0; for (let i = 0; i < count; i++) length = length * 256 + input[start++]; }
    if (!(start + length <= input.length)) fail('Invalid DER key', 'DataError');
    return [input[at], input.subarray(start, start + length), start + length];
  };
  const items = input => { const list = []; for (let at = 0; at < input.length;) { const [tag, value, next] = der(input, at); list.push([tag, value]); at = next; } return list; };
  const tlv = (tag, ...parts) => {
    const n = parts.reduce((sum, part) => sum + part.length, 0), head = [tag, ...(n < 128 ? [n] : n < 256 ? [129, n] : [130, n >> 8, n & 255])], result = new Uint8Array(head.length + n);
    result.set(head); let at = head.length; for (const part of parts) { result.set(part, at); at += part.length; } return result;
  };
  const trim = value => { let i = 0; while (i < value.length - 1 && !value[i]) i++; return value.subarray(i); };
  const int = value => { value = trim(value); return tlv(2, ...(value[0] > 127 ? [Uint8Array.of(0)] : []), value); };
  const oid = hex => tlv(6, Uint8Array.fromHex(hex));
  const secretLength = (kind, algorithm) => algorithm.length ?? (kind === 'AES-GCM' ? 256 : sizes[name(algorithm.hash ?? 'SHA-256')] > 32 ? 1024 : 512);
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
  const derive = (algorithm, data, length) => {
    const hash = name(algorithm.hash), pbkdf2 = data.algorithm.name === 'PBKDF2';
    if (!sizes[hash]) fail('Unsupported digest algorithm');
    if (length == null || length % 8 || (pbkdf2 && !algorithm.iterations)) fail('Invalid derivation parameters', 'OperationError');
    const salt = encodeBytes(bytes(algorithm.salt)), digest = hash.replace('-', '').toLowerCase();
    try { return decodeBytes(call('web.crypto.' + (pbkdf2 ? 'pbkdf2' : 'hkdf'), ...(pbkdf2 ? [data.private, salt, algorithm.iterations, length / 8, digest] : [digest, data.private, salt, encodeBytes(bytes(algorithm.info)), length / 8]))).buffer; }
    catch (error) { fail(error.message, 'OperationError'); }
  };
  const subtle = {
    digest: base.subtle.digest,
    async generateKey(algorithm, extractable, usages) {
      const kind = name(algorithm), curve = algorithm.namedCurve; usages = Array.from(usages);
      if (kind === 'ECDSA' || kind === 'Ed25519') {
        if (kind === 'ECDSA' && !curves[curve]) fail('Unsupported named curve');
        if (usages.some(u => !['sign', 'verify'].includes(u)) || !usages.includes('sign')) fail('Invalid key usages', 'SyntaxError');
        const material = call('web.crypto.generate', kind === 'ECDSA' ? curve : 'ed25519'), key = { public: decodeBytes(material.public), algorithm: kind === 'ECDSA' ? { name: kind, namedCurve: curve } : { name: kind } };
        return { privateKey: create({ ...key, private: decodeBytes(material.private), type: 'private', extractable: !!extractable, usages: usages.filter(u => u === 'sign') }), publicKey: create({ ...key, type: 'public', extractable: true, usages: usages.filter(u => u === 'verify') }) };
      }
      if (oids[kind]) fail('RSA key generation is not supported');
      if (!['HMAC', 'AES-GCM'].includes(kind)) fail('Unsupported key algorithm');
      const length = secretLength(kind, algorithm);
      if (!Number.isInteger(length) || length <= 0 || length % 8 || (kind === 'AES-GCM' && ![128, 256].includes(length))) fail('Invalid key length', 'OperationError');
      const secret = new Uint8Array(length / 8); base.getRandomValues(secret);
      return subtle.importKey('raw', secret, algorithm, extractable, usages);
    },
    async importKey(format, input, algorithm, extractable, usages) {
      const kind = name(algorithm), kdf = kind === 'PBKDF2' || kind === 'HKDF'; usages = Array.from(usages);
      let data, allowed;
      if (kind === 'HMAC' || kind === 'AES-GCM' || kdf) {
        const secret = format === 'raw' ? bytes(input) : format === 'jwk' && !kdf && input.kty === 'oct' ? decode(input.k) : fail('Unsupported secret key format');
        if (!secret.length || (kind === 'AES-GCM' && ![16, 32].includes(secret.length))) fail('Invalid key length', 'DataError');
        if (kdf && extractable) fail('Derivation keys are not extractable', 'SyntaxError');
        allowed = kind === 'HMAC' ? ['sign', 'verify'] : kdf ? ['deriveBits', 'deriveKey'] : ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'];
        data = { type: 'secret', private: secret.slice(), algorithm: { name: kind, ...(kdf ? {} : { length: secret.length * 8 }), ...(kind === 'HMAC' ? { hash: { name: name(algorithm.hash ?? 'SHA-256') } } : {}) } };
      } else if (oids[kind]) {
        const rsa = kind.startsWith('RSA'), curve = kind === 'ECDSA' ? algorithm.namedCurve : undefined, size = curves[curve]?.[1], hash = name(algorithm.hash);
        if ((kind === 'ECDSA' && !size) || (rsa && !sizes[hash])) fail('Unsupported key parameters');
        try {
          const source = format === 'jwk' ? input : bytes(input);
          if (format === 'raw' && !rsa) data = { type: 'public', public: source.slice() };
          else if (format === 'jwk') {
            const field = key => decode(input[key]), type = input.d === undefined ? 'public' : 'private';
            if (input.kty !== (rsa ? 'RSA' : curve ? 'EC' : 'OKP') || (!rsa && input.crv !== (curve ?? 'Ed25519')) || (rsa && input.alg !== undefined && input.alg !== (kind === 'RSA-PSS' ? 'PS' : 'RS') + hash.slice(4))) fail('Invalid JWK', 'DataError');
            data = rsa ? { type, public: tlv(0x30, int(field('n')), int(field('e'))), private: type === 'private' ? tlv(0x30, int(Uint8Array.of(0)), ...['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'].map(key => int(field(key)))) : undefined }
              : { type, public: curve ? Uint8Array.of(4, ...field('x'), ...field('y')) : field('x'), private: type === 'private' ? field('d') : undefined };
          } else if (format === 'spki' || format === 'pkcs8') {
            const list = items(der(source)[1]), [[, id], parameter] = items(list[format === 'spki' ? 0 : 1][1]), key = list[format === 'spki' ? 1 : 2][1];
            if (id.toHex() !== oids[kind] || (curve && parameter?.[1].toHex() !== curves[curve][0])) fail('Key does not match the algorithm', 'DataError');
            if (format === 'spki') data = { type: 'public', public: key.slice(1) };
            else if (rsa) { const [, n, e] = items(der(key)[1]); data = { type: 'private', private: key.slice(), public: tlv(0x30, int(n[1]), int(e[1])) }; }
            else if (curve) {
              const fields = items(der(key)[1]), point = fields.find(([tag]) => tag === 0xa1);
              if (!point) fail('EC private keys without a public key are not supported');
              data = { type: 'private', private: fields[1][1].slice(), public: der(point[1])[1].slice(1) };
            } else { const seed = der(key)[1].slice(); data = { type: 'private', private: seed, public: decodeBytes(call('web.crypto.generate', 'ed25519', seed).public) }; }
          } else fail('Unsupported key format');
          const length = data.private?.length;
          if (curve ? data.public.length !== 1 + 2 * size || data.public[0] !== 4 || (length !== undefined && length !== size) : !rsa && (data.public.length !== 32 || (length !== undefined && length !== 32))) fail('Invalid key length', 'DataError');
          const [n, e] = rsa ? items(der(data.public)[1]).map(([, value]) => trim(value)) : [];
          data.algorithm = rsa ? { name: kind, modulusLength: (n.length - 1) * 8 + 32 - Math.clz32(n[0]), publicExponent: e.slice(), hash: { name: hash } } : curve ? { name: kind, namedCurve: curve } : { name: kind };
        } catch (error) { if (error instanceof DOMException) throw error; fail('Invalid key data', 'DataError'); }
        allowed = [data.type === 'private' ? 'sign' : 'verify'];
      } else fail('Unsupported key algorithm');
      if (usages.some(u => !allowed.includes(u)) || (!usages.length && data.type !== 'public')) fail('Invalid key usages', 'SyntaxError');
      if (format === 'jwk' && ((input.ext === false && extractable) || (input.key_ops && usages.some(u => !input.key_ops.includes(u))))) fail('JWK usage mismatch', 'DataError');
      return create({ ...data, extractable: !!extractable, usages });
    },
    async exportKey(format, key) {
      const data = keys.get(key);
      if (!data?.extractable) fail('Key is not extractable', 'InvalidAccessError');
      const { name: kind, namedCurve, hash } = data.algorithm, rsa = kind.startsWith('RSA'), secret = data.type === 'private';
      if (format === 'raw') return data.type === 'secret' || (!secret && !rsa) ? (data.public ?? data.private).slice().buffer : fail('Raw export needs a public or secret key', rsa ? 'NotSupportedError' : 'InvalidAccessError');
      if (format === 'spki' || format === 'pkcs8') {
        if (data.type === 'secret') fail('Unsupported key export format');
        if (secret !== (format === 'pkcs8')) fail('Key type does not match the format', 'InvalidAccessError');
        const id = tlv(0x30, oid(oids[kind]), ...(rsa ? [Uint8Array.of(5, 0)] : namedCurve ? [oid(curves[namedCurve][0])] : []));
        return (secret ? tlv(0x30, int(Uint8Array.of(0)), id, tlv(4, rsa ? data.private : namedCurve ? tlv(0x30, int(Uint8Array.of(1)), tlv(4, data.private), tlv(0xa1, tlv(3, Uint8Array.of(0), data.public))) : tlv(4, data.private))) : tlv(0x30, id, tlv(3, Uint8Array.of(0), data.public))).buffer;
      }
      if (format !== 'jwk') fail('Unsupported key export format');
      const common = { key_ops: [...data.usages], ext: true }, d = secret ? { d: encode(data.private) } : {};
      if (rsa) { const fields = items(der(secret ? data.private : data.public)[1]).slice(+secret); return { ...common, kty: 'RSA', alg: (kind === 'RSA-PSS' ? 'PS' : 'RS') + hash.name.slice(4), ...Object.fromEntries(fields.map(([, value], i) => [['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'][i], encode(trim(value))])) }; }
      if (namedCurve) { const size = curves[namedCurve][1]; return { ...common, kty: 'EC', crv: namedCurve, x: encode(data.public.subarray(1, 1 + size)), y: encode(data.public.subarray(1 + size)), ...d }; }
      if (kind === 'Ed25519') return { ...common, kty: 'OKP', crv: kind, alg: kind, x: encode(data.public), ...d };
      return { ...common, kty: 'oct', k: encode(data.private), alg: kind === 'HMAC' ? 'HS' + hash.name.slice(4) : 'A' + data.algorithm.length + 'GCM' };
    },
    async deriveBits(algorithm, key, length) { return derive(algorithm, access(key, 'deriveBits', algorithm), length); },
    async deriveKey(algorithm, key, derived, extractable, usages) {
      const data = access(key, 'deriveKey', algorithm), kind = name(derived);
      if (kind !== 'HMAC' && kind !== 'AES-GCM') fail('Unsupported derived key algorithm');
      return subtle.importKey('raw', derive(algorithm, data, secretLength(kind, derived)), derived, extractable, usages);
    },
  };
  for (const operation of ['sign', 'verify']) subtle[operation] = async (algorithm, key, ...rest) => {
    const data = access(key, operation, algorithm), { name: kind, namedCurve, hash } = data.algorithm, [signature, input] = operation === 'sign' ? [new Uint8Array(0), rest[0]] : rest;
    let digest = hash?.name;
    if (kind === 'ECDSA' && (digest = name(algorithm.hash)) !== (namedCurve === 'P-256' ? 'SHA-256' : 'SHA-384')) fail(`${namedCurve} keys sign with ${namedCurve === 'P-256' ? 'SHA-256' : 'SHA-384'} only`);
    if (kind === 'RSA-PSS' && algorithm.saltLength !== sizes[digest]) fail('RSA-PSS salts must be as long as the digest');
    const args = [{ Ed25519: 'ed25519', 'RSASSA-PKCS1-v1_5': 'rsa', 'RSA-PSS': 'rsa-pss' }[kind] ?? kind, data.private ?? [], data.public ?? [], encodeBytes(bytes(input)), encodeBytes(bytes(signature)), digest];
    try { const result = call('web.crypto.' + operation, ...args); return operation === 'sign' ? decodeBytes(result).buffer : result; }
    catch (error) { fail(error.message, 'OperationError'); }
  };
  for (const operation of ['encrypt', 'decrypt']) subtle[operation] = async (algorithm, key, input) => {
    const data = access(key, operation, algorithm);
    if (data.algorithm.name !== 'AES-GCM' || (algorithm.tagLength !== undefined && algorithm.tagLength !== 128)) fail('Only AES-GCM with a 128 bit tag is supported');
    try { return decodeBytes(call('web.crypto.' + operation, data.private, encodeBytes(bytes(algorithm.iv)), algorithm.additionalData ? encodeBytes(bytes(algorithm.additionalData)) : [], encodeBytes(bytes(input)))).buffer; }
    catch (error) { fail(error.message, 'OperationError'); }
  };
  globalThis.__appsNodeCrypto = {
    webcrypto: null,
    randomUUID: base.randomUUID,
    randomBytes(size, callback) {
      const result = globalThis.Buffer ? Buffer.alloc(size) : new Uint8Array(size);
      for (let offset = 0; offset < size; offset += 65536) base.getRandomValues(result.subarray(offset, offset + 65536));
      if (callback) { queueMicrotask(() => callback(null, result)); return; } return result;
    },
  };
  globalThis.CryptoKey = CryptoKey;
  // The codec output goes through JSON, so byte fields and an RSA publicExponent travel as strings.
  globalThis.__appsCryptoCodec = { encode: key => { const data = {...keys.get(key)}; for(const field of ['private','public'])if(data[field])data[field]=encode(data[field]);if(data.algorithm.publicExponent)data.algorithm={...data.algorithm,publicExponent:encode(data.algorithm.publicExponent)};return data; }, decode: value => { const data={...value};for(const field of ['private','public'])if(data[field])data[field]=typeof data[field]==='string'?decode(data[field]):new Uint8Array(data[field]);if(data.algorithm.publicExponent)data.algorithm={...data.algorithm,publicExponent:decode(data.algorithm.publicExponent)};return create(data); } };
  const result = Object.freeze({ ...base, subtle: Object.freeze(subtle) });
  __appsNodeCrypto.webcrypto = result;
  return result;
};
