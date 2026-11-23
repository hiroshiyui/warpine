// SPDX-License-Identifier: GPL-3.0-or-later
//
// somidl — the SOM IDL compiler of Warpine's C SDK (ADR-0122).
//
//   somidl [-I dir]... [-D name[=value]]... [-s emitters] [-o dir] [--dump] file.idl
//
// Reads a SOM IDL file (CORBA 2.0 IDL with SOM's implementation section)
// through the SDK tools' C preprocessor, with __SOMIDL__ defined. This
// stage has the front end only: `--dump` prints the syntax tree. The
// emitters (`-s h,ih,xh,xih,c,xc,def`) come with stage S1's later batches.

use std::path::PathBuf;
use std::process;

#[path = "../cpp.rs"]
#[allow(dead_code)]
mod cpp;
#[path = "../somidl/mod.rs"]
#[allow(dead_code)]
mod somidl;

#[derive(Debug, Default, PartialEq)]
struct Args {
    include_dirs: Vec<PathBuf>,
    defines: Vec<String>,
    emitters: Option<String>,
    out_dir: Option<PathBuf>,
    dump: bool,
    input: PathBuf,
}

fn usage() -> ! {
    eprintln!("usage: somidl [-I dir]... [-D name[=value]]... [-s emitters] [-o dir] [--dump] file.idl");
    process::exit(2);
}

/// The command line; `-Idir` and `-Dname` may also be written joined.
fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut input = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let mut value = |flag: &str| -> Result<String, String> {
            if arg.len() > flag.len() { return Ok(arg[flag.len()..].to_string()); }
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--dump" => a.dump = true,
            s if s.starts_with("-I") => a.include_dirs.push(PathBuf::from(value("-I")?)),
            s if s.starts_with("-D") => a.defines.push(value("-D")?),
            s if s.starts_with("-s") => a.emitters = Some(value("-s")?),
            s if s.starts_with("-o") => a.out_dir = Some(PathBuf::from(value("-o")?)),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => {
                if input.is_some() { return Err("only one input file".into()); }
                input = Some(PathBuf::from(s));
            }
        }
        i += 1;
    }
    a.input = input.ok_or("no input file")?;
    Ok(a)
}

