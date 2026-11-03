//! Users, how they sign in, and what they keep: secrets, MCP servers, profiles, webhooks.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{blob, fk, fk_cascade, index, int, key, row_id, serial_id, text};

/// The user tables.
pub(super) fn write(s: &mut Script) {
    for table in [
        users,
        identities,
        user_secrets,
        front_leases,
        user_mcp_servers,
        user_mcp_oauth,
        mcp_oauth_pending,
        user_profiles,
        user_webhooks,
    ] {
        table(s);
    }
}

/// The `users` table.
fn users(s: &mut Script) {
    s.schema(
        Table::create()
            .table("users")
            .col(serial_id("id"))
            .col(int("created_at").not_null())
            .col(text("email")),
    );
    s.schema(index("users_email", "users", &["email"]).unique());
}

/// The `identities` table.
fn identities(s: &mut Script) {
    s.schema(
        Table::create()
            .table("identities")
            .col(text("identity").primary_key())
            .col(int("user").not_null())
            .col(int("linked_at").not_null())
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.schema(&index("identities_user", "identities", &["user"]));
}

/// The `user_secrets` table.
fn user_secrets(s: &mut Script) {
    s.schema(
        Table::create()
            .table("user_secrets")
            .col(int("user").not_null())
            .col(text("name").not_null())
            .col(blob("value").not_null())
            .col(int("updated_at").not_null())
            .primary_key(&mut key(&["user", "name"]))
            .foreign_key(&mut fk("user", "users", "id")),
    );
}

/// The `front_leases` table.
fn front_leases(s: &mut Script) {
    s.schema(
        Table::create()
            .table("front_leases")
            .col(text("token_hash").primary_key())
            .col(text("front").not_null())
            .col(int("user").not_null())
            .col(int("expires_at").not_null())
            .foreign_key(&mut fk("user", "users", "id")),
    );
}

/// The `user_mcp_servers` table.
fn user_mcp_servers(s: &mut Script) {
    s.schema(
        Table::create()
            .table("user_mcp_servers")
            .col(int("user").not_null())
            .col(text("name").not_null())
            .col(text("url").not_null())
            .col(text("header").not_null())
            .col(text("secret").not_null())
            .col(int("created_at").not_null())
            .col(int("updated_at").not_null())
            .primary_key(&mut key(&["user", "name"]))
            .foreign_key(&mut fk("user", "users", "id")),
    );
}

/// The `user_mcp_oauth` table.
fn user_mcp_oauth(s: &mut Script) {
    s.schema(
        Table::create()
            .table("user_mcp_oauth")
            .col(int("user").not_null())
            .col(text("name").not_null())
            .col(blob("value").not_null())
            .col(int("expires_at"))
            .col(int("updated_at").not_null())
            .col(int("signed_out_at"))
            .primary_key(&mut key(&["user", "name"]))
            .foreign_key(&mut fk_cascade("user", "users", "id")),
    );
}

/// The `mcp_oauth_pending` table.
fn mcp_oauth_pending(s: &mut Script) {
    s.schema(
        Table::create()
            .table("mcp_oauth_pending")
            .col(text("state").primary_key())
            .col(int("user").not_null())
            .col(text("name").not_null())
            .col(blob("value").not_null())
            .col(int("expires_at").not_null())
            .foreign_key(&mut fk_cascade("user", "users", "id")),
    );
}

/// The `user_profiles` table.
fn user_profiles(s: &mut Script) {
    s.schema(
        Table::create()
            .table("user_profiles")
            .col(int("user").not_null().primary_key())
            .col(text("repo").not_null())
            .col(text("git_ref"))
            .col(int("updated_at").not_null())
            .foreign_key(&mut fk("user", "users", "id")),
    );
}

/// The `user_webhooks` table.
fn user_webhooks(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("user_webhooks")
            .col(id)
            .col(int("user").not_null())
            .col(text("url").not_null())
            .col(text("events").not_null())
            .col(blob("key").not_null())
            .col(int("created_at").not_null())
            .col(int("last_ok_at"))
            .col(int("failing_since"))
            .col(text("last_error"))
            .col(text("state").not_null().default("waiting_for_secret"))
            .col(text("header"))
            .col(blob("header_value"))
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.schema(&index("user_webhooks_user", "user_webhooks", &["user"]));
}
