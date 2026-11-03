// Inventario de endpoints del legacy (Express). Uso: node tools/inventory.mjs D:\Discord_Faceit
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const VERBS = 'get|post|put|patch|delete|all';
const NOT_MIDDLEWARE = new Set(['async', 'function']);

function joinPath(prefix, p) {
  if (!prefix) return p;
  return p === '/' ? prefix : prefix + p;
}

// Sustituye comentarios por espacios (conserva saltos de linea y posiciones),
// respetando strings y template literals.
export function stripComments(src) {
  let out = '';
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (c === '/' && n === '/') {
      while (i < src.length && src[i] !== '\n') { out += ' '; i++; }
    } else if (c === '/' && n === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      for (; i < stop; i++) out += src[i] === '\n' ? '\n' : ' ';
    } else if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < src.length && src[j] !== c) { if (src[j] === '\\') j++; j++; }
      out += src.slice(i, j + 1);
      i = j + 1;
    } else { out += c; i++; }
  }
  return out;
}

// Lee un literal de string en src[i] (comilla). Devuelve [valor, indice siguiente].
function readString(src, i) {
  const q = src[i];
  let j = i + 1;
  while (j < src.length && src[j] !== q) { if (src[j] === '\\') j++; j++; }
  return [src.slice(i + 1, j), j + 1];
}

// Lee un array literal de strings en src[i]=='[' con conciencia de comillas y profundidad.
// Devuelve [strings, indice siguiente].
function readStringArray(src, i) {
  const items = [];
  let depth = 0;
  let j = i;
  while (j < src.length) {
    const c = src[j];
    if (c === '"' || c === "'" || c === '`') {
      const [v, next] = readString(src, j);
      if (depth === 1) items.push(v);
      j = next;
      continue;
    }
    if (c === '[') depth++;
    else if (c === ']') { depth--; if (depth === 0) return [items, j + 1]; }
    j++;
  }
  return [items, j];
}

function resolveConstArray(src, name) {
  const re = new RegExp(String.raw`\b(?:const|let|var)\s+${name}\s*=\s*\[`);
  const m = re.exec(src);
  if (!m) return null;
  return readStringArray(src, m.index + m[0].length - 1)[0];
}

export function parseRoutes(rawSource, mountPrefix = '') {
  const source = stripComments(rawSource);
  const callRe = new RegExp(String.raw`\b(?:router|app)\.(${VERBS})\(`, 'g');
  const out = [];
  let m;
  while ((m = callRe.exec(source))) {
    const method = m[1].toUpperCase();
    const line = source.slice(0, m.index).split('\n').length;
    let i = m.index + m[0].length;
    i += /^\s*/.exec(source.slice(i, i + 200))[0].length;

    let paths = [];
    let unresolved = null;
    if (source[i] === '[') {
      [paths, i] = readStringArray(source, i);
    } else if (/['"`]/.test(source[i])) {
      let v;
      [v, i] = readString(source, i);
      paths = [v];
    } else {
      const id = /^[A-Za-z_$][\w$]*/.exec(source.slice(i, i + 200));
      if (!id) continue;
      i += id[0].length;
      const resolved = resolveConstArray(source, id[0]);
      if (resolved) paths = resolved;
      else { unresolved = id[0]; paths = [`!!UNRESOLVED:${id[0]}!!`]; }
    }

    const mws = [];
    const MW_RE = /^\s*,\s*([A-Za-z_$][\w$.]*(?:\([^()]*\))?)\s*(?=,)/;
    for (;;) {
      const mm = MW_RE.exec(source.slice(i, i + 300));
      if (!mm) break;
      if (NOT_MIDDLEWARE.has(mm[1])) break;
      mws.push(mm[1]);
      i += mm[0].length;
    }
    const auth = mws.length ? mws.join(', ') : 'none';
    for (const p of paths) {
      const e = { method, path: unresolved ? p : joinPath(mountPrefix, p), auth, line };
      if (unresolved) e.unresolved = unresolved;
      out.push(e);
    }
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
  const bad = groups.flatMap((g) => g.routes.filter((r) => r.unresolved).map((r) => `${g.origin}:${r.line} (${r.unresolved})`));
  if (bad.length) lines.push('', '> **ADVERTENCIA: rutas con path no resuelto, el inventario esta INCOMPLETO:** ' + bad.join(', '));
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
