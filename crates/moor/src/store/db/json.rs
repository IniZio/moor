//! SQL reading a field of a JSON text column, written for each backend.

use super::Backend;

/// SQL for the integer field `key` of the JSON text `col`, NULL when it is absent.
pub(crate) fn int(backend: Backend, col: &str, key: &str) -> String {
    match backend {
        Backend::Sqlite => format!("json_extract({col}, '$.{key}')"),
        Backend::Postgres => format!("(({col})::json ->> '{key}')::bigint"),
    }
}

/// SQL for the boolean field `key` of the JSON text `col` as 1 or 0, NULL when it is absent.
pub(crate) fn flag(backend: Backend, col: &str, key: &str) -> String {
    match backend {
        Backend::Sqlite => format!("json_extract({col}, '$.{key}')"),
        Backend::Postgres => format!("(({col})::json ->> '{key}')::boolean::int"),
    }
}