fn run(args: &Args) -> Result<String, String> {
    let defs = somidl::parse_file(&args.input, &args.include_dirs, &args.defines)?;
    if args.dump {
        return Ok(somidl::dump::dump(&defs));
    }
    Err(format!("{}: parsed; the emitters ({}) are not implemented yet (ADR-0122, stage S1): use --dump",
                args.input.display(), args.emitters.as_deref().unwrap_or("h,ih")))
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => { eprintln!("somidl: {e}"); usage(); }
    };
    match run(&args) {
        Ok(text) => print!("{text}"),
        Err(e) => {
            eprintln!("somidl: {e}");
            process::exit(1);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use somidl::ast::*;
    use somidl::parser;

    fn dump(text: &str) -> String {
        match somidl::parse_in_memory(text.as_bytes()) {
            Ok(defs) => somidl::dump::dump(&defs),
            Err(e) => panic!("{e}"),
        }
    }

    fn error(text: &str) -> String {
        somidl::parse_in_memory(text.as_bytes()).expect_err("an error")
    }

    #[test]
    fn interfaces_attributes_and_operations() {
        let text = "interface Hello : SOMObject {\n\
                    \x20 readonly attribute string<8> who, where;\n\
                    \x20 oneway void wave(in long times);\n\
                    \x20 long add(in long a, inout short b, out unsigned long c) raises(Bad) context(\"x\", \"y\");\n\
                    };\n";
        assert_eq!(dump(text), "interface Hello : SOMObject\n\
                                \x20 readonly attribute string<8> who, where\n\
                                \x20 oneway operation void wave(in long times)\n\
                                \x20 operation long add(in long a, inout short b, out unsigned long c) raises(Bad) context(\"x\", \"y\")\n");
    }

    #[test]
    fn modules_types_and_constants() {
        let text = "module M {\n\
                    \x20 interface Fwd;\n\
                    \x20 const long SIZE = (1 << 3) + 2;\n\
                    \x20 const string NAME = \"a\" \"b\";\n\
                    \x20 typedef sequence<sequence<long, 4>> Grid;\n\
                    \x20 struct Point { long x, y; octet tag[SIZE]; };\n\
                    \x20 enum Color { red, green };\n\
                    \x20 const long TWICE = M::SIZE * 2 > 4 ? ::M::SIZE : 0;\n\
                    \x20 interface I : ::M::Fwd, SOMObject { typedef any Thing; };\n\
                    };\n";
        assert_eq!(dump(text), "module M\n\
                                \x20 forward Fwd\n\
                                \x20 const long SIZE = 10\n\
                                \x20 const string NAME = \"ab\"\n\
                                \x20 typedef sequence<sequence<long, 4>> Grid\n\
                                \x20 struct Point\n\
                                \x20   long x, y\n\
                                \x20   octet tag[10]\n\
                                \x20 enum Color { red, green }\n\
                                \x20 const long TWICE = 10\n\
                                \x20 interface I : ::M::Fwd, SOMObject\n\
                                \x20   typedef any Thing\n");
    }

    /// The implementation section: release order, class modifiers,
    /// member modifiers, instance variables and passthru lines.
    #[test]
    fn the_implementation_section() {
        let text = "interface A : SOMObject {\n\
                    \x20 void f();\n\
                    #ifdef __SOMIDL__\n\
                    \x20 implementation {\n\
                    \x20   releaseorder: f, _get_x;\n\
                    \x20   majorversion = 1; dllname = \"a.dll\"; metaclass = M::Meta; callstyle = oidl;\n\
                    \x20   somInit: override; f: init, namelookup;\n\
                    \x20   long count; M::Thing t; string names[2];\n\
                    \x20   passthru C_h = \"#include <os2.h>\" \"#define X 1\";\n\
                    \x20   persistent;\n\
                    \x20 };\n\
                    #endif\n\
                    };\n";
        let d = dump(text);
        assert_eq!(d, "interface A : SOMObject\n\
                       \x20 operation void f()\n\
                       \x20 implementation\n\
                       \x20   releaseorder f, _get_x\n\
                       \x20   majorversion = 1\n\
                       \x20   dllname = \"a.dll\"\n\
                       \x20   metaclass = M::Meta\n\
                       \x20   callstyle = oidl\n\
                       \x20   somInit: override\n\
                       \x20   f: init, namelookup\n\
                       \x20   data long count\n\
                       \x20   data M::Thing t\n\
                       \x20   data string names[2]\n\
                       \x20   passthru C_h\n\
                       \x20     \"#include <os2.h>\"\n\
                       \x20     \"#define X 1\"\n\
                       \x20   persistent\n");
        let mut no_somidl = somidl::parse_source(text.as_bytes(), std::path::Path::new("a.idl"), &[], &["X".into()], false)
            .expect("parses");
        let Definition::Interface(i) = no_somidl.remove(0) else { panic!() };
        assert!(i.items.iter().any(|it| matches!(it, InterfaceItem::Implementation(_))), "__SOMIDL__ is defined");
    }

    #[test]
    fn mistakes_are_reported_with_their_place() {
        assert!(error("interface A {\n void f(long x);\n};\n").contains("input.idl:2: expected in, out or inout"));
        assert!(error("interface A { void f() };").contains("expected ;"));
        assert!(error("module M { interface A {};").contains("not closed"));
        assert!(error("union U switch (long) { case 1: long a; };").contains("not supported yet"));
        assert!(error("typedef string<0> S;").contains("positive"));
        assert!(error("const long X = Y;").contains("unknown name Y"));
        assert!(error("#include <somobj.idl>\n").contains("cannot find include file"), "no file is read");
        let deep = "module M {".repeat(parser::MAX_NESTING + 1);
        assert!(error(&deep).contains("nested too deeply"));
        let deep_seq = format!("typedef {}long{} T;", "sequence<".repeat(parser::MAX_NESTING + 1), ">".repeat(parser::MAX_NESTING + 1));
        assert!(error(&deep_seq).contains("nested too deeply"));
    }

    #[test]
    fn the_command_line() {
        let argv: Vec<String> = ["-Iinc", "-D", "A=1", "-sh,ih", "-o", "out", "--dump", "x.idl"].map(String::from).to_vec();
        let a = parse_args(&argv).expect("parses");
        assert_eq!(a, Args { include_dirs: vec!["inc".into()], defines: vec!["A=1".into()], emitters: Some("h,ih".into()),
                             out_dir: Some("out".into()), dump: true, input: "x.idl".into() });
        assert!(parse_args(&["-I".into()]).is_err());
        assert!(parse_args(&["a.idl".into(), "b.idl".into()]).is_err());
        assert!(parse_args(&["--what".into(), "a.idl".into()]).is_err());
        assert!(parse_args(&[]).is_err());
    }

    /// Open Watcom's SOM samples parse in place (read, never copied), with
    /// stand-ins for the root classes' IDL, which the SDK gains in a later
    /// batch.
    #[test]
    fn open_watcom_s_sample_idl_parses() {
        let samples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/watcom/samples/os2/som");
        if !samples.is_dir() { return; }
        let roots = tempfile::tempdir().expect("tempdir");
        std::fs::write(roots.path().join("somobj.idl"), "#ifndef somobj_idl\n#define somobj_idl\ninterface SOMObject {};\n#endif\n").unwrap();
        std::fs::write(roots.path().join("somcls.idl"), "#ifndef somcls_idl\n#define somcls_idl\ninterface SOMClass : SOMObject {};\n#endif\n").unwrap();
        let inc = [roots.path().to_path_buf()];
        let hello = somidl::parse_file(&samples.join("helloc/hello.idl"), &inc, &[]).expect("hello.idl");
        let text = somidl::dump::dump(&hello);
        assert!(text.contains("interface Hello : SOMObject\n  operation void sayHello()\n  implementation\n    releaseorder sayHello\n"), "{text}");
        let animals = somidl::parse_file(&samples.join("animals/animals.idl"), &inc, &[]).expect("animals.idl");
        let text = somidl::dump::dump(&animals);
        for want in ["module Animals\n  forward M_Animal\n  interface Animal : SOMObject\n",
                     "    typedef sequence<Animal> AnimalInstances\n",
                     "    readonly attribute string genus\n",
                     "      functionprefix = a_\n      metaclass = M_Animal\n      data long nsound\n      name: noset\n",
                     "  interface M_Animal : SOMClass\n    operation Animal newAnimal(in string name, in string sound)\n",
                     "  interface Dog : Animal\n", "  interface LittleDog : Dog\n"] {
            assert!(text.contains(want), "{want:?} missing in\n{text}");
        }
    }
}
