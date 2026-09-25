//! Compression analysis and block planning.
//!
//! `Auto` benchmarks candidate codecs on a representative sample of the
//! payload and measures compressed size, compression and decompression
//! throughput and encoder memory. The decoder's contribution to the
//! installer binary is included in the decision. Nothing is assumed.

use std::io::{Read, Write};
use std::time::Instant;

use inst_model::project::CompressionProfile;
use inst_payload::format::{Codec, Filter};
use inst_payload::{BlockPlan, CodecParams};

use crate::scan::{BinaryKind, ScannedFile};

/// Approximate size a decoder adds to a size-optimized, LTO-linked
/// installer. Measured on this runtime; refined by size analysis builds.
pub const fn decoder_cost(codec: Codec) -> u64 {
    match codec {
        Codec::None => 0,
        Codec::Zstd => 95 << 10,
        Codec::Xz => 75 << 10,
    }
}

/// Encoder working memory estimate per thread.
pub fn encoder_memory(p: &CodecParams) -> u64 {
    match p.codec {
        Codec::None => 1 << 20,
        Codec::Zstd => {
            let wlog = u32::from(p.window_log.unwrap_or(match p.level {
                0..=5 => 21,
                6..=15 => 23,
                _ => 25,
            }));
            (1u64 << wlog) * 6
        }
        // xz presets 0..9: dictionary 256 KiB .. 64 MiB, ~11x dict to encode.
        Codec::Xz => {
            let dict: u64 = [
                256 << 10,
                1 << 20,
                2 << 20,
                4 << 20,
                4 << 20,
                8 << 20,
                8 << 20,
                16 << 20,
                32 << 20,
                64 << 20,
            ][usize::from(p.level.min(9))];
            dict * 11 + (16 << 20)
        }
    }
}

#[derive(Clone, Debug)]
pub struct CandidateResult {
    pub name: &'static str,
    pub params: CodecParams,
    pub sample_bytes: u64,
    pub compressed_bytes: u64,
    pub encode_mib_s: f64,
    pub decode_mib_s: f64,
    pub encoder_memory: u64,
    /// Estimated full payload size + decoder cost.
    pub estimated_total: u64,
}

