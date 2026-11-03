// Genera crates/web/src/routes/products.json a partir de STORE_PRODUCTS de routes/publicApi.js
// del legacy. Solo lectura del legacy: lee el archivo como texto, extrae el literal del array y lo
// evalua con node:vm (sin imports ni red).
//
// Uso: node tools/gen-products.mjs [ruta-publicApi.js] [salida.json]
//   por defecto: D:/Discord_Faceit/routes/publicApi.js -> crates/web/src/routes/products.json
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

export function extractProducts(source) {
  const marker = /export\s+const\s+STORE_PRODUCTS\s*=\s*\[/.exec(source);
  if (!marker) throw new Error('STORE_PRODUCTS no encontrado');
  const start = marker.index + marker[0].length - 1;
  // Recorre el literal respetando cadenas para encontrar el corchete de cierre.
  let depth = 0;
  let quote = null;
  for (let i = start; i < source.length; i++) {
    const c = source[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
    } else if (c === "'" || c === '"' || c === '`') quote = c;
    else if (c === '[') depth++;
    else if (c === ']' && --depth === 0) {
      const literal = source.slice(start, i + 1);
      return vm.runInNewContext(`(${literal})`, Object.create(null), { timeout: 1000 });
    }
  }
  throw new Error('literal de STORE_PRODUCTS sin cerrar');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const src = process.argv[2] || 'D:/Discord_Faceit/routes/publicApi.js';
  const out =
    process.argv[3] ||
    path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'crates', 'web', 'src', 'routes', 'products.json');
  const products = extractProducts(fs.readFileSync(src, 'utf8'));
  fs.writeFileSync(out, JSON.stringify(products, null, 2) + '\n');
  console.log(`${products.length} productos -> ${out}`);
}
