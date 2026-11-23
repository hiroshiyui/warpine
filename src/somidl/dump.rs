// SPDX-License-Identifier: GPL-3.0-or-later
//
// `somidl --dump`: the syntax tree as indented text, one declaration per
// line, for tests and for checking what the parser understood. The format
// is Warpine's own (ADR-0122).

use super::ast::*;
use std::fmt::Write;

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Void => f.write_str("void"),
            Type::Short => f.write_str("short"),
            Type::Long => f.write_str("long"),
            Type::UShort => f.write_str("unsigned short"),
            Type::ULong => f.write_str("unsigned long"),
            Type::Float => f.write_str("float"),
            Type::Double => f.write_str("double"),
            Type::Char => f.write_str("char"),
            Type::Boolean => f.write_str("boolean"),
            Type::Octet => f.write_str("octet"),
            Type::Any => f.write_str("any"),
            Type::String(None) => f.write_str("string"),
            Type::String(Some(n)) => write!(f, "string<{n}>"),
            Type::Sequence(t, None) => write!(f, "sequence<{t}>"),
            Type::Sequence(t, Some(n)) => write!(f, "sequence<{t}, {n}>"),
            Type::Named(n) => write!(f, "{n}"),
        }
    }
}

fn declarators(names: &[Declarator]) -> String {
    names.iter().map(|d| {
        let dims: String = d.dims.iter().map(|n| format!("[{n}]")).collect();
        format!("{}{dims}", d.name)
    }).collect::<Vec<_>>().join(", ")
}

fn value(v: &ModValue) -> String {
    match v {
        ModValue::Name(n) => n.to_string(),
        ModValue::Int(i) => i.to_string(),
        ModValue::Str(s) => format!("{s:?}"),
    }
}

fn type_decl(out: &mut String, pad: &str, d: &TypeDecl) {
    match d {
        TypeDecl::Typedef(m) => { let _ = writeln!(out, "{pad}typedef {} {}", m.ty, declarators(&m.names)); }
        TypeDecl::Struct { name, members } => {
            let _ = writeln!(out, "{pad}struct {name}");
            for m in members { let _ = writeln!(out, "{pad}  {} {}", m.ty, declarators(&m.names)); }
        }
        TypeDecl::Enum { name, items } => { let _ = writeln!(out, "{pad}enum {name} {{ {} }}", items.join(", ")); }
        TypeDecl::Const { ty, name, value } => {
            let v = match value { ConstValue::Int(i) => i.to_string(), ConstValue::Str(s) => format!("{s:?}") };
            let _ = writeln!(out, "{pad}const {ty} {name} = {v}");
        }
    }
}

fn definition(out: &mut String, depth: usize, d: &Definition) {
    let pad = "  ".repeat(depth);
    match d {
        Definition::Module { name, defs } => {
            let _ = writeln!(out, "{pad}module {name}");
            for d in defs { definition(out, depth + 1, d); }
        }
        Definition::Forward(name) => { let _ = writeln!(out, "{pad}forward {name}"); }
        Definition::Type(t, _) => type_decl(out, &pad, t),
        Definition::Interface(i) => {
            let parents = i.parents.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let _ = writeln!(out, "{pad}interface {}{}{parents}", i.name, if parents.is_empty() { "" } else { " : " });
            for item in &i.items { interface_item(out, &format!("{pad}  "), item); }
        }
    }
}

fn interface_item(out: &mut String, pad: &str, item: &InterfaceItem) {
    match item {
        InterfaceItem::Type(t) => type_decl(out, pad, t),
        InterfaceItem::Attribute(a) => {
            let _ = writeln!(out, "{pad}{}attribute {} {}", if a.readonly { "readonly " } else { "" }, a.ty, a.names.join(", "));
        }
        InterfaceItem::Operation(op) => {
            let params = op.params.iter().map(|p| {
                let dir = match p.dir { Direction::In => "in", Direction::Out => "out", Direction::InOut => "inout" };
                format!("{dir} {} {}", p.ty, p.name)
            }).collect::<Vec<_>>().join(", ");
            let _ = write!(out, "{pad}{}operation {} {}({params})", if op.oneway { "oneway " } else { "" }, op.ret, op.name);
            if !op.raises.is_empty() {
                let _ = write!(out, " raises({})", op.raises.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
            }
            if !op.context.is_empty() {
                let _ = write!(out, " context({})", op.context.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", "));
            }
            out.push('\n');
        }
        InterfaceItem::Implementation(items) => {
            let _ = writeln!(out, "{pad}implementation");
            for it in items {
                match it {
                    ImplItem::ReleaseOrder(n) => { let _ = writeln!(out, "{pad}  releaseorder {}", n.join(", ")); }
                    ImplItem::ClassModifier { name, value: Some(v) } => { let _ = writeln!(out, "{pad}  {name} = {}", value(v)); }
                    ImplItem::ClassModifier { name, value: None } => { let _ = writeln!(out, "{pad}  {name}"); }
                    ImplItem::MemberModifiers { member, modifiers } => {
                        let mods = modifiers.iter().map(|(m, v)| match v {
                            Some(v) => format!("{m} = {}", value(v)),
                            None => m.clone(),
                        }).collect::<Vec<_>>().join(", ");
                        let _ = writeln!(out, "{pad}  {member}: {mods}");
                    }
                    ImplItem::InstanceVar(m) => { let _ = writeln!(out, "{pad}  data {} {}", m.ty, declarators(&m.names)); }
                    ImplItem::Passthru { target, lines } => {
                        let _ = writeln!(out, "{pad}  passthru {target}");
                        for l in lines { let _ = writeln!(out, "{pad}    {l:?}"); }
                    }
                }
            }
        }
    }
}

/// The definitions as indented text.
pub fn dump(defs: &[Definition]) -> String {
    let mut out = String::new();
    for d in defs { definition(&mut out, 0, d); }
    out
}
