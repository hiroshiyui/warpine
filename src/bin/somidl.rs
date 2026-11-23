// SPDX-License-Identifier: GPL-3.0-or-later
//
// somidl — the SOM IDL compiler of Warpine's C SDK (ADR-0122).
//
//   somidl [-I dir]... [-D name[=value]]... [-s emitters] [-o dir] [--dump] file.idl
//
// Reads a SOM IDL file (CORBA 2.0 IDL with SOM's implementation section)
// through the SDK tools' C preprocessor, with __SOMIDL__ defined, and
// writes bindings for the interfaces of that file (`-s`, default h,ih):
// h / xh (C / C++ usage), ih / xih (implementation), c / xc (a template,
// written only when the file does not exist yet) and def (a class
// library's exports), into -o's directory (default: the current one).
// `--dump` prints the syntax tree instead.

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

/// The emitters run when `-s` is not given.
const DEFAULT_EMITTERS: &str = "h,ih";

/// Parse the input; `--dump` gives the tree as text, else the emitters
/// write their files into the output directory (the current one by
/// default). The `c` template is written only when the file is not there
/// yet, so it never overwrites an implementation.
fn run(args: &Args) -> Result<String, String> {
    let defs = somidl::parse_file(&args.input, &args.include_dirs, &args.defines)?;
    if args.dump {
        return Ok(somidl::dump::dump(&defs));
    }
    let stem = args.input.file_stem().and_then(|s| s.to_str()).ok_or("the input has no file name")?;
    let kinds: Vec<&str> = args.emitters.as_deref().unwrap_or(DEFAULT_EMITTERS).split(',')
        .map(str::trim).filter(|k| !k.is_empty()).collect();
    let outputs = somidl::emit_c::emit(&defs, stem, &kinds).map_err(|e| format!("{}: {e}", args.input.display()))?;
    let dir = args.out_dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let mut written = String::new();
    for o in outputs {
        let path = dir.join(&o.name);
        if (o.name.ends_with(".c") || o.name.ends_with(".xc")) && path.exists() {
            written.push_str(&format!("{}: kept (it exists)\n", path.display()));
            continue;
        }
        std::fs::write(&path, o.text).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push_str(&format!("{}\n", path.display()));
    }
    Ok(written)
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
        assert!(error("union U switch (long) { long a; };").contains("expected case or default"));
        assert!(error("union U (long) { case 1: long a; };").contains("expected switch"));
        assert!(error("exception E { long a; ").contains("not closed"));
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

    fn emit(text: &str, kinds: &[&str]) -> Result<Vec<somidl::emit_c::Output>, String> {
        let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/include");
        let defs = somidl::parse_source(text.as_bytes(), std::path::Path::new("t.idl"), &[sdk], &[], true)?;
        somidl::emit_c::emit(&defs, "t", kinds)
    }

    fn emitted(text: &str, kind: &str) -> String {
        emit(text, &[kind]).unwrap_or_else(|e| panic!("{e}")).remove(0).text
    }

    const COUNTER: &str = "#include <somobj.idl>\n\
        interface Counter : SOMObject {\n\
        \x20 long add(in long n);\n\
        \x20 void peek(out long calls, inout string s);\n\
        \x20 implementation {\n\
        \x20   releaseorder: peek, add;\n\
        \x20   functionprefix = c_; majorversion = 1; minorversion = 2;\n\
        \x20   long total; Counter next;\n\
        \x20   somInit: override; somDestruct: override;\n\
        \x20 };\n\
        };\n\
        interface Kid : Counter { implementation { add: override; }; };\n";

    /// The usage bindings: method types, NewClass, ClassData in release
    /// order, the class macro, `_method` macros with the Environment.
    #[test]
    fn usage_bindings() {
        let h = emitted(COUNTER, "h");
        for want in ["#include <som.h>", "#define Counter_MajorVersion 1\n#define Counter_MinorVersion 2",
                     "typedef SOMObject Counter;",
                     "typedef long SOMLINK somTP_Counter_add( Counter *somSelf, Environment *ev, long n );",
                     "typedef void SOMLINK somTP_Counter_peek( Counter *somSelf, Environment *ev, long *calls, string *s );",
                     "    SOMClass *classObject;\n    somMToken peek;\n    somMToken add;\n} SOMDLINK CounterClassData;",
                     "#define _Counter (CounterClassData.classObject ? CounterClassData.classObject : CounterNewClass( Counter_MajorVersion, Counter_MinorVersion ))",
                     "#define CounterNew() ((Counter *)_somNew( _Counter ))",
                     "#define Counter_add( somSelf, ev, n ) \\\n    SOM_Resolve( somSelf, Counter, add )( (Counter *)(somSelf), (ev), (n) )",
                     "#define _add Counter_add", "SOMEXTERN SOMClass * SOMLINK KidNewClass("] {
            assert!(h.contains(want), "{want:?} missing in\n{h}");
        }
    }

    /// The implementation bindings: instance data and its macros, the
    /// prototypes (overrides take the ancestor's signature, oidl ones no
    /// Environment), parent calls, and the static class information.
    #[test]
    fn implementation_bindings() {
        let ih = emitted(COUNTER, "ih");
        for want in ["#include \"t.h\"", "typedef struct CounterData_struct {\n    long total;\n    Counter *next;\n} CounterData;",
                     "somDataResolve( (SOMObject *)(somSelf), CounterCClassData.instanceDataToken )",
                     "#define _total (somThis->total)",
                     "SOM_Scope long SOMLINK c_add( Counter *somSelf, Environment *ev, long n );",
                     "SOM_Scope void SOMLINK c_somInit( Counter *somSelf );",
                     "SOM_Scope void SOMLINK c_somDestruct( Counter *somSelf, octet doFree, somDestructCtrl *ctrl );",
                     "#define Counter_parent_SOMObject_somInit( somSelf ) \\\n    SOM_ParentNumResolve( SOMObject, 1, CounterCClassData.parentMtab, somInit )( (SOMObject *)(somSelf) )",
                     "{ &CounterClassData.add, &somMN_Counter_add, 0, (somMethodProc *)c_add, 0, 0 },",
                     "{ &somON_Counter_somInit, (somMethodProc *)c_somInit },",
                     "    1, 2, 2, 0, 0, 0, 1, 2, sizeof( CounterData ), 2, 1, &somCN_Counter_s, 0, 0,",
                     "#define Kid_parent_Counter_add( somSelf, ev, n ) \\\n    SOM_ParentNumResolve( Counter, 1, KidCClassData.parentMtab, add )( (Counter *)(somSelf), (ev), (n) )",
                     "#define KidGetData( somSelf ) ((void)(somSelf), (KidData *)0)",
                     "    CounterNewClass( Counter_MajorVersion, Counter_MinorVersion );\n    return( somBuildClass( 1, &somSCI_Kid, majorVersion, minorVersion ) );"] {
            assert!(ih.contains(want), "{want:?} missing in\n{ih}");
        }
        assert!(!ih.contains("SOMObject_MajorVersion"), "the root classes' NewClass takes any version");
    }

    #[test]
    fn template_and_def() {
        let c = emitted(COUNTER, "c");
        assert!(c.contains("SOM_Scope long SOMLINK c_add( Counter *somSelf, Environment *ev, long n )\n{"), "{c}");
        assert!(c.contains("    return( (long)0 );"), "{c}");
        assert!(c.contains("    Counter_parent_somInit( somSelf );"), "{c}");
        assert!(c.contains("    return( Kid_parent_add( somSelf, ev, n ) );"), "{c}");
        assert_eq!(emitted(COUNTER, "def"), "CounterNewClass\nCounterClassData\nCounterCClassData\nKidNewClass\nKidClassData\nKidCClassData\n");
    }

    /// Mistakes are refused by name, with their place.
    #[test]
    fn mistakes_are_refused_by_name() {
        let err = |text: &str| emit(text, &["h"]).err().unwrap_or_default();
        let base = "#include <somcls.idl>\n";
        assert!(err(&format!("{base}interface A : SOMObject {{ implementation {{ metaclass = Nowhere; }}; }};")).contains("metaclass Nowhere"));
        assert!(err(&format!("{base}interface A : SOMObject {{ void f(); implementation {{ f: init; }}; }};")).contains("inout somInitCtrl ctrl"));
        assert!(err(&format!("{base}interface A : SOMObject {{ implementation {{ directinitclasses = \"Nope\"; }}; }};")).contains("directinitclasses names Nope"));
        assert!(err(&format!("{base}interface A : SOMObject {{ implementation {{ g: override; }}; }};")).contains("overrides no method"));
        assert!(err(&format!("{base}interface A : SOMObject {{ implementation {{ releaseorder: g; }}; }};")).contains("does not declare"));
        assert!(err("interface A : Nowhere { };").contains("not declared"));
        assert!(emit("interface A { };", &["xyz"]).err().unwrap_or_default().contains("unknown emitter"));
    }

    /// Batch 5b: module-scoped names, attributes (accessors, their data,
    /// implementations unless noget / noset, none with nodata), sequences
    /// and an explicit metaclass, made before the class.
    #[test]
    fn modules_attributes_sequences_and_metaclasses() {
        let text = "#include <somcls.idl>\n\
            module Zoo {\n\
            \x20 interface Keeper;\n\
            \x20 interface Pet : SOMObject {\n\
            \x20   attribute string name;\n\
            \x20   readonly attribute long legs;\n\
            \x20   attribute sequence<Pet> friends;\n\
            \x20   attribute long age;\n\
            \x20   implementation { releaseorder: _get_name, _set_name, _get_legs, _get_friends, _set_friends, _get_age, _set_age;\n\
            \x20     metaclass = Keeper; functionprefix = p_; legs: nodata; age: noset; };\n\
            \x20 };\n\
            \x20 interface Keeper : SOMClass { Pet adopt(in string name); };\n\
            \x20 interface Cat : Pet { implementation { _get_legs: override; }; };\n\
            };\n";
        let h = emitted(text, "h");
        for want in ["typedef SOMObject Zoo_Pet;", "typedef SOMObject Zoo_Keeper;",
                     "typedef struct {\n    unsigned long _maximum;\n    unsigned long _length;\n    Zoo_Pet **_buffer;\n} _IDL_SEQUENCE_Zoo_Pet;",
                     "typedef string SOMLINK somTP_Zoo_Pet__get_name( Zoo_Pet *somSelf, Environment *ev );",
                     "typedef void SOMLINK somTP_Zoo_Pet__set_name( Zoo_Pet *somSelf, Environment *ev, string name );",
                     "typedef _IDL_SEQUENCE_Zoo_Pet SOMLINK somTP_Zoo_Pet__get_friends(",
                     "#define __get_name Zoo_Pet__get_name",
                     "typedef Zoo_Pet *SOMLINK somTP_Zoo_Keeper_adopt( Zoo_Keeper *somSelf, Environment *ev, string name );"] {
            assert!(h.contains(want), "{want:?} missing in\n{h}");
        }
        assert!(!h.contains("_set_legs"), "readonly: no setter");
        let ih = emitted(text, "ih");
        for want in ["typedef struct Zoo_PetData_struct {\n    string name;\n    _IDL_SEQUENCE_Zoo_Pet friends;\n    long age;\n} Zoo_PetData;",
                     "SOM_Scope string SOMLINK p__get_name( Zoo_Pet *somSelf, Environment *ev )\n{",
                     "    return( somThis->name );", "    somThis->name = name;",
                     "static char somCN_Zoo_Pet_t[] = \"Zoo::Pet\";", "static char somMC_Zoo_Pet_t[] = \"Zoo::Keeper\";",
                     "    SOMObjectNewClass( 0, 0 );\n    Zoo_KeeperNewClass( Zoo_Keeper_MajorVersion, Zoo_Keeper_MinorVersion );",
                     "SOM_Scope long SOMLINK _get_legs( Zoo_Cat *somSelf, Environment *ev );",
                     "#define Zoo_Cat_parent_Zoo_Pet__get_legs( somSelf, ev ) \\\n    SOM_ParentNumResolve( Zoo_Pet, 1, Zoo_CatCClassData.parentMtab, _get_legs )"] {
            assert!(ih.contains(want), "{want:?} missing in\n{ih}");
        }
        assert!(!ih.contains("return( somThis->legs )"), "nodata: the user implements it");
        assert!(!ih.contains("somThis->age = age;"), "noset: the user implements it");
        let c = emitted(text, "c");
        assert!(c.contains("p__get_legs(") && c.contains("p__set_age(") && !c.contains("p__get_name("), "{c}");
    }

    /// Stage S2: the C++ bindings. The class is a C++ class deriving from
    /// its parent's, with SOM_CPP_CLASS and inline methods through the
    /// tokens; the implementation bindings are the C ones over the .xh.
    #[test]
    fn cpp_bindings() {
        let xh = emitted(COUNTER, "xh");
        for want in ["#include <som.xh>\n#include <somapi.h>", "class Counter;\nclass Kid;",
                     "class Counter : public SOMObject {\npublic:\n    SOM_CPP_CLASS( Counter )",
                     "    long add( Environment *ev, long n ) { return SOM_Resolve( this, Counter, add )( this, ev, n ); }",
                     "    void peek( Environment *ev, long *calls, string *s ) { SOM_Resolve( this, Counter, peek )( this, ev, calls, s ); }",
                     "class Kid : public Counter {"] {
            assert!(xh.contains(want), "{want:?} missing in\n{xh}");
        }
        assert!(!xh.contains("typedef SOMObject Counter;") && !xh.contains("#define _add"), "{xh}");
        let xih = emitted(COUNTER, "xih");
        assert!(xih.contains("#include \"t.xh\"") && xih.contains("static char somCN_Counter_t[] = \"Counter\";"), "{xih}");
        let xc = emitted(COUNTER, "xc");
        assert!(xc.starts_with("/* t.xc: generated by somidl") && xc.contains("#include \"t.xih\""), "{xc}");
    }

    /// Stage S3: two parents (each made, named, and callable as a parent),
    /// directinitclasses set after somBuildClass, an init method (in C++
    /// also a constructor), later parents' methods forwarded in C++.
    #[test]
    fn multiple_inheritance_and_initializers() {
        let text = "#include <somobj.idl>\n\
            interface L : SOMObject { long left(); implementation { somInit: override; }; };\n\
            interface R : SOMObject { long right(in long x); };\n\
            interface LR : L, R {\n\
            \x20 void withX(inout somInitCtrl ctrl, in long x);\n\
            \x20 implementation { directinitclasses = \"R, L\"; withX: init; somInit: override; };\n\
            };\n";
        let ih = emitted(text, "ih");
        for want in ["static somId somPN_LR[2] = { &somPN_LR_0_s, &somPN_LR_1_s };",
                     "static char somDI_LR_0_t[] = \"R\";", "static somId somDI_LR[2] = { &somDI_LR_0_s, &somDI_LR_1_s };",
                     "    LNewClass( L_MajorVersion, L_MinorVersion );\n    RNewClass( R_MajorVersion, R_MinorVersion );",
                     "        if( cls ) WrpSomSetDirectInitClasses( cls, 2, somDI_LR );",
                     "#define LR_parent_L_somInit( somSelf ) \\\n    SOM_ParentNumResolve( SOMObject, 1,",
                     "#define LR_parent_R_somInit( somSelf ) \\\n    SOM_ParentNumResolve( SOMObject, 2,",
                     "SOM_Scope void SOMLINK withX( LR *somSelf, Environment *ev, somInitCtrl *ctrl, long x );"] {
            assert!(ih.contains(want), "{want:?} missing in\n{ih}");
        }
        assert!(ih.contains(", 0, 0, 0, 0, 0, 1, 2, &somCN_LR_s,"), "two parents in the class information:\n{ih}");
        let xh = emitted(text, "xh");
        for want in ["class LR : public L {",
                     "    long right( Environment *ev, long x ) { return SOM_Resolve( this, R, right )( (R *)this, ev, x ); }",
                     "    LR( Environment *ev, long x ) { if( somIsObj( this ) && (SOMObject *)_somGetClass( this ) == (SOMObject *)(_LR) ) SOM_Resolve( this, LR, withX )( this, ev, 0, x ); }"] {
            assert!(xh.contains(want), "{want:?} missing in\n{xh}");
        }
    }

    /// Stage S4: exceptions (a struct and its ex_ id, scoped by module and
    /// interface), unions (_d and _u, labels as written) and any.
    #[test]
    fn exceptions_unions_and_any() {
        let text = "module Shop {\n\
            \x20 exception OutOf { string item; long left; };\n\
            \x20 exception Closed {};\n\
            \x20 interface Till {\n\
            \x20   exception Jam { long code; };\n\
            \x20   union Pay switch (long) { case 1: case 2: long cents; default: string note; };\n\
            \x20   Pay take(in any what) raises(OutOf, Jam);\n\
            \x20 };\n\
            };\n";
        let d = dump(text);
        for want in ["  exception OutOf\n    string item\n    long left\n", "  exception Closed\n",
                     "    exception Jam\n", "    union Pay switch (long)\n      1, 2: long cents\n      default: string note\n",
                     "operation Pay take(in any what) raises(OutOf, Jam)"] {
            assert!(d.contains(want), "{want:?} missing in\n{d}");
        }
        let h = emitted(text, "h");
        for want in ["#define ex_Shop_OutOf \"Shop::OutOf\"\ntypedef struct Shop_OutOf {\n    string item;\n    long left;\n} Shop_OutOf;",
                     "typedef struct Shop_Closed {\n    long somEmpty_;\n} Shop_Closed;",
                     "#define ex_Shop_Till_Jam \"Shop::Till::Jam\"",
                     "typedef struct Shop_Till_Pay {\n    long _d;\n    union {\n        long cents; /* 1, 2 */\n        string note; /* default */\n    } _u;\n} Shop_Till_Pay;",
                     "typedef Shop_Till_Pay SOMLINK somTP_Shop_Till_take( Shop_Till *somSelf, Environment *ev, any what );"] {
            assert!(h.contains(want), "{want:?} missing in\n{h}");
        }
    }

    /// The C template is written only when its file is not there.
    #[test]
    fn the_template_never_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let idl = dir.path().join("t.idl");
        std::fs::write(&idl, "interface T { void f(); };\n").unwrap();
        std::fs::write(dir.path().join("t.c"), "mine\n").unwrap();
        let args = Args { emitters: Some("h,c".into()), out_dir: Some(dir.path().to_path_buf()), input: idl, ..Args::default() };
        let report = run(&args).expect("runs");
        assert!(report.contains("t.c: kept"), "{report}");
        assert_eq!(std::fs::read_to_string(dir.path().join("t.c")).unwrap(), "mine\n");
        assert!(std::fs::read_to_string(dir.path().join("t.h")).unwrap().contains("TNewClass"));
    }

    /// Open Watcom's SOM samples parse in place (read, never copied), with
    /// the SDK's root classes' IDL, which parse too.
    #[test]
    fn open_watcom_s_sample_idl_parses() {
        let samples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/watcom/samples/os2/som");
        if !samples.is_dir() { return; }
        let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/include");
        let inc = [sdk.clone()];
        for root in ["somobj.idl", "somcls.idl", "somcm.idl"] {
            let text = somidl::dump::dump(&somidl::parse_file(&sdk.join(root), &inc, &[]).expect(root));
            assert!(text.contains("    releaseorder somInit, somUninit, somFree,") || !root.starts_with("somobj"), "{text}");
        }
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
