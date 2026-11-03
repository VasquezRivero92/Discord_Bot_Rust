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
