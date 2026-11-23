// SPDX-License-Identifier: GPL-3.0-or-later
//
// somidl, the SOM IDL compiler of Warpine's SDK (ADR-0122). The front end:
// the shared C preprocessor (`src/cpp.rs`, with `__SOMIDL__` defined),
// then the parser to a syntax tree. The emitters follow in later stages.
//
// The including crate declares `mod cpp` (`#[path = "../cpp.rs"]`) at
// its root, beside this module.

pub mod ast;
pub mod dump;
pub mod parser;

use crate::cpp::{Error, Preprocessor};
use std::path::{Path, PathBuf};

/// The names an IDL file sees defined (ADR-0122, unverified).
pub const PREDEFINED: &[&str] = &["__SOMIDL__"];

/// Parse `text`, the contents of `input`; included files are searched
/// next to it and in `include_dirs`, and read only when `reads_files`.
/// `defines` are `-D` arguments (`NAME` or `NAME=value`).
pub fn parse_source(text: &[u8], input: &Path, include_dirs: &[PathBuf], defines: &[String], reads_files: bool)
                    -> Result<Vec<ast::Definition>, String> {
    let mut pre = Preprocessor::new(include_dirs.to_vec(), PREDEFINED);
    pre.reads_files = reads_files;
    for d in defines {
        pre.define_cmdline(d).map_err(|e| e.msg)?;
    }
    let describe = |e: Error, files: &[PathBuf]| match e.loc {
        Some(l) => {
            let file = files.get(l.file).map_or("?".into(), |f| f.display().to_string());
            if l.line > 0 { format!("{file}:{}: {}", l.line, e.msg) } else { format!("{file}: {}", e.msg) }
        }
        None => e.msg,
    };
    if let Err(e) = pre.run_text(text, input, true, None) {
        return Err(describe(e, &pre.files));
    }
    parser::parse(&pre.out).map_err(|e| describe(e, &pre.files))
}

/// Parse the file at `input`.
pub fn parse_file(input: &Path, include_dirs: &[PathBuf], defines: &[String]) -> Result<Vec<ast::Definition>, String> {
    let text = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    parse_source(&text, input, include_dirs, defines, true)
}

/// Parse `text` alone, reading no file (the fuzz target's entry, ADR-0112).
pub fn parse_in_memory(text: &[u8]) -> Result<Vec<ast::Definition>, String> {
    parse_source(text, Path::new("input.idl"), &[], &[], false)
}
