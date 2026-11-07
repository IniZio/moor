use super::{BlobsConfig, S3Blobs};
use crate::config::parse_controller;

#[test]
fn no_entry_keeps_blobs_under_the_state_directory() {
    let cfg = parse_controller("c", "settle_every_secs: 5\n").unwrap();
    assert_eq!(cfg.blobs, BlobsConfig::Fs);
    let cfg = parse_controller("c", "blobs: { kind: fs }\n").unwrap();
    assert_eq!(cfg.blobs, BlobsConfig::Fs);
}

#[test]
fn an_s3_entry_names_an_endpoint_and_a_bucket() {
    let text = "blobs:\n  kind: s3\n  endpoint: https://blobs.example\n  bucket: moor\n  \
                prefix: nightly\n  credentials_dir: /var/lib/moor/blobs\n";
    assert_eq!(
        parse_controller("c", text).unwrap().blobs,
        BlobsConfig::S3(S3Blobs {
            endpoint: "https://blobs.example".into(),
            bucket: "moor".into(),
            region: None,
            prefix: Some("nightly".into()),
            credentials_dir: Some("/var/lib/moor/blobs".into()),
        })
    );
}

#[test]
fn a_bad_entry_is_refused_with_the_key_at_fault() {
    for (text, key) in [
        (
            "blobs: { kind: s3, endpoint: blobs.example, bucket: b }\n",
            "blobs.endpoint",
        ),
        (
            "blobs: { kind: s3, endpoint: http://b, bucket: ' ' }\n",
            "blobs.bucket",
        ),
    ] {
        let err = parse_controller("c", text).unwrap_err().to_string();
        assert!(err.contains(key), "{err}");
    }
    assert!(parse_controller("c", "blobs: { kind: tape }\n").is_err());
    assert!(parse_controller(
        "c",
        "blobs: { kind: s3, endpoint: http://b, bucket: b, secret: x }\n"
    )
    .is_err());
}
