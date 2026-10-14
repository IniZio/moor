//! Recursive-descent parser for executable GraphQL documents.

use super::lex::{lex, Tok};

/// Deepest nesting of selection sets and values the parser accepts.
pub(super) const MAX_DEPTH: usize = 32;

/// An argument or default value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    /// A `$variable` reference.
    Var(String),
    /// A string literal.
    Str(String),
    /// A number, boolean, null or enum value; the content is not needed.
    Scalar,
    /// An enum value or `true`, `false`, `null`.
    Enum(String),
    /// A list literal.
    List(Vec<Self>),
    /// An object literal.
    Obj(Vec<(String, Self)>),
}

/// A field in a selection set.
#[derive(Debug, Clone)]
pub(super) struct Field {
    /// Alias, when given.
    pub(super) alias: Option<String>,
    /// Field name.
    pub(super) name: String,
    /// Arguments in source order.
    pub(super) args: Vec<(String, Value)>,
    /// Sub-selection; empty for a leaf.
    pub(super) sel: Vec<Sel>,
}

impl Field {
    /// Returns the response key: the alias, else the name.
    pub(super) fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// One entry of a selection set.
#[derive(Debug, Clone)]
pub(super) enum Sel {
    /// A field.
    Field(Field),
    /// A `...Name` fragment spread.
    Spread(String),
    /// An inline fragment; the type condition is not needed.
    Inline(Option<String>, Vec<Self>),
}

/// Operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// `query`
    Query,
    /// `mutation`
    Mutation,
    /// `subscription`
    Subscription,
}

/// An operation definition.
#[derive(Debug, Clone)]
pub(super) struct Op {
    /// Operation type.
    pub(super) kind: Kind,
    /// Variable names with their default values.
    pub(super) defaults: Vec<(String, Value)>,
    /// Root selection set.
    pub(super) sel: Vec<Sel>,
}

/// A parsed document.
#[derive(Debug, Clone, Default)]
pub(super) struct Doc {
    /// Operation definitions.
    pub(super) ops: Vec<Op>,
    /// Fragment definitions by name.
    pub(super) frags: Vec<(String, String, Vec<Sel>)>,
}

impl Doc {
    /// Returns the type condition and selection set of a fragment.
    pub(super) fn frag(&self, name: &str) -> Option<(&str, &[Sel])> {
        self.frags
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, on, sel)| (on.as_str(), sel.as_slice()))
    }
}

/// Parses a document.
pub(super) fn parse(src: &str) -> Result<Doc, String> {
    let mut parser = Parser {
        toks: lex(src)?,
        pos: 0,
        depth: 0,
    };
    let mut doc = Doc::default();
    while parser.peek().is_some() {
        parser.definition(&mut doc)?;
    }
    Ok(doc)
}

/// Cursor over the tokens.
struct Parser {
    /// Tokens.
    toks: Vec<Tok>,
    /// Index of the next token.
    pos: usize,
    /// Current nesting.
    depth: usize,
}

