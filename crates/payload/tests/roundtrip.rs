//! End-to-end payload tests: build → locate → extract → verify, plus
//! corruption and hostile-input handling.

use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use inst_payload::format::{Codec, Filter, Index};
use inst_payload::read::{self, ExtractBuffers, ExtractOptions, PlainExtract};
use inst_payload::write::{self, BlockPlan, InputFile, NoObserver, WriteOptions};
use inst_payload::{CodecParams, Payload, PayloadError};

struct Fixture {
    _dir: tempfile::TempDir,
    src: PathBuf,
    inputs: Vec<InputFile>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    let files: Vec<(&str, Vec<u8>, bool)> = vec![
        ("app.exe", (0..300_000u32).flat_map(|i| (i * 7).to_le_bytes()).collect(), true),
        ("lib/libfoo.so.1", b"\x7fELF fake library".repeat(5000), true),
        ("data/config.json", br#"{"key": "value"}"#.to_vec(), false),
        ("data/empty.txt", Vec::new(), false),
        ("docs/ملف عربي.txt", "مرحبا بالعالم".repeat(100).into_bytes(), false),
        ("docs/Türkçe.md", "Kurulum tamamlandı".repeat(50).into_bytes(), false),
    ];
    let mut inputs = Vec::new();
    for (rel, content, exec) in files {
        let path = src.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, &content).expect("write");
        inputs.push(InputFile {
            rel_path: rel.to_owned(),
            source: path,
            size: content.len() as u64,
            executable: exec,
            component: 0,
        });
    }
    Fixture {
        _dir: dir,
        src,
        inputs,
    }
}

fn build(fx: &Fixture, plans: &[BlockPlan], prefix: &[u8]) -> (Vec<u8>, write::WriteSummary) {
    let mut out = Vec::from(prefix);
    let opts = WriteOptions {
        threads: 3,
        temp_dir: fx.src.clone(),
        buffer_size: 64 << 10,
    };
    let summary = write::write_payload(
        &mut out,
        prefix.len() as u64,
        &fx.inputs,
        plans,
        &["logs".to_owned(), "plugins/empty".to_owned()],
        &opts,
        &NoObserver,
    )
    .expect("write payload");
    (out, summary)
}

fn extract_all(bytes: &[u8], root: &Path) -> Result<Payload, PayloadError> {
    let mut cursor = Cursor::new(bytes);
    let payload = Payload::locate(&mut cursor)?;
    let mut bufs = ExtractBuffers::default();
    read::extract_dirs(&payload, root, &mut PlainExtract, &mut bufs)?;
    for i in 0..payload.index.blocks.len() {
        let raw = payload.open_block(&mut cursor, i)?;
        read::extract_block(
            &payload,
            i,
            raw,
            root,
            &mut PlainExtract,
            &mut bufs,
            ExtractOptions::default(),
        )?;
    }
    Ok(payload)
}

fn assert_tree_matches(fx: &Fixture, root: &Path) {
    for input in &fx.inputs {
        let extracted = fs::read(root.join(&input.rel_path)).expect("extracted file");
        let original = fs::read(&input.source).expect("source file");
        assert_eq!(extracted, original, "{}", input.rel_path);
    }
    assert!(root.join("logs").is_dir());
    assert!(root.join("plugins/empty").is_dir());
}

fn plans(codecs: &[CodecParams], n: usize) -> Vec<BlockPlan> {
    // Round-robin files over the given codecs.
    let mut plans: Vec<BlockPlan> = codecs
        .iter()
        .map(|p| BlockPlan {
            params: *p,
            files: Vec::new(),
        })
        .collect();
    for i in 0..n {
        let k = i % plans.len();
        plans[k].files.push(i);
    }
    plans
}

