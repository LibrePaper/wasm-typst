import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const modulePath = path.resolve(process.argv[2] ?? path.join(root, 'dist', 'typst-external.wasm'));
const mirror = path.resolve(process.argv[3] ?? path.join(root, 'mirror'));
const limit = 25 * 1024 * 1024;

function digest(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

const wasm = await readFile(modulePath);
if (wasm.byteLength > limit) {
  throw new Error(`external-font WASM is ${wasm.byteLength} bytes; release limit is ${limit}`);
}
const { instance } = await WebAssembly.instantiate(wasm, {});
if (instance.exports.default_fonts_required?.() !== 1 || typeof instance.exports.add_default_font !== 'function') {
  throw new Error('mirror requires the external-font browser build');
}
const wasmHash = digest(wasm);

const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--format-version', '1', '--manifest-path', path.join(root, 'Cargo.toml')], { encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 }));
const assets = metadata.packages.find((pkg) => pkg.name === 'typst-assets' && pkg.version === '0.15.1');
if (!assets) throw new Error('cargo metadata did not resolve pinned typst-assets 0.15.1');
const packageRoot = path.dirname(assets.manifest_path);
const source = await readFile(path.join(packageRoot, 'src', 'lib.rs'), 'utf8');
const block = source.match(/pub fn fonts\(\)[\s\S]*?\n\s*\[([\s\S]*?)\]\s*\.into_iter\(\)/);
if (!block) throw new Error('could not read font iteration list from pinned typst-assets source');
const names = [...block[1].matchAll(/asset!\("fonts\/([^\"]+)"\)/g)].map((match) => match[1]);
if (names.length !== 17) throw new Error(`expected 17 pinned default fonts, found ${names.length}`);

const mirrorFonts = path.join(mirror, 'fonts');
const revision = path.join(mirror, wasmHash);
await mkdir(mirrorFonts, { recursive: true });
await mkdir(revision, { recursive: true });
await copyFile(path.join(root, 'LICENSE'), path.join(mirror, 'LICENSE'));
await copyFile(path.join(root, 'NOTICE'), path.join(mirror, 'NOTICE'));
await copyFile(path.join(root, 'NOTICE.upstream'), path.join(mirror, 'NOTICE.upstream'));

const fonts = [];
for (const name of names) {
  const bytes = await readFile(path.join(packageRoot, 'files', 'fonts', name));
  if (bytes.byteLength > limit) throw new Error(`${name} exceeds the 25 MiB static asset limit`);
  const sha256 = digest(bytes);
  const ext = path.extname(name).toLowerCase();
  if (!['.otf', '.ttf'].includes(ext)) throw new Error(`unsupported default-font extension: ${name}`);
  const basename = `${sha256}${ext}`;
  const destination = path.join(mirrorFonts, basename);
  await writeFile(destination, bytes);
  fonts.push({ url: `../fonts/${basename}`, sha256, size: bytes.byteLength });
}

const manifest = Buffer.from(`${JSON.stringify({ fonts }, null, 2)}\n`);
const manifestHash = digest(manifest);
await writeFile(path.join(revision, 'typst.wasm'), wasm);
await writeFile(path.join(revision, 'fonts.json'), manifest);

// Verify every published path against the exact bytes the manifest describes.
for (const font of fonts) {
  const file = path.resolve(revision, font.url);
  const bytes = await readFile(file);
  if (bytes.byteLength !== font.size || digest(bytes) !== font.sha256) {
    throw new Error(`font mirror validation failed for ${font.url}`);
  }
}
const publishedWasm = await readFile(path.join(revision, 'typst.wasm'));
if (digest(publishedWasm) !== wasmHash || publishedWasm.byteLength > limit) {
  throw new Error('WASM mirror validation failed');
}

console.log(`mirror/${wasmHash}/typst.wasm (${wasm.byteLength} bytes)`);
console.log(`mirror/${wasmHash}/fonts.json (${fonts.length} fonts)`);
console.log(`fonts.json sha256 ${manifestHash}`);