impl CandidateResult {
    pub fn ratio(&self) -> f64 {
        if self.sample_bytes == 0 {
            1.0
        } else {
            self.compressed_bytes as f64 / self.sample_bytes as f64
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompressionDecision {
    pub profile: CompressionProfile,
    pub params: CodecParams,
    /// Separate parameters for executable blocks (BCJ filter), if any.
    pub exe_params: Option<CodecParams>,
    pub block_size: u64,
    pub candidates: Vec<CandidateResult>,
    pub reason: String,
}

fn candidates(filter: Filter) -> Vec<(&'static str, CodecParams)> {
    vec![
        ("zstd-3", CodecParams::zstd(3)),
        (
            "zstd-19-long",
            CodecParams {
                window_log: Some(26),
                ..CodecParams::zstd(19)
            },
        ),
        ("xz-6", CodecParams::xz(6, Filter::None)),
        (
            "xz-9e+bcj",
            CodecParams {
                extreme: true,
                ..CodecParams::xz(9, filter)
            },
        ),
    ]
}

/// Builds a sample of at most `limit` bytes: slices spread over all files
/// (weighted by size), so large and small files are represented.
fn sample(files: &[ScannedFile], limit: u64) -> std::io::Result<Vec<u8>> {
    let total: u64 = files.iter().map(|f| f.size).sum();
    if total <= limit {
        let mut out = Vec::with_capacity(total as usize);
        for f in files {
            std::fs::File::open(&f.abs)?.read_to_end(&mut out)?;
        }
        return Ok(out);
    }
    let mut out = Vec::with_capacity(limit as usize);
    let chunk = 256u64 << 10;
    // Take `chunk`-sized slices at a stride across the concatenated payload.
    let stride = (total / (limit / chunk).max(1)).max(chunk);
    let mut next = 0u64;
    let mut offset = 0u64;
    for f in files {
        let end = offset + f.size;
        while next < end && (out.len() as u64) < limit {
            let within = next - offset;
            let take = chunk.min(f.size - within) as usize;
            let mut file = std::fs::File::open(&f.abs)?;
            std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(within))?;
            let start = out.len();
            out.resize(start + take, 0);
            let n = file.read(&mut out[start..])?;
            out.truncate(start + n);
            next += stride;
        }
        offset = end;
    }
    Ok(out)
}

fn measure(
    name: &'static str,
    params: CodecParams,
    data: &[u8],
    total: u64,
) -> std::io::Result<CandidateResult> {
    let started = Instant::now();
    let mut compressed = Vec::with_capacity(data.len() / 2);
    let mut enc = inst_payload::codec::encoder(&params, &mut compressed)?;
    enc.write_all(data)?;
    enc.finish()?;
    let enc_time = started.elapsed().as_secs_f64().max(1e-6);
    let started = Instant::now();
    let mut dec = inst_payload::codec::decoder(params.codec, &compressed[..])?;
    let mut sink = [0u8; 64 << 10];
    let mut n = 0u64;
    loop {
        let r = dec.read(&mut sink)?;
        if r == 0 {
            break;
        }
        n += r as u64;
    }
    let dec_time = started.elapsed().as_secs_f64().max(1e-6);
    debug_assert_eq!(n, data.len() as u64);
    let mib = data.len() as f64 / (1024.0 * 1024.0);
    let ratio = if data.is_empty() {
        1.0
    } else {
        compressed.len() as f64 / data.len() as f64
    };
    Ok(CandidateResult {
        name,
        params,
        sample_bytes: data.len() as u64,
        compressed_bytes: compressed.len() as u64,
        encode_mib_s: mib / enc_time,
        decode_mib_s: mib / dec_time,
        encoder_memory: encoder_memory(&params),
        estimated_total: (ratio * total as f64) as u64 + decoder_cost(params.codec),
    })
}

pub fn filter_for(arch: inst_model::Arch) -> Filter {
    match arch {
        inst_model::Arch::X64 => Filter::X86,
        inst_model::Arch::Arm64 => Filter::Arm64,
    }
}

/// Chooses codec parameters for the payload.
pub fn decide(
    profile: CompressionProfile,
    files: &[ScannedFile],
    arch: inst_model::Arch,
    block_size_mib: u32,
) -> std::io::Result<CompressionDecision> {
    let total: u64 = files.iter().map(|f| f.size).sum();
    let filter = filter_for(arch);
    let explicit_block = u64::from(block_size_mib) << 20;
    let pick_block = |default: u64| {
        if explicit_block > 0 {
            explicit_block
        } else {
            default
        }
    };
    let fixed = |params: CodecParams, exe: Option<CodecParams>, block: u64, reason: &str| {
        CompressionDecision {
            profile,
            params,
            exe_params: exe,
            block_size: pick_block(block),
            candidates: Vec::new(),
            reason: reason.to_owned(),
        }
    };
    Ok(match profile {
        CompressionProfile::None => {
            fixed(CodecParams::STORED, None, 256 << 20, "compression disabled")
        }
        CompressionProfile::FastInstall => fixed(
            CodecParams::zstd(3),
            None,
            16 << 20,
            "fastest decompression, parallel blocks",
        ),
        CompressionProfile::Balanced => fixed(
            CodecParams {
                window_log: Some(25),
                ..CodecParams::zstd(15)
            },
            None,
            64 << 20,
            "good ratio with fast decompression",
        ),
        CompressionProfile::SmallestSize => {
            let xz = CodecParams {
                extreme: true,
                ..CodecParams::xz(9, Filter::None)
            };
            fixed(
                xz,
                Some(CodecParams { filter, ..xz }),
                1 << 30,
                "maximum ratio, solid blocks",
            )
        }
        CompressionProfile::Auto => {
            if total < (1 << 20) {
                return Ok(fixed(
                    CodecParams::zstd(19),
                    None,
                    64 << 20,
                    "payload under 1 MiB: benchmarking would cost more than it saves",
                ));
            }
            let data = sample(files, 32 << 20)?;
            let results: Vec<CandidateResult> = candidates(filter)
                .into_iter()
                .map(|(n, p)| measure(n, p, &data, total))
                .collect::<Result<_, _>>()?;
            let best_size = results
                .iter()
                .min_by_key(|r| r.estimated_total)
                .expect("candidates");
            let zstd_best = results
                .iter()
                .filter(|r| r.params.codec == Codec::Zstd)
                .min_by_key(|r| r.estimated_total)
                .expect("zstd candidate");
            // Prefer zstd unless xz is meaningfully smaller: zstd decodes
            // several times faster, which shortens every installation.
            let gain = 1.0 - best_size.estimated_total as f64 / zstd_best.estimated_total as f64;
            let (chosen, reason) = if best_size.params.codec == Codec::Xz && gain >= 0.05 {
                (
                    best_size,
                    format!(
                        "{} is {:.1}% smaller than the best zstd candidate",
                        best_size.name,
                        gain * 100.0
                    ),
                )
            } else {
                (
                    zstd_best,
                    format!(
                        "{} chosen: within {:.1}% of the smallest candidate and {:.0}x faster to decompress",
                        zstd_best.name,
                        gain.max(0.0) * 100.0,
                        zstd_best.decode_mib_s / best_size.decode_mib_s.max(1e-6)
                    ),
                )
            };
            let params = CodecParams {
                filter: Filter::None,
                ..chosen.params
            };
            let exe_params =
                (params.codec == Codec::Xz).then_some(CodecParams { filter, ..params });
            let block = if params.codec == Codec::Xz {
                256 << 20
            } else {
                64 << 20
            };
            CompressionDecision {
                profile,
                params,
                exe_params,
                block_size: pick_block(block),
                candidates: results,
                reason,
            }
        }
    })
}

/// Groups files into blocks: executables separately when a BCJ filter
/// applies, files sorted by extension then path for locality, blocks cut
/// at the target size. Indices refer to `files`.
pub fn plan_blocks(
    files: &[ScannedFile],
    d: &CompressionDecision,
    min_blocks: usize,
) -> Vec<BlockPlan> {
    let mut groups: Vec<(CodecParams, Vec<usize>)> = Vec::new();
    let (exe, other): (Vec<usize>, Vec<usize>) = (0..files.len()).partition(|&i| {
        d.exe_params.is_some() && matches!(files[i].binary, Some(BinaryKind::Elf | BinaryKind::Pe))
    });
    if let Some(p) = d.exe_params
        && !exe.is_empty()
    {
        groups.push((p, exe));
    }
    groups.push((d.params, other));

    let total: u64 = files.iter().map(|f| f.size).sum();
    // Enough blocks for parallel extraction on multi-core machines.
    let block_size = if min_blocks > 1 && total / d.block_size < min_blocks as u64 {
        (total / min_blocks as u64).max(8 << 20).min(d.block_size)
    } else {
        d.block_size
    };
    let mut plans = Vec::new();
    for (params, mut idx) in groups {
        if idx.is_empty() {
            continue;
        }
        idx.sort_by(|&a, &b| {
            let ext = |i: usize| {
                files[i]
                    .rel
                    .rsplit_once('.')
                    .map_or("", |(_, e)| e)
                    .to_ascii_lowercase()
            };
            ext(a)
                .cmp(&ext(b))
                .then_with(|| files[a].rel.cmp(&files[b].rel))
        });
        let mut current = Vec::new();
        let mut size = 0u64;
        for i in idx {
            if size > 0 && size + files[i].size > block_size {
                plans.push(BlockPlan {
                    params,
                    files: std::mem::take(&mut current),
                });
                size = 0;
            }
            size += files[i].size;
            current.push(i);
        }
        if !current.is_empty() {
            plans.push(BlockPlan {
                params,
                files: current,
            });
        }
    }
    plans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(rel: &str, size: u64, binary: Option<BinaryKind>) -> ScannedFile {
        ScannedFile {
            rel: rel.into(),
            abs: PathBuf::new(),
            size,
            binary,
            executable: binary.is_some(),
        }
    }

    #[test]
    fn plans_cover_every_file_once_and_respect_size() {
        let files: Vec<ScannedFile> = (0..50)
            .map(|i| {
                file(
                    &format!("f{i}.{}", if i % 2 == 0 { "txt" } else { "dll" }),
                    3 << 20,
                    (i % 5 == 0).then_some(BinaryKind::Pe),
                )
            })
            .collect();
        let d = CompressionDecision {
            profile: CompressionProfile::SmallestSize,
            params: CodecParams::xz(9, Filter::None),
            exe_params: Some(CodecParams::xz(9, Filter::X86)),
            block_size: 16 << 20,
            candidates: vec![],
            reason: String::new(),
        };
        let plans = plan_blocks(&files, &d, 1);
        let mut seen: Vec<usize> = plans.iter().flat_map(|p| p.files.clone()).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
        for p in &plans {
            let size: u64 = p.files.iter().map(|&i| files[i].size).sum();
            assert!(size <= 16 << 20 || p.files.len() == 1);
            let exes = p
                .files
                .iter()
                .filter(|&&i| files[i].binary.is_some())
                .count();
            assert!(
                exes == 0 || exes == p.files.len(),
                "executables are grouped separately"
            );
            if exes > 0 {
                assert_eq!(p.params.filter, Filter::X86);
            }
        }
    }

    #[test]
    fn auto_benchmarks_real_data() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut files = Vec::new();
        for i in 0..8 {
            let p = tmp.path().join(format!("f{i}.txt"));
            let content: Vec<u8> = (0..300_000u32)
                .flat_map(|x| format!("line {} {}\n", x % 977, i).into_bytes())
                .take(400_000)
                .collect();
            std::fs::write(&p, &content).expect("write");
            files.push(ScannedFile {
                rel: format!("f{i}.txt"),
                abs: p,
                size: content.len() as u64,
                binary: None,
                executable: false,
            });
        }
        let d = decide(CompressionProfile::Auto, &files, inst_model::Arch::X64, 0).expect("decide");
        assert_eq!(d.candidates.len(), 4);
        assert!(
            d.candidates.iter().all(|c| c.ratio() < 0.5),
            "{:?}",
            d.candidates
        );
        assert!(!d.reason.is_empty());
    }
}
