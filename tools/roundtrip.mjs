// A check of the package-and-font round trip against the built module and the
// live registry: compile, read `needs`, fetch what is missing, add it to the
// map, compile again. Needs the network. Usage:
//
//   node tools/roundtrip.mjs [out.pdf]
//
// This is also the smallest correct host loop, for whoever writes the one the
// editor uses.
import { readFileSync, writeFileSync } from "node:fs";
import { gunzipSync } from "node:zlib";

const wasm = new WebAssembly.Instance(new WebAssembly.Module(readFileSync("dist/typst.wasm")), {}).exports;
const enc = new TextEncoder(), dec = new TextDecoder();

function call(name, ...args) {
  const written = args.map((v) => {
    const bytes = typeof v === "string" ? enc.encode(v) : v;
    const p = wasm.alloc(bytes.length);
    new Uint8Array(wasm.memory.buffer, p, bytes.length).set(bytes);
    return [p, bytes.length];
  });
  try { return wasm[name](...written.flat()); }
  finally { for (const [p, n] of written) wasm.dealloc(p, n); }
}
const read = (ptr, len) => new Uint8Array(wasm.memory.buffer, ptr, len).slice();
const json = (lenFn, ptrFn) => JSON.parse(dec.decode(read(ptrFn(), lenFn())) || "null");

// A tar reader: 512-byte headers, name at 0..100, size (octal) at 124..136,
// type at 156, ustar prefix at 345..500.
function* untar(buf) {
  for (let at = 0; at + 512 <= buf.length; ) {
    const h = buf.subarray(at, at + 512);
    if (h.every((b) => b === 0)) break;
    const str = (a, b) => dec.decode(h.subarray(a, b)).replace(/\0.*$/s, "");
    const size = parseInt(str(124, 136), 8) || 0;
    const prefix = str(345, 500);
    let name = (prefix ? prefix + "/" : "") + str(0, 100);
    if (h[156] === 48 || h[156] === 0) yield [name, buf.subarray(at + 512, at + 512 + size)];
    at += 512 + Math.ceil(size / 512) * 512;
  }
}

const files = new Map();
async function supply(needs) {
  for (const p of needs.packages) {
    console.log(`fetching ${p.url}`);
    const tar = gunzipSync(Buffer.from(await (await fetch(p.url)).arrayBuffer()));
    let n = 0;
    for (const [name, body] of untar(tar)) { files.set(`${p.dir}/${name.replace(/^\.\//, "")}`, body); n++; }
    console.log(`  ${n} files under ${p.dir}`);
  }
  for (const f of needs.fonts) {
    if (f === "tex gyre cursor") files.set("fonts/cursor.otf", readFileSync("tests/typst-corpus/fonts/texgyrecursor-regular.otf"));
    else console.log(`no font for ${f}`);
  }
}

const source = `#set text(font: "TeX Gyre Cursor")
#import "@preview/cetz:0.3.4"
= A drawing
#cetz.canvas({ import cetz.draw: *; circle((0, 0)); rect((1, 1), (2, 2)) })
`;
wasm.set_today(2026, 9, 9);
for (let round = 1; round <= 6; round++) {
  wasm.clear_files();
  for (const [path, body] of files) call("add_file", path, body);
  call("set_main", "main.typ");
  const len = call("compile", source, "t");
  const needs = json(wasm.needs, wasm.needs_ptr);
  const diags = json(wasm.diagnostics, wasm.diagnostics_ptr);
  console.log(`round ${round}: ok=${wasm.ok()} kind=${wasm.output_kind()} bytes=${len} needs=${JSON.stringify(needs)}`);
  for (const d of diags) console.log(`  ${d.severity}: ${d.message} (${d.file || "main"}:${d.line})`);
  if (wasm.ok() && needs.packages.length === 0 && needs.fonts.length === 0) {
    writeFileSync(process.argv[2] || "/dev/null", read(wasm.output_ptr(), len));
    console.log("done"); process.exit(0);
  }
  await supply(needs);
}
console.log("gave up"); process.exit(1);
