//! Stand-in for the `zstd` crate (C bindings) with only what `parquet` uses, so Zstandard
//! pages can be read on wasm32-unknown-unknown without a C toolchain or imports.
//! Both directions use ruzstd: levels 1-2 compress like zstd level 1 ("Fastest"), level 3 and
//! up like zstd level 3 ("Default"), the level writers use when none is given.

use std::io::{self, Read, Write};

pub mod zstd_safe {
    /// The frame header's content size is optional; parquet falls back to the page's
    /// uncompressed size, which Parquet files always record.
    pub fn get_frame_content_size(_src: &[u8]) -> Result<Option<u64>, usize> {
        Ok(None)
    }

    /// Worst-case compressed size (same formula as libzstd's ZSTD_COMPRESSBOUND).
    pub fn compress_bound(n: usize) -> usize {
        n + (n >> 8) + if n < (128 << 10) { ((128 << 10) - n) >> 11 } else { 0 }
    }
}

pub mod bulk {
    use super::*;

    pub struct Decompressor<'a>(std::marker::PhantomData<&'a ()>);

    impl Decompressor<'_> {
        pub fn new() -> io::Result<Self> {
            Ok(Self(std::marker::PhantomData))
        }

        /// Decodes every frame in `src` and writes the bytes to `dst`.
        pub fn decompress_to_buffer<W: Write + ?Sized>(&mut self, src: &[u8], dst: &mut W) -> io::Result<usize> {
            let mut rest = src;
            let mut out = Vec::new();
            while !rest.is_empty() {
                let mut dec = ruzstd::decoding::StreamingDecoder::new(&mut rest)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("zstd: {e}")))?;
                dec.read_to_end(&mut out)?;
            }
            dst.write_all(&out)?;
            Ok(out.len())
        }
    }

    pub struct Compressor<'a>(i32, std::marker::PhantomData<&'a ()>);

    impl Compressor<'_> {
        pub fn new(level: i32) -> io::Result<Self> {
            Ok(Self(level, std::marker::PhantomData))
        }

        /// Compresses `src` into one Zstandard frame and writes it to `dst`.
        pub fn compress_to_buffer<W: Write + ?Sized>(&mut self, src: &[u8], dst: &mut W) -> io::Result<usize> {
            use ruzstd::encoding::CompressionLevel;
            let level = if self.0 >= 3 { CompressionLevel::Default } else { CompressionLevel::Fastest };
            let out = ruzstd::encoding::compress_to_vec(src, level);
            dst.write_all(&out)?;
            Ok(out.len())
        }
    }
}
