import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const modulePath = path.resolve(process.argv[2] ?? path.join(root, 'dist', 'typst.wasm'));
const mirror = path.resolve(process.argv[3] ?? path.join(root, 'mirror'));
const syntaxPath = path.resolve(process.argv[4] ?? path.join(root, 'dist', 'typst-syntax.wasm'));

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
const syntax = await readFile(syntaxPath);
const { instance: syntaxInstance } = await WebAssembly.instantiate(syntax, {});
for (const name of ['memory', 'alloc', 'dealloc', 'kinds', 'kinds_len', 'source_new', 'source_edit', 'source_free', 'source_tree', 'tree_len']) {
  if (!(name in syntaxInstance.exports)) throw new Error(`syntax module is missing required export: ${name}`);
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

// The parser module is addressed by the digest of its own bytes, as every
// module on the mirror is, so the two pins move independently.
const syntaxHash = digest(syntax);
await mkdir(path.join(mirror, syntaxHash), { recursive: true });
await writeFile(path.join(mirror, syntaxHash, 'typst-syntax.wasm'), syntax);
if (digest(await readFile(path.join(mirror, syntaxHash, 'typst-syntax.wasm'))) !== syntaxHash) {
  throw new Error('syntax mirror validation failed');
}
console.log(`mirror/${syntaxHash}/typst-syntax.wasm (${syntax.byteLength} bytes)`);
console.log(`mirror/${wasmHash}/typst.wasm (${wasm.byteLength} bytes)`);
