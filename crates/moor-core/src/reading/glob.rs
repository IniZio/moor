//! How a `proxy.allow` entry matches a request and covers another entry, segment by segment:
//! `*` is one segment or part of one, `**` any number of segments.

/// Stands for the Box's repo when a line names none, so `{repo}` stays two segments.
const ANY_REPO_OF_BOX: &str = "\u{1}owner/\u{1}name";

/// `entry`'s methods and path with `{repo}` as `repo` and `{any_repo}` as any one repo; none
/// when it is not `METHODS /path`.
fn split<'a>(entry: &'a str, box_repo: Option<&str>) -> Option<(Vec<&'a str>, String)> {
    let (methods, path) = entry.split_once(' ')?;
    let path = path
        .replace("{any_repo}", "*/*")
        .replace("{repo}", box_repo.unwrap_or(ANY_REPO_OF_BOX));
    Some((methods.split('|').collect(), path))
}

/// Whether `methods` take `method`.
fn takes(methods: &[&str], method: &str) -> bool {
    methods.iter().any(|m| *m == "*" || *m == method)
}

/// One segment against a pattern whose `*` stands for any run of characters.
fn segment(pattern: &str, seg: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == seg,
        Some((head, tail)) => seg.strip_prefix(head).is_some_and(|rest| {
            (0..=rest.len())
                .filter_map(|i| rest.get(i..))
                .any(|r| segment(tail, r))
        }),
    }
}

/// Whether the segments `p` fall under the glob segments `g`; `one` judges a glob segment
/// against one of `p`.
fn walk(g: &[&str], p: &[&str], one: &dyn Fn(&str, &str) -> bool) -> bool {
    match (g.split_first(), p.split_first()) {
        (None, None) => true,
        (Some((&"**", gr)), _) => {
            walk(gr, p, one) || p.split_first().is_some_and(|(_, pr)| walk(g, pr, one))
        }
        (Some((a, gr)), Some((b, pr))) => one(a, b) && walk(gr, pr, one),
        _ => false,
    }
}

/// Whether the request `method path` passes `entry`, with `{repo}` as `repo`.
pub(super) fn lets(entry: &str, repo: &str, method: &str, path: &str) -> bool {
    let Some((methods, glob)) = split(entry, Some(repo)) else {
        return false;
    };
    let g: Vec<_> = glob.split('/').collect();
    let p: Vec<_> = path.split('/').collect();
    takes(&methods, method) && walk(&g, &p, &segment)
}
