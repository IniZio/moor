use super::{needed_to_resume, Memory, ResumeAsk, Shortfall};

const HOST: u64 = 30 * 1024;
const RESERVE: u64 = 4096 + HOST / 10;
const CEILING: u64 = 23 * 1024;
const REAL_USE: u64 = 6 * 1024;
const NOW: i64 = 1_000;

fn memory(available_mib: u64) -> Memory {
    Memory {
        total_mib: HOST,
        reserve_mib: None,
        available_mib,
        psi_centi: None,
        reported_at: NOW,
        starting: 0,
        disk_free_mib: None,
        disk_total_mib: None,
    }
}

fn ask(needed_mib: u64, running: usize) -> ResumeAsk {
    ResumeAsk {
        needed_mib: Some(needed_mib),
        running,
        unreported: false,
    }
}

#[test]
fn a_resume_is_allowed_or_blocked_by_what_it_needs() {
    let needed = |reported| needed_to_resume(reported, Some(CEILING));
    // available, reported by the provider, Boxes running, may it resume
    let table = [
        // A light Box runs beside a 23 GiB ceiling that really uses 6 GiB.
        (20 * 1024, None, 1, false),
        (20 * 1024, Some(REAL_USE), 1, true),
        // Exactly the need above the reserve is enough; a MiB less is not.
        (RESERVE + REAL_USE, Some(REAL_USE), 1, true),
        (RESERVE + REAL_USE - 1, Some(REAL_USE), 1, false),
        // Nothing running: the margin replaces the reserve.
        (CEILING + 1024, None, 0, true),
        (CEILING + 1023, None, 0, false),
        (REAL_USE + 1024, Some(REAL_USE), 0, true),
        (REAL_USE + 1023, Some(REAL_USE), 0, false),
        // A provider's zero says nothing.
        (20 * 1024, Some(0), 1, false),
    ];
    for (available, reported, running, allowed) in table {
        let asked = ResumeAsk {
            needed_mib: needed(reported),
            ..ask(0, running)
        };
        let got = memory(available).resume_shortfall(NOW, asked);
        assert_eq!(
            got.is_none(),
            allowed,
            "available {available}, reported {reported:?}, running {running}: {got:?}"
        );
    }
}

#[test]
fn needed_falls_back_from_the_provider_to_the_guest() {
    assert_eq!(
        needed_to_resume(Some(REAL_USE), Some(CEILING)),
        Some(REAL_USE)
    );
    assert_eq!(needed_to_resume(None, Some(CEILING)), Some(CEILING));
    assert_eq!(needed_to_resume(Some(0), Some(CEILING)), Some(CEILING));
    assert_eq!(needed_to_resume(None, None), None);
}

#[test]
fn a_box_of_unknown_size_keeps_the_reserve_even_with_nothing_running() {
    let unknown = ResumeAsk {
        needed_mib: None,
        ..ask(0, 0)
    };
    assert_eq!(
        memory(RESERVE + 2047).resume_shortfall(NOW, unknown),
        Some(Shortfall::Low {
            free_mib: 2047,
            needs_mib: 2048
        })
    );
    assert_eq!(memory(RESERVE + 2048).resume_shortfall(NOW, unknown), None);
}

#[test]
fn a_box_starting_counts_against_a_resume() {
    let m = Memory {
        starting: 1,
        ..memory(RESERVE + REAL_USE)
    };
    assert_eq!(
        m.resume_shortfall(NOW, ask(REAL_USE, 1)),
        Some(Shortfall::Low {
            free_mib: REAL_USE - 2048,
            needs_mib: REAL_USE
        })
    );
}

#[test]
fn a_stale_report_pressure_or_an_unreported_box_blocks_any_resume() {
    let m = memory(HOST);
    assert_eq!(
        m.resume_shortfall(NOW + 61, ask(1, 0)),
        Some(Shortfall::Stale)
    );
    let pressed = Memory {
        psi_centi: Some(1000),
        ..m
    };
    assert_eq!(
        pressed.resume_shortfall(NOW, ask(1, 0)),
        Some(Shortfall::Pressure)
    );
    let unreported = ResumeAsk {
        unreported: true,
        ..ask(1, 1)
    };
    assert_eq!(
        m.resume_shortfall(NOW, unreported),
        Some(Shortfall::Unreported)
    );
}
