// SPDX-License-Identifier: GPL-3.0-or-later
//
// somidl's C and C++ emitters (ADR-0122, stages S1 and S2): the usage
// bindings (`h`; C++ `xh`), the implementation bindings (`ih` / `xih`), an
// implementation template (`c` / `xc`) and a class library's exports
// (`def`), from the interfaces of the main IDL file. In C++ a SOM class
// is a C++ class without virtual functions whose methods call through the
// ClassData tokens (som.xh's SOM_CPP_CLASS: new through somNewNoInit, the
// constructor's somDefaultInit, delete through somDestruct). The names follow the SOM Programming Guide's description of the C
// bindings (`<Class>NewClass`, `<Class>ClassData`, `<Class>CClassData`,
// `_method` macros, `<Class>GetData`, `<Class>MethodDebug`, `SOM_Scope`,
// `somSelf`, `somThis`); the text is Warpine's own.
//
// An interface in a module has the scoped C name `Module_Interface` and
// the SOM class name "Module::Interface". Attributes are `_get_` / `_set_`
// methods over an instance variable of the same name (unless `nodata`),
// implemented by the bindings unless `noget` / `noset`. Sequences are
// `_IDL_SEQUENCE_<element>` structs (`sequence(T)` in som.h).
//
// `<Class>NewClass` builds the class through `somBuildClass` from a
// `somStaticClassInfo` (layout version 1: static methods and overrides),
// after its parent's and explicit metaclass's `NewClass`. Multiple
// inheritance, `init` methods and `any` are refused for now (stages S3
// and S4).

use super::ast::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// The root classes: their bindings are in the SDK's som.h, and their
/// `NewClass` accepts any version.
const ROOT_CLASSES: &[&str] = &["SOMObject", "SOMClass", "SOMClassMgr"];

/// One emitted file: its name and text.
pub struct Output {
    pub name: String,
    pub text: String,
}

/// An interface, as the bindings name it, with its methods (operations
/// and attribute accessors).
struct Known<'a> {
    iface: &'a Interface,
    /// `callstyle = oidl`: no `Environment *` argument.
    oidl: bool,
    /// The C name: the module scope and the name, joined by `_`.
    cname: String,
    /// The SOM class name: the module scope and the name, joined by `::`.
    som_name: String,
    ops: Vec<Operation>,
}

fn collect<'a>(defs: &'a [Definition], scope: &mut Vec<String>, out: &mut Vec<(&'a Interface, Vec<String>)>) {
    for d in defs {
        match d {
            Definition::Module { name, defs } => {
                scope.push(name.clone());
                collect(defs, scope, out);
                scope.pop();
            }
            Definition::Interface(i) => out.push((i, scope.clone())),
            _ => {}
        }
    }
}

fn impl_items(i: &Interface) -> impl Iterator<Item = &ImplItem> {
    i.items.iter().filter_map(|it| if let InterfaceItem::Implementation(v) = it { Some(v) } else { None }).flatten()
}

fn class_modifier<'a>(i: &'a Interface, name: &str) -> Option<&'a ModValue> {
    impl_items(i).find_map(|it| match it {
        ImplItem::ClassModifier { name: n, value: Some(v) } if n == name => Some(v),
        _ => None,
    })
}

fn modifier_text(v: &ModValue) -> String {
    match v {
        ModValue::Name(n) => n.parts.last().cloned().unwrap_or_default(),
        ModValue::Int(i) => i.to_string(),
        ModValue::Str(s) => s.clone(),
    }
}

/// The member modifiers given for `member`.
fn member_modifiers<'a>(i: &'a Interface, member: &str) -> Vec<&'a str> {
    impl_items(i).filter_map(|it| match it {
        ImplItem::MemberModifiers { member: m, modifiers } if m == member => Some(modifiers),
        _ => None,
    }).flatten().map(|(m, _)| m.as_str()).collect()
}

/// The names of the methods `i` overrides.
fn overrides(i: &Interface) -> Vec<&str> {
    impl_items(i).filter_map(|it| match it {
        ImplItem::MemberModifiers { member, modifiers } if modifiers.iter().any(|(m, _)| m == "override") => Some(member.as_str()),
        _ => None,
    }).collect()
}

fn attributes(i: &Interface) -> impl Iterator<Item = (&Attribute, &String)> {
    i.items.iter().filter_map(|it| if let InterfaceItem::Attribute(a) = it { Some(a) } else { None })
        .flat_map(|a| a.names.iter().map(move |n| (a, n)))
}

fn accessor(name: String, ret: Type, params: Vec<Param>) -> Operation {
    Operation { oneway: false, ret, name, params, raises: Vec::new(), context: Vec::new() }
}

