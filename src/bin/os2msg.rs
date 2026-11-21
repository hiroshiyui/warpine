// SPDX-License-Identifier: GPL-3.0-or-later
//
// os2msg — the message compiler of Warpine's C SDK (ADR-0105).
//
//   os2msg [--codepage N] input.txt [-o output.msg]
//
// Compiles a message source into a message file in Warpine's own format
// (`src/loader/msgfile.rs`), which `DosGetMessage` reads from a drive or,
// given to `lx-link`, from the program's bound message segment:
//
//   ; a comment
//   ABC
//   ABC0001E: The file %1 was not found.
//   ABC0002I: A message
//   on two lines.
//   ABC0003P: Continue (Y/N)?
//
// The texts' bytes are kept as they are; `--codepage` records the
// codepage they are in (default 0: the reading process's).

use std::path::PathBuf;
use std::process;

#[path = "../loader/msgfile.rs"]
#[allow(dead_code)] // the segment helpers are lx-link's and Warpine's
mod msgfile;

struct Args {
    input: PathBuf,
    output: PathBuf,
    codepage: u16,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut input = None;
    let mut output = None;
    let mut codepage = 0u16;
    let mut it = raw.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => output = Some(PathBuf::from(it.next().ok_or("-o needs a file")?)),
            "--codepage" => {
                let v = it.next().ok_or("--codepage needs a number")?;
                codepage = v.parse().map_err(|_| format!("bad codepage {v:?}"))?;
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if input.is_none() => input = Some(PathBuf::from(s)),
            s => return Err(format!("more than one input ({s})")),
        }
    }
    let input = input.ok_or("usage: os2msg [--codepage N] input.txt [-o output.msg]")?;
    let output = output.unwrap_or_else(|| input.with_extension("msg"));
    Ok(Args { input, output, codepage })
}

fn run(raw: &[String]) -> Result<(), String> {
    let args = parse_args(raw)?;
    let source = std::fs::read(&args.input).map_err(|e| format!("{}: {e}", args.input.display()))?;
    let file = msgfile::compile(&source, args.codepage).map_err(|e| format!("{}: {e}", args.input.display()))?;
    let bytes = msgfile::write(&file).map_err(|e| format!("{}: {e}", args.input.display()))?;
    std::fs::write(&args.output, bytes).map_err(|e| format!("{}: {e}", args.output.display()))
}

fn main() {
    let raw: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&raw) {
        eprintln!("os2msg: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> { std::iter::once("os2msg").chain(v.iter().copied()).map(String::from).collect() }

    #[test]
    fn arguments_name_the_files_and_the_codepage() {
        let a = parse_args(&args(&["x.txt"])).expect("args");
        assert_eq!((a.input, a.output, a.codepage), ("x.txt".into(), "x.msg".into(), 0));
        let a = parse_args(&args(&["--codepage", "850", "x.txt", "-o", "y.msg"])).expect("args");
        assert_eq!((a.output, a.codepage), ("y.msg".into(), 850));
        for bad in [&[][..], &["-q", "x.txt"], &["a.txt", "b.txt"], &["--codepage", "x", "a.txt"], &["a.txt", "-o"]] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn compiles_a_source_into_a_file_warpine_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("m.txt");
        std::fs::write(&src, "XYZ\nXYZ0003W: Careful: %1.\n").expect("write");
        run(&args(&["--codepage", "437", src.to_str().unwrap()])).expect("run");
        let f = msgfile::parse(&std::fs::read(dir.path().join("m.msg")).expect("read")).expect("parse");
        assert_eq!((f.codepage, f.first), (437, 3));
        assert_eq!(f.get(3).unwrap().text, b"Careful: %1.\r\n");
        std::fs::write(&src, "XYZ\nnot a message\n").expect("write");
        assert!(run(&args(&[src.to_str().unwrap()])).unwrap_err().contains("line 2"));
    }
}
