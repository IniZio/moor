#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use tokio::io::AsyncWriteExt;

use super::{read_frame, write_frame, Exit, Frame, MAX_PAYLOAD};
use crate::link::boxes::Size;

/// What `frames` read back as, after crossing a pipe.
async fn round_trip(frames: &[Frame<'_>]) -> Vec<String> {
    let (mut near, mut far) = tokio::io::duplex(64 * 1024);
    for frame in frames {
        write_frame(&mut near, frame).await.unwrap();
    }
    drop(near);
    let mut buf = Vec::new();
    let mut read = Vec::new();
    while let Some(frame) = read_frame(&mut far, &mut buf).await.unwrap() {
        read.push(format!("{frame:?}"));
    }
    read
}

#[tokio::test]
async fn every_kind_of_frame_reads_back_as_written() {
    let frames = [
        Frame::In(b"ls\n"),
        Frame::InEnd,
        Frame::Out(b"out"),
        Frame::Err(b""),
        Frame::Resize(Size {
            cols: 132,
            rows: 50,
        }),
        Frame::Exit(Exit::Code(-3)),
        Frame::Exit(Exit::Signal(9)),
        Frame::Exit(Exit::Down),
    ];
    let want: Vec<String> = frames.iter().map(|f| format!("{f:?}")).collect();
    assert_eq!(round_trip(&frames).await, want);
}

#[tokio::test]
async fn the_header_is_a_kind_byte_and_a_big_endian_length() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &Frame::Out(b"hi")).await.unwrap();
    assert_eq!(bytes, [2, 0, 0, 0, 2, b'h', b'i']);
}

#[tokio::test]
async fn a_stream_that_ends_between_frames_ends_cleanly() {
    let mut buf = Vec::new();
    assert_eq!(
        read_frame(&mut tokio::io::empty(), &mut buf).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_frame_cut_short_is_an_error() {
    let mut buf = Vec::new();
    let mut cut: &[u8] = &[2, 0, 0, 0, 5, b'h'];
    assert!(read_frame(&mut cut, &mut buf).await.is_err());
}

#[tokio::test]
async fn a_length_over_the_cap_is_refused_before_anything_is_allocated() {
    let (mut near, mut far) = tokio::io::duplex(64);
    let too_long = u32::try_from(MAX_PAYLOAD + 1).unwrap();
    let mut header = vec![2u8];
    header.extend_from_slice(&too_long.to_be_bytes());
    near.write_all(&header).await.unwrap();
    let mut buf = Vec::new();
    let err = read_frame(&mut far, &mut buf).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(buf.capacity() < MAX_PAYLOAD);
    let mut sink = Vec::new();
    let long = vec![0u8; MAX_PAYLOAD + 1];
    assert!(write_frame(&mut sink, &Frame::Out(&long)).await.is_err());
}

#[tokio::test]
async fn an_unknown_kind_and_a_malformed_payload_are_errors() {
    let mut buf = Vec::new();
    let mut unknown: &[u8] = &[99, 0, 0, 0, 0];
    assert!(read_frame(&mut unknown, &mut buf).await.is_err());
    let mut resize: &[u8] = &[4, 0, 0, 0, 1, 7];
    assert!(read_frame(&mut resize, &mut buf).await.is_err());
    let mut exit: &[u8] = &[5, 0, 0, 0, 1, 9];
    assert!(read_frame(&mut exit, &mut buf).await.is_err());
}
