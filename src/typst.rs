//! Typst, compiled to the PDF document a document is stored as.
//!
//! PDF is the canonical Typst output. The editor uses pdf.js for its text
//! layer, which keeps the same annotation and source-navigation data model as
//! the other renderers while preserving Typst's paged layout.
//!
//! Everything the compiler may read is handed to it here. The main source is
//! the document being edited; anything it imports is asked of a `Files`
//! reader the caller supplies -- the folder beside the document on the command
//! line, the file map in the browser. A package is asked of the same reader
//! under a path of its own (`@preview/cetz/0.3.4/src/lib.typ`), and a font
//! file the document brings is layered over the embedded ones. There is no
//! network and no clock the document did not get from its host, so a document
//! cannot reach anything it was not given. The sandboxing that a subprocess
//! would need arranging -- a scratch directory, a root, a timeout -- is a
//! property of this design rather than a thing to remember.
//!
//! What the module cannot do is fetch. So a compile also answers what it went
//! looking for and did not find -- packages, font families -- as [`Needs`],
//! and the host fetches those, adds them to the map, and compiles again. A
//! document with everything in reach compiles the same way here as under the
//! `typst` binary.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use typst::diag::{
    FileError, FileResult, PackageError, Severity as TypstSeverity, SourceDiagnostic,
};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::package::PackageSpec;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_layout::PagedDocument;

use crate::diagnostic::{Compiled, Diagnostic, RenderedDocument, Severity};
use crate::page;

/// The compiler this crate is built against, for the version a publisher is
/// told when their own typst differs.
pub const VERSION: &str = "0.15";

/// Reads a file a document imports, by its path relative to the document's
/// root. `None` is "not found", which is what an import of something outside
/// the root, or of anything the host never supplied, gets.
///
/// A package's files are asked for under [`Package::dir`]: the reader that
/// answers `@preview/cetz/0.3.4/src/lib.typ` with the file of that name inside
/// the package's archive has made the package importable.
pub type Files<'a> = &'a (dyn Fn(&Path) -> Option<Vec<u8>> + Sync);

/// Font files a document brings with it, each as its name and its bytes: the
/// equivalent of `--font-path` for a host that has no directory. Only files
/// [`is_font`] says yes to are worth passing.
pub type FontFiles<'a> = &'a [(String, Vec<u8>)];

/// What a package is named by: `@preview/cetz:0.3.4`, in its three parts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl Package {
    fn of(spec: &PackageSpec) -> Package {
        Package {
            namespace: spec.namespace.to_string(),
            name: spec.name.to_string(),
            version: spec.version.to_string(),
        }
    }

    /// The directory the reader is asked for the package's files under:
    /// `@preview/cetz/0.3.4`, so that `lib.typ` in the archive is the path
    /// `@preview/cetz/0.3.4/lib.typ`.
    pub fn dir(&self) -> String {
        format!("@{}/{}/{}", self.namespace, self.name, self.version)
    }

    /// Where the registry serves the package's archive, for the `preview`
    /// namespace: a gzipped tar of the package directory.
    pub fn url(&self) -> Option<String> {
        (self.namespace == "preview").then(|| {
            format!(
                "https://packages.typst.org/preview/{}-{}.tar.gz",
                self.name, self.version
            )
        })
    }
}

/// What a compile went looking for and did not find: the packages that were
/// imported and the font families that were named. The host fetches these,
/// puts them in the map, and compiles again; an empty list means the document
/// had everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    pub packages: Vec<Package>,
    /// Family names, lowercased as typst matches them.
    pub fonts: Vec<String>,
}

impl Needs {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.fonts.is_empty()
    }

    /// As JSON, for the host on the other side of the WebAssembly boundary:
    /// `{"packages":[{"namespace":..,"name":..,"version":..,"dir":..,"url":..}],
    /// "fonts":[..]}`. Written by hand, like the diagnostics list, so that the
    /// module does not carry a serialiser for a hundred bytes.
    pub fn json(&self) -> String {
        let packages: Vec<String> = self
            .packages
            .iter()
            .map(|package| {
                format!(
                    "{{\"namespace\":{},\"name\":{},\"version\":{},\"dir\":{},\"url\":{}}}",
                    quote(&package.namespace),
                    quote(&package.name),
                    quote(&package.version),
                    quote(&package.dir()),
                    package
                        .url()
                        .map(|url| quote(&url))
                        .unwrap_or_else(|| "null".to_string()),
                )
            })
            .collect();
        let fonts: Vec<String> = self.fonts.iter().map(|font| quote(font)).collect();
        format!(
            "{{\"packages\":[{}],\"fonts\":[{}]}}",
            packages.join(","),
            fonts.join(",")
        )
    }
}