/// The methods `i` introduces: its operations, then its attributes'
/// `_get_` and (unless readonly) `_set_` methods.
fn methods_of(i: &Interface) -> Vec<Operation> {
    let mut ops: Vec<Operation> = i.items.iter()
        .filter_map(|it| if let InterfaceItem::Operation(o) = it { Some(o.clone()) } else { None }).collect();
    for (a, n) in attributes(i) {
        ops.push(accessor(format!("_get_{n}"), a.ty.clone(), Vec::new()));
        if !a.readonly {
            ops.push(accessor(format!("_set_{n}"), Type::Void, vec![Param { dir: Direction::In, ty: a.ty.clone(), name: n.clone() }]));
        }
    }
    ops
}

struct Emitter<'a> {
    /// Every interface, by its simple name.
    known: HashMap<&'a str, Known<'a>>,
    /// Interface-level types: simple name to C name (`Class_name`).
    type_names: HashMap<String, String>,
    /// The sequence types used: struct name to element C type.
    sequences: RefCell<BTreeMap<String, String>>,
}

type Res<T> = Result<T, String>;

impl<'a> Emitter<'a> {
    fn new(defs: &'a [Definition]) -> Self {
        let mut all = Vec::new();
        collect(defs, &mut Vec::new(), &mut all);
        let mut type_names = HashMap::new();
        let known = all.iter().map(|(i, scope)| {
            let oidl = class_modifier(i, "callstyle").is_some_and(|v| modifier_text(v) == "oidl");
            let path: Vec<&str> = scope.iter().map(String::as_str).chain([i.name.as_str()]).collect();
            let cname = path.join("_");
            for it in &i.items {
                if let InterfaceItem::Type(t) = it {
                    for n in type_decl_names(t) { type_names.insert(n.to_string(), format!("{cname}_{n}")); }
                }
            }
            (i.name.as_str(), Known { iface: i, oidl, som_name: path.join("::"), cname, ops: methods_of(i) })
        }).collect();
        Emitter { known, type_names, sequences: RefCell::new(BTreeMap::new()) }
    }

    /// The C name of class `name` (a root class keeps its own).
    fn cname(&self, name: &str) -> String {
        self.known.get(name).map_or_else(|| name.to_string(), |k| k.cname.clone())
    }

    fn som_name(&self, name: &str) -> String {
        self.known.get(name).map_or_else(|| name.to_string(), |k| k.som_name.clone())
    }

    /// The C type of `t` as written in a declaration.
    fn c_type(&self, t: &Type) -> Res<String> {
        Ok(match t {
            Type::Void => "void".into(),
            Type::Short => "short".into(),
            Type::Long => "long".into(),
            Type::UShort => "unsigned short".into(),
            Type::ULong => "unsigned long".into(),
            Type::Float => "float".into(),
            Type::Double => "double".into(),
            Type::Char => "char".into(),
            Type::Boolean => "boolean".into(),
            Type::Octet => "octet".into(),
            Type::String(_) => "string".into(),
            Type::Any => return Err("`any` is not supported yet (ADR-0122, stage S4)".into()),
            Type::Sequence(elem, _) => {
                let elem = self.c_type(elem)?;
                let tag: String = elem.trim_end_matches(['*', ' ']).chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                let name = format!("_IDL_SEQUENCE_{tag}");
                self.sequences.borrow_mut().entry(name.clone()).or_insert(elem);
                name
            }
            Type::Named(n) => {
                let last = n.parts.last().cloned().unwrap_or_default();
                if let Some(k) = self.known.get(last.as_str()) { format!("{} *", k.cname) }
                else if ROOT_CLASSES.contains(&last.as_str()) { format!("{last} *") }
                else { self.type_names.get(&last).cloned().unwrap_or(last) }
            }
        })
    }

    /// A parameter: `in` by value, `out` / `inout` through a pointer.
    fn c_param(&self, p: &Param) -> Res<String> {
        let ty = self.c_type(&p.ty)?;
        let star = if p.dir == Direction::In { "" } else { "*" };
        Ok(format!("{}{star}{}", spaced(&ty), p.name))
    }

    fn params(&self, op: &Operation, oidl: bool) -> Res<Vec<String>> {
        let mut v = Vec::new();
        if !oidl { v.push("Environment *ev".to_string()); }
        for p in &op.params { v.push(self.c_param(p)?); }
        Ok(v)
    }

    /// Where method `name` is declared: class `start` or its ancestors.
    fn find_method_from(&self, start: &str, name: &str) -> Option<(&Known<'a>, &Operation)> {
        let k = self.known.get(start)?;
        if let Some(op) = k.ops.iter().find(|o| o.name == name) { return Some((k, op)); }
        self.find_method(k.iface, name)
    }

