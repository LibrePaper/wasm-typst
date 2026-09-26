# wasm-typst

The [Typst](https://typst.app) compiler rendering to PDF and experimental HTML,
compiled to WebAssembly.

Part of [LibrePaper](https://github.com/LibrePaper). The module is what a
LibrePaper editor previews a typst document with, and it is the same pinned
compiler that renders a document when one is published — so the same source
cannot render two ways.

The build is around thirty megabytes before compression: the compiler and its
default fonts. A host fetches it only for someone who actually opens a typst
document.

## The pin

`typst`, `typst-layout`, `typst-pdf` and `typst-assets` are pinned to exactly
`0.15.1`, and pinned together. A document renders the way the compiler that
rendered it behaves, so anything else rendering LibrePaper typst documents has
to be this same version. Moving the pin is a deliberate change with a version
bump here, not a `cargo update`.

The default fonts are embedded rather than fetched, so a document compiles here
exactly as it does under the `typst` binary: the same faces, and the maths font
without which maths does not compile at all.

## Building

```sh
rustup target add wasm32-unknown-unknown   # once
make build                                 # -> dist/typst.wasm  (slow)
make test                                  # the renderer's tests, natively
```

The native test suite includes the PDF corpus under `tests/typst-corpus/`:
imports, embedded assets, repeated compilation, structured diagnostics, a
package under `packages/preview/mini/0.1.0`, and a font file the document
brings.

Nothing but cargo is needed. There is no bindgen step, and no Docker: this is
plain Rust from crates.io, which is the reason the typst engine has never had
the supply-chain question the TeX engines do.

## Serving it

`make compress` writes `dist/typst.wasm.br` and `dist/typst.wasm.gz` beside the
module. This matters more here than anywhere else in LibrePaper: it is the
largest thing a reader ever fetches, and brotli takes two thirds of it off.

| | Size | Saving |
| --- | ---: | ---: |
| raw | 32.0 MiB | |
| brotli, q11 | **9.5 MiB** | 70.3% |
| gzip, level 9 | 13.6 MiB | 57.4% |

Brotli is worth 4.1 MiB over gzip on this file — the difference between the two
is itself larger than most of what a page loads. Quality 11 with a 16 MB window
(`lgwin` 24) is the most the format allows a browser to decode; above 24 is
brotli's large-window extension, which no browser implements. It costs about a
minute, once, at build time.

Serve the precompressed file with `Content-Encoding: br`, `Vary:
Accept-Encoding`, and — this is the one that bites —
`Content-Type: application/wasm`. `WebAssembly.instantiateStreaming` rejects
anything else, and the content type describes the module, not the encoding it
arrived in.

The compression step and `tools/roundtrip.mjs`, which checks the package and
font round trip against the live registry, are the only things in this
repository that need node. They use `node:zlib`, so there is nothing to
install, and a build without node still produces the module.

## The interface

Plain WebAssembly exports over linear memory rather than wasm-bindgen, so the
loader on the other side is a few lines of JavaScript and no CLI version has to
match a crate version. The caller allocates, writes UTF-8 into the module's
memory, calls, and reads the result back out.

| Export | What it does |
| --- | --- |
| `alloc` / `dealloc` | reserve and release memory for arguments |
| `compile(source, title)` | compile to PDF; returns the length of the result |
| `compile_html(source, title)` | compile to experimental, self-contained HTML; same ABI and result handling |
| `output_ptr` / `ok` / `output_kind` | where the result is, whether it is a document, and its format (1 = HTML, 2 = PDF) |
| `diagnostics` / `diagnostics_ptr` | what the compiler had to say, as JSON |
| `failure_page(title)` | the diagnostics dressed as a document |
| `title_of(source)` | the first level-one heading |
| `add_file` / `clear_files` / `set_main` | the file map a document is compiled against |
| `set_asset_url(path, url)` | accepted and ignored; typst embeds figures in both PDF and HTML output |
| `set_today(y, m, d)` | what `datetime.today()` answers — the module has no clock, so the host hands it one |
| `word_diff(old, new)` | the shared word-level diff, as JSON |
| `needs` / `needs_ptr` | what the last compile could not find — packages and font families — as JSON |

A compile leaves two results side by side: the document where `output_ptr`
points, and the diagnostics where `diagnostics_ptr` does. Two rather than one
envelope, because the document is a megabyte and the list is a hundred bytes,
and wrapping the first to carry the second would be an encode and a decode of
the wrong thing on every keystroke.

Diagnostic columns count UTF-16 code units, because that is what an editor
counts in. See `src/abi.rs`, which documents the convention in full.

There is no directory in a browser: a document reaches exactly the files the
host put in the map with `add_file`, and nothing else. Packages and fonts are
files like any other, and the next section is how they get there.

## Packages and fonts

The module cannot fetch. So a compile also answers what it went looking for
and did not find, and the host fetches that, adds it to the map, and compiles
again:

```json
{"packages":[{"namespace":"preview","name":"cetz","version":"0.3.4",
              "dir":"@preview/cetz/0.3.4",
              "url":"https://packages.typst.org/preview/cetz-0.3.4.tar.gz"}],
 "fonts":["tex gyre cursor"]}
```

**Packages.** An `#import "@preview/cetz:0.3.4"` asks the file map for the
package's files under `@preview/cetz/0.3.4/`, so the host unpacks the archive
the registry serves at `url` and adds each entry under `dir`. The registry
allows cross-origin requests and serves a version's archive with a 90-day
cache lifetime; a published version never changes, so there is nothing to
mirror. A package's own dependencies surface on the next round, and a document
that compiles cleanly needs nothing more: a few rounds at most, each one
cached by the browser. A missing file *inside* a package the host supplied is a
broken package, and is reported as a file not found rather than a package to
fetch again.

**Fonts.** Typst never loads fonts from a project, but this module does: every
font file in the map (`.ttf`, `.otf`, `.ttc`, `.otc`, under any path) is
layered over the embedded defaults, which is what `--font-path` does for the
binary. A family the document names and the map lacks is set in the fallback,
warned about, and listed under `fonts` — lowercased, because that is how typst
matches a family name, and so how a font index should be keyed. Which fonts a
host can offer is the host's decision; the module only says which were asked
for.

The fonts in the map are parsed once per distinct set and remembered, so a
keystroke does not re-parse them; only the set changing does.

The same round trip is the native side's to make: `typst::render` takes the
reader and the font files, and answers an `Outcome` whose `needs` is the same
list. A server resolves a package directory against a cache on disk and
downloads on a miss; the bytes are the same, so preview and publication still
agree.

## Keeping in step

The page template, the diagnostics type, the word diff and the WebAssembly
interface come from
[wasm-helpers](https://github.com/LibrePaper/wasm-helpers),
whose version is the interface's version: if a host has to be called
differently, that crate changes and this one fails to compile until it is
rebuilt.

The word diff inside it is vendored from the application repository, because
the editor's history panel asks this module for the same diff the native side
computes. Changes to it belong upstream first.

## Licence

The code in this repository is MIT. See [LICENSE](LICENSE).

What `typst.wasm` contains is not only this code. It statically links the Typst
compiler, which is Apache-2.0, and embeds the assets shipped by
`typst-assets`, which are under five distinct sets of terms — the SIL Open Font
License 1.1 (Libertinus Serif, with Reserved Font Names), the GUST Font License
(New Computer Modern), a separate Distribution Exception for `NewCM10-Regular`,
a BSD-style PDFium licence (the Foxit base-14 faces), and CC0 (the ICC
profiles). Their notices must travel with the built artifact — see
[NOTICE](NOTICE).
