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
  parseArgs,
  randomIpv4,
  ArgError,
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
      if (k !== 'GET /api/products') assert.ok(r.p50 <= r.p95 && r.p95 <= r.p99, `${k} percentiles`);
      const c = r.concurrent;
      if (c.p50 !== null) assert.ok(c.p50 <= c.p95 && c.p95 <= c.p99, `${k} concurrent percentiles`);
    }
    assert.equal(out.endpoints['GET /api/products'].errors, 10);
    assert.equal(out.endpoints['GET /api/products'].concurrent.errors, 4);
    assert.equal(out.endpoints['GET /api/products'].p50, null, 'solo 500: sin percentiles');
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
  const r = out.endpoints['GET /api/ping'];
  assert.equal(r.errors, 4);
  assert.equal(r.concurrent.errors, 2);
  assert.deepEqual([r.p50, r.p95, r.p99], [null, null, null]);
  assert.deepEqual([r.concurrent.p50, r.concurrent.p95, r.concurrent.p99], [null, null, null]);
});

test('measure: fase secuencial y concurrente se separan', async () => {
  // El stub responde rapido en secuencial y lento solo bajo rafaga (>1 peticion en vuelo).
  let inflight = 0;
  const server = http.createServer((req, res) => {
    inflight++;
    const delay = inflight > 1 ? 120 : 0;
    setTimeout(() => { inflight--; res.end('ok'); }, delay);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  try {
    const out = await measure({
      base: `http://127.0.0.1:${server.address().port}`, pid: process.pid, jwtSecret: 's',
      sequential: 3, concurrent: 4,
    });
    const r = out.endpoints['GET /api/ping'];
    assert.ok(r.p99 < 60, `secuencial rapido: ${r.p99}`);
    assert.ok(r.concurrent.p99 >= 100, `rafaga lenta: ${r.concurrent.p99}`);
    assert.equal(r.errors, 0);
  } finally {
    server.close();
  }
});

test('measure: una ruta que nunca responde cuenta como error por timeout', async () => {
  const sockets = new Set();
  const server = http.createServer((req, res) => {
    if (req.url === '/health') return; // nunca responde
    res.end('ok');
  });
  server.on('connection', (s) => sockets.add(s));
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  try {
    const t0 = Date.now();
    const out = await measure({
      base: `http://127.0.0.1:${server.address().port}`, pid: process.pid, jwtSecret: 's',
      sequential: 1, concurrent: 1, timeoutMs: 200,
    });
    assert.ok(Date.now() - t0 < 5000, 'termina pronto');
    const h = out.endpoints['GET /health'];
    assert.equal(h.errors, 2);
    assert.equal(h.p50, null);
    assert.equal(out.endpoints['GET /api/ping'].errors, 0);
  } finally {
    for (const s of sockets) s.destroy();
    server.close();
  }
});

test('measure: pid sin RSS legible es error', async () => {
  await assert.rejects(
    measure({ base: 'http://127.0.0.1:1', pid: 2147483646, jwtSecret: 's', sequential: 1, concurrent: 1 }),
    /RSS/,
  );
});

const okArgs = ['--base', 'http://127.0.0.1:3001', '--pid', '123', '--jwt-secret', 'x'];

test('parseArgs: valores por defecto y env', () => {
  const c = parseArgs(['--base', 'http://h:1', '--pid', '9'], { BASELINE_JWT_SECRET: 'e' });
  assert.deepEqual(c, {
    base: 'http://h:1', pid: 9, out: undefined, jwtSecret: 'e', guildId: '0',
    sequential: 200, concurrent: 50, timeoutMs: 10000, xff: true,
  });
  const d = parseArgs([...okArgs, '--sequential', '5', '--concurrent', '2', '--timeout-ms', '300', '--out', 'o.json']);
  assert.equal(d.sequential, 5);
  assert.equal(d.concurrent, 2);
  assert.equal(d.timeoutMs, 300);
  assert.equal(d.out, 'o.json');
});

test('parseArgs: entradas invalidas lanzan ArgError', () => {
  const bad = [
    [['--pid', '1', '--jwt-secret', 'x'], /--base/],
    [['--base', 'nope', '--pid', '1', '--jwt-secret', 'x'], /URL/],
    [['--base', 'ftp://h', '--pid', '1', '--jwt-secret', 'x'], /http/],
    [[...okArgs, '--sequential', '0'], /--sequential/],
    [[...okArgs, '--concurrent', 'abc'], /--concurrent/],
    [[...okArgs, '--timeout-ms', '-5'], /--timeout-ms/],
    [['--base', 'http://h', '--pid', '0', '--jwt-secret', 'x'], /--pid/],
    [['--base', 'http://h', '--pid', '1.5', '--jwt-secret', 'x'], /--pid/],
    [['--base', 'http://h', '--jwt-secret', 'x'], /--pid/],
    [['--base', 'http://h', '--pid', '1'], /jwt/i],
    [[...okArgs, '--out', '--guild-id', '1'], /--out requiere/],
    [[...okArgs, '--out'], /--out requiere/],
    [[...okArgs, '--bogus', '1'], /desconocida/],
  ];
  for (const [argv, re] of bad) {
    assert.throws(() => parseArgs(argv, {}), (e) => e instanceof ArgError && re.test(e.message), argv.join(' '));
  }
});

// X-Forwarded-For aleatorio por peticion: cada una cae en su propio cubo del rate limit
// (Node `trust proxy 1` y Rust usan la entrada derecha), asi no se mide un muro de 429.
function startXffStub() {
  const seen = [];
  const server = http.createServer((req, res) => {
    seen.push(req.headers['x-forwarded-for']);
    res.end('ok');
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => {
    resolve({ server, seen, base: `http://127.0.0.1:${server.address().port}` });
  }));
}

test('randomIpv4: IPv4 valida', () => {
  for (let i = 0; i < 200; i++) {
    const ip = randomIpv4();
    assert.match(ip, /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/);
    assert.ok(ip.split('.').every((o) => Number(o) >= 1 && Number(o) <= 254), ip);
  }
});

test('measure: cada peticion lleva un X-Forwarded-For distinto por defecto', async () => {
  const { server, seen, base } = await startXffStub();
  try {
    await measure({ base, pid: process.pid, jwtSecret: 's', sequential: 5, concurrent: 3 });
    assert.equal(seen.length, 7 * 8);
    assert.ok(seen.every((v) => typeof v === 'string' && /^[\d.]+$/.test(v)), String(seen));
    assert.equal(new Set(seen).size, seen.length, 'IPs distintas');
  } finally {
    server.close();
  }
});

test('measure: xff=false no envia X-Forwarded-For', async () => {
  const { server, seen, base } = await startXffStub();
  try {
    await measure({ base, pid: process.pid, jwtSecret: 's', sequential: 2, concurrent: 2, xff: false });
    assert.equal(seen.length, 7 * 4);
    assert.ok(seen.every((v) => v === undefined), String(seen));
  } finally {
    server.close();
  }
});

test('parseArgs: --no-xff desactiva la cabecera', () => {
  const c = parseArgs([...okArgs, '--no-xff']);
  assert.equal(c.xff, false);
  const d = parseArgs(['--no-xff', ...okArgs]);
  assert.equal(d.xff, false);
  assert.throws(() => parseArgs([...okArgs, '--no-xff', 'x']), ArgError);
});
