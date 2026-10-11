use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }

    /// Decoded (header, claims) of the Bearer JWT; None when the token is not a JWT.
    pub fn jwt(&self) -> Option<(Value, Value)> {
        let mut parts = self.bearer()?.split('.');
        let (h, c, _sig) = (parts.next()?, parts.next()?, parts.next()?);
        let decode = |s: &str| serde_json::from_slice(&base64url_decode(s)?).ok();
        Some((decode(h)?, decode(c)?))
    }

    pub fn basic(&self) -> Option<String> {
        let raw = self.header("authorization")?.strip_prefix("Basic ")?;
        String::from_utf8(base64url_decode(raw)?).ok()
    }
}

/// Decodes standard or URL-safe base64, padded or not.
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub location: Option<String>,
}

impl Reply {
    pub fn json(status: u16, body: &Value) -> Reply {
        Reply {
            status,
            body: body.to_string(),
            location: None,
        }
    }

    pub fn ok(body: &Value) -> Reply {
        Reply::json(200, body)
    }

    pub fn redirect(status: u16, location: &str) -> Reply {
        Reply {
            status,
            body: String::new(),
            location: Some(location.to_string()),
        }
    }

    pub fn message(status: u16, message: &str) -> Reply {
        Reply::json(status, &json!({ "message": message }))
    }

    pub fn oauth_error(error: &str) -> Reply {
        Reply::ok(&json!({ "error": error }))
    }
}
