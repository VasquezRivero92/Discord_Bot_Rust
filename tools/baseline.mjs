// Linea base de rendimiento. Mide RSS de un proceso y p50/p95/p99 por endpoint contra cualquier base URL
// (Node legacy ahora, servidor Rust despues). Uso:
//   node tools/baseline.mjs --base http://127.0.0.1:3001 --pid <pid> --out docs/baseline-node.json
//   [--jwt-secret <secreto de prueba> | env BASELINE_JWT_SECRET] [--guild-id 0] [--sequential 200] [--concurrent 50]
//   [--timeout-ms 10000]
// p50/p95/p99 de primer nivel = fase secuencial (solo 2xx); `concurrent` = fase en rafaga (solo 2xx).
// Sin muestras 2xx los percentiles son null. `errors` de primer nivel suma ambas fases.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

const execFileAsync = promisify(execFile);

// Percentil por rango mas cercano. Arreglo vacio -> null.
export function percentile(values, p) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length, Math.max(1, rank)) - 1];
}

// JWT HS256 de prueba (sin dependencias). El secreto debe coincidir con el de la instancia medida.
export function signTestJwt(secret, nowSec = Math.floor(Date.now() / 1000)) {
  const enc = (o) => Buffer.from(JSON.stringify(o)).toString('base64url');
  const header = enc({ alg: 'HS256', typ: 'JWT' });
  const payload = enc({
    userId: 'baseline-user',
    username: 'baseline',
    avatar: null,
    isAdmin: true,
    role: 'admin',
    servidor: null,
    iat: nowSec,
    exp: nowSec + 3600,
  });
  const sig = crypto.createHmac('sha256', secret).update(`${header}.${payload}`).digest('base64url');
  return `${header}.${payload}.${sig}`;
}

// Salida de `tasklist /FO CSV /NH`: "node.exe","1234","Console","1","123,456 K" -> MB.
export function parseTasklistRss(csv) {
  const line = csv.split(/\r?\n/).find((l) => l.startsWith('"'));
  if (!line) return null;
  const cols = line.split('","').map((c) => c.replace(/^"|"$/g, ''));
  const kb = Number(String(cols[4] ?? '').replace(/\D/g, ''));
  return Number.isFinite(kb) && kb > 0 ? kb / 1024 : null;
}

// RSS en MB del proceso `pid`.
export async function readRssMb(pid) {
  if (process.platform === 'win32') {
    const { stdout } = await execFileAsync('tasklist', ['/FI', `PID eq ${pid}`, '/FO', 'CSV', '/NH']);
    return parseTasklistRss(stdout);
  }
  try {
    const status = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
    const m = /VmRSS:\s+(\d+)\s*kB/.exec(status);
    if (m) return Number(m[1]) / 1024;
  } catch { /* sin /proc (p.ej. macOS) */ }
  const { stdout } = await execFileAsync('ps', ['-o', 'rss=', '-p', String(pid)]);
  const kb = Number(stdout.trim());
  return Number.isFinite(kb) && kb > 0 ? kb / 1024 : null;
}

export function buildEndpoints(guildId = '0') {
  const e = (p, auth = false) => ({ method: 'GET', path: p, auth });
  return [
    e('/api/ping'),
    e('/health'),
    e('/api/products'),
    e('/api/lobby/me'),
    e('/api/admin/me', true),
    e(`/api/admin/guilds/${guildId}/settings`, true),
    e('/api/admin/map-catalog', true),
  ];
}

export const DEFAULT_TIMEOUT_MS = 10000;

async function timedRequest(url, headers, timeoutMs) {
  const t0 = performance.now();
  let ok = false;
  try {
    const res = await fetch(url, { headers, signal: AbortSignal.timeout(timeoutMs) });
    await res.arrayBuffer();
    ok = res.status >= 200 && res.status < 300;
  } catch { /* conexion fallida o timeout: error */ }
  return { ms: performance.now() - t0, ok };
}

