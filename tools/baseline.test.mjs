import { test } from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import http from 'node:http';
import {
  percentile,
  signTestJwt,
  parseTasklistRss,
  readRssMb,
  buildEndpoints,
  measure,
} from './baseline.mjs';

test('percentile: nearest-rank', () => {
  assert.equal(percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 95), 10);
  assert.equal(percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 50), 5);
  assert.equal(percentile([5], 50), 5);
  assert.equal(percentile([3, 1, 2], 100), 3);
});

test('percentile: arreglo vacio devuelve null y no muta la entrada', () => {
  assert.equal(percentile([], 50), null);
  const v = [3, 1, 2];
  percentile(v, 50);
  assert.deepEqual(v, [3, 1, 2]);
});

test('signTestJwt: cabecera, payload y firma HS256 verificables', () => {
  const secret = 'secreto-de-prueba';
  const token = signTestJwt(secret, 1_700_000_000);
  const [h, p, s] = token.split('.');
  const dec = (x) => JSON.parse(Buffer.from(x, 'base64url').toString('utf8'));
  assert.deepEqual(dec(h), { alg: 'HS256', typ: 'JWT' });
  const payload = dec(p);
  assert.equal(payload.isAdmin, true);
  assert.equal(payload.role, 'admin');
  assert.equal(payload.servidor, null);
  for (const k of ['userId', 'username', 'avatar']) assert.ok(k in payload, k);
  assert.equal(payload.iat, 1_700_000_000);
  assert.equal(payload.exp, 1_700_000_000 + 3600);
  const expected = crypto.createHmac('sha256', secret).update(`${h}.${p}`).digest('base64url');
  assert.equal(s, expected);
});

test('parseTasklistRss: lee la columna Mem Usage', () => {
  const csv = '"node.exe","1234","Console","1","123,456 K"\r\n';
  assert.equal(parseTasklistRss(csv), 123456 / 1024);
  assert.equal(parseTasklistRss('INFO: No tasks are running which match the specified criteria.'), null);
});

test('readRssMb: lee el RSS del proceso actual', async () => {
  const mb = await readRssMb(process.pid);
  assert.ok(Number.isFinite(mb) && mb > 1 && mb < 100000, String(mb));
});

test('buildEndpoints: lista los endpoints configurados', () => {
  const e = buildEndpoints('42');
  assert.deepEqual(e.map((x) => `${x.method} ${x.path}`), [
    'GET /api/ping',
    'GET /health',
    'GET /api/products',
    'GET /api/lobby/me',
    'GET /api/admin/me',
    'GET /api/admin/guilds/42/settings',
    'GET /api/admin/map-catalog',
  ]);
  assert.deepEqual(e.map((x) => x.auth), [false, false, false, false, true, true, true]);
});

function startStub() {
  const hits = new Map();
  const server = http.createServer((req, res) => {
    hits.set(req.url, (hits.get(req.url) ?? 0) + 1);
    if (req.url === '/api/products') { res.statusCode = 500; return res.end('boom'); }
    if (req.url.startsWith('/api/admin/') && !/auth_token=/.test(req.headers.cookie ?? '')) {
      res.statusCode = 401; return res.end('no');
    }
    res.end('ok');
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => {
    resolve({ server, hits, base: `http://127.0.0.1:${server.address().port}` });
  }));
}

test('measure contra stub local', async () => {
  const { server, hits, base } = await startStub();
  try {
    const out = await measure({
      base, pid: process.pid, jwtSecret: 's', guildId: '0', sequential: 6, concurrent: 4,
    });
    assert.ok(out.rssMb > 1);
    const keys = Object.keys(out.endpoints);
    assert.equal(keys.length, 7);
    for (const k of keys) {
      const r = out.endpoints[k];
      assert.ok(r.p50 <= r.p95 && r.p95 <= r.p99, `${k} percentiles`);
    }
    assert.equal(out.endpoints['GET /api/products'].errors, 10);
    assert.equal(out.endpoints['GET /api/ping'].errors, 0);
    assert.equal(out.endpoints['GET /api/admin/me'].errors, 0, 'cookie enviada');
    assert.equal(out.endpoints['GET /api/lobby/me'].errors, 0);
    assert.equal(hits.get('/api/ping'), 10);
    assert.equal(hits.get('/api/admin/map-catalog'), 10);
  } finally {
    server.close();
  }
});

test('measure: peticiones fallidas (conexion rechazada) cuentan como error', async () => {
  const { server, base } = await startStub();
  await new Promise((r) => server.close(r));
  const out = await measure({
    base, pid: process.pid, jwtSecret: 's', guildId: '0', sequential: 2, concurrent: 2,
  });
  assert.equal(out.endpoints['GET /api/ping'].errors, 4);
});
