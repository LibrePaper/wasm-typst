//! Typst to the PDF a LibrePaper document is stored as.
//!
//! The compiler is built to WebAssembly and loaded by an editor, which
//! previews with it, and it is the same pinned compiler that renders a
//! document when one is published. That is the whole point of the module:
//! preview and stored PDF come out of one implementation at one version, so
//! the same source cannot render two ways.
//!
//! `typst.wasm` is around thirty megabytes -- the compiler and its default
//! fonts -- so a host fetches it only for someone who actually opens a typst
//! document. See `src/abi.rs` for the interface it exposes, which is plain
//! exports over linear memory rather than wasm-bindgen: a loader for it is a
//! few lines of JavaScript and the build needs nothing but cargo.

pub mod diagnostic;
pub mod page;
pub mod typst;

/// The word diff, vendored so that this module and the application compute the
/// same one. Only `diff` is reached from here, so a native build of this crate
/// sees the rest as unused.
#[allow(dead_code)]
mod text;

#[cfg(target_arch = "wasm32")]
mod abi;
