use std::path::Path;

use wasm_helpers::diagnostic::RenderedDocument;
use wasm_typst::typst::{compile_pdf, no_files, FontFiles, Package};

const PAPER: &str = include_str!("../tests/typst-corpus/paper.typ");
const LONG: &str = include_str!("../tests/typst-corpus/long.typ");
const BROKEN: &str = include_str!("../tests/typst-corpus/broken.typ");
const PACKAGED: &str = include_str!("../tests/typst-corpus/packaged.typ");
const CURSOR: &[u8] = include_bytes!("typst-corpus/fonts/texgyrecursor-regular.otf");

const NO_FONTS: FontFiles = &[];

/// The folder beside the document, without the package.
fn corpus_file(path: &Path) -> Option<Vec<u8>> {
    match path.to_str()? {
        "lib.typ" => Some(include_bytes!("typst-corpus/lib.typ").to_vec()),
        "refs.bib" => Some(include_bytes!("typst-corpus/refs.bib").to_vec()),
        "asset.svg" => Some(include_bytes!("typst-corpus/asset.svg").to_vec()),
        _ => None,
    }
}

/// The same folder, with `@preview/mini:0.1.0` unpacked where the compiler
/// asks for it: under the package's directory, as a host that fetched the
/// archive would put it.
fn corpus_with_package(path: &Path) -> Option<Vec<u8>> {
    match path.to_str()? {
        "@preview/mini/0.1.0/typst.toml" => {
            Some(include_bytes!("typst-corpus/packages/preview/mini/0.1.0/typst.toml").to_vec())
        }
        "@preview/mini/0.1.0/lib.typ" => {
            Some(include_bytes!("typst-corpus/packages/preview/mini/0.1.0/lib.typ").to_vec())
        }
        "@preview/mini/0.1.0/util.typ" => {
            Some(include_bytes!("typst-corpus/packages/preview/mini/0.1.0/util.typ").to_vec())
        }
        _ => corpus_file(path),
    }
}

fn mini() -> Package {
    Package {
        namespace: "preview".into(),
        name: "mini".into(),
        version: "0.1.0".into(),
    }
}

#[test]
fn corpus_compiles_to_typed_pdf_with_imports_and_assets() {
    let outcome = compile_pdf(PAPER, "paper.typ", &corpus_file, NO_FONTS, None);
    let compiled = outcome.compiled;
    assert!(
        compiled.errors().next().is_none(),
        "{:?}",
        compiled.diagnostics
    );
    let Some(RenderedDocument::Pdf(pdf)) = compiled.output else {
        panic!("expected PDF output, got {:?}", compiled.output);
    };
    assert!(pdf.starts_with(b"%PDF-"), "missing PDF header");
    assert!(pdf.len() > 1_000, "fixture PDF is unexpectedly tiny");
    assert!(outcome.needs.is_empty(), "{:?}", outcome.needs);
}

#[test]
fn long_fixture_has_multiple_pages_and_repeated_compiles_are_independent() {
    let first = compile_pdf(LONG, "long.typ", &no_files, NO_FONTS, None).compiled;
    let second = compile_pdf(LONG, "long.typ", &no_files, NO_FONTS, None).compiled;
    for compiled in [first, second] {
        let Some(RenderedDocument::Pdf(pdf)) = compiled.output else {
            panic!("expected PDF output, got {:?}", compiled.diagnostics);
        };
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(
            pdf.len() > 4_000,
            "long fixture did not produce a paged-sized PDF"
        );
    }
}

#[test]
fn missing_import_is_a_structured_error_without_partial_pdf() {
    let outcome = compile_pdf(BROKEN, "broken.typ", &no_files, NO_FONTS, None);
    let compiled = outcome.compiled;
    assert!(compiled.output.is_none());
    let diagnostic = compiled.errors().next().expect("missing diagnostic");
    assert!(!diagnostic.message.is_empty());
    assert!(compiled.diagnostics_json().contains("error"));
    // A sibling file that is not there is not a package to fetch.
    assert!(outcome.needs.is_empty(), "{:?}", outcome.needs);
}

