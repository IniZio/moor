//! Root fields beyond `repository`: `search`, `__type` and `node` results.

use serde_json::Value as Json;

use super::super::parse::{Field, Value};
use super::super::Reason;
use super::{deny_field, Policy};

/// Subfields of `viewer` that are allowed.
const VIEWER_FIELDS: &[&str] = &["login", "id", "__typename"];

/// Arguments `search` may carry.
const SEARCH_ARGS: &[&str] = &["query", "type", "first", "last", "after", "before"];

impl<'a> Policy<'a> {
    /// Allows `search(type: ISSUE, query: ...)` confined to the Task repo.
    pub(super) fn search(&mut self, field: &'a Field) -> Result<(), Reason> {
        Self::only_args(field, SEARCH_ARGS)?;
        let kind = field.args.iter().find(|(k, _)| k == "type");
        let issue = match kind.map(|(_, v)| v) {
            Some(Value::Enum(name)) => name == "ISSUE",
            Some(var @ Value::Var(_)) => self.resolve(var)? == Json::String("ISSUE".to_owned()),
            _ => false,
        };
        if !issue {
            return Err(Reason::new("`search` needs `type: ISSUE`"));
        }
        super::query::search_scope(&self.arg_str(field, "query")?, self.repo)?;
        self.connection(&field.sel, "pr", 2)
    }

    /// Allows the `fields { name }` schema probe on `__type`.
    pub(super) fn introspect(&mut self, field: &'a Field) -> Result<(), Reason> {
        Self::only_args(field, &["name"])?;
        for fields in self.fields(&field.sel, 2)? {
            Self::only_args(fields, &["includeDeprecated"])?;
            if fields.name != "fields" {
                return Err(deny_field(&format!("__type.{}", fields.name)));
            }
            for sub in self.fields(&fields.sel, 3)? {
                if sub.name != "name" || !sub.sel.is_empty() {
                    return Err(deny_field(&format!("__type.fields.{}", sub.name)));
                }
            }
        }
        Ok(())
    }

    /// Checks the selection of a `node` result, read as a pull request.
    pub(super) fn node_result(&mut self, field: &'a Field) -> Result<(), Reason> {
        self.pr_only = true;
        let fields = self.fields(&field.sel, 2);
        self.pr_only = false;
        self.check_fields(&fields?, "pr", 2)
    }

    /// Requires `repository(owner, name)` to name the Task repo.
    pub(super) fn repository(&self, field: &Field) -> Result<(), Reason> {
        Self::only_args(field, &["owner", "name"])?;
        let owner = self.arg_str(field, "owner")?;
        let name = self.arg_str(field, "name")?;
        if owner.eq_ignore_ascii_case(&self.repo.owner)
            && name.eq_ignore_ascii_case(&self.repo.name)
        {
            return Ok(());
        }
        Err(Reason::new(format!(
            "field `repository` names `{owner}/{name}`, not the Task repo"
        )))
    }

    /// Collects the IDs of `nodes(ids: ...)`.
    pub(super) fn node_list(&mut self, field: &Field) -> Result<(), Reason> {
        let arg = field.args.iter().find(|(k, _)| k == "ids");
        let items = match arg.map(|(_, v)| self.resolve(v)) {
            Some(Ok(Json::Array(items))) => items,
            Some(Err(reason)) => return Err(reason),
            _ => return Err(Reason::new("`nodes` needs an `ids` list")),
        };
        for item in items {
            let Json::String(id) = item else {
                return Err(Reason::new("`nodes` ids must be strings"));
            };
            self.note(id);
        }
        Ok(())
    }

    /// Allows `viewer` with only identity subfields.
    pub(super) fn viewer(&mut self, field: &'a Field) -> Result<(), Reason> {
        Self::only_args(field, &[])?;
        for sub in self.fields(&field.sel, 2)? {
            if !VIEWER_FIELDS.contains(&sub.name.as_str()) || !sub.sel.is_empty() {
                return Err(Reason::new(format!(
                    "`viewer` subfield `{}` is not allowed",
                    sub.name
                )));
            }
        }
        Ok(())
    }
}