/// JSON's own escaping.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A compile: the document and its diagnostics in the shape every renderer
/// answers in, and beside them what this one could not find.
#[derive(Debug)]
pub struct Outcome {
    pub compiled: Compiled,
    pub needs: Needs,
}

/// A set of fonts the compiler may set text in: a book to choose from, and the
/// faces it names. The embedded set is parsed once; a document's own font files
/// are parsed once per distinct set, because parsing them on every keystroke
/// would dwarf the compile itself.
struct Fonts {
    book: LazyHash<FontBook>,
    faces: Vec<Font>,
    /// Font files that yielded no face at all, by name, so the document is
    /// told rather than left wondering why its font is not there.
    unreadable: Vec<String>,
}

impl Fonts {
    fn new(extra: FontFiles) -> Fonts {
        let mut book = FontBook::new();
        let mut faces = Vec::new();
        let mut unreadable = Vec::new();
        let mut push = |data: Bytes| {
            let before = faces.len();
            for font in Font::iter(data) {
                book.push(font.info().clone());
                faces.push(font);
            }
            faces.len() > before
        };
        for data in typst_assets::fonts() {
            push(Bytes::new(data.to_vec()));
        }
        for (name, data) in extra {
            if !push(Bytes::new(data.clone())) {
                unreadable.push(name.clone());
            }
        }
        Fonts {
            book: LazyHash::new(book),
            faces,
            unreadable,
        }
    }
}

/// The fonts every compile uses: the set typst itself ships, embedded, so a
/// document sets in the browser exactly as it does under the typst binary --
/// including maths, which needs a maths font and fails outright without one --
/// with the document's own font files, if any, layered over them.
fn fonts(extra: FontFiles) -> Arc<Fonts> {
    static EMBEDDED: OnceLock<Arc<Fonts>> = OnceLock::new();
    // One slot: an editor compiles the same document's fonts on every
    // keystroke, and the slot remembers that set. The next document's set
    // replaces it rather than accumulating beside it.
    static LAST: Mutex<Option<(u128, Arc<Fonts>)>> = Mutex::new(None);
    if extra.is_empty() {
        return EMBEDDED.get_or_init(|| Arc::new(Fonts::new(&[]))).clone();
    }
    let key = typst::utils::hash128(&extra);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((known, fonts)) = &*last {
        if *known == key {
            return fonts.clone();
        }
    }
    let fonts = Arc::new(Fonts::new(extra));
    *last = Some((key, fonts.clone()));
    fonts
}

/// Says whether a file is a font the compiler can read.
pub fn is_font(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".ttf", ".otf", ".ttc", ".otc"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

/// The library, built once for Typst's normal paged output.
fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| LazyHash::new(Library::builder().build()))
}

/// One document, the files it may import, its fonts, and the date.
struct DocumentWorld<'a> {
    main: Source,
    files: Files<'a>,
    fonts: Arc<Fonts>,
    today: Option<Datetime>,
    /// The packages the compiler asked for and the reader did not have.
    missing: Mutex<Vec<Package>>,
}

impl DocumentWorld<'_> {
    fn read(&self, id: FileId) -> FileResult<Vec<u8>> {
        let inside = Path::new(id.vpath().get_without_slash());
        match id.root() {
            VirtualRoot::Project => {
                (self.files)(inside).ok_or_else(|| FileError::NotFound(inside.into()))
            }
            VirtualRoot::Package(spec) => {
                let package = Package::of(spec);
                let path = Path::new(&package.dir()).join(inside);
                if let Some(bytes) = (self.files)(&path) {
                    return Ok(bytes);
                }
                // Every package has a manifest. If the reader has not even
                // that, the package is what is missing, and it is reported as
                // one thing to fetch rather than as a file that is not there.
                let manifest = Path::new(&package.dir()).join("typst.toml");
                if (self.files)(&manifest).is_some() {
                    return Err(FileError::NotFound(inside.into()));
                }
                let mut missing = self
                    .missing
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !missing.contains(&package) {
                    missing.push(package);
                }
                Err(FileError::Package(PackageError::NotFound(spec.clone())))
            }
        }
    }
}

