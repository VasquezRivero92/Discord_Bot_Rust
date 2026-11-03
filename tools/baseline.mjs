// Linea base de rendimiento. Mide RSS de un proceso y p50/p95/p99 por endpoint contra cualquier base URL
// (Node legacy ahora, servidor Rust despues). Uso:
//   node tools/baseline.mjs --base http://127.0.0.1:3001 --pid <pid> --out docs/baseline-node.json
//   [--jwt-secret <secreto de prueba> | env BASELINE_JWT_SECRET] [--guild-id 0] [--sequential 200] [--concurrent 50]
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

async function timedRequest(url, headers) {
  const t0 = performance.now();
  let ok = false;
  try {
    const res = await fetch(url, { headers });
    await res.arrayBuffer();
    ok = res.status >= 200 && res.status < 300;
  } catch { /* conexion fallida: error */ }
  return { ms: performance.now() - t0, ok };
}

const round = (n) => (n === null ? null : Math.round(n * 1000) / 1000);

export async function measure({ base, pid, jwtSecret, guildId = '0', sequential = 200, concurrent = 50 }) {
  const rssMb = await readRssMb(pid);
  const cookie = `auth_token=${signTestJwt(jwtSecret)}`;
  const endpoints = {};
  for (const ep of buildEndpoints(guildId)) {
    const url = new URL(ep.path, base).toString();
    const headers = ep.auth ? { cookie } : {};
    const results = [];
    for (let i = 0; i < sequential; i++) results.push(await timedRequest(url, headers));
    results.push(...await Promise.all(Array.from({ length: concurrent }, () => timedRequest(url, headers))));
    const times = results.map((r) => r.ms);
    endpoints[`${ep.method} ${ep.path}`] = {
      p50: round(percentile(times, 50)),
      p95: round(percentile(times, 95)),
      p99: round(percentile(times, 99)),
      errors: results.filter((r) => !r.ok).length,
    };
  }
  return { rssMb: rssMb === null ? null : round(rssMb), endpoints };
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith('--')) args[argv[i].slice(2)] = argv[++i];
  }
  return args;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const a = parseArgs(process.argv.slice(2));
  const jwtSecret = a['jwt-secret'] ?? process.env.BASELINE_JWT_SECRET;
  if (!a.base || !a.pid || !jwtSecret) {
    console.error('uso: node tools/baseline.mjs --base <url> --pid <pid> [--out <json>] [--jwt-secret <s> | BASELINE_JWT_SECRET] [--guild-id 0] [--sequential 200] [--concurrent 50]');
    process.exit(1);
  }
  const result = await measure({
    base: a.base,
    pid: Number(a.pid),
    jwtSecret,
    guildId: a['guild-id'] ?? '0',
    sequential: Number(a.sequential ?? 200),
    concurrent: Number(a.concurrent ?? 50),
  });
  const json = JSON.stringify(result, null, 2) + '\n';
  if (a.out) { fs.writeFileSync(a.out, json); console.log(`escrito ${a.out}`); } else process.stdout.write(json);
}
