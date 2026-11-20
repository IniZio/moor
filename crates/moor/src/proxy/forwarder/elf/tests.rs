#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test fixtures"
)]

use super::interpreter;

/// A little-endian ELF64 header with one program header of `kind`, whose bytes are `body`.
pub(crate) fn elf64(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![0_u8; 64 + 56];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[0x20..0x28].copy_from_slice(&64_u64.to_le_bytes());
    b[0x36..0x38].copy_from_slice(&56_u16.to_le_bytes());
    b[0x38..0x3a].copy_from_slice(&1_u16.to_le_bytes());
    b[64..68].copy_from_slice(&kind.to_le_bytes());
    b[72..80].copy_from_slice(&120_u64.to_le_bytes());
    b[96..104].copy_from_slice(&u64::try_from(body.len()).unwrap().to_le_bytes());
    b.extend_from_slice(body);
    b
}

/// A big-endian ELF32 header with one program header of `kind`, whose bytes are `body`.
fn elf32_big(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![0_u8; 52 + 32];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 1;
    b[5] = 2;
    b[0x1c..0x20].copy_from_slice(&52_u32.to_be_bytes());
    b[0x2a..0x2c].copy_from_slice(&32_u16.to_be_bytes());
    b[0x2c..0x2e].copy_from_slice(&1_u16.to_be_bytes());
    b[52..56].copy_from_slice(&kind.to_be_bytes());
    b[56..60].copy_from_slice(&84_u32.to_be_bytes());
    b[68..72].copy_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    b.extend_from_slice(body);
    b
}

#[test]
fn an_executable_that_names_a_loader_is_dynamic() {
    let loader = b"/lib64/ld-linux-x86-64.so.2\0";
    assert_eq!(
        interpreter(&elf64(3, loader)).as_deref(),
        Some("/lib64/ld-linux-x86-64.so.2")
    );
    assert_eq!(
        interpreter(&elf32_big(3, b"/lib/ld.so.1\0")).as_deref(),
        Some("/lib/ld.so.1")
    );
}

#[test]
fn an_executable_with_no_loader_a_script_and_a_cut_header_are_not_dynamic() {
    assert_eq!(interpreter(&elf64(1, b"")), None);
    assert_eq!(
        interpreter(&elf64(2, b"")),
        None,
        "a static PIE has PT_DYNAMIC"
    );
    assert_eq!(interpreter(b"#!/bin/sh\nexit 0\n"), None);
    assert_eq!(interpreter(&elf64(3, b"/lib/ld.so\0")[..70]), None);
}
