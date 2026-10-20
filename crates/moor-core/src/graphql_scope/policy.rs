//! The scope policy: which root fields, mutations and nested fields a request may use.

use serde_json::{Map, Value as Json};

use super::parse::{Doc, Field, Kind, Op, Sel, Value, MAX_DEPTH};
use super::schema::{self, Next};

mod query;
mod roots;
use super::{NodeId, Reason, RepoRef};

/// Most selections visited while expanding fragments.
const MAX_VISITS: usize = 5_000;

/// Allowed mutations with the input keys that hold node IDs; the first key is required.
const MUTATIONS: &[(&str, &[&str])] = &[
    ("createPullRequest", &["repositoryId", "headRepositoryId"]),
    ("updatePullRequest", &["pullRequestId"]),
    ("markPullRequestReadyForReview", &["pullRequestId"]),
    ("disablePullRequestAutoMerge", &["pullRequestId"]),
    ("requestReviewsByLogin", &["pullRequestId"]),
    ("replaceActorsForAssignable", &["assignableId"]),
    ("convertPullRequestToDraft", &["pullRequestId"]),
];

/// Checks the only operation of a document and returns the node IDs that need a check.
pub(super) fn run(
    doc: &Doc,
    vars: &Map<String, Json>,
    repo: &RepoRef,
) -> Result<Vec<NodeId>, Reason> {
    let op = match doc.ops.as_slice() {
        [op] => op,
        [] => return Err(Reason::new("no operation")),
        _ => return Err(Reason::new("more than one operation")),
    };
    if op.kind == Kind::Subscription {
        return Err(Reason::new("subscriptions are not allowed"));
    }
    let mut policy = Policy {
        doc,
        op,
        vars,
        repo,
        visits: 0,
        pr_only: false,
        ids: Vec::new(),
    };
    policy.root()?;
    Ok(policy.ids)
}

/// State of one check.
struct Policy<'a> {
    /// The document.
    doc: &'a Doc,
    /// The operation being checked.
    op: &'a Op,
    /// Request variables.
    vars: &'a Map<String, Json>,
    /// The Box repo.
    repo: &'a RepoRef,
    /// Selections visited so far.
    visits: usize,
    /// When set, type conditions must be `PullRequest`.
    pr_only: bool,
    /// Node IDs collected, without repeats.
    ids: Vec<NodeId>,
}

/// Builds the refusal for a field.
fn deny_field(name: &str) -> Reason {
    Reason::new(format!("field `{name}` is not allowed"))
}

/// Converts a value to JSON, resolving variables through `lookup`.
fn to_json(value: &Value, lookup: &dyn Fn(&str) -> Result<Json, Reason>) -> Result<Json, Reason> {
    Ok(match value {
        Value::Var(name) => lookup(name)?,
        Value::Str(text) => Json::String(text.clone()),
        Value::Scalar | Value::Enum(_) => Json::Null,
        Value::List(items) => Json::Array(
            items
                .iter()
                .map(|v| to_json(v, lookup))
                .collect::<Result<_, _>>()?,
        ),
        Value::Obj(fields) => Json::Object(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), to_json(v, lookup)?)))
                .collect::<Result<_, Reason>>()?,
        ),
    })
}

impl<'a> Policy<'a> {
    /// Resolves an argument value, using variable defaults for missing variables.
    fn resolve(&self, value: &Value) -> Result<Json, Reason> {
        to_json(value, &|name| {
            if let Some(found) = self.vars.get(name) {
                return Ok(found.clone());
            }
            let default = self.op.defaults.iter().find(|(n, _)| n == name);
            let no_vars = |_: &str| Err(Reason::new("variable default uses a variable"));
            default.map_or_else(
                || Err(Reason::new(format!("variable `${name}` is not provided"))),
                |(_, v)| to_json(v, &no_vars),
            )
        })
    }

    /// Resolves a string argument of `field`.
    fn arg_str(&self, field: &Field, key: &str) -> Result<String, Reason> {
        let arg = field.args.iter().find(|(k, _)| k == key);
        match arg.map(|(_, v)| self.resolve(v)) {
            Some(Ok(Json::String(text))) => Ok(text),
            Some(Err(reason)) => Err(reason),
            _ => Err(Reason::new(format!(
                "`{}` needs a string `{key}` argument",
                field.name
            ))),
        }
    }