    /// The ancestors of class `start` (itself included), by simple name.
    fn ancestry(&self, start: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut todo = vec![start.to_string()];
        while let Some(n) = todo.pop() {
            if out.contains(&n) || out.len() > 256 { continue; }
            if let Some(k) = self.known.get(n.as_str()) {
                todo.extend(k.iface.parents.iter().filter_map(|p| p.parts.last().cloned()));
            }
            out.push(n);
        }
        out
    }

    /// Where method `name` is declared: the interface among `cls`'s
    /// ancestors (first parents first) and the method.
    fn find_method(&self, cls: &Interface, name: &str) -> Option<(&Known<'a>, &Operation)> {
        let mut todo: Vec<&str> = cls.parents.iter().rev().filter_map(|p| p.parts.last().map(String::as_str)).collect();
        let mut seen = 0;
        while let Some(n) = todo.pop() {
            seen += 1;
            if seen > 256 { break; }
            let Some(k) = self.known.get(n) else { continue };
            if let Some(op) = k.ops.iter().find(|o| o.name == name) { return Some((k, op)); }
            todo.extend(k.iface.parents.iter().rev().filter_map(|p| p.parts.last().map(String::as_str)));
        }
        None
    }

    /// A typedef, struct, enum or constant; interface-level ones (`prefix`
    /// the class's C name and `_`) are named after the class.
    fn type_decl(&self, out: &mut String, prefix: &str, d: &TypeDecl) -> Res<()> {
        match d {
            TypeDecl::Typedef(m) => {
                for n in &m.names {
                    let _ = writeln!(out, "typedef {}{prefix}{}{};", spaced(&self.c_type(&m.ty)?), n.name, dims(&n.dims));
                }
            }
            TypeDecl::Struct { name, members } => {
                let _ = writeln!(out, "typedef struct {prefix}{name} {{");
                for m in members {
                    for n in &m.names {
                        let _ = writeln!(out, "    {}{}{};", spaced(&self.c_type(&m.ty)?), n.name, dims(&n.dims));
                    }
                }
                let _ = writeln!(out, "}} {prefix}{name};");
            }
            TypeDecl::Enum { name, items } => {
                // CORBA's C mapping: an unsigned long, the values from 1.
                let _ = writeln!(out, "typedef unsigned long {prefix}{name};");
                for (k, it) in items.iter().enumerate() {
                    let _ = writeln!(out, "#define {prefix}{it} {}UL", k + 1);
                }
            }
            TypeDecl::Const { ty, name, value } => {
                let v = match value { ConstValue::Int(i) => format!("(({}){i})", self.c_type(ty)?), ConstValue::Str(s) => format!("{s:?}") };
                let _ = writeln!(out, "#define {prefix}{name} {v}");
            }
        }
        Ok(())
    }
}

fn type_decl_names(t: &TypeDecl) -> Vec<&str> {
    match t {
        TypeDecl::Typedef(m) => m.names.iter().map(|d| d.name.as_str()).collect(),
        TypeDecl::Struct { name, .. } | TypeDecl::Enum { name, .. } => vec![name.as_str()],
        TypeDecl::Const { .. } => Vec::new(),
    }
}

fn spaced(ty: &str) -> String {
    if ty.ends_with('*') { ty.to_string() } else { format!("{ty} ") }
}

fn dims(d: &[u32]) -> String {
    d.iter().map(|n| format!("[{n}]")).collect()
}

/// The include guard / file stem of the main file.
fn guard(stem: &str) -> String {
    stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// An accessor the bindings implement: the method and its attribute.
struct AutoAccessor {
    op: Operation,
    attr: String,
    get: bool,
}

/// A class's view for emitting, checked against what the stage covers.
struct Class<'a> {
    i: &'a Interface,
    /// Its C name, and its parents' and metaclass's (simple) names.
    name: String,
    parent: String,
    parents: Vec<String>,
    /// `directinitclasses`: the ancestors its initializers visit.
    direct_init: Vec<String>,
    /// Its `init` methods (user-defined initializers).
    inits: Vec<String>,
    meta: Option<String>,
    oidl: bool,
    prefix: String,
    major: i64,
    minor: i64,
    /// Introduced methods in release order (then any not in it).
    methods: Vec<Operation>,
    overrides: Vec<&'a str>,
    /// Instance variables, attributes' data included.
    data: Vec<Member>,
    auto: Vec<AutoAccessor>,
}

