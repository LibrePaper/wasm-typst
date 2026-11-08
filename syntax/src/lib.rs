//! Typst's parser alone, behind plain `extern "C"` exports.
//!
//! No wasm-bindgen: the host writes text into memory it got from `alloc`,
//! calls an export, and reads numbers or a buffer back out of linear memory.
//!
//! A source is a handle (never 0). `source_edit` reparses incrementally, so a
//! keystroke costs far less than parsing the whole file. `source_tree` writes
//! the whole syntax tree as a Lezer tree buffer: nodes in postorder, four u32
//! each, `[kind, from, to, size]`, offsets in UTF-16 code units and `size`
//! four times the node count of the subtree, the node itself included.

use std::cell::RefCell;
use typst_syntax::{Source, SyntaxKind, SyntaxNode};

/// Every kind, in discriminant order. Line i of `kinds()` is kind i. The glob
/// import stays in this block: `SyntaxKind::None` would shadow `Option::None`.
const KINDS: [SyntaxKind; 137] = {
    use SyntaxKind::*;
    [
        End,
        Error,
        Shebang,
        LineComment,
        BlockComment,
        Markup,
        Text,
        Space,
        Linebreak,
        Parbreak,
        Escape,
        Shorthand,
        SmartQuote,
        Strong,
        Emph,
        Raw,
        RawLang,
        RawDelim,
        RawTrimmed,
        Link,
        Label,
        Ref,
        RefMarker,
        Heading,
        HeadingMarker,
        ListItem,
        ListMarker,
        EnumItem,
        EnumMarker,
        TermItem,
        TermMarker,
        Equation,
        Math,
        MathText,
        MathIdent,
        MathFieldAccess,
        MathShorthand,
        MathAlignPoint,
        MathCall,
        MathArgs,
        MathDelimited,
        MathAttach,
        MathPrimes,
        MathFrac,
        MathRoot,
        Hash,
        LeftBrace,
        RightBrace,
        LeftBracket,
        RightBracket,
        LeftParen,
        RightParen,
        Comma,
        Semicolon,
        Colon,
        Star,
        Underscore,
        Dollar,
        Plus,
        Minus,
        Slash,
        Hat,
        Dot,
        Eq,
        EqEq,
        ExclEq,
        Lt,
        LtEq,
        Gt,
        GtEq,
        PlusEq,
        HyphEq,
        StarEq,
        SlashEq,
        Dots,
        Arrow,
        Root,
        Bang,
        Not,
        And,
        Or,
        None,
        Auto,
        Let,
        Set,
        Show,
        Context,
        If,
        Else,
        For,
        In,
        While,
        Break,
        Continue,
        Return,
        Import,
        Include,
        As,
        Code,
        Ident,
        Bool,
        Int,
        Float,
        Numeric,
        Str,
        CodeBlock,
        ContentBlock,
        Parenthesized,
        Array,
        Dict,
        Named,
        Keyed,
        Unary,
        Binary,
        FieldAccess,
        FuncCall,
        Args,
        Spread,
        Closure,
        Params,
        LetBinding,
        SetRule,
        ShowRule,
        Contextual,
        Conditional,
        WhileLoop,
        ForLoop,
        ModuleImport,
        ImportItems,
        ImportItemPath,
        RenamedImportItem,
        ModuleInclude,
        LoopBreak,
        LoopContinue,
        FuncReturn,
        Destructuring,
        DestructAssignment,
    ]
};

// The list must be in discriminant order and end at the last variant.
const _: () = {
    let mut i = 0;
    while i < KINDS.len() {
        assert!(KINDS[i] as usize == i);
        i += 1;
    }
    assert!(KINDS[KINDS.len() - 1] as usize == SyntaxKind::DestructAssignment as usize);
};

thread_local! {
    // Handle table: a handle is index + 1, and a freed slot is None.
    static SOURCES: RefCell<Vec<Option<Source>>> = const { RefCell::new(Vec::new()) };
    // The tree buffer, reused by every `source_tree`.
    static OUT: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    // Byte offset to UTF-16 offset, for text that is not ASCII.
    static TABLE: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    // The kind names, built on first use.
    static NAMES: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocate `len` bytes of linear memory for the host to write into.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Free a buffer from `alloc`, with the `len` it was allocated with.
///
/// # Safety
/// `ptr` and `len` must be exactly what `alloc` returned and was given.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

/// The kind names joined by `\n`; line i is kind i.
#[no_mangle]
pub extern "C" fn kinds() -> *const u8 {
    NAMES.with(|n| {
        let mut n = n.borrow_mut();
        if n.is_empty() {
            let names: Vec<String> = KINDS.iter().map(|k| format!("{k:?}")).collect();
            *n = names.join("\n").into_bytes();
        }
        n.as_ptr()
    })
}

/// Length in bytes of what `kinds` points at.
#[no_mangle]
pub extern "C" fn kinds_len() -> u32 {
    kinds();
    NAMES.with(|n| n.borrow().len() as u32)
}

unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).ok()
}

