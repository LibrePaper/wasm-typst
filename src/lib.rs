//! Typst to the PDF a LibrePaper document is stored as.
//!
//! The compiler is built to WebAssembly and loaded by an editor, which previews
//! with it, and it is the same pinned compiler that renders a document when one
//! is published. That is the whole point of the module: preview and stored PDF
//! come out of one implementation at one version, so the same source cannot
//! render two ways.
//!
//! `typst.wasm` is around thirty megabytes -- the compiler and its default
//! fonts -- so a host fetches it only for someone who actually opens a typst
//! document. What this repository holds is the compiler binding; the page
//! template, the diagnostics and the WebAssembly interface it answers through
//! come from `wasm-helpers`, whose version is the interface's
//! version.

/// The shared page template and the shape a compile answers in, re-exported so
/// that `crate::page` and `crate::diagnostic` mean here what they mean in every
/// other renderer.
pub use wasm_helpers::{diagnostic, page};

pub mod typst;

#[cfg(target_arch = "wasm32")]
mod abi;
