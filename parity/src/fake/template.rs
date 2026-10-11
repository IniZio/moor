use super::flag_value;
use std::path::Path;

pub struct TemplateCtx<'a> {
    pub argv: &'a [String],
    pub n: u32,
    pub box_root: &'a Path,
    pub root: &'a Path,
    pub cwd: &'a Path,
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn placeholder(name: &str, ctx: &TemplateCtx) -> Option<String> {
    if let Some(rest) = name.strip_prefix("argv[") {
        let idx = rest.strip_suffix(']')?;
        let n: usize = idx.parse().ok()?;
        return Some(ctx.argv.get(n).cloned().unwrap_or_default());
    }
    if let Some(flag) = name.strip_prefix("flag:") {
        return Some(flag_value(ctx.argv, flag).unwrap_or_default());
    }
    match name {
        "n" => Some(ctx.n.to_string()),
        "box" => Some(ctx.box_root.display().to_string()),
        "root" => Some(ctx.root.display().to_string()),
        "cwd" => Some(ctx.cwd.display().to_string()),
        "last" => Some(ctx.argv.last().cloned().unwrap_or_default()),
        "post" => {
            let post = match ctx.argv.iter().position(|a| a == "--") {
                Some(p) => &ctx.argv[p + 1..],
                None => &[][..],
            };
            Some(
                post.iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        _ => None,
    }
}

pub fn render(template: &str, ctx: &TemplateCtx) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            if let Some(val) = placeholder(&after[..close], ctx) {
                out.push_str(&val);
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}