impl World for DocumentWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.fonts.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        let bytes = self.read(id)?;
        let text = String::from_utf8(bytes).map_err(|_| FileError::InvalidUtf8)?;
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.read(id).map(Bytes::new)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.faces.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        self.today
    }
}

/// The document being compiled, named for its imports' sake: a sibling file
/// resolves relative to it, at the root of the world.
fn main_id(name: &str) -> FileId {
    let name = if name.is_empty() { "main.typ" } else { name };
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(name).expect("a document name is a valid path"),
    )
    .intern()
}

/// A world with nothing to read but the document: what a single-file document
/// with no packages compiles in, anywhere.
pub fn no_files(_: &Path) -> Option<Vec<u8>> {
    None
}

/// A calendar date, for `datetime.today()`; the host supplies it, since the
/// engine has no clock of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Today {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Today {
    /// Today's date in UTC, from the system clock. Civil-from-days, so it
    /// needs no calendar crate.
    pub fn now() -> Option<Today> {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        let days = seconds.div_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        Some(Today {
            year: (if m <= 2 { y + 1 } else { y }) as i32,
            month: m,
            day: d,
        })
    }
}

/// The message typst warns with when a document names a family the book does
/// not have. The name after it is what the host goes and fetches -- in
/// lowercase, because typst lowercases a family before it looks one up, so a
/// font index is best keyed the same way.
const UNKNOWN_FAMILY: &str = "unknown font family: ";

/// Compiles a source to a PDF with Typst's paged layout. A document that does
/// not compile is an ordinary state of an editor rather than an exceptional
/// one, so a diagnostic comes back as an ordinary result and the caller decides
/// how to show it -- and so does what the compile could not find.
pub fn compile_pdf(
    source: &str,
    name: &str,
    files: Files,
    fonts: FontFiles,
    today: Option<Today>,
) -> Outcome {
    let world = DocumentWorld {
        main: Source::new(main_id(name), source.to_string()),
        files,
        fonts: self::fonts(fonts),
        today: today.and_then(|t| Datetime::from_ymd(t.year, t.month, t.day)),
        missing: Mutex::new(Vec::new()),
    };

    let compiled = typst::compile::<PagedDocument>(&world);
    let mut diagnostics = describe(&world, &compiled.warnings);
    let output = match compiled.output {
        Ok(document) => match typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()) {
            Ok(pdf) => Some(RenderedDocument::Pdf(pdf)),
            Err(errors) => {
                diagnostics.extend(describe(&world, &errors));
                None
            }
        },
        Err(errors) => {
            diagnostics.extend(describe(&world, &errors));
            None
        }
    };
    // Each browser edit uses a fresh world, so retaining every source tree in
    // Typst's global constrained-memoization cache grows WASM linearly. Keep
    // one generation for immediate reuse while evicting older revisions after
    // PDF export has finished using the layout.
    typst::comemo::evict(1);

    let mut needs = Needs {
        packages: std::mem::take(
            &mut *world
                .missing
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        ),
        fonts: Vec::new(),
    };
    needs.packages.sort();
    for diagnostic in &diagnostics {
        if let Some(family) = diagnostic.message.strip_prefix(UNKNOWN_FAMILY) {
            if !needs.fonts.iter().any(|known| known == family) {
                needs.fonts.push(family.to_string());
            }
        }
    }
    for name in &world.fonts.unreadable {
        diagnostics.push(Diagnostic::spanless(
            Severity::Warning,
            format!("could not read any font from {name}"),
        ));
    }

    // Errors first: a list read top to bottom, and a badge that jumps to the
    // first thing worth looking at.
    diagnostics.sort_by_key(|diagnostic| !diagnostic.is_error());
    if output.is_none() && !diagnostics.iter().any(Diagnostic::is_error) {
        diagnostics.push(Diagnostic::spanless(
            Severity::Error,
            "typst could not compile this",
        ));
    }
    Outcome {
        compiled: Compiled {
            output,
            diagnostics,
        },
        needs,
    }
}

