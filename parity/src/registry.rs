pub struct Case {
    pub id: &'static str,
    pub covers: &'static [&'static str],
    pub tags: &'static [&'static str],
    pub run: fn(),
}

inventory::collect!(Case);

pub fn cases() -> Vec<&'static Case> {
    let mut all: Vec<&'static Case> = inventory::iter::<Case>.into_iter().collect();
    all.sort_by_key(|c| c.id);
    check_unique(&all);
    if let Err(errs) = validate(&all) {
        panic!("invalid case tags:\n{}", errs.join("\n"));
    }
    all
}

pub fn validate(cases: &[&Case]) -> Result<(), Vec<String>> {
    let errs: Vec<String> = cases
        .iter()
        .filter_map(|c| parse_tags(c.tags).err().map(|e| format!("{}: {e}", c.id)))
        .collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Both,
    NewOnly(String),
}

impl Tag {
    pub fn label(&self) -> String {
        match self {
            Tag::Both => "both".to_string(),
            Tag::NewOnly(cap) => format!("new_only({cap})"),
        }
    }
}

pub fn parse_tags(tags: &[&str]) -> Result<Tag, String> {
    match tags {
        [] | ["both"] => Ok(Tag::Both),
        [one] => one
            .strip_prefix("new_only(")
            .and_then(|r| r.strip_suffix(')'))
            .filter(|cap| !cap.is_empty() && !cap.contains(['(', ')']))
            .map(|cap| Tag::NewOnly(cap.to_string()))
            .ok_or_else(|| format!("unknown tag: {one}")),
        many => Err(format!("expected at most one tag, got {}", many.len())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail(String),
    XFail,
    StaleTag,
}

impl Verdict {
    pub fn result(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail(_) => "fail",
            Verdict::XFail => "xfail",
            Verdict::StaleTag => "stale_tag",
        }
    }

    pub fn ok(&self) -> bool {
        matches!(self, Verdict::Pass | Verdict::XFail)
    }
}

pub fn verdict(tag: &Tag, caps: &[&str], outcome: Result<(), String>) -> Verdict {
    let expect_fail = matches!(tag, Tag::NewOnly(c) if !caps.contains(&c.as_str()));
    match (expect_fail, outcome) {
        (false, Ok(())) => Verdict::Pass,
        (false, Err(m)) => Verdict::Fail(m),
        (true, Err(_)) => Verdict::XFail,
        (true, Ok(())) => Verdict::StaleTag,
    }
}

fn check_unique(cases: &[&Case]) {
    for pair in cases.windows(2) {
        assert!(
            pair[0].id != pair[1].id,
            "duplicate case id: {}",
            pair[0].id
        );
    }
}

#[macro_export]
macro_rules! parity {
    ($id:ident, covers = [$($c:expr),* $(,)?], tags = [$($t:expr),* $(,)?], $body:block) => {
        fn $id() $body
        $crate::inventory::submit! {
            $crate::registry::Case {
                id: stringify!($id),
                covers: &[$($c),*],
                tags: &[$($t),*],
                run: $id,
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn case(id: &'static str) -> Case {
        Case {
            id,
            covers: &[],
            tags: &[],
            run: || {},
        }
    }

    #[test]
    fn distinct_ids_pass() {
        check_unique(&[&case("a"), &case("b")]);
    }

    #[test]
    #[should_panic(expected = "duplicate case id: dup")]
    fn duplicate_ids_panic() {
        check_unique(&[&case("a"), &case("dup"), &case("dup")]);
    }

    #[test]
    fn parse_tags_accepts() {
        assert_eq!(parse_tags(&[]), Ok(Tag::Both));
        assert_eq!(parse_tags(&["both"]), Ok(Tag::Both));
        assert_eq!(parse_tags(&["new_only(gc)"]), Ok(Tag::NewOnly("gc".into())));
    }

    #[test]
    fn parse_tags_rejects() {
        for bad in [
            &["bogus"][..],
            &["new_only()"],
            &["new_only(gc"],
            &["new_only(a(b))"],
            &["both", "new_only(gc)"],
            &["both", "both"],
        ] {
            assert!(parse_tags(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn tag_labels() {
        assert_eq!(Tag::Both.label(), "both");
        assert_eq!(Tag::NewOnly("gc".into()).label(), "new_only(gc)");
    }

    #[test]
    fn validate_rejects_bad_tags() {
        let good = case("good");
        let bad = Case {
            tags: &["nope"],
            ..case("bad")
        };
        assert!(validate(&[&good]).is_ok());
        let errs = validate(&[&good, &bad]).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].starts_with("bad:"));
    }

    #[test]
    fn verdict_both() {
        assert_eq!(verdict(&Tag::Both, &[], Ok(())), Verdict::Pass);
        assert_eq!(
            verdict(&Tag::Both, &[], Err("x".into())),
            Verdict::Fail("x".into())
        );
    }

    #[test]
    fn verdict_new_only_cap_absent_fails_is_xfail() {
        let t = Tag::NewOnly("gc".into());
        let v = verdict(&t, &["other"], Err("x".into()));
        assert_eq!(v, Verdict::XFail);
        assert!(v.ok());
        assert_eq!(v.result(), "xfail");
    }

    #[test]
    fn stale_new_only_tag_fails() {
        let t = Tag::NewOnly("gc".into());
        let v = verdict(&t, &[], Ok(()));
        assert_eq!(v, Verdict::StaleTag);
        assert!(!v.ok());
        assert_eq!(v.result(), "stale_tag");
    }

    #[test]
    fn verdict_new_only_cap_present_behaves_like_both() {
        let t = Tag::NewOnly("gc".into());
        assert_eq!(verdict(&t, &["gc"], Ok(())), Verdict::Pass);
        let v = verdict(&t, &["gc"], Err("x".into()));
        assert_eq!(v, Verdict::Fail("x".into()));
        assert!(!v.ok());
        assert_eq!(v.result(), "fail");
    }
}