impl Parser {
    /// Returns the next token.
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    /// Consumes the next token.
    fn bump(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    /// Consumes a punctuator when it is next.
    fn eat(&mut self, c: char) -> bool {
        let hit = self.peek() == Some(&Tok::Punct(c));
        if hit {
            self.pos += 1;
        }
        hit
    }

    /// Consumes a required punctuator.
    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}`"))
        }
    }

    /// Consumes a name.
    fn name(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Name(name)) => Ok(name),
            _ => Err("expected a name".to_owned()),
        }
    }

    /// Enters one nesting level.
    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("nesting too deep".to_owned());
        }
        Ok(())
    }

    /// Parses one top-level definition.
    fn definition(&mut self, doc: &mut Doc) -> Result<(), String> {
        let head = match self.peek() {
            Some(Tok::Punct('{')) => "query".to_owned(),
            Some(Tok::Name(name)) => name.clone(),
            _ => return Err("expected an operation or fragment".to_owned()),
        };
        let kind = match head.as_str() {
            "query" => Kind::Query,
            "mutation" => Kind::Mutation,
            "subscription" => Kind::Subscription,
            "fragment" => return self.fragment(doc),
            other => return Err(format!("unexpected `{other}`")),
        };
        if matches!(self.peek(), Some(Tok::Name(_))) {
            self.pos += 1;
            if matches!(self.peek(), Some(Tok::Name(_))) {
                self.pos += 1;
            }
        }
        let defaults = self.var_defs()?;
        self.directives()?;
        let sel = self.selection_set()?;
        doc.ops.push(Op {
            kind,
            defaults,
            sel,
        });
        Ok(())
    }

    /// Parses `fragment Name on Type { ... }`.
    fn fragment(&mut self, doc: &mut Doc) -> Result<(), String> {
        self.pos += 1;
        let name = self.name()?;
        if name == "on" || doc.frag(&name).is_some() {
            return Err(format!("bad fragment name `{name}`"));
        }
        if self.name()? != "on" {
            return Err("expected `on`".to_owned());
        }
        let on = self.name()?;
        self.directives()?;
        let sel = self.selection_set()?;
        doc.frags.push((name, on, sel));
        Ok(())
    }

    /// Parses an optional variable definition list.
    fn var_defs(&mut self) -> Result<Vec<(String, Value)>, String> {
        let mut out = Vec::new();
        if !self.eat('(') {
            return Ok(out);
        }
        while !self.eat(')') {
            let Some(Tok::Var(name)) = self.bump() else {
                return Err("expected a variable definition".to_owned());
            };
            self.expect(':')?;
            self.type_ref()?;
            if self.eat('=') {
                out.push((name, self.value()?));
            }
            self.directives()?;
        }
        Ok(out)
    }

    /// Parses a type reference and ignores it.
    fn type_ref(&mut self) -> Result<(), String> {
        self.enter()?;
        if self.eat('[') {
            self.type_ref()?;
            self.expect(']')?;
        } else {
            self.name()?;
        }
        self.eat('!');
        self.depth -= 1;
        Ok(())
    }

    /// Parses and ignores directives.
    fn directives(&mut self) -> Result<(), String> {
        while self.eat('@') {
            self.name()?;
            self.args()?;
        }
        Ok(())
    }

    /// Parses an optional argument list.
    fn args(&mut self) -> Result<Vec<(String, Value)>, String> {
        let mut out = Vec::new();
        if !self.eat('(') {
            return Ok(out);
        }
        while !self.eat(')') {
            let name = self.name()?;
            self.expect(':')?;
            out.push((name, self.value()?));
        }
        Ok(out)
    }

    /// Parses a value.
    fn value(&mut self) -> Result<Value, String> {
        self.enter()?;
        let value = match self.bump() {
            Some(Tok::Var(name)) => Value::Var(name),
            Some(Tok::Str(text)) => Value::Str(text),
            Some(Tok::Num) => Value::Scalar,
            Some(Tok::Name(name)) => Value::Enum(name),
            Some(Tok::Punct('[')) => self.list()?,
            Some(Tok::Punct('{')) => self.object()?,
            _ => return Err("expected a value".to_owned()),
        };
        self.depth -= 1;
        Ok(value)
    }

    /// Parses list items after `[`.
    fn list(&mut self) -> Result<Value, String> {
        let mut items = Vec::new();
        while !self.eat(']') {
            items.push(self.value()?);
        }
        Ok(Value::List(items))
    }

    /// Parses object fields after `{`.
    fn object(&mut self) -> Result<Value, String> {
        let mut fields = Vec::new();
        while !self.eat('}') {
            let name = self.name()?;
            self.expect(':')?;
            fields.push((name, self.value()?));
        }
        Ok(Value::Obj(fields))
    }

    /// Parses `{ ... }` with at least one entry.
    fn selection_set(&mut self) -> Result<Vec<Sel>, String> {
        self.enter()?;
        self.expect('{')?;
        let mut out = Vec::new();
        while !self.eat('}') {
            out.push(self.selection()?);
        }
        if out.is_empty() {
            return Err("empty selection set".to_owned());
        }
        self.depth -= 1;
        Ok(out)
    }

    /// Parses a field, spread or inline fragment.
    fn selection(&mut self) -> Result<Sel, String> {
        if self.peek() != Some(&Tok::Spread) {
            return self.field().map(Sel::Field);
        }
        self.pos += 1;
        match self.peek() {
            Some(Tok::Name(name)) if name != "on" => {
                let name = name.clone();
                self.pos += 1;
                self.directives()?;
                Ok(Sel::Spread(name))
            }
            _ => {
                let on = if matches!(self.peek(), Some(Tok::Name(_))) {
                    self.pos += 1;
                    Some(self.name()?)
                } else {
                    None
                };
                self.directives()?;
                let sel = self.selection_set()?;
                Ok(Sel::Inline(on, sel))
            }
        }
    }

    /// Parses a field.
    fn field(&mut self) -> Result<Field, String> {
        let first = self.name()?;
        let (alias, name) = if self.eat(':') {
            (Some(first), self.name()?)
        } else {
            (None, first)
        };
        let args = self.args()?;
        self.directives()?;
        let sel = if self.peek() == Some(&Tok::Punct('{')) {
            self.selection_set()?
        } else {
            Vec::new()
        };
        Ok(Field {
            alias,
            name,
            args,
            sel,
        })
    }
}