/// Turns typst's diagnostics into the shape every host reads, mapping each
/// span through the world that compiled: the file it names, and the line and
/// column its byte range falls at.
fn describe(world: &DocumentWorld, diagnostics: &[SourceDiagnostic]) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let mut hints: Vec<String> = diagnostic
                .hints
                .iter()
                .map(|hint| hint.v.to_string())
                .collect();
            // The trace says where a failure inside a function was reached
            // from; it is a hint about the same diagnostic, not a structure of
            // its own.
            hints.extend(
                diagnostic
                    .trace
                    .iter()
                    .map(|point| format!("error occurred {}", point.v)),
            );
            let mut described = Diagnostic {
                severity: match diagnostic.severity {
                    TypstSeverity::Error => Severity::Error,
                    TypstSeverity::Warning => Severity::Warning,
                },
                message: diagnostic.message.to_string(),
                hints,
                ..Diagnostic::default()
            };
            if let (Some(id), Some(range)) = (diagnostic.span.id(), world.range(diagnostic.span)) {
                if id != world.main.id() {
                    described.file = file_name(id);
                }
                if let Ok(source) = world.source(id) {
                    let (line, column) = place(&source, range.start);
                    let (end_line, end_column) = place(&source, range.end);
                    described.line = line;
                    described.column = column;
                    described.end_line = end_line;
                    described.end_column = end_column;
                }
            }
            described
        })
        .collect()
}

/// A file as the host knows it: its path in the map, which for a file inside a
/// package is under the package's directory.
fn file_name(id: FileId) -> String {
    let inside = id.vpath().get_without_slash();
    match id.root() {
        VirtualRoot::Project => inside.to_string(),
        VirtualRoot::Package(spec) => format!("{}/{}", Package::of(spec).dir(), inside),
    }
}

/// A byte offset as a one-based line and column, the column counted in UTF-16
/// code units because that is what the editor on the other side counts in.
fn place(source: &Source, byte: usize) -> (usize, usize) {
    let lines = source.lines();
    let Some(line) = lines.byte_to_line(byte) else {
        return (0, 0);
    };
    let start = lines.line_to_byte(line).unwrap_or(byte);
    let head = source.text().get(start..byte).unwrap_or("");
    (
        line + 1,
        head.chars().map(char::len_utf16).sum::<usize>() + 1,
    )
}

/// Compiles a source to the PDF every Typst document is stored as.
pub fn render(
    source: &str,
    title: &str,
    name: &str,
    files: Files,
    fonts: FontFiles,
    today: Option<Today>,
) -> Outcome {
    let _ = title;
    compile_pdf(source, name, files, fonts, today)
}

/// Says whether a filename is one this renders.
pub fn is_typst(name: &str) -> bool {
    name.to_lowercase().ends_with(".typ")
}

