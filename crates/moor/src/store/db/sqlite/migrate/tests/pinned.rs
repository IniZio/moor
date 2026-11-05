//! The checksums of the migrations already applied.

use moor_migrate::Backend;

#[test]
fn applied_migrations_are_never_edited() {
    let pinned = |backend| -> Vec<(i32, u64)> {
        moor_migrate::steps(moor_schema::STEPS, backend)
            .unwrap()
            .iter()
            .map(|m| (m.version(), m.checksum()))
            .collect()
    };
    assert_eq!(
        (pinned(Backend::Sqlite), pinned(Backend::Postgres)),
        (
            vec![
                (45, 1_311_846_121_584_832_248),
                (46, 858_562_308_596_054_046),
                (47, 14_823_850_939_632_049_515),
                (48, 1_108_073_109_587_260_101)
            ],
            vec![
                (45, 3_089_193_402_876_792_007),
                (46, 2_751_851_257_713_979_344),
                (47, 31_985_148_182_827_830),
                (48, 7_673_564_160_075_829_101)
            ]
        ),
        "a schema change is a new step, and a sea-query upgrade must not change what an \
         applied one renders"
    );
}