/// Parse the UTF-8 text at `ptr` into a new source. Returns its handle, or 0
/// if the bytes are not valid UTF-8.
///
/// # Safety
/// `ptr..ptr+len` must be readable memory.
#[no_mangle]
pub unsafe extern "C" fn source_new(ptr: *const u8, len: usize) -> u32 {
    let Some(t) = text(ptr, len) else { return 0 };
    let src = Source::detached(t);
    SOURCES.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(i) = s.iter().position(Option::is_none) {
            s[i] = Some(src);
            i as u32 + 1
        } else {
            s.push(Some(src));
            s.len() as u32
        }
    })
}

/// Replace the UTF-16 range `[from, to)` with the UTF-8 text at `ptr`,
/// reparsing incrementally. Returns 1, or 0 and changes nothing if the handle
/// is unknown, the range is out of bounds or backwards, or the text is not
/// valid UTF-8.
///
/// # Safety
/// `ptr..ptr+len` must be readable memory.
#[no_mangle]
pub unsafe extern "C" fn source_edit(
    handle: u32,
    from: usize,
    to: usize,
    ptr: *const u8,
    len: usize,
) -> u32 {
    let Some(with) = text(ptr, len) else { return 0 };
    if from > to {
        return 0;
    }
    SOURCES.with(|s| {
        let mut s = s.borrow_mut();
        let Some(Some(src)) = s.get_mut((handle as usize).wrapping_sub(1)) else {
            return 0;
        };
        let lines = src.lines();
        // Out of range gives None; an index inside a surrogate pair rounds up
        // to the next character, so `edit` never sees a bad byte range.
        let (Some(a), Some(b)) = (lines.utf16_to_byte(from), lines.utf16_to_byte(to)) else {
            return 0;
        };
        src.edit(a..b, with);
        1
    })
}

/// Drop a source. An unknown handle is ignored.
#[no_mangle]
pub extern "C" fn source_free(handle: u32) {
    SOURCES.with(|s| {
        if let Some(slot) = s.borrow_mut().get_mut((handle as usize).wrapping_sub(1)) {
            *slot = None;
        }
    })
}

/// Write the nodes of `node` (starting at byte `start`) in postorder.
fn walk(node: &SyntaxNode, start: usize, u16_of: &dyn Fn(usize) -> u32, out: &mut Vec<u32>) {
    let first = out.len();
    let mut at = start;
    for child in node.children() {
        walk(child, at, u16_of, out);
        at += child.len();
    }
    let size = (out.len() - first) as u32 + 4;
    out.extend([
        node.kind() as u32,
        u16_of(start),
        u16_of(start + node.len()),
        size,
    ]);
}

/// Flatten the whole tree into the reused buffer and return a pointer to it;
/// `tree_len` says how many u32s. An unknown handle gives an empty buffer.
#[no_mangle]
pub extern "C" fn source_tree(handle: u32) -> *const u32 {
    SOURCES.with(|s| {
        let s = s.borrow();
        OUT.with(|o| {
            let mut o = o.borrow_mut();
            o.clear();
            let Some(Some(src)) = s.get((handle as usize).wrapping_sub(1)) else {
                return o.as_ptr();
            };
            let text = src.text();
            if text.is_ascii() {
                walk(src.root(), 0, &|b| b as u32, &mut o);
            } else {
                // One prefix table per call keeps the conversion linear.
                TABLE.with(|t| {
                    let mut t = t.borrow_mut();
                    t.clear();
                    t.resize(text.len() + 1, 0);
                    let mut u = 0u32;
                    for (i, c) in text.char_indices() {
                        // Bytes inside a character map to the character's start.
                        for k in 0..c.len_utf8() {
                            t[i + k] = u;
                        }
                        u += c.len_utf16() as u32;
                    }
                    t[text.len()] = u;
                    walk(src.root(), 0, &|b| t[b], &mut o);
                });
            }
            o.as_ptr()
        })
    })
}

