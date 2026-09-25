//! Pluggable block codecs.
//!
//! Each codec is behind a cargo feature. A generated installer enables only
//! the decoders its payload actually uses; the Studio enables all encoders.

use std::io::{self, BufRead, Read};

use crate::format::{Codec, Filter};

/// Encoder parameters for one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecParams {
    pub codec: Codec,
    pub filter: Filter,
    /// Codec-specific level: zstd 1..=22, xz preset 0..=9.
    pub level: u8,
    /// xz "extreme" presets.
    pub extreme: bool,
    /// zstd long-distance matching window (log2), `None` = codec default.
    /// Must not exceed 27 so that default decoders accept the stream.
    pub window_log: Option<u8>,
    /// Encoder worker threads inside one block (0 = single-threaded).
    pub threads: u8,
}

impl CodecParams {
    pub const STORED: CodecParams = CodecParams {
        codec: Codec::None,
        filter: Filter::None,
        level: 0,
        extreme: false,
        window_log: None,
        threads: 0,
    };

    pub const fn zstd(level: u8) -> CodecParams {
        CodecParams {
            codec: Codec::Zstd,
            filter: Filter::None,
            level,
            extreme: false,
            window_log: None,
            threads: 0,
        }
    }

    pub const fn xz(preset: u8, filter: Filter) -> CodecParams {
        CodecParams {
            codec: Codec::Xz,
            filter,
            level: preset,
            extreme: false,
            window_log: None,
            threads: 0,
        }
    }
}

/// Maximum memory an xz decoder may use. LZMA2 preset 9 needs ~65 MiB.
pub const XZ_DECODER_MEMLIMIT: u64 = 1 << 30;

/// Largest zstd window accepted by decoders (the zstd default limit).
pub const ZSTD_MAX_WINDOW_LOG: u8 = 27;

fn unsupported(codec: Codec) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("codec '{}' is not compiled into this binary", codec.name()),
    )
}

/// Returns whether `codec` can be decoded by this binary.
pub const fn decoder_available(codec: Codec) -> bool {
    match codec {
        Codec::None => true,
        Codec::Zstd => cfg!(feature = "zstd"),
        Codec::Xz => cfg!(feature = "xz"),
    }
}

/// Wraps `input` (the compressed bytes of one block) in a streaming decoder.
pub fn decoder<'a, R: BufRead + 'a>(codec: Codec, input: R) -> io::Result<Box<dyn Read + 'a>> {
    match codec {
        Codec::None => Ok(Box::new(input)),
        #[cfg(feature = "zstd")]
        Codec::Zstd => {
            let mut dec = zstd::stream::read::Decoder::with_buffer(input)?;
            dec.window_log_max(u32::from(ZSTD_MAX_WINDOW_LOG))?;
            Ok(Box::new(dec.single_frame()))
        }
        #[cfg(feature = "xz")]
        Codec::Xz => {
            let stream = liblzma::stream::Stream::new_stream_decoder(XZ_DECODER_MEMLIMIT, 0)
                .map_err(io::Error::other)?;
            Ok(Box::new(liblzma::bufread::XzDecoder::new_stream(input, stream)))
        }
        #[allow(unreachable_patterns)]
        other => Err(unsupported(other)),
    }
}

#[cfg(feature = "write")]
pub use enc::{BlockEncoder, encoder};

#[cfg(feature = "write")]
mod enc {
    use super::*;
    use std::io::Write;

    /// A streaming block encoder. `finish` flushes the codec trailer.
    pub trait BlockEncoder<'a>: Write + 'a {
        fn finish(self: Box<Self>) -> io::Result<()>;
    }

    struct Stored<W>(W);

    impl<W: Write> Write for Stored<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl<'a, W: Write + 'a> BlockEncoder<'a> for Stored<W> {
        fn finish(mut self: Box<Self>) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[cfg(feature = "zstd")]
    impl<'a, W: Write + 'a> BlockEncoder<'a> for zstd::stream::write::Encoder<'a, W> {
        fn finish(self: Box<Self>) -> io::Result<()> {
            let mut w = zstd::stream::write::Encoder::finish(*self)?;
            w.flush()
        }
    }

    #[cfg(feature = "xz")]
    impl<'a, W: Write + 'a> BlockEncoder<'a> for liblzma::write::XzEncoder<W> {
        fn finish(self: Box<Self>) -> io::Result<()> {
            let mut w = liblzma::write::XzEncoder::finish(*self)?;
            w.flush()
        }
    }

    /// Creates a streaming encoder writing compressed bytes to `out`.
    pub fn encoder<'a, W: Write + 'a>(
        params: &CodecParams,
        out: W,
    ) -> io::Result<Box<dyn BlockEncoder<'a> + 'a>> {
        match params.codec {
            Codec::None => Ok(Box::new(Stored(out))),
            #[cfg(feature = "zstd")]
            Codec::Zstd => {
                let level = i32::from(params.level.clamp(1, 22));
                let mut enc = zstd::stream::write::Encoder::new(out, level)?;
                enc.include_checksum(false)?;
                enc.include_contentsize(false)?;
                if let Some(wlog) = params.window_log {
                    enc.long_distance_matching(true)?;
                    enc.window_log(u32::from(wlog.min(ZSTD_MAX_WINDOW_LOG)))?;
                }
                Ok(Box::new(enc))
            }
            #[cfg(feature = "xz")]
            Codec::Xz => {
                use liblzma::stream::{Check, Filters, LzmaOptions, PRESET_EXTREME, Stream};
                let mut preset = u32::from(params.level.min(9));
                if params.extreme {
                    preset |= PRESET_EXTREME;
                }
                let opts = LzmaOptions::new_preset(preset).map_err(io::Error::other)?;
                let mut filters = Filters::new();
                match params.filter {
                    Filter::None => {}
                    Filter::X86 => {
                        filters.x86();
                    }
                    Filter::Arm64 => {
                        filters.arm64();
                    }
                }
                filters.lzma2(&opts);
                let stream =
                    Stream::new_stream_encoder(&filters, Check::None).map_err(io::Error::other)?;
                Ok(Box::new(liblzma::write::XzEncoder::new_stream(out, stream)))
            }
            #[allow(unreachable_patterns)]
            other => Err(unsupported(other)),
        }
    }
}

#[cfg(all(test, feature = "write"))]
mod tests {
    use super::*;
    use std::io::Write;

    fn roundtrip(params: CodecParams, data: &[u8]) {
        let mut compressed = Vec::new();
        let mut enc = encoder(&params, &mut compressed).expect("encoder");
        enc.write_all(data).expect("write");
        enc.finish().expect("finish");
        let mut out = Vec::new();
        decoder(params.codec, &compressed[..])
            .expect("decoder")
            .read_to_end(&mut out)
            .expect("decode");
        assert_eq!(out, data, "{params:?}");
    }

    #[test]
    fn all_codecs_roundtrip() {
        let data: Vec<u8> = (0..200_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        roundtrip(CodecParams::STORED, &data);
        #[cfg(feature = "zstd")]
        {
            roundtrip(CodecParams::zstd(3), &data);
            roundtrip(
                CodecParams {
                    window_log: Some(24),
                    ..CodecParams::zstd(19)
                },
                &data,
            );
            roundtrip(CodecParams::zstd(3), &[]);
        }
        #[cfg(feature = "xz")]
        {
            roundtrip(CodecParams::xz(6, Filter::None), &data);
            roundtrip(CodecParams::xz(6, Filter::X86), &data);
            roundtrip(CodecParams::xz(1, Filter::Arm64), &data);
            roundtrip(CodecParams::xz(0, Filter::None), &[]);
        }
    }
}