// A package the host has unpacked under its directory imports like it does
// under the typst binary, its own relative imports included.
#[test]
fn a_package_in_the_map_imports() {
    let outcome = compile_pdf(
        PACKAGED,
        "packaged.typ",
        &corpus_with_package,
        NO_FONTS,
        None,
    );
    assert!(
        outcome.compiled.errors().next().is_none(),
        "{:?}",
        outcome.compiled.diagnostics
    );
    let pdf = outcome.compiled.into_pdf_result().expect("no pdf");
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(outcome.needs.is_empty(), "{:?}", outcome.needs);
}

// A package the host has not supplied is one thing to fetch, named as the
// registry names it, and the error says the package was not found rather than
// that some file was. Every compile says so, not only the first: the host's
// loop asks again after each fetch, and the compiler's cache must not hide the
// answer.
#[test]
fn a_missing_package_is_a_need() {
    for _ in 0..2 {
        let outcome = compile_pdf(PACKAGED, "packaged.typ", &corpus_file, NO_FONTS, None);
        assert!(outcome.compiled.output.is_none());
        let error = outcome.compiled.errors().next().expect("no error");
        assert!(
            error.message.contains("package not found"),
            "{}",
            error.message
        );
        assert_eq!(error.line, 3, "{error:?}");
        assert_eq!(outcome.needs.packages, vec![mini()]);
        assert!(outcome.needs.fonts.is_empty());
        assert_eq!(mini().dir(), "@preview/mini/0.1.0");
        assert_eq!(
            mini().url().as_deref(),
            Some("https://packages.typst.org/preview/mini-0.1.0.tar.gz")
        );
        assert!(outcome
            .needs
            .json()
            .contains("\"dir\":\"@preview/mini/0.1.0\""));
    }
}

// A file missing inside a package the host did supply is a broken package, not
// a package to fetch again.
#[test]
fn a_missing_file_inside_a_supplied_package_is_not_a_need() {
    let without_util = |path: &Path| -> Option<Vec<u8>> {
        (path != Path::new("@preview/mini/0.1.0/util.typ"))
            .then(|| corpus_with_package(path))
            .flatten()
    };
    let outcome = compile_pdf(PACKAGED, "packaged.typ", &without_util, NO_FONTS, None);
    assert!(outcome.compiled.output.is_none());
    let error = outcome.compiled.errors().next().expect("no error");
    assert!(error.message.contains("not found"), "{}", error.message);
    assert_eq!(error.file, "@preview/mini/0.1.0/lib.typ", "{error:?}");
    assert!(outcome.needs.is_empty(), "{:?}", outcome.needs);
}

// A font file the document brings is a font the document can set text in,
// exactly as `typst --font-path` would make it; without it the family is
// unknown, the page is set in the fallback, and the family is what the host is
// told to fetch.
#[test]
fn a_font_in_the_map_is_a_font_the_document_can_use() {
    let source = "#set text(font: \"TeX Gyre Cursor\")\n= Set in Cursor\n\nprose\n";
    let without = compile_pdf(source, "font.typ", &no_files, NO_FONTS, None);
    assert!(without.compiled.output.is_some());
    assert_eq!(without.needs.fonts, vec!["tex gyre cursor".to_string()]);
    assert!(without
        .compiled
        .warnings()
        .any(|warning| warning.message == "unknown font family: tex gyre cursor"));

    let fonts = vec![(
        "fonts/texgyrecursor-regular.otf".to_string(),
        CURSOR.to_vec(),
    )];
    let with = compile_pdf(source, "font.typ", &no_files, &fonts, None);
    assert!(with.needs.is_empty(), "{:?}", with.needs);
    assert_eq!(
        with.compiled.warnings().count(),
        0,
        "{:?}",
        with.compiled.diagnostics
    );
    let pdf = with.compiled.into_pdf_result().expect("no pdf");
    assert!(pdf.starts_with(b"%PDF-"));
    // The face is embedded in the PDF under its own name.
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("TeXGyreCursor"),
        "the PDF does not embed the font"
    );

    // The same set again is the cached set; a different set is parsed anew.
    let again = compile_pdf(source, "font.typ", &no_files, &fonts, None);
    assert!(again.needs.is_empty());
    let junk = vec![("fonts/junk.ttf".to_string(), b"not a font".to_vec())];
    let unreadable = compile_pdf(source, "font.typ", &no_files, &junk, None);
    assert!(unreadable
        .compiled
        .warnings()
        .any(|warning| warning.message == "could not read any font from fonts/junk.ttf"));
    assert_eq!(unreadable.needs.fonts, vec!["tex gyre cursor".to_string()]);
}