    /// Requires that `field` has exactly the arguments in `allowed`, each at most once.
    fn only_args(field: &Field, allowed: &[&str]) -> Result<(), Reason> {
        for (i, (key, _)) in field.args.iter().enumerate() {
            let repeated = field.args.iter().take(i).any(|(k, _)| k == key);
            if repeated || !allowed.contains(&key.as_str()) {
                return Err(Reason::new(format!(
                    "`{}` argument `{key}` is not allowed",
                    field.name
                )));
            }
        }
        Ok(())
    }

    /// Records a node ID once.
    fn note(&mut self, id: String) {
        let id = NodeId::new(id);
        if !self.ids.contains(&id) {
            self.ids.push(id);
        }
    }

    /// Expands inline fragments and spreads into the fields they select.
    fn flatten(
        &mut self,
        sel: &'a [Sel],
        depth: usize,
        stack: &mut Vec<&'a str>,
        out: &mut Vec<&'a Field>,
    ) -> Result<(), Reason> {
        if depth > MAX_DEPTH {
            return Err(Reason::new("query nested too deep"));
        }
        for item in sel {
            self.visits += 1;
            if self.visits > MAX_VISITS {
                return Err(Reason::new("query too large after expanding fragments"));
            }
            match item {
                Sel::Field(field) => out.push(field),
                Sel::Inline(on, inner) => {
                    self.check_type(on.as_deref())?;
                    self.flatten(inner, depth + 1, stack, out)?;
                }
                Sel::Spread(name) => self.spread(name, depth, stack, out)?,
            }
        }
        Ok(())
    }

    /// Refuses a type condition other than `PullRequest` under a `node` result.
    fn check_type(&self, on: Option<&str>) -> Result<(), Reason> {
        match on {
            Some(ty) if self.pr_only && ty != "PullRequest" => Err(Reason::new(format!(
                "node results may only be read as PullRequest, not `{ty}`"
            ))),
            _ => Ok(()),
        }
    }

    /// Expands one fragment spread.
    fn spread(
        &mut self,
        name: &'a str,
        depth: usize,
        stack: &mut Vec<&'a str>,
        out: &mut Vec<&'a Field>,
    ) -> Result<(), Reason> {
        if stack.contains(&name) {
            return Err(Reason::new(format!("fragment `{name}` is cyclic")));
        }
        let doc: &'a Doc = self.doc;
        let (on, body) = doc
            .frag(name)
            .ok_or_else(|| Reason::new(format!("unknown fragment `{name}`")))?;
        self.check_type(Some(on))?;
        stack.push(name);
        self.flatten(body, depth + 1, stack, out)?;
        stack.pop();
        Ok(())
    }

