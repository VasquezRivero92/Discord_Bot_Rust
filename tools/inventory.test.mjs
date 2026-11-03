import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseRoutes } from './inventory.mjs';

test('router.get con middleware y prefijo', () => {
  const r = parseRoutes("router.get('/me', requireAuth, async (req,res)=>{})", '/api/admin');
  assert.deepEqual(r, [{ method: 'GET', path: '/api/admin/me', auth: 'requireAuth', line: 1 }]);
});

test('array de paths produce una entrada por path', () => {
  const r = parseRoutes("x\nrouter.post(['/demo-login','/login'], authLimiter, async (req,res)=>{})", '/auth');
  assert.equal(r.length, 2);
  assert.deepEqual(r.map((e) => e.path), ['/auth/demo-login', '/auth/login']);
  assert.equal(r[0].method, 'POST');
  assert.equal(r[0].line, 2);
});

test('app.get sin middleware => auth none', () => {
  const r = parseRoutes("app.get('/health', (req, res) => {})", '');
  assert.deepEqual(r, [{ method: 'GET', path: '/health', auth: 'none', line: 1 }]);
});

test('array multilinea y varios middlewares', () => {
  const src = "app.get([\n  '/a',\n  '/b'\n], optionalAuth, requireSteamApiKey, (req, res) => {})";
  const r = parseRoutes(src, '');
  assert.deepEqual(r.map((e) => [e.path, e.auth, e.line]), [
    ['/a', 'optionalAuth, requireSteamApiKey', 1],
    ['/b', 'optionalAuth, requireSteamApiKey', 1],
  ]);
});

test('corchetes anidados dentro de un path (:id([1-8]))', () => {
  const src = "app.get([\n  '/halloween/user:id',\n  '/halloween/:id([1-8])',\n  '/Halloween/:id([1-8])'\n], (req, res) => {})";
  const r = parseRoutes(src, '');
  assert.deepEqual(r.map((e) => e.path), ['/halloween/user:id', '/halloween/:id([1-8])', '/Halloween/:id([1-8])']);
});

test('app.all se reporta como ALL', () => {
  const r = parseRoutes("app.all(['/api/points', '/api/club/points'], async (req, res) => {})", '');
  assert.deepEqual(r.map((e) => [e.method, e.path]), [['ALL', '/api/points'], ['ALL', '/api/club/points']]);
});

test('path variable se resuelve desde const del mismo archivo', () => {
  const src = "const spa = [\n  '/', '/tienda', // comentario\n  '/checkout'\n];\napp.get(spa, (req, res) => {})";
  const r = parseRoutes(src, '');
  assert.deepEqual(r.map((e) => e.path), ['/', '/tienda', '/checkout']);
  assert.ok(r.every((e) => e.line === 5 && !e.unresolved));
});

test('identificador sin resolver se marca explicitamente', () => {
  const r = parseRoutes('app.get(mystery, (req, res) => {})', '');
  assert.equal(r.length, 1);
  assert.equal(r[0].unresolved, 'mystery');
  assert.match(r[0].path, /UNRESOLVED/);
});

test('rutas comentadas no se cuentan', () => {
  const src = "// router.get('/a', h)\n/* app.get('/b', h)\n app.get('/c', h) */\nrouter.get('/real', h)";
  const r = parseRoutes(src, '');
  assert.deepEqual(r.map((e) => [e.path, e.line]), [['/real', 4]]);
});

test('un // dentro de un string no se toma como comentario', () => {
  const r = parseRoutes("const u='http://x'; router.get('/ok', h)", '');
  assert.deepEqual(r.map((e) => e.path), ['/ok']);
});