impl<'a> Emitter<'a> {
    fn class(&self, i: &'a Interface) -> Res<Class<'a>> {
        let ctx = |m: &str| format!("interface {}: {m}", i.name);
        let mut parents: Vec<String> = i.parents.iter().filter_map(|p| p.parts.last().cloned()).collect();
        if parents.is_empty() { parents.push("SOMObject".into()); }
        for parent in &parents {
            if !ROOT_CLASSES.contains(&parent.as_str()) && !self.known.contains_key(parent.as_str()) {
                return Err(ctx(&format!("parent {parent} is not declared (#include its IDL)")));
            }
        }
        let parent = parents[0].clone();
        // directinitclasses: a name, or names in a string ("A, B").
        let direct_init: Vec<String> = match class_modifier(i, "directinitclasses") {
            Some(ModValue::Str(s)) => s.split(',').map(|n| n.trim().rsplit("::").next().unwrap_or("").to_string())
                .filter(|n| !n.is_empty()).collect(),
            Some(v) => vec![modifier_text(v)],
            None => Vec::new(),
        };
        for d in &direct_init {
            if !ROOT_CLASSES.contains(&d.as_str()) && !self.known.contains_key(d.as_str()) {
                return Err(ctx(&format!("directinitclasses names {d}, which is not declared")));
            }
        }
        let meta = class_modifier(i, "metaclass").map(modifier_text);
        if let Some(m) = &meta && !ROOT_CLASSES.contains(&m.as_str()) && !self.known.contains_key(m.as_str()) {
            return Err(ctx(&format!("metaclass {m} is not declared")));
        }
        let known = self.known.get(i.name.as_str()).ok_or_else(|| ctx("not collected"))?;
        let int = |m: &str| class_modifier(i, m).and_then(|v| if let ModValue::Int(n) = v { Some(*n) } else { None }).unwrap_or(0);
        let order: Vec<&str> = impl_items(i).filter_map(|it| if let ImplItem::ReleaseOrder(v) = it { Some(v) } else { None })
            .flatten().map(String::as_str).collect();
        let mut methods = Vec::new();
        for n in &order {
            match known.ops.iter().find(|o| o.name == *n) {
                Some(o) => methods.push(o.clone()),
                None => return Err(ctx(&format!("releaseorder names {n}, which it does not declare"))),
            }
        }
        for o in &known.ops {
            if !methods.iter().any(|m: &Operation| m.name == o.name) { methods.push(o.clone()); }
        }
        // An initializer takes its control block first (SOM Programming Guide).
        let mut inits = Vec::new();
        for o in &methods {
            if !member_modifiers(i, &o.name).contains(&"init") { continue; }
            let ctrl = o.params.first().is_some_and(|p| p.dir == Direction::InOut
                && matches!(&p.ty, Type::Named(n) if n.parts.last().is_some_and(|l| l == "somInitCtrl")));
            if !ctrl || o.ret != Type::Void {
                return Err(ctx(&format!("{}: an init method returns void and takes `inout somInitCtrl ctrl` first", o.name)));
            }
            inits.push(o.name.clone());
        }
        let overrides = overrides(i);
        for o in &overrides {
            if self.find_method(i, o).is_none() {
                return Err(ctx(&format!("{o} overrides no method of its ancestors (#include their IDL)")));
            }
        }
        let mut data: Vec<Member> = impl_items(i).filter_map(|it| if let ImplItem::InstanceVar(m) = it { Some(m.clone()) } else { None }).collect();
        let mut auto = Vec::new();
        for (a, n) in attributes(i) {
            let mods = member_modifiers(i, n);
            if mods.contains(&"nodata") { continue; }
            data.push(Member { ty: a.ty.clone(), names: vec![Declarator { name: n.clone(), dims: Vec::new() }] });
            let find = |m: String| known.ops.iter().find(|o| o.name == m).cloned();
            if !mods.contains(&"noget") && let Some(op) = find(format!("_get_{n}")) {
                auto.push(AutoAccessor { op, attr: n.clone(), get: true });
            }
            if !a.readonly && !mods.contains(&"noset") && let Some(op) = find(format!("_set_{n}")) {
                auto.push(AutoAccessor { op, attr: n.clone(), get: false });
            }
        }
        Ok(Class {
            i, name: known.cname.clone(), parent, parents, direct_init, inits, meta, oidl: known.oidl,
            prefix: class_modifier(i, "functionprefix").map(modifier_text).unwrap_or_default(),
            major: int("majorversion"), minor: int("minorversion"),
            methods, overrides, data, auto,
        })
    }

    /// The `NewClass` call that makes class `name` (parents, metaclass).
    fn new_class_call(&self, name: &str) -> String {
        if ROOT_CLASSES.contains(&name) { return format!("{name}NewClass( 0, 0 )"); }
        let c = self.cname(name);
        format!("{c}NewClass( {c}_MajorVersion, {c}_MinorVersion )")
    }

    fn proto(&self, c: &Class, op: &Operation, oidl: bool) -> Res<String> {
        let params = self.params(op, oidl)?;
        Ok(format!("{}SOMLINK {}{}( {} *somSelf{} )", spaced(&self.c_type(&op.ret)?), c.prefix, op.name, c.name,
                   params.iter().map(|p| format!(", {p}")).collect::<String>()))
    }

    // ─── h ───────────────────────────────────────────────────────────────

    fn usage(&self, c: &Class, out: &mut String, cpp: bool) -> Res<()> {
        let n = &c.name;
        let _ = writeln!(out, "/* ---- {n} ---- */");
        let _ = writeln!(out, "#define {n}_MajorVersion {}\n#define {n}_MinorVersion {}", c.major, c.minor);
        for it in &c.i.items {
            if let InterfaceItem::Type(t) = it { self.type_decl(out, &format!("{n}_"), t)?; }
        }
        for m in &c.methods {
            let params = self.params(m, c.oidl)?;
            let ret = spaced(&self.c_type(&m.ret)?);
            let _ = writeln!(out, "typedef {ret}SOMLINK somTP_{n}_{}( {n} *somSelf{} );", m.name,
                             params.iter().map(|p| format!(", {p}")).collect::<String>());
            let _ = writeln!(out, "typedef somTP_{n}_{0} *somTD_{n}_{0};", m.name);
        }
        let _ = writeln!(out, "SOMEXTERN SOMClass * SOMLINK {n}NewClass( integer4 majorVersion, integer4 minorVersion );");
        let _ = writeln!(out, "SOMEXTERN struct {n}ClassDataStructure {{\n    SOMClass *classObject;");
        for m in &c.methods { let _ = writeln!(out, "    somMToken {};", m.name); }
        let _ = writeln!(out, "}} SOMDLINK {n}ClassData;");
        let _ = writeln!(out, "SOMEXTERN struct {n}CClassDataStructure {{\n    somMethodTabs parentMtab;\n    somDToken instanceDataToken;\n}} SOMDLINK {n}CClassData;");
        let _ = writeln!(out, "#define _{n} ({n}ClassData.classObject ? {n}ClassData.classObject : {})", self.new_class_call(&c.i.name));
        if cpp {
            // A C++ class: no virtual functions, its methods inline calls
            // through the ClassData tokens; som.xh's SOM_CPP_CLASS gives
            // new, delete and the constructor.
            let _ = writeln!(out, "class {n} : public {} {{\npublic:\n    SOM_CPP_CLASS( {n} )", self.cname(&c.parent));
            for m in &c.methods {
                let params = self.params(m, c.oidl)?;
                let mut args: Vec<String> = vec!["this".into()];
                if !c.oidl { args.push("ev".into()); }
                args.extend(m.params.iter().map(|p| p.name.clone()));
                let ret = self.c_type(&m.ret)?;
                let ret_kw = if m.ret == Type::Void { "" } else { "return " };
                let _ = writeln!(out, "    {}{}( {} ) {{ {ret_kw}SOM_Resolve( this, {n}, {} )( {} ); }}", spaced(&ret), m.name,
                                 params.join(", "), m.name, args.join(", "));
            }
            // Later parents' methods (the C++ class derives from the first
            // parent only: one method-table word), as forwarding members.
            let first = self.ancestry(&c.parent);
            let mut done: Vec<String> = Vec::new();
            for later in c.parents.iter().skip(1) {
                for anc in self.ancestry(later) {
                    if first.contains(&anc) || done.contains(&anc) || ROOT_CLASSES.contains(&anc.as_str()) { continue; }
                    let Some(k) = self.known.get(anc.as_str()) else { continue };
                    for m in &k.ops {
                        let params = self.params(m, k.oidl)?;
                        let mut args: Vec<String> = vec![format!("({} *)this", k.cname)];
                        if !k.oidl { args.push("ev".into()); }
                        args.extend(m.params.iter().map(|p| p.name.clone()));
                        let ret_kw = if m.ret == Type::Void { "" } else { "return " };
                        let _ = writeln!(out, "    {}{}( {} ) {{ {ret_kw}SOM_Resolve( this, {}, {} )( {} ); }}", spaced(&self.c_type(&m.ret)?),
                                         m.name, params.join(", "), k.cname, m.name, args.join(", "));
                    }
                    done.push(anc);
                }
            }
            // An init method is also a constructor taking its arguments
            // (the control block 0); a SOM object of another class (a C++
            // base) or a non-SOM object is left alone, as SOM_CPP_CLASS's.
            for name in &c.inits {
                let Some(m) = c.methods.iter().find(|m| &m.name == name) else { continue };
                let mut params = Vec::new();
                if !c.oidl { params.push("Environment *ev".to_string()); }
                for p in m.params.iter().skip(1) { params.push(self.c_param(p)?); }
                if params.is_empty() { continue; }
                let mut args = vec!["this".to_string()];
                if !c.oidl { args.push("ev".into()); }
                args.push("0".into());
                args.extend(m.params.iter().skip(1).map(|p| p.name.clone()));
                let _ = writeln!(out, "    {n}( {} ) {{ if( somIsObj( this ) && (SOMObject *)_somGetClass( this ) == (SOMObject *)(_{n}) ) SOM_Resolve( this, {n}, {} )( {} ); }}",
                                 params.join(", "), m.name, args.join(", "));
            }
            let _ = writeln!(out, "}};\n");
            return Ok(());
        }
        let _ = writeln!(out, "#define {n}New() (({n} *)_somNew( _{n} ))");
        for m in &c.methods {
            let mut args: Vec<String> = Vec::new();
            if !c.oidl { args.push("ev".into()); }
            args.extend(m.params.iter().map(|p| p.name.clone()));
            let formal: String = args.iter().map(|a| format!(", {a}")).collect();
            let actual: String = args.iter().map(|a| format!(", ({a})")).collect();
            let _ = writeln!(out, "#define {n}_{0}( somSelf{formal} ) \\\n    SOM_Resolve( somSelf, {n}, {0} )( ({n} *)(somSelf){actual} )", m.name);
            let _ = writeln!(out, "#ifndef SOMGD_{0}\n#define _{0} {n}_{0}\n#endif", m.name);
        }
        out.push('\n');
        Ok(())
    }

    // ─── ih ──────────────────────────────────────────────────────────────

    fn implementation(&self, c: &Class, out: &mut String) -> Res<()> {
        let n = &c.name;
        let _ = writeln!(out, "/* ---- {n} ---- */");
        let _ = writeln!(out, "typedef struct {n}Data_struct {{");
        if c.data.is_empty() { let _ = writeln!(out, "    char somEmpty_;"); }
        for m in &c.data {
            for d in &m.names {
                let _ = writeln!(out, "    {}{}{};", spaced(&self.c_type(&m.ty)?), d.name, dims(&d.dims));
            }
        }
        let _ = writeln!(out, "}} {n}Data;");
        let size = if c.data.is_empty() { "0".to_string() } else { format!("sizeof( {n}Data )") };
        if c.data.is_empty() {
            // No instance data: nothing to resolve (a null token is an error).
            let _ = writeln!(out, "#define {n}GetData( somSelf ) ((void)(somSelf), ({n}Data *)0)");
        } else {
            let _ = writeln!(out, "#define {n}GetData( somSelf ) \\\n    (({n}Data *)somDataResolve( (SOMObject *)(somSelf), {n}CClassData.instanceDataToken ))");
        }
        for m in &c.data {
            for d in &m.names { let _ = writeln!(out, "#define _{0} (somThis->{0})", d.name); }
        }
        let _ = writeln!(out, "#ifndef {n}MethodDebug\n#define {n}MethodDebug( c, m ) SOMMethodDebug( c, m )\n#endif");
        // The implementations' prototypes.
        for m in &c.methods {
            let _ = writeln!(out, "SOM_Scope {};", self.proto(c, m, c.oidl)?);
        }
        for o in &c.overrides {
            let (k, op) = self.find_method(c.i, o).ok_or_else(|| format!("{o}: not found"))?;
            let _ = writeln!(out, "SOM_Scope {};", self.proto(c, op, k.oidl)?);
            // The parent's implementation.
            let mut args: Vec<String> = Vec::new();
            if !k.oidl { args.push("ev".into()); }
            args.extend(op.params.iter().map(|q| q.name.clone()));
            let formal: String = args.iter().map(|a| format!(", {a}")).collect();
            let actual: String = args.iter().map(|a| format!(", ({a})")).collect();
            let intro = &k.cname;
            // Each parent that has the method (parents from 1), and the
            // first of them as `<Class>_parent_<method>`.
            let mut first = true;
            for (num, parent) in c.parents.iter().enumerate() {
                if self.find_method_from(parent, o).is_none() && !ROOT_CLASSES.contains(&parent.as_str()) { continue; }
                let pc = self.cname(parent);
                let mut names = vec![format!("{n}_parent_{pc}_{o}")];
                if first { names.push(format!("{n}_parent_{o}")); first = false; }
                for macro_name in names {
                    let _ = writeln!(out, "#define {macro_name}( somSelf{formal} ) \\\n    SOM_ParentNumResolve( {intro}, {}, {n}CClassData.parentMtab, {o} )( ({intro} *)(somSelf){actual} )", num + 1);
                }
            }
        }
        // The accessors the bindings implement.
        for a in &c.auto {
            let _ = writeln!(out, "SOM_Scope {}\n{{\n    {n}Data *somThis = {n}GetData( somSelf );", self.proto(c, &a.op, c.oidl)?);
            if !c.oidl { out.push_str("    (void)ev;\n"); }
            if a.get { let _ = writeln!(out, "    return( somThis->{} );\n}}", a.attr); }
            else { let _ = writeln!(out, "    somThis->{0} = {0};\n}}", a.attr); }
        }
        // ClassData and the class's static information.
        let _ = writeln!(out, "struct {n}ClassDataStructure SOMDLINK {n}ClassData;");
        let _ = writeln!(out, "struct {n}CClassDataStructure SOMDLINK {n}CClassData;");
        let _ = writeln!(out, "static char somCN_{n}_t[] = \"{}\";\nstatic char *somCN_{n}_s = somCN_{n}_t;", self.som_name(&c.i.name));
        let ids = |out: &mut String, tag: &str, names: &[String]| {
            for (k, p) in names.iter().enumerate() {
                let _ = writeln!(out, "static char {tag}_{n}_{k}_t[] = \"{}\";\nstatic char *{tag}_{n}_{k}_s = {tag}_{n}_{k}_t;", self.som_name(p));
            }
            let list: Vec<String> = (0..names.len()).map(|k| format!("&{tag}_{n}_{k}_s")).collect();
            let _ = writeln!(out, "static somId {tag}_{n}[{}] = {{ {} }};", names.len().max(1),
                             if list.is_empty() { "0".to_string() } else { list.join(", ") });
        };
        ids(out, "somPN", &c.parents);
        if !c.direct_init.is_empty() { ids(out, "somDI", &c.direct_init); }
        if let Some(meta) = &c.meta {
            let _ = writeln!(out, "static char somMC_{n}_t[] = \"{}\";\nstatic char *somMC_{n}_s = somMC_{n}_t;", self.som_name(meta));
        }
        for m in &c.methods {
            let _ = writeln!(out, "static char somMN_{n}_{0}_t[] = \"{0}\";\nstatic char *somMN_{n}_{0}_s = somMN_{n}_{0}_t;\nstatic somId somMN_{n}_{0} = &somMN_{n}_{0}_s;", m.name);
        }
        for o in &c.overrides {
            let _ = writeln!(out, "static char somON_{n}_{o}_t[] = \"{o}\";\nstatic char *somON_{n}_{o}_s = somON_{n}_{o}_t;\nstatic somId somON_{n}_{o} = &somON_{n}_{o}_s;");
        }
        let _ = writeln!(out, "static somStaticMethod_t somSM_{n}[] = {{");
        for m in &c.methods {
            let _ = writeln!(out, "    {{ &{n}ClassData.{0}, &somMN_{n}_{0}, 0, (somMethodProc *){1}{0}, 0, 0 }},", m.name, c.prefix);
        }
        let _ = writeln!(out, "    {{ 0, 0, 0, 0, 0, 0 }}\n}};");
        let _ = writeln!(out, "static somOverrideMethod_t somOM_{n}[] = {{");
        for o in &c.overrides {
            let _ = writeln!(out, "    {{ &somON_{n}_{o}, (somMethodProc *){}{o} }},", c.prefix);
        }
        let _ = writeln!(out, "    {{ 0, 0 }}\n}};");
        let meta = if c.meta.is_some() { format!("&somMC_{n}_s") } else { "0".into() };
        let _ = writeln!(out, "static somStaticClassInfo somSCI_{n} = {{\n    1, {}, {}, 0, 0, 0, {}, {}, {size}, {}, {}, &somCN_{n}_s, {meta}, 0,\n    somPN_{n}, &{n}ClassData, &{n}CClassData,\n    somSM_{n}, somOM_{n}, 0, 0, 0, 0, 0\n}};",
            c.methods.len(), c.overrides.len(), c.major, c.minor, c.methods.len(), c.parents.len());
        let _ = writeln!(out, "SOMClass * SOMLINK {n}NewClass( integer4 majorVersion, integer4 minorVersion )\n{{");
        let _ = writeln!(out, "    if( {n}ClassData.classObject ) return( {n}ClassData.classObject );\n    somEnvironmentNew();");
        for parent in &c.parents { let _ = writeln!(out, "    {};", self.new_class_call(parent)); }
        if let Some(meta) = &c.meta { let _ = writeln!(out, "    {};", self.new_class_call(meta)); }
        if c.direct_init.is_empty() {
            let _ = writeln!(out, "    return( somBuildClass( 1, &somSCI_{n}, majorVersion, minorVersion ) );\n}}\n");
        } else {
            // directinitclasses: the ancestors its initializers visit.
            let _ = writeln!(out, "    {{\n        SOMClass *cls = somBuildClass( 1, &somSCI_{n}, majorVersion, minorVersion );\n        if( cls ) WrpSomSetDirectInitClasses( cls, {}, somDI_{n} );\n        return( cls );\n    }}\n}}\n",
                             c.direct_init.len());
        }
        Ok(())
    }

    // ─── c (template) ────────────────────────────────────────────────────

    fn template(&self, c: &Class, out: &mut String) -> Res<()> {
        let n = &c.name;
        let body = |out: &mut String, name: &str, ret: &Type, parent_call: Option<String>| -> Res<()> {
            let _ = writeln!(out, "{{\n    {n}Data *somThis = {n}GetData( somSelf );\n    (void)somThis;\n    {n}MethodDebug( \"{n}\", \"{name}\" );");
            if let Some(call) = parent_call {
                let ret_kw = if *ret == Type::Void { "" } else { "return( " };
                let close = if *ret == Type::Void { "" } else { " )" };
                let _ = writeln!(out, "    {ret_kw}{call}{close};");
            } else if *ret != Type::Void {
                let _ = writeln!(out, "    return( ({})0 );", self.c_type(ret)?);
            }
            out.push_str("}\n\n");
            Ok(())
        };
        for m in &c.methods {
            if c.auto.iter().any(|a| a.op.name == m.name) { continue; }
            let _ = writeln!(out, "SOM_Scope {}", self.proto(c, m, c.oidl)?);
            body(out, &m.name, &m.ret, None)?;
        }
        for o in &c.overrides {
            let (k, op) = self.find_method(c.i, o).ok_or_else(|| format!("{o}: not found"))?;
            let _ = writeln!(out, "SOM_Scope {}", self.proto(c, op, k.oidl)?);
            let mut args = vec!["somSelf".to_string()];
            if !k.oidl { args.push("ev".into()); }
            args.extend(op.params.iter().map(|q| q.name.clone()));
            body(out, o, &op.ret, Some(format!("{n}_parent_{o}( {} )", args.join(", "))))?;
        }
        Ok(())
    }
}

/// The emitters `kinds` (`h`, `ih`, `c`, `def`) for the interfaces of the
/// main file of `defs`, whose stem is `stem`.
pub fn emit(defs: &[Definition], stem: &str, kinds: &[&str]) -> Result<Vec<Output>, String> {
    let e = Emitter::new(defs);
    let mut all = Vec::new();
    collect(defs, &mut Vec::new(), &mut all);
    let main: Vec<&Interface> = all.into_iter().map(|(i, _)| i).filter(|i| i.file == 0).collect();
    let classes = main.iter().map(|i| e.class(i)).collect::<Res<Vec<_>>>()?;
    let g = guard(stem);
    let head = |kind: &str| format!("/* {stem}.{kind}: generated by somidl from {stem}.idl (ADR-0122); do not edit. */\n");
    let mut outputs = Vec::new();
    for &kind in kinds {
        let mut out = head(kind);
        match kind {
            "h" | "xh" => {
                let cpp = kind == "xh";
                let (base, objects) = if cpp { ("som.xh", "class") } else { ("som.h", "typedef") };
                let _ = writeln!(out, "#ifndef SOM_{g}_{kind}\n#define SOM_{g}_{kind}\n\n#include <{base}>\n#include <somapi.h>\n");
                // Every class's object type first: they name one another.
                for c in &classes {
                    if objects == "class" { let _ = writeln!(out, "class {};", c.name); }
                    else { let _ = writeln!(out, "#ifndef {0}_defined\n#define {0}_defined\ntypedef SOMObject {0};\n#endif", c.name); }
                }
                let mut body = String::new();
                for d in defs {
                    if let Definition::Type(t, 0) = d { e.type_decl(&mut body, "", t)?; }
                }
                for c in &classes { e.usage(c, &mut body, cpp)?; }
                // The sequence types the declarations use, before them.
                for (name, elem) in e.sequences.borrow().iter() {
                    let _ = writeln!(out, "#ifndef {name}_defined\n#define {name}_defined\ntypedef struct {{\n    unsigned long _maximum;\n    unsigned long _length;\n    {}*_buffer;\n}} {name};\n#endif",
                                     spaced(elem));
                }
                out.push('\n');
                out.push_str(&body);
                out.push_str("#endif\n");
            }
            "ih" | "xih" => {
                let usage = if kind == "xih" { "xh" } else { "h" };
                let _ = writeln!(out, "#ifndef SOM_{g}_{kind}\n#define SOM_{g}_{kind}\n\n#include \"{stem}.{usage}\"\n");
                for c in &classes { e.implementation(c, &mut out)?; }
                out.push_str("#endif\n");
            }
            "c" | "xc" => {
                let imp = if kind == "xc" { "xih" } else { "ih" };
                let _ = writeln!(out, "#include \"{stem}.{imp}\"\n");
                for c in &classes { e.template(c, &mut out)?; }
            }
            "def" => {
                out = String::new();
                for c in &classes {
                    let _ = writeln!(out, "{0}NewClass\n{0}ClassData\n{0}CClassData", c.name);
                }
            }
            other => return Err(format!("unknown emitter {other} (h, ih, c, xh, xih, xc, def)")),
        }
        outputs.push(Output { name: format!("{stem}.{kind}"), text: out });
    }
    Ok(outputs)
}