    /// Returns the fields of a selection set after expansion.
    fn fields(&mut self, sel: &'a [Sel], depth: usize) -> Result<Vec<&'a Field>, Reason> {
        let mut out = Vec::new();
        self.flatten(sel, depth, &mut Vec::new(), &mut out)?;
        Ok(out)
    }

    /// Checks every root field of the operation.
    fn root(&mut self) -> Result<(), Reason> {
        let op: &'a Op = self.op;
        let fields = self.fields(&op.sel, 1)?;
        for (i, field) in fields.iter().enumerate() {
            if fields.iter().take(i).any(|f| f.key() == field.key()) {
                return Err(Reason::new(format!(
                    "response key `{}` is used twice",
                    field.key()
                )));
            }
            if field.name == "__typename" {
                continue;
            }
            match op.kind {
                Kind::Mutation => self.mutation(field)?,
                _ => self.query(field)?,
            }
        }
        Ok(())
    }

    /// Checks a root query field.
    fn query(&mut self, field: &'a Field) -> Result<(), Reason> {
        match field.name.as_str() {
            "repository" => {
                self.repository(field)?;
                self.nested(&field.sel, schema::REPO, 2)
            }
            "viewer" => self.viewer(field),
            "node" => {
                Self::only_args(field, &["id"])?;
                let id = self.arg_str(field, "id")?;
                self.note(id);
                self.node_result(field)
            }
            "nodes" => {
                Self::only_args(field, &["ids"])?;
                self.node_list(field)?;
                self.node_result(field)
            }
            "search" => self.search(field),
            "__type" => self.introspect(field),
            other => Err(deny_field(other)),
        }
    }

    /// Checks an allow-listed mutation and collects its node IDs.
    fn mutation(&mut self, field: &'a Field) -> Result<(), Reason> {
        if matches!(
            field.name.as_str(),
            "mergePullRequest" | "enablePullRequestAutoMerge"
        ) {
            return Err(Reason::new(format!(
                "`{}`: merging is not allowed from the Box",
                field.name
            )));
        }
        let Some((_, keys)) = MUTATIONS.iter().find(|(name, _)| *name == field.name) else {
            return Err(deny_field(&field.name));
        };
        Self::only_args(field, &["input"])?;
        let input = field.args.iter().find(|(k, _)| k == "input");
        let input = match input.map(|(_, v)| self.resolve(v)) {
            Some(Ok(Json::Object(input))) => input,
            Some(Err(reason)) => return Err(reason),
            _ => {
                return Err(Reason::new(format!(
                    "`{}` needs an input object",
                    field.name
                )))
            }
        };
        for (i, key) in keys.iter().enumerate() {
            match input.get(*key) {
                Some(Json::String(id)) => self.note(id.clone()),
                None if i > 0 => {}
                _ => {
                    return Err(Reason::new(format!(
                        "`{}` input needs a string `{key}`",
                        field.name
                    )))
                }
            }
        }
        self.nested(&field.sel, schema::PAYLOAD, 2)
    }

    /// Checks a selection set against the allow-list of its context.
    fn nested(&mut self, sel: &'a [Sel], ctx: &str, depth: usize) -> Result<(), Reason> {
        let fields = self.fields(sel, depth)?;
        self.check_fields(&fields, ctx, depth)
    }

    /// Checks expanded fields against the allow-list of their context.
    fn check_fields(
        &mut self,
        fields: &[&'a Field],
        ctx: &str,
        depth: usize,
    ) -> Result<(), Reason> {
        let shape = schema::shape(ctx).ok_or_else(|| Reason::new(format!("no context `{ctx}`")))?;
        for field in fields.iter().copied() {
            let name = field.name.as_str();
            if name == "__typename" || shape.leaves.contains(&name) {
                if !field.sel.is_empty() {
                    return Err(Reason::new(format!("`{ctx}.{name}` takes no selection")));
                }
                continue;
            }
            match shape.links.iter().find(|(n, _)| *n == name) {
                Some((_, Next::Ctx(next))) => self.nested(&field.sel, next, depth + 1)?,
                Some((_, Next::Conn(next))) => self.connection(&field.sel, next, depth + 1)?,
                None => return Err(Reason::new(format!("field `{ctx}.{name}` is not allowed"))),
            }
        }
        Ok(())
    }

    /// Checks the selection of a connection whose items are in context `ctx`.
    fn connection(&mut self, sel: &'a [Sel], ctx: &str, depth: usize) -> Result<(), Reason> {
        for field in self.fields(sel, depth)? {
            match field.name.as_str() {
                "nodes" => self.nested(&field.sel, ctx, depth + 1)?,
                "edges" => self.edges(&field.sel, ctx, depth + 1)?,
                "pageInfo" => self.leaves(&field.sel, schema::PAGE_INFO, depth + 1)?,
                "totalCount" | "issueCount" | "__typename" if field.sel.is_empty() => {}
                other => {
                    return Err(Reason::new(format!(
                        "connection field `{other}` is not allowed"
                    )))
                }
            }
        }
        Ok(())
    }

    /// Checks the selection of a connection's `edges`.
    fn edges(&mut self, sel: &'a [Sel], ctx: &str, depth: usize) -> Result<(), Reason> {
        for field in self.fields(sel, depth)? {
            match field.name.as_str() {
                "node" => self.nested(&field.sel, ctx, depth + 1)?,
                "cursor" | "__typename" if field.sel.is_empty() => {}
                other => return Err(Reason::new(format!("edge field `{other}` is not allowed"))),
            }
        }
        Ok(())
    }

    /// Requires every selected field to be a leaf from `allowed`.
    fn leaves(&mut self, sel: &'a [Sel], allowed: &[&str], depth: usize) -> Result<(), Reason> {
        for field in self.fields(sel, depth)? {
            if !allowed.contains(&field.name.as_str()) || !field.sel.is_empty() {
                return Err(deny_field(&field.name));
            }
        }
        Ok(())
    }
}
