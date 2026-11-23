// SPDX-License-Identifier: GPL-3.0-or-later
//
// The SOM IDL parser (ADR-0122): preprocessed tokens to the syntax tree:
// modules, interfaces and forward declarations, attributes, operations,
// typedefs, structs, enums, constants, exceptions, unions, the basic
// types, `any`, strings and sequences, and the implementation section.

use super::ast::*;
use crate::cpp::{err, Error, Eval, Kind, Loc, Result, Tok};

/// How deeply modules, structs and sequences may nest.
pub const MAX_NESTING: usize = 32;

pub struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    depth: usize,
    /// The second half of a `>>` that closed two sequences at once.
    pending_gt: bool,
    /// Integer constants declared so far, by name (scoped names by their
    /// last part), for later constant expressions.
    consts: std::collections::HashMap<String, i64>,
}

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Tok]) -> Self {
        Parser { toks, pos: 0, depth: 0, pending_gt: false, consts: Default::default() }
    }

    /// The whole file.
    pub fn specification(&mut self) -> Result<Vec<Definition>> {
        let mut defs = Vec::new();
        while self.peek().is_some() {
            defs.push(self.definition()?);
        }
        Ok(defs)
    }

    // ─── Tokens ──────────────────────────────────────────────────────────────

    fn peek(&self) -> Option<&'a Tok> { self.toks.get(self.pos) }
    fn peek_at(&self, n: usize) -> Option<&'a Tok> { self.toks.get(self.pos + n) }

    fn loc(&self) -> Loc {
        self.peek().or(self.toks.last()).map_or(Loc::default(), |t| t.loc)
    }

    fn at_punct(&self, p: &str) -> bool { self.peek().is_some_and(|t| t.is_punct(p)) }
    fn at_word(&self, w: &str) -> bool { self.peek().and_then(Tok::ident) == Some(w) }

    fn eat_punct(&mut self, p: &str) -> bool {
        let found = self.at_punct(p);
        if found { self.pos += 1; }
        found
    }

    fn eat_word(&mut self, w: &str) -> bool {
        let found = self.at_word(w);
        if found { self.pos += 1; }
        found
    }

    fn expect_punct(&mut self, p: &str) -> Result<()> {
        if self.eat_punct(p) { return Ok(()); }
        err(self.loc(), format!("expected {p}, found {}", self.describe()))
    }

    fn ident(&mut self) -> Result<String> {
        match self.peek().and_then(Tok::ident) {
            Some(s) => { self.pos += 1; Ok(s.to_string()) }
            None => err(self.loc(), format!("expected a name, found {}", self.describe())),
        }
    }

    fn describe(&self) -> String {
        self.peek().map_or_else(|| "the end of the file".to_string(), |t| format!("{:?}", t.text))
    }

    /// `::` is two `:` tokens with nothing between them.
    fn at_scope(&self) -> bool {
        self.at_punct(":") && self.peek_at(1).is_some_and(|t| t.is_punct(":") && !t.space_before)
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_NESTING { return err(self.loc(), "nested too deeply"); }
        Ok(())
    }

    fn leave(&mut self) { self.depth -= 1; }

    // ─── Definitions ─────────────────────────────────────────────────────────

    fn definition(&mut self) -> Result<Definition> {
        if self.eat_word("module") {
            let name = self.ident()?;
            self.expect_punct("{")?;
            self.enter()?;
            let mut defs = Vec::new();
            while !self.eat_punct("}") {
                if self.peek().is_none() { return err(self.loc(), format!("module {name} is not closed")); }
                defs.push(self.definition()?);
            }
            self.leave();
            self.expect_punct(";")?;
            return Ok(Definition::Module { name, defs });
        }
        if self.eat_word("interface") {
            return self.interface();
        }
        let file = self.loc().file;
        let decl = self.type_decl()?;
        self.expect_punct(";")?;
        Ok(Definition::Type(decl, file))
    }

    fn interface(&mut self) -> Result<Definition> {
        let file = self.loc().file;
        let name = self.ident()?;
        if self.eat_punct(";") { return Ok(Definition::Forward(name)); }
        let mut parents = Vec::new();
        if self.eat_punct(":") {
            loop {
                parents.push(self.scoped_name()?);
                if !self.eat_punct(",") { break; }
            }
        }
        self.expect_punct("{")?;
        let mut items = Vec::new();
        while !self.eat_punct("}") {
            if self.peek().is_none() { return err(self.loc(), format!("interface {name} is not closed")); }
            items.push(self.interface_item()?);
        }
        self.expect_punct(";")?;
        Ok(Definition::Interface(Interface { name, file, parents, items }))
    }

    fn interface_item(&mut self) -> Result<InterfaceItem> {
        let item = if self.eat_word("implementation") {
            InterfaceItem::Implementation(self.implementation()?)
        } else if self.at_word("attribute") || self.at_word("readonly") {
            InterfaceItem::Attribute(self.attribute()?)
        } else if ["typedef", "struct", "enum", "const", "union", "exception"].iter().any(|w| self.at_word(w)) {
            InterfaceItem::Type(self.type_decl()?)
        } else {
            InterfaceItem::Operation(self.operation()?)
        };
        self.expect_punct(";")?;
        Ok(item)
    }

    fn attribute(&mut self) -> Result<Attribute> {
        let readonly = self.eat_word("readonly");
        if !self.eat_word("attribute") { return err(self.loc(), "expected attribute after readonly"); }
        let ty = self.type_spec()?;
        let mut names = vec![self.ident()?];
        while self.eat_punct(",") { names.push(self.ident()?); }
        Ok(Attribute { readonly, ty, names })
    }

    fn operation(&mut self) -> Result<Operation> {
        let oneway = self.eat_word("oneway");
        let ret = self.type_spec()?;
        let name = self.ident()?;
        self.expect_punct("(")?;
        let mut params = Vec::new();
        if !self.eat_punct(")") {
            loop {
                let dir = if self.eat_word("in") { Direction::In }
                    else if self.eat_word("out") { Direction::Out }
                    else if self.eat_word("inout") { Direction::InOut }
                    else { return err(self.loc(), format!("expected in, out or inout in {name}")); };
                let ty = self.type_spec()?;
                params.push(Param { dir, ty, name: self.ident()? });
                if self.eat_punct(")") { break; }
                self.expect_punct(",")?;
            }
        }
        let mut raises = Vec::new();
        if self.eat_word("raises") {
            self.expect_punct("(")?;
            loop {
                raises.push(self.scoped_name()?);
                if self.eat_punct(")") { break; }
                self.expect_punct(",")?;
            }
        }
        let mut context = Vec::new();
        if self.eat_word("context") {
            self.expect_punct("(")?;
            loop {
                context.push(self.string()?);
                if self.eat_punct(")") { break; }
                self.expect_punct(",")?;
            }
        }
        Ok(Operation { oneway, ret, name, params, raises, context })
    }

    // ─── Types ───────────────────────────────────────────────────────────────

    fn type_decl(&mut self) -> Result<TypeDecl> {
        let loc = self.loc();
        if self.eat_word("typedef") {
            return Ok(TypeDecl::Typedef(self.member()?));
        }
        if self.eat_word("struct") {
            let name = self.ident()?;
            self.expect_punct("{")?;
            self.enter()?;
            let mut members = Vec::new();
            while !self.eat_punct("}") {
                if self.peek().is_none() { return err(self.loc(), format!("struct {name} is not closed")); }
                members.push(self.member()?);
                self.expect_punct(";")?;
            }
            self.leave();
            return Ok(TypeDecl::Struct { name, members });
        }
        if self.eat_word("enum") {
            let name = self.ident()?;
            self.expect_punct("{")?;
            let mut items = vec![self.ident()?];
            while self.eat_punct(",") { items.push(self.ident()?); }
            self.expect_punct("}")?;
            return Ok(TypeDecl::Enum { name, items });
        }
        if self.eat_word("const") {
            let ty = self.type_spec()?;
            let name = self.ident()?;
            self.expect_punct("=")?;
            let value = if let Some(Kind::Str(_)) = self.peek().map(|t| &t.kind) {
                ConstValue::Str(self.string()?)
            } else {
                let v = self.int_expr(false)?;
                self.consts.insert(name.clone(), v);
                ConstValue::Int(v)
            };
            return Ok(TypeDecl::Const { ty, name, value });
        }
        if self.eat_word("exception") {
            let name = self.ident()?;
            self.expect_punct("{")?;
            self.enter()?;
            let mut members = Vec::new();
            while !self.eat_punct("}") {
                if self.peek().is_none() { return err(self.loc(), format!("exception {name} is not closed")); }
                members.push(self.member()?);
                self.expect_punct(";")?;
            }
            self.leave();
            return Ok(TypeDecl::Exception { name, members });
        }
        if self.eat_word("union") {
            let name = self.ident()?;
            if !self.eat_word("switch") { return err(self.loc(), format!("union {name}: expected switch")); }
            self.expect_punct("(")?;
            let disc = self.type_spec()?;
            self.expect_punct(")")?;
            self.expect_punct("{")?;
            self.enter()?;
            let mut cases = Vec::new();
            while !self.eat_punct("}") {
                if self.peek().is_none() { return err(self.loc(), format!("union {name} is not closed")); }
                let mut labels = Vec::new();
                loop {
                    if self.eat_word("default") {
                        labels.push("default".to_string());
                    } else if self.eat_word("case") {
                        // The label as written, up to its colon.
                        let mut text = String::new();
                        while let Some(t) = self.peek() {
                            if t.is_punct(":") && !self.at_scope() { break; }
                            if !text.is_empty() && t.space_before { text.push(' '); }
                            text.push_str(&t.text);
                            self.pos += 1;
                        }
                        if text.is_empty() { return err(self.loc(), "a case needs a label"); }
                        labels.push(text);
                    } else {
                        break;
                    }
                    self.expect_punct(":")?;
                }
                if labels.is_empty() { return err(self.loc(), format!("union {name}: expected case or default")); }
                let member = self.member()?;
                self.expect_punct(";")?;
                cases.push(UnionCase { labels, member });
            }
            self.leave();
            return Ok(TypeDecl::Union { name, disc, cases });
        }
        err(loc, format!("expected a definition, found {}", self.describe()))
    }

    /// A type and its declarators (`long a, b[4]`).
    fn member(&mut self) -> Result<Member> {
        let ty = self.type_spec()?;
        let mut names = vec![self.declarator()?];
        while self.eat_punct(",") { names.push(self.declarator()?); }
        Ok(Member { ty, names })
    }

    fn declarator(&mut self) -> Result<Declarator> {
        let name = self.ident()?;
        let mut dims = Vec::new();
        while self.eat_punct("[") {
            dims.push(self.bound(false)?);
            self.expect_punct("]")?;
        }
        Ok(Declarator { name, dims })
    }

    pub fn type_spec(&mut self) -> Result<Type> {
        let Some(word) = self.peek().and_then(Tok::ident) else {
            if self.at_punct(":") { return Ok(Type::Named(self.scoped_name()?)); }
            return err(self.loc(), format!("expected a type, found {}", self.describe()));
        };
        let simple = match word {
            "void" => Some(Type::Void),
            "short" => Some(Type::Short),
            "long" => Some(Type::Long),
            "float" => Some(Type::Float),
            "double" => Some(Type::Double),
            "char" => Some(Type::Char),
            "boolean" => Some(Type::Boolean),
            "octet" => Some(Type::Octet),
            "any" => Some(Type::Any),
            _ => None,
        };
        if let Some(t) = simple {
            self.pos += 1;
            return Ok(t);
        }
        match word {
            "unsigned" => {
                self.pos += 1;
                if self.eat_word("short") { Ok(Type::UShort) }
                else if self.eat_word("long") { Ok(Type::ULong) }
                else { err(self.loc(), "expected short or long after unsigned") }
            }
            "string" => {
                self.pos += 1;
                if !self.eat_punct("<") { return Ok(Type::String(None)); }
                let bound = self.bound(true)?;
                self.expect_gt()?;
                Ok(Type::String(Some(bound)))
            }
            "sequence" => {
                self.pos += 1;
                self.expect_punct("<")?;
                self.enter()?;
                let inner = self.type_spec()?;
                self.leave();
                let bound = if self.eat_punct(",") { Some(self.bound(true)?) } else { None };
                self.expect_gt()?;
                Ok(Type::Sequence(Box::new(inner), bound))
            }
            _ => Ok(Type::Named(self.scoped_name()?)),
        }
    }

    /// `>`, or the first half of `>>` (the second closes the next).
    fn expect_gt(&mut self) -> Result<()> {
        if self.pending_gt {
            self.pending_gt = false;
            self.pos += 1;
            return Ok(());
        }
        if self.eat_punct(">") { return Ok(()); }
        if self.at_punct(">>") {
            self.pending_gt = true;
            return Ok(());
        }
        err(self.loc(), format!("expected >, found {}", self.describe()))
    }

    pub fn scoped_name(&mut self) -> Result<ScopedName> {
        let global = self.at_scope();
        if global { self.pos += 2; }
        let mut parts = vec![self.ident()?];
        while self.at_scope() {
            self.pos += 2;
            parts.push(self.ident()?);
        }
        Ok(ScopedName { global, parts })
    }

    // ─── Values ──────────────────────────────────────────────────────────────

    /// A C integer expression up to `;`, `,` or `]`; inside angle brackets
    /// (`in_angle`) also up to `>` or `>>`, which closes two of them.
    fn int_expr(&mut self, in_angle: bool) -> Result<i64> {
        let start = self.pos;
        let mut nest = 0usize;
        while let Some(t) = self.peek() {
            if nest == 0 && (t.is_punct(";") || t.is_punct(",") || t.is_punct("]")
                             || (in_angle && (t.is_punct(">") || t.is_punct(">>")))) { break; }
            if t.is_punct("(") { nest += 1; }
            if t.is_punct(")") {
                if nest == 0 { break; }
                nest -= 1;
            }
            self.pos += 1;
        }
        if start == self.pos { return err(self.loc(), "expected a value"); }
        // Constants declared earlier become their values (`::` dropped,
        // a scoped name by its last part).
        let toks = &self.toks[start..self.pos];
        let mut slice: Vec<Tok> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            // `::` (two adjacent colons): drop it and the scope before it.
            if t.is_punct(":") && toks.get(i + 1).is_some_and(|n| n.is_punct(":") && !n.space_before) {
                if slice.last().is_some_and(|p| p.ident().is_some()) { slice.pop(); }
                i += 2;
                continue;
            }
            match t.ident().and_then(|n| self.consts.get(n)) {
                Some(&v) => slice.push(Tok { kind: Kind::Num(v, false), text: v.to_string(), ..t.clone() }),
                None => slice.push(t.clone()),
            }
            i += 1;
        }
        let mut ev = Eval { toks: &slice, pos: 0, unknown_is_zero: false };
        let v = ev.ternary()?;
        if ev.pos != slice.len() { return err(slice[ev.pos].loc, "bad constant expression"); }
        Ok(v.0)
    }

    /// An array, string or sequence bound: a positive constant.
    fn bound(&mut self, in_angle: bool) -> Result<u32> {
        let loc = self.loc();
        match u32::try_from(self.int_expr(in_angle)?) {
            Ok(n) if n > 0 => Ok(n),
            _ => err(loc, "a bound must be a positive number"),
        }
    }

    /// Adjacent string literals, joined.
    fn string(&mut self) -> Result<String> {
        let mut out = Vec::new();
        let mut any = false;
        while let Some(Kind::Str(s)) = self.peek().map(|t| &t.kind) {
            out.extend_from_slice(s);
            self.pos += 1;
            any = true;
        }
        if !any { return err(self.loc(), format!("expected a string, found {}", self.describe())); }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    // ─── The implementation section ─────────────────────────────────────────

    fn implementation(&mut self) -> Result<Vec<ImplItem>> {
        self.expect_punct("{")?;
        let mut items = Vec::new();
        while !self.eat_punct("}") {
            if self.peek().is_none() { return err(self.loc(), "implementation is not closed"); }
            items.push(self.impl_item()?);
            self.expect_punct(";")?;
        }
        Ok(items)
    }

    fn impl_item(&mut self) -> Result<ImplItem> {
        if self.at_word("releaseorder") && self.peek_at(1).is_some_and(|t| t.is_punct(":")) {
            self.pos += 2;
            let mut names = vec![self.ident()?];
            while self.eat_punct(",") { names.push(self.ident()?); }
            return Ok(ImplItem::ReleaseOrder(names));
        }
        if self.eat_word("passthru") {
            let target = self.ident()?;
            self.expect_punct("=")?;
            let mut lines = Vec::new();
            while let Some(Kind::Str(s)) = self.peek().map(|t| &t.kind) {
                lines.push(String::from_utf8_lossy(s).into_owned());
                self.pos += 1;
            }
            if lines.is_empty() { return err(self.loc(), "passthru needs its lines as strings"); }
            return Ok(ImplItem::Passthru { target, lines });
        }
        // `name = value`, a bare `name`, `member: modifiers`, or an
        // instance variable (a type, then declarators).
        let next = self.peek_at(1);
        let is_name = self.peek().and_then(Tok::ident).is_some();
        if is_name && next.is_some_and(|t| t.is_punct("=")) {
            let name = self.ident()?;
            self.pos += 1;
            return Ok(ImplItem::ClassModifier { name, value: Some(self.mod_value()?) });
        }
        if is_name && next.is_some_and(|t| t.is_punct(";")) {
            return Ok(ImplItem::ClassModifier { name: self.ident()?, value: None });
        }
        let scoped_type = next.is_some_and(|t| t.is_punct(":"))
            && self.peek_at(2).is_some_and(|t| t.is_punct(":") && !t.space_before);
        if is_name && next.is_some_and(|t| t.is_punct(":")) && !scoped_type {
            let member = self.ident()?;
            self.pos += 1;
            let mut modifiers = Vec::new();
            loop {
                let m = self.ident()?;
                let value = if self.eat_punct("=") { Some(self.mod_value()?) } else { None };
                modifiers.push((m, value));
                if !self.eat_punct(",") { break; }
            }
            return Ok(ImplItem::MemberModifiers { member, modifiers });
        }
        Ok(ImplItem::InstanceVar(self.member()?))
    }

    fn mod_value(&mut self) -> Result<ModValue> {
        match self.peek().map(|t| &t.kind) {
            Some(Kind::Str(_)) => Ok(ModValue::Str(self.string()?)),
            Some(Kind::Ident(_)) => Ok(ModValue::Name(self.scoped_name()?)),
            Some(Kind::Punct(":")) => Ok(ModValue::Name(self.scoped_name()?)),
            Some(_) => Ok(ModValue::Int(self.int_expr(false)?)),
            None => err(self.loc(), "expected a value"),
        }
    }
}

/// Parse preprocessed tokens.
pub fn parse(toks: &[Tok]) -> std::result::Result<Vec<Definition>, Error> {
    Parser::new(toks).specification()
}