#[test]
fn roundtrip_every_codec_and_block_layout() {
    let fx = fixture();
    let n = fx.inputs.len();
    let layouts: Vec<Vec<CodecParams>> = vec![
        vec![CodecParams::STORED],
        vec![CodecParams::zstd(3)],
        vec![CodecParams::xz(6, Filter::X86)],
        // Grouped-solid with mixed codecs.
        vec![CodecParams::zstd(1), CodecParams::xz(1, Filter::None), CodecParams::STORED],
    ];
    for layout in layouts {
        let (bytes, summary) = build(&fx, &plans(&layout, n), b"MZ-fake-runtime-prefix");
        assert_eq!(bytes.len() % 8, 0, "payload must end aligned");
        let root = tempfile::tempdir().expect("tempdir");
        let payload = extract_all(&bytes, root.path()).expect("extract");
        assert_eq!(payload.index, summary.index);
        assert_tree_matches(&fx, root.path());
        read::verify_blocks(&payload, &mut Cursor::new(&bytes), |_| {}).expect("verify");
    }
}

#[cfg(unix)]
#[test]
fn preserves_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture();
    let (bytes, _) = build(&fx, &plans(&[CodecParams::zstd(3)], fx.inputs.len()), b"");
    let root = tempfile::tempdir().expect("tempdir");
    extract_all(&bytes, root.path()).expect("extract");
    let mode = |p: &str| fs::metadata(root.path().join(p)).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode("app.exe"), 0o755);
    assert_eq!(mode("data/config.json"), 0o644);
}

#[test]
fn no_payload_is_reported_as_not_found() {
    let exe = b"\x7fELF just a runtime without payload".repeat(10);
    assert!(matches!(
        Payload::locate(&mut Cursor::new(&exe)),
        Err(PayloadError::NotFound)
    ));
}

#[test]
fn any_single_byte_corruption_is_detected_and_never_committed() {
    let fx = fixture();
    let n = fx.inputs.len();
    for layout in [vec![CodecParams::zstd(3)], vec![CodecParams::STORED], vec![CodecParams::xz(1, Filter::None)]] {
        let prefix = b"PREFIX";
        let (bytes, summary) = build(&fx, &plans(&layout, n), prefix);
        let payload_start = prefix.len();
        // Flip one byte at many positions across header, blocks, index, footer.
        let step = (bytes.len() - payload_start) / 97 + 1;
        let mut checked = 0;
        for pos in (payload_start..bytes.len()).step_by(step) {
            let mut corrupt = bytes.clone();
            corrupt[pos] ^= 0x5a;
            let root = tempfile::tempdir().expect("tempdir");
            match extract_all(&corrupt, root.path()) {
                Ok(_) => {
                    // The only acceptable success is when the flipped byte was
                    // alignment padding (not covered by any hash).
                    let padding_start = bytes.len() as u64 - 72 - summary.padding;
                    assert!(
                        (pos as u64) >= padding_start && (pos as u64) < bytes.len() as u64 - 72,
                        "corruption at {pos} went undetected"
                    );
                }
                Err(e) => {
                    assert!(
                        e.is_corruption() || matches!(e, PayloadError::Io(_)),
                        "unexpected error kind at {pos}: {e}"
                    );
                    // Whatever was committed must be byte-identical to the source.
                    for input in &fx.inputs {
                        let p = root.path().join(&input.rel_path);
                        if p.exists() {
                            assert_eq!(fs::read(&p).expect("read"), fs::read(&input.source).expect("read"));
                        }
                    }
                }
            }
            checked += 1;
        }
        assert!(checked > 50);
    }
}

#[test]
fn truncated_file_is_rejected() {
    let fx = fixture();
    let (bytes, _) = build(&fx, &plans(&[CodecParams::zstd(3)], fx.inputs.len()), b"");
    for cut in [1usize, 72, 100, bytes.len() / 2] {
        let truncated = &bytes[..bytes.len() - cut];
        assert!(Payload::locate(&mut Cursor::new(truncated)).is_err());
    }
}

