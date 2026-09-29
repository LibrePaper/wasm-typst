import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const modulePath = path.resolve(process.argv[2] ?? path.join(root, 'dist', 'typst.wasm'));
const mirror = path.resolve(process.argv[3] ?? path.join(root, 'mirror'));

function digest(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

const wasm = await readFile(modulePath);
const { instance } = await WebAssembly.instantiate(wasm, {});
const exports = instance.exports;
for (const name of ['memory', 'alloc', 'dealloc', 'compile', 'compile_html', 'output_ptr', 'needs']) {
  if (!(name in exports)) throw new Error(`WASM module is missing required export: ${name}`);
}
if ('default_fonts_required' in exports || 'add_default_font' in exports) {
  throw new Error('WASM module uses the obsolete external-font ABI');
}
const wasmHash = digest(wasm);
const revision = path.join(mirror, wasmHash);
await mkdir(mirror, { recursive: true });
await mkdir(revision, { recursive: true });
await copyFile(path.join(root, 'LICENSE'), path.join(mirror, 'LICENSE'));
await copyFile(path.join(root, 'NOTICE'), path.join(mirror, 'NOTICE'));
await copyFile(path.join(root, 'NOTICE.upstream'), path.join(mirror, 'NOTICE.upstream'));
await writeFile(path.join(revision, 'typst.wasm'), wasm);
const publishedWasm = await readFile(path.join(revision, 'typst.wasm'));
if (digest(publishedWasm) !== wasmHash) {
  throw new Error('WASM mirror validation failed');
}

console.log(`mirror/${wasmHash}/typst.wasm (${wasm.byteLength} bytes)`);
