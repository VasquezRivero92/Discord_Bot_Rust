// Genera crates/web/tests/fixtures/jwt_vectors.json con el `jsonwebtoken` de Node.
// Uso: LEGACY_DIR=D:/Discord_Faceit node tools/gen-jwt-vectors.mjs [legacyDir]
// El secreto es un valor de prueba, nunca el de produccion.
import { createRequire } from 'node:module';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const legacyDir = process.argv[2] || process.env.LEGACY_DIR || 'D:/Discord_Faceit';
const require = createRequire(join(legacyDir, 'package.json'));
const jwt = require(join(legacyDir, 'node_modules', 'jsonwebtoken'));

const SECRET = 'test-secret-do-not-use-in-prod';
const IAT = 1767225600; // 2026-01-01T00:00:00Z
const EXP_FUTURE = 4102444800; // 2100-01-01T00:00:00Z
const EXP_PAST = 1700000000; // 2023-11-14

const adminClaims = {
  userId: '111111111111111111',
  username: 'AdminUser',
  avatar: 'abc123',
  isAdmin: true,
  role: 'global_admin',
  servidor: '222222222222222222',
};
const userClaims = {
  userId: '333333333333333333',
  username: 'PlainUser',
  avatar: null,
  isAdmin: false,
  role: 'user',
  servidor: null,
};

const sign = (claims, secret, opts = {}) =>
  jwt.sign({ ...claims, iat: IAT, exp: EXP_FUTURE }, secret, { algorithm: 'HS256', ...opts });

const b64 = (o) => Buffer.from(JSON.stringify(o)).toString('base64url');
const algNone = `${b64({ alg: 'none', typ: 'JWT' })}.${b64({ ...adminClaims, iat: IAT, exp: EXP_FUTURE })}.`;

const vectors = {
  secret: SECRET,
  vectors: [
    { name: 'valid_admin', token: sign(adminClaims, SECRET), expect: 'valid', claims: { ...adminClaims, iat: IAT, exp: EXP_FUTURE }, is_admin_user: true },
    { name: 'valid_non_admin', token: sign(userClaims, SECRET), expect: 'valid', claims: { ...userClaims, iat: IAT, exp: EXP_FUTURE }, is_admin_user: false },
    { name: 'expired', token: jwt.sign({ ...adminClaims, iat: EXP_PAST - 3600, exp: EXP_PAST }, SECRET, { algorithm: 'HS256' }), expect: 'invalid' },
    { name: 'wrong_secret', token: sign(adminClaims, 'another-secret'), expect: 'invalid' },
    { name: 'alg_none', token: algNone, expect: 'invalid' },
    { name: 'hs384', token: sign(adminClaims, SECRET, { algorithm: 'HS384' }), expect: 'invalid' },
    { name: 'garbage', token: 'not.a.jwt', expect: 'invalid' },
  ],
};

const out = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'crates', 'web', 'tests', 'fixtures', 'jwt_vectors.json');
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(vectors, null, 2) + '\n');
console.log(`wrote ${out}`);