#[test]
fn authenticode_style_trailer_is_skipped() {
    // Build a payload after a fake PE header, then append a fake certificate
    // table and point the security directory at it.
    let fx = fixture();
    let mut prefix = vec![0u8; 1024];
    prefix[..2].copy_from_slice(b"MZ");
    prefix[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    prefix[0x80..0x84].copy_from_slice(b"PE\0\0");
    prefix[0x80 + 20..0x80 + 22].copy_from_slice(&240u16.to_le_bytes());
    let opt = 0x80 + 24;
    prefix[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    prefix[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
    let (mut bytes, _) = build(&fx, &plans(&[CodecParams::zstd(3)], fx.inputs.len()), &prefix);
    let cert_offset = bytes.len() as u32;
    let cert = vec![0xabu8; 1000];
    bytes.extend_from_slice(&cert);
    let sec = opt + 112 + 32;
    bytes[sec..sec + 4].copy_from_slice(&cert_offset.to_le_bytes());
    bytes[sec + 4..sec + 8].copy_from_slice(&(cert.len() as u32).to_le_bytes());

    let root = tempfile::tempdir().expect("tempdir");
    extract_all(&bytes, root.path()).expect("extract signed");
    assert_tree_matches(&fx, root.path());
}

#[test]
fn rejects_unsafe_and_duplicate_paths_at_build_time() {
    let fx = fixture();
    let mut inputs = fx.inputs.clone();
    inputs[0].rel_path = "../escape.exe".to_owned();
    let plans = plans(&[CodecParams::STORED], inputs.len());
    let err = write::write_payload(
        &mut Vec::new(),
        0,
        &inputs,
        &plans,
        &[],
        &WriteOptions::default(),
        &NoObserver,
    )
    .expect_err("must reject traversal");
    assert!(matches!(err, PayloadError::UnsafePath(..)));

    let mut inputs = fx.inputs.clone();
    inputs[1].rel_path = "APP.EXE".to_owned();
    let err = write::write_payload(
        &mut Vec::new(),
        0,
        &inputs,
        &plans,
        &[],
        &WriteOptions::default(),
        &NoObserver,
    )
    .expect_err("must reject case-insensitive duplicate");
    assert!(matches!(err, PayloadError::DuplicatePath(..)));
}

#[test]
fn source_change_during_build_is_detected() {
    let fx = fixture();
    let mut inputs = fx.inputs.clone();
    inputs[2].size += 1;
    let err = write::write_payload(
        &mut Vec::new(),
        0,
        &inputs,
        &plans(&[CodecParams::zstd(1)], inputs.len()),
        &[],
        &WriteOptions::default(),
        &NoObserver,
    )
    .expect_err("size mismatch");
    assert!(matches!(err, PayloadError::SourceChanged(_)));
}

#[test]
fn existing_symlink_target_is_refused() {
    #[cfg(unix)]
    {
        let fx = fixture();
        let (bytes, _) = build(&fx, &plans(&[CodecParams::STORED], fx.inputs.len()), b"");
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let victim = outside.path().join("victim");
        File::create(&victim).expect("create").write_all(b"keep").expect("write");
        std::os::unix::fs::symlink(&victim, root.path().join("app.exe")).expect("symlink");
        let err = extract_all(&bytes, root.path()).expect_err("must refuse symlink");
        assert!(matches!(err, PayloadError::Io(_)), "{err}");
        assert_eq!(fs::read(&victim).expect("read"), b"keep");
    }
}

mod hostile {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn index_decoder_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = Index::decode(&bytes, u64::MAX);
        }

        #[test]
        fn locate_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let _ = Payload::locate(&mut Cursor::new(&bytes));
        }
    }

    #[test]
    fn mutated_real_index_never_panics() {
        let fx = fixture();
        let (bytes, summary) = build(&fx, &plans(&[CodecParams::STORED], fx.inputs.len()), b"");
        let mut encoded = Vec::new();
        summary.index.encode(&mut encoded);
        assert_eq!(Index::decode(&encoded, u64::MAX).expect("decode"), summary.index);
        for i in 0..encoded.len() {
            for mask in [0x01u8, 0x80, 0xff] {
                let mut m = encoded.clone();
                m[i] ^= mask;
                let _ = Index::decode(&m, bytes.len() as u64);
            }
        }
        let _ = Codec::Zstd;
    }
}
