// SPDX-License-Identifier: GPL-3.0-or-later
//
// The syntax tree of a SOM IDL file (ADR-0122): CORBA 2.0 IDL with SOM's
// `implementation` section, as the SOM Programming Guide describes it.

/// A name, possibly scoped (`Animals::Dog`, `::SOMObject`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedName {
    /// Whether it started with `::` (the global scope).
    pub global: bool,
    pub parts: Vec<String>,
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.global { f.write_str("::")?; }
        f.write_str(&self.parts.join("::"))
    }
}

/// A type as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Void,
    Short,
    Long,
    UShort,
    ULong,
    Float,
    Double,
    Char,
    Boolean,
    Octet,
    Any,
    /// `string`, or `string<bound>`.
    String(Option<u32>),
    /// `sequence<T>`, or `sequence<T, bound>`.
    Sequence(Box<Type>, Option<u32>),
    /// An interface, a typedef, a struct or an enum, by name.
    Named(ScopedName),
}

/// A declarator: a name and its array dimensions (`x[4][2]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declarator {
    pub name: String,
    pub dims: Vec<u32>,
}

/// A struct member, a typedef or an instance variable: one type, one or
/// more declarators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub ty: Type,
    pub names: Vec<Declarator>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstValue {
    Int(i64),
    Str(String),
}

/// A type or constant declared in a module or an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDecl {
    Typedef(Member),
    Struct { name: String, members: Vec<Member> },
    Enum { name: String, items: Vec<String> },
    Const { ty: Type, name: String, value: ConstValue },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub dir: Direction,
    pub ty: Type,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub oneway: bool,
    pub ret: Type,
    pub name: String,
    pub params: Vec<Param>,
    /// `raises (…)`: the exceptions' names (used from stage S4).
    pub raises: Vec<ScopedName>,
    /// `context (…)`: the context strings.
    pub context: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub readonly: bool,
    pub ty: Type,
    pub names: Vec<String>,
}

/// A modifier's value: `= value` in the implementation section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModValue {
    Name(ScopedName),
    Int(i64),
    Str(String),
}

/// One statement of an `implementation` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplItem {
    /// `releaseorder: a, b, c;`
    ReleaseOrder(Vec<String>),
    /// A class modifier: `name = value;` (`metaclass`, `majorversion`,
    /// `functionprefix`, `dllname`, …), or a bare `name;`.
    ClassModifier { name: String, value: Option<ModValue> },
    /// `member: modifier, modifier = value;` — method and attribute
    /// modifiers (`override`, `init`, `noset`, …).
    MemberModifiers { member: String, modifiers: Vec<(String, Option<ModValue>)> },
    /// An instance variable.
    InstanceVar(Member),
    /// `passthru C_h = "line" "line";` (the target's name and the lines).
    Passthru { target: String, lines: Vec<String> },
}

/// What an interface's body holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceItem {
    Type(TypeDecl),
    Attribute(Attribute),
    Operation(Operation),
    Implementation(Vec<ImplItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub parents: Vec<ScopedName>,
    pub items: Vec<InterfaceItem>,
}

/// A definition in a file or a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Definition {
    Module { name: String, defs: Vec<Definition> },
    Interface(Interface),
    /// `interface Name;` — a forward declaration.
    Forward(String),
    Type(TypeDecl),
}
