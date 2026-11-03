// Inventario de endpoints del legacy (Express). Uso: node tools/inventory.mjs D:\Discord_Faceit
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const VERBS = 'get|post|put|patch|delete';
const CALL_RE = new RegExp(String.raw`\b(?:router|app)\.(${VERBS})\(`, 'g');
const NOT_MIDDLEWARE = new Set(['async', 'function']);

function joinPath(prefix, p) {
  if (!prefix) return p;
  return p === '/' ? prefix : prefix + p;
}

export function parseRoutes(source, mountPrefix = '') {
  const out = [];
  CALL_RE.lastIndex = 0;
  let m;
  while ((m = CALL_RE.exec(source))) {
    const method = m[1].toUpperCase();
    const line = source.slice(0, m.index).split('\n').length;
    let i = m.index + m[0].length;
    const rest = (k) => source.slice(k);

    // Paths: array literal, string literal o identificador.
    let paths = [];
    let ws = /^\s*/.exec(rest(i))[0].length;
    i += ws;
    if (source[i] === '[') {
      const end = source.indexOf(']', i);
      const body = source.slice(i + 1, end);
      paths = [...body.matchAll(/(['"`])((?:\.|(?!\1).)*)\1/g)].map((x) => x[2]);
      i = end + 1;
    } else if (/['"`]/.test(source[i])) {
      const q = source[i];
      const end = source.indexOf(q, i + 1);
      paths = [source.slice(i + 1, end)];
      i = end + 1;
    } else {
      const id = /^[A-Za-z_$][\w$.]*/.exec(rest(i));
      if (!id) continue;
      paths = [`<${id[0]}>`];
      i += id[0].length;
    }

    // Middlewares: identificadores (o llamadas) antes del handler.
    const mws = [];
    const MW_RE = /^\s*,\s*([A-Za-z_$][\w$.]*(?:\([^()]*\))?)\s*(?=,)/;
    for (;;) {
      const mm = MW_RE.exec(rest(i));
      if (!mm) break;
      const name = mm[1];
      if (NOT_MIDDLEWARE.has(name)) break;
      mws.push(name);
      i += mm[0].length;
    }
    const auth = mws.length ? mws.join(', ') : 'none';
    for (const p of paths) out.push({ method, path: joinPath(mountPrefix, p), auth, line });
  }
  return out;
}

const ROUTE_FILES = [
  ['api.js', '/api/admin'],
  ['auth.js', '/auth'],
  ['cs2Api.js', '/api/cs2'],
  ['lobby.js', '/api/lobby'],
  ['publicApi.js', '/api'],
];

const HEADER = '| metodo | ruta | auth | origen | linea | colecciones | usa Discord | fase | notas |';
const SEP = '|---|---|---|---|---|---|---|---|---|';

function readManual(docPath) {
  const manual = new Map();
  if (!fs.existsSync(docPath)) return manual;
  for (const l of fs.readFileSync(docPath, 'utf8').split('\n')) {
    if (!l.startsWith('| ') || l.startsWith('| metodo')) continue;
    const c = l.split('|').slice(1, -1).map((s) => s.trim());
    if (c.length === 9) manual.set(`${c[0]} ${c[1]} ${c[3]}`, c.slice(5));
  }
  return manual;
}

export function buildDoc(legacyDir, manual = new Map()) {
  const groups = [];
  const indexSrc = fs.readFileSync(path.join(legacyDir, 'index.js'), 'utf8');
  groups.push({ origin: 'index.js', routes: parseRoutes(indexSrc, '') });
  for (const [f, prefix] of ROUTE_FILES) {
    const src = fs.readFileSync(path.join(legacyDir, 'routes', f), 'utf8');
    groups.push({ origin: `routes/${f}`, prefix, routes: parseRoutes(src, prefix) });
  }
  const lines = ['# Inventario de endpoints del legacy', '',
    'Generado por `node tools/inventory.mjs <ruta-legacy>`. Las columnas `colecciones`, `usa Discord`, `fase` y `notas` son manuales y se conservan al regenerar (clave: metodo + ruta + origen).',
    '', '`usa Discord`: REST (fetch/REST de Discord), gateway (cache del gateway de discord.js), no. `fase`: 1, 2, 3 o proxy. `?` = sin clasificar.', '',
    '## Recuento por archivo (llamadas `router.<verbo>` / `app.<verbo>`)', '',
    '| origen | prefijo | llamadas | entradas (rutas expandidas) |', '|---|---|---|---|'];
  for (const g of groups) {
    const calls = new Set(g.routes.map((r) => r.line)).size;
    lines.push(`| ${g.origin} | ${g.prefix ?? '-'} | ${calls} | ${g.routes.length} |`);
  }
  for (const g of groups) {
    lines.push('', `## ${g.origin}`, '', HEADER, SEP);
    for (const r of g.routes) {
      const m = manual.get(`${r.method} ${r.path} ${g.origin}`) ?? ['?', '?', '?', ''];
      lines.push(`| ${r.method} | ${r.path} | ${r.auth} | ${g.origin} | ${r.line} | ${m.join(' | ')} |`);
    }
  }
  return lines.join('\n') + '\n';
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const legacy = process.argv[2];
  if (!legacy) { console.error('uso: node tools/inventory.mjs <ruta-legacy>'); process.exit(1); }
  const docPath = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'docs', 'endpoint-inventory.md');
  const doc = buildDoc(legacy, readManual(docPath));
  fs.writeFileSync(docPath, doc);
  console.log(`escrito ${docPath}`);
}
