//! The nineteen exports a host calls, and nothing else.
//!
//! Each one wraps the shared implementation in `wasm_helpers::abi`.
//! They are written out rather than generated because a `#[no_mangle]` export
//! has to be compiled into the `cdylib` that ships and cannot be inherited from
//! a dependency -- and because eighteen signatures you can read beat a macro
//! that writes them and reports its errors somewhere else.
//!
//! Two of them are this module's alone. `needs` and `needs_ptr` say what the
//! last compile went looking for and did not find -- packages and font
//! families -- as JSON, so the host can fetch them, `add_file` them, and
//! compile again. A loader that has never heard of them loses nothing but
//! packages.
//!
//! `set_asset_url` is accepted and ignored: typst reads a figure out of the
//! file map and embeds it in PDF and HTML output itself, so it needs no URL.
//! It is exported anyway, so that one loader drives every renderer without
//! first asking which it has.

use wasm_helpers::abi;
use wasm_helpers::diagnostic::Compiled;

/// What the last compile could not find, as JSON, for `needs` to hand over.
static mut NEEDS: Option<Vec<u8>> = None;

/// What this module is: typst, reading only the files the host handed over.
/// There is no directory in a browser, and a document reaches nothing else --
/// a package is the files the host put under its directory, and a font is a
/// font file the host put anywhere in the map.
fn render(source: &str, title: &str) -> Compiled {
    let today = abi::today().map(|(year, month, day)| crate::typst::Today {
        year,
        month: month as u8,
        day: day as u8,
    });
    let name = abi::main_name();
    let fonts: Vec<(String, Vec<u8>)> = abi::files()
        .into_iter()
        .filter(|(path, _)| crate::typst::is_font(path))
        .collect();
    let outcome =
        crate::typst::render(source, title, &name, &|path| abi::file(path), &fonts, today);
    unsafe { NEEDS = Some(outcome.needs.json().into_bytes()) }
    outcome.compiled
}

fn render_html(source: &str, title: &str) -> Compiled {
    let today = abi::today().map(|(year, month, day)| crate::typst::Today {
        year,
        month: month as u8,
        day: day as u8,
    });
    let name = abi::main_name();
    let fonts: Vec<(String, Vec<u8>)> = abi::files()
        .into_iter()
        .filter(|(path, _)| crate::typst::is_font(path))
        .collect();
    let _ = title;
    let outcome = crate::typst::compile_html(source, &name, &|path| abi::file(path), &fonts, today);
    unsafe { NEEDS = Some(outcome.needs.json().into_bytes()) }
    outcome.compiled
}

fn heading(source: &str) -> String {
    crate::typst::title_of(source)
}

#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    abi::alloc(len)
}

/// # Safety
/// `pointer` and `len` must be exactly what a previous `alloc` returned.
#[no_mangle]
pub unsafe extern "C" fn dealloc(pointer: *mut u8, len: usize) {
    abi::dealloc(pointer, len)
}

/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn compile(
    source: *const u8,
    source_len: usize,
    title: *const u8,
    title_len: usize,
) -> usize {
    let source = abi::text_at(source, source_len);
    let title = abi::text_at(title, title_len);
    abi::answer_compiled(render(source, title))
}

/// Compile to Typst's experimental HTML output. The signature intentionally
/// matches `compile`; the host can select the target without changing the
/// file, font, diagnostics, or needs ABI.
///
/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn compile_html(
    source: *const u8,
    source_len: usize,
    title: *const u8,
    title_len: usize,
) -> usize {
    let source = abi::text_at(source, source_len);
    let title = abi::text_at(title, title_len);
    abi::answer_compiled(render_html(source, title))
}

/// # Safety
/// `source` and `len` must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn title_of(source: *const u8, len: usize) -> usize {
    abi::answer(Ok(heading(abi::text_at(source, len))))
}

/// # Safety
/// `title` and `len` must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn failure_page(title: *const u8, len: usize) -> usize {
    abi::failure_page(title, len)
}

/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn word_diff(
    old: *const u8,
    old_len: usize,
    new: *const u8,
    new_len: usize,
) -> usize {
    abi::word_diff(old, old_len, new, new_len)
}

/// # Safety
/// The pointers and lengths must describe memory written into this module.
#[no_mangle]
pub unsafe extern "C" fn add_file(
    path: *const u8,
    path_len: usize,
    body: *const u8,
    body_len: usize,
) {
    abi::add_file(path, path_len, body, body_len)
}

#[no_mangle]
pub extern "C" fn clear_files() {
    abi::clear_files()
}

/// # Safety
/// The pointers and lengths must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn set_asset_url(
    path: *const u8,
    path_len: usize,
    url: *const u8,
    url_len: usize,
) {
    abi::set_asset_url(path, path_len, url, url_len)
}

/// # Safety
/// The pointer and length must describe UTF-8 written into this module.
#[no_mangle]
pub unsafe extern "C" fn set_main(path: *const u8, len: usize) {
    abi::set_main(path, len)
}

/// What `datetime.today()` answers: the module has no clock, so the host hands
/// it one.
#[no_mangle]
pub extern "C" fn set_today(year: i32, month: u32, day: u32) {
    abi::set_today(year, month, day)
}

#[no_mangle]
pub extern "C" fn output_ptr() -> *const u8 {
    abi::output_ptr()
}

#[no_mangle]
pub extern "C" fn ok() -> u32 {
    abi::ok()
}

#[no_mangle]
pub extern "C" fn output_kind() -> u32 {
    abi::output_kind()
}

#[no_mangle]
pub extern "C" fn diagnostics() -> usize {
    abi::diagnostics()
}

#[no_mangle]
pub extern "C" fn diagnostics_ptr() -> *const u8 {
    abi::diagnostics_ptr()
}

/// The length of what the last compile went looking for and did not find, as
/// JSON: `{"packages":[{"namespace","name","version","dir","url"}],"fonts":[]}`.
/// The host fetches each, adds a package's files under `dir` and a font file
/// under any name, and compiles again; an empty list means the document had
/// everything. Zero before any compile.
#[no_mangle]
pub extern "C" fn needs() -> usize {
    unsafe {
        match &*std::ptr::addr_of!(NEEDS) {
            Some(bytes) => bytes.len(),
            None => 0,
        }
    }
}

/// Where that JSON starts.
#[no_mangle]
pub extern "C" fn needs_ptr() -> *const u8 {
    unsafe {
        match &*std::ptr::addr_of!(NEEDS) {
            Some(bytes) => bytes.as_ptr(),
            None => std::ptr::null(),
        }
    }
}
