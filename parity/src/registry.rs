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
    all
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
}
