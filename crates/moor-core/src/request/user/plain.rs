//! Arguments of the secret, enroll and proxy-release user requests.

use serde::{Deserialize, Serialize};

use super::SecretValue;
use crate::user::{Email, Identity, SecretName};

/// Arguments of `secret-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
    /// The secret's value.
    pub value: SecretValue,
}

/// Arguments of `secret-delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDeleteReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
}

/// Arguments of `enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The email the front verified for that identity; only a front socket may send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<Email>,
}

/// Arguments of `proxy-release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyReleaseReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The token `proxy-lease` granted.
    pub token: SecretValue,
}