/// The document's first level-one heading, which is the obvious title when
/// none was given: typst has no metadata this side can see without compiling.
pub fn title_of(source: &str) -> String {
    page::first_heading(source, '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_FONTS: FontFiles = &[];

    // A document may import what sits beside it, through the reader the host
    // supplies, and nothing the reader does not know.
    #[test]
    fn imports_resolve_through_the_reader() {
        let files = |path: &Path| -> Option<Vec<u8>> {
            (path == Path::new("lib.typ")).then(|| b"#let greeting = \"hello from lib\"".to_vec())
        };
        let pdf = compile_pdf(
            "#import \"lib.typ\": greeting\n#greeting\n",
            "main.typ",
            &files,
            NO_FONTS,
            None,
        )
        .compiled
        .into_pdf_result()
        .expect("the import did not resolve");
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(compile_pdf(
            "#import \"missing.typ\": x\n",
            "main.typ",
            &files,
            NO_FONTS,
            None
        )
        .compiled
        .output
        .is_none());
    }

    #[test]
    fn today_is_what_the_host_says() {
        let today = Today {
            year: 2026,
            month: 9,
            day: 4,
        };
        let pdf = compile_pdf(
            "#datetime.today().display()\n",
            "",
            &no_files,
            NO_FONTS,
            Some(today),
        )
        .compiled
        .into_pdf_result()
        .expect("compile");
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(compile_pdf(
            "#datetime.today().display()\n",
            "",
            &no_files,
            NO_FONTS,
            None
        )
        .compiled
        .output
        .is_none());
        assert!(Today::now().is_some());
    }

    // Nobody reads a message and then goes looking for the line, so the line
    // comes with the message.
    #[test]
    fn an_error_says_where_it_is() {
        let compiled =
            compile_pdf("= T\n\nsome prose\n\n$x\n", "", &no_files, NO_FONTS, None).compiled;
        assert!(compiled.output.is_none());
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert!(first.is_error());
        assert!(!first.message.is_empty());
        assert_eq!(first.line, 5, "{first:?}");
        assert!(first.column >= 1, "{first:?}");
        assert!(compiled
            .diagnostics_json()
            .contains("\"severity\":\"error\""));
    }

    // A failure inside an imported file is a failure in that file, and the
    // list says so rather than pointing at the line that imported it.
    #[test]
    fn an_error_in_an_imported_file_names_it() {
        let files = |path: &Path| -> Option<Vec<u8>> {
            (path == Path::new("lib.typ")).then(|| b"#let x = colour\n".to_vec())
        };
        let compiled = compile_pdf(
            "#import \"lib.typ\": x\n#x\n",
            "main.typ",
            &files,
            NO_FONTS,
            None,
        )
        .compiled;
        assert!(compiled.output.is_none());
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert_eq!(first.file, "lib.typ", "{first:?}");
        assert_eq!(first.line, 1, "{first:?}");
    }

    // A warning is not a failure: the page is painted and the warning is
    // reported beside it -- and the family the document wanted is what the
    // host is told to fetch.
    #[test]
    fn a_warning_comes_back_beside_a_page() {
        let outcome = compile_pdf(
            "#set text(font: \"No Such Font At All\")\n= T\n\nprose\n",
            "",
            &no_files,
            NO_FONTS,
            None,
        );
        let compiled = outcome.compiled;
        assert!(compiled.output.is_some(), "{:?}", compiled.diagnostics);
        assert!(
            compiled
                .warnings()
                .any(|warning| warning.message.contains("unknown font family")),
            "an unknown font family warned about nothing"
        );
        assert_eq!(compiled.errors().count(), 0);
        // Typst lowercases a family before it looks it up, so the need is
        // lowercase too: what a font index should be keyed by.
        assert_eq!(outcome.needs.fonts, vec!["no such font at all".to_string()]);
        assert!(outcome.needs.packages.is_empty());
    }

    // The editor counts in UTF-16 code units, so the engine does the counting
    // once rather than every host decoding the document a second time.
    #[test]
    fn columns_count_utf16_units() {
        let compiled = compile_pdf("= T\n\n🙂 $x\n", "", &no_files, NO_FONTS, None).compiled;
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert_eq!(first.line, 3, "{first:?}");
        assert_eq!(first.column, 4, "{first:?}");
    }

    #[test]
    fn titles_and_names() {
        assert_eq!(title_of("== Sub\n= The Title\n"), "The Title");
        assert!(is_typst("paper.TYP"));
        assert!(!is_typst("paper.md"));
        assert!(is_font("Inter.TTF"));
        assert!(is_font("a.otf") && is_font("a.ttc") && is_font("a.otc"));
        assert!(!is_font("a.woff2"));
    }

    #[test]
    fn needs_as_json() {
        let needs = Needs {
            packages: vec![Package {
                namespace: "preview".into(),
                name: "cetz".into(),
                version: "0.3.4".into(),
            }],
            fonts: vec!["Fira \"Sans\"".into()],
        };
        assert_eq!(
            needs.json(),
            "{\"packages\":[{\"namespace\":\"preview\",\"name\":\"cetz\",\"version\":\"0.3.4\",\
             \"dir\":\"@preview/cetz/0.3.4\",\
             \"url\":\"https://packages.typst.org/preview/cetz-0.3.4.tar.gz\"}],\
             \"fonts\":[\"Fira \\\"Sans\\\"\"]}"
        );
        assert_eq!(Needs::default().json(), "{\"packages\":[],\"fonts\":[]}");
        let local = Package {
            namespace: "local".into(),
            name: "mine".into(),
            version: "1.0.0".into(),
        };
        assert_eq!(local.url(), None);
        assert!(Needs::default().is_empty());
    }
}