/// Number of u32s written by the last `source_tree`.
#[no_mangle]
pub extern "C" fn tree_len() -> u32 {
    OUT.with(|o| o.borrow().len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(t: &str) -> u32 {
        let h = unsafe { source_new(t.as_ptr(), t.len()) };
        assert_ne!(h, 0);
        h
    }

    fn edit(h: u32, from: usize, to: usize, t: &str) -> u32 {
        unsafe { source_edit(h, from, to, t.as_ptr(), t.len()) }
    }

    fn tree(h: u32) -> Vec<u32> {
        let p = source_tree(h);
        let n = tree_len() as usize;
        unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
    }

    fn names() -> Vec<String> {
        let p = kinds();
        let n = kinds_len() as usize;
        let b = unsafe { std::slice::from_raw_parts(p, n) };
        std::str::from_utf8(b)
            .unwrap()
            .split('\n')
            .map(String::from)
            .collect()
    }

    #[test]
    fn kinds_in_order() {
        let n = names();
        assert_eq!(n.len(), KINDS.len());
        assert_eq!(n[0], "End");
        assert_eq!(n[SyntaxKind::Markup as usize], "Markup");
        assert_eq!(n[SyntaxKind::Heading as usize], "Heading");
        assert_eq!(n[SyntaxKind::Strong as usize], "Strong");
        assert_eq!(n[KINDS.len() - 1], "DestructAssignment");
        for (i, k) in KINDS.iter().enumerate() {
            assert_eq!(*k as usize, i);
            assert_eq!(n[i], format!("{k:?}"));
        }
    }

    /// Checks postorder, sizes and containment; returns the node count.
    fn check(buf: &[u32], utf16_len: u32) -> usize {
        assert_eq!(buf.len() % 4, 0);
        let count = buf.len() / 4;
        let root = &buf[buf.len() - 4..];
        assert_eq!(root[0], SyntaxKind::Markup as u32);
        assert_eq!((root[1], root[2]), (0, utf16_len));
        assert_eq!(root[3] as usize, buf.len());
        // A stack of (index, subtree start index) walks the postorder buffer.
        let mut open: Vec<usize> = Vec::new();
        for i in 0..count {
            let size = buf[i * 4 + 3] as usize;
            assert!(size >= 4 && size.is_multiple_of(4) && size <= (i + 1) * 4);
            let first = i + 1 - size / 4;
            // Children are the open nodes that start inside this subtree.
            let mut kids = Vec::new();
            while let Some(&k) = open.last() {
                if k >= first {
                    kids.push(k);
                    open.pop();
                } else {
                    break;
                }
            }
            let mut covered = 1;
            let (from, to) = (buf[i * 4 + 1], buf[i * 4 + 2]);
            assert!(from <= to);
            for &k in &kids {
                covered += buf[k * 4 + 3] as usize / 4;
                assert!(buf[k * 4 + 1] >= from && buf[k * 4 + 2] <= to);
            }
            assert_eq!(covered, size / 4);
            open.push(i);
        }
        assert_eq!(open, vec![count - 1]);
        count
    }

    #[test]
    fn well_formed() {
        for t in [
            "",
            "= Title\n\nSome *bold* text and $x^2$.\n#let a = (1, 2)\n",
            "#{ ( [",
        ] {
            let h = new(t);
            let b = tree(h);
            check(&b, t.encode_utf16().count() as u32);
            source_free(h);
        }
    }

    #[test]
    fn utf16_offsets() {
        let t = "é *😀* 漢字 #let x = \"é\"\n😀";
        let h = new(t);
        let b = tree(h);
        check(&b, t.encode_utf16().count() as u32);
        // Strong spans `*😀*`: é and a space precede it, the emoji is two units.
        let strong = b
            .chunks(4)
            .find(|n| n[0] == SyntaxKind::Strong as u32)
            .unwrap();
        assert_eq!((strong[1], strong[2]), (2, 6));
        // Every offset lands on a character boundary of the UTF-16 text.
        let units: Vec<u16> = t.encode_utf16().collect();
        for n in b.chunks(4) {
            for o in [n[1], n[2]] {
                assert!((o as usize) <= units.len());
                assert!(!(0xDC00..0xE000).contains(&units.get(o as usize).copied().unwrap_or(0)));
            }
        }
    }

    #[test]
    fn edit_matches_fresh() {
        let mut text = String::from("= é Title\n\nSome *bold* 😀 text.\n#let a = (1, 2)\n");
        let h = new(&text);
        // (from, to, with) in UTF-16 units of the current text.
        let steps: [(usize, usize, &str); 5] = [
            (2, 3, "Ê"),
            (14, 14, "*"),
            (0, 0, "== "),
            (10, 12, "漢"),
            (0, 0, "#"),
        ];
        for (from, to, with) in steps {
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut next = String::from_utf16(&units[..from]).unwrap();
            next.push_str(with);
            next.push_str(&String::from_utf16(&units[to..]).unwrap());
            assert_eq!(edit(h, from, to, with), 1);
            text = next;
            let fresh = new(&text);
            assert_eq!(tree(h), tree(fresh), "after {with:?}");
            source_free(fresh);
        }
        source_free(h);
    }

    #[test]
    fn bad_edit_returns_zero() {
        let h = new("abc");
        let before = tree(h);
        assert_eq!(edit(h, 0, 4, "x"), 0);
        assert_eq!(edit(h, 4, 4, "x"), 0);
        assert_eq!(edit(h, 2, 1, "x"), 0);
        assert_eq!(edit(0, 0, 0, "x"), 0);
        assert_eq!(edit(999, 0, 0, "x"), 0);
        assert_eq!(unsafe { source_edit(h, 0, 0, [0xffu8].as_ptr(), 1) }, 0);
        assert_eq!(tree(h), before);
        source_free(h);
        source_free(h);
        assert!(tree(h).is_empty());
    }
}