const round = (n) => (n === null ? null : Math.round(n * 1000) / 1000);

// Percentiles solo sobre muestras exitosas; errors = fallidas de ese conjunto.
function stats(results) {
  const times = results.filter((r) => r.ok).map((r) => r.ms);
  return {
    p50: round(percentile(times, 50)),
    p95: round(percentile(times, 95)),
    p99: round(percentile(times, 99)),
    errors: results.length - times.length,
  };
}

export async function measure({
  base, pid, jwtSecret, guildId = '0', sequential = 200, concurrent = 50, timeoutMs = DEFAULT_TIMEOUT_MS,
}) {
  let rssMb = null;
  try { rssMb = await readRssMb(pid); } catch { /* se informa abajo */ }
  if (rssMb === null) throw new Error(`no se pudo leer el RSS del pid ${pid}`);
  const cookie = `auth_token=${signTestJwt(jwtSecret)}`;
  const endpoints = {};
  for (const ep of buildEndpoints(guildId)) {
    const url = new URL(ep.path, base).toString();
    const headers = ep.auth ? { cookie } : {};
    const seq = [];
    for (let i = 0; i < sequential; i++) seq.push(await timedRequest(url, headers, timeoutMs));
    const conc = await Promise.all(Array.from({ length: concurrent }, () => timedRequest(url, headers, timeoutMs)));
    const s = stats(seq);
    const c = stats(conc);
    endpoints[`${ep.method} ${ep.path}`] = { ...s, errors: s.errors + c.errors, concurrent: c };
  }
  return { rssMb: round(rssMb), endpoints };
}

export class ArgError extends Error {}

const VALUE_FLAGS = new Set(['base', 'pid', 'out', 'jwt-secret', 'guild-id', 'sequential', 'concurrent', 'timeout-ms']);

function posInt(name, raw, def) {
  if (raw === undefined) return def;
  if (!/^[1-9]\d*$/.test(raw)) throw new ArgError(`--${name} debe ser un entero positivo (recibido: ${raw})`);
  return Number(raw);
}

// Valida argv y entorno; lanza ArgError con mensaje en espanol.
export function parseArgs(argv, env = {}) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const tok = argv[i];
    if (!tok.startsWith('--')) throw new ArgError(`argumento inesperado: ${tok}`);
    const name = tok.slice(2);
    if (!VALUE_FLAGS.has(name)) throw new ArgError(`opcion desconocida: ${tok}`);
    const v = argv[i + 1];
    if (v === undefined || v.startsWith('--')) throw new ArgError(`--${name} requiere un valor`);
    a[name] = v;
    i++;
  }
  if (a.base === undefined) throw new ArgError('falta --base');
  let url;
  try { url = new URL(a.base); } catch { throw new ArgError(`--base no es una URL valida: ${a.base}`); }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new ArgError('--base debe ser http o https');
  if (a.pid === undefined) throw new ArgError('falta --pid');
  const jwtSecret = a['jwt-secret'] ?? env.BASELINE_JWT_SECRET;
  if (!jwtSecret) throw new ArgError('falta --jwt-secret o la variable BASELINE_JWT_SECRET');
  return {
    base: a.base,
    pid: posInt('pid', a.pid),
    out: a.out,
    jwtSecret,
    guildId: a['guild-id'] ?? '0',
    sequential: posInt('sequential', a.sequential, 200),
    concurrent: posInt('concurrent', a.concurrent, 50),
    timeoutMs: posInt('timeout-ms', a['timeout-ms'], DEFAULT_TIMEOUT_MS),
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { out, ...cfg } = parseArgs(process.argv.slice(2), process.env);
    const json = JSON.stringify(await measure(cfg), null, 2) + '\n';
    if (out) { fs.writeFileSync(out, json); console.log(`escrito ${out}`); } else process.stdout.write(json);
  } catch (e) {
    console.error(`error: ${e.message}`);
    process.exit(2);
  }
}
