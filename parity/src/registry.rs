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
    all
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
