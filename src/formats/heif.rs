//! HEIF / HEIC decoder backed by the pure-Rust `heic-rs` crate.
//!
//! `heic-rs` decodes the HEVC still pictures an iPhone or macOS writes,
//! including grid tiles and the `clap` / `irot` / `imir` transforms.
//! Those container transforms are authoritative in HEIF, so the loader
//! does not apply the EXIF orientation on top of them.

use heic_rs::context::Context;
use heic_rs::meta::iref::RefKind;
use heic_rs::{ChromaFormat, DecodeOptions as HeicOptions, PixelLayout};

use crate::{Error, Frame, Image, MemoryFormat, Result, Texture};

use super::DecodeOptions;

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Image> {
    let info = heic_rs::probe(bytes).map_err(map_err)?;
    opts.limits.check_dimensions(info.width, info.height, 1)?;
    // heic-rs 0.1.4 misreads 4:2:2 chroma and still reports success.
    if info.chroma == ChromaFormat::Yuv422 {
        return Err(Error::Decoder {
            format: "heif",
            message: "4:2:2 chroma is not supported".into(),
        });
    }

    let ctx = Context::open(bytes).map_err(map_err)?;
    let primary = info.primary_item;
    let props = ctx.props(primary).map_err(map_err)?;
    let alpha = ctx.alpha_item(primary).map_err(map_err)?;
    let premultiplied = alpha.is_some_and(|alpha| {
        ctx.meta.refs.iter().any(|r| {
            r.kind == RefKind::Other(*b"prem") && r.from == primary && r.to.contains(&alpha)
        })
    });

    let (layout, format) = match (info.bit_depth > 8, alpha.is_some(), premultiplied) {
        (false, false, _) => (PixelLayout::Rgb8, MemoryFormat::R8g8b8),
        (false, true, false) => (PixelLayout::Rgba8, MemoryFormat::R8g8b8a8),
        (false, true, true) => (PixelLayout::Rgba8, MemoryFormat::R8g8b8a8Premultiplied),
        (true, false, _) => (PixelLayout::Rgb16, MemoryFormat::R16g16b16),
        (true, true, false) => (PixelLayout::Rgba16, MemoryFormat::R16g16b16a16),
        (true, true, true) => (PixelLayout::Rgba16, MemoryFormat::R16g16b16a16Premultiplied),
    };
    // A private pool spawned here inherits the worker's sandbox; rayon's
    // global pool may already belong to the host and run unsandboxed.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let heic_opts = HeicOptions::default()
        .with_layout(layout)
        .with_max_pixels(Some(opts.limits.max_pixels))
        .with_alpha(alpha.is_some())
        .with_transforms(true)
        .with_threads(Some(threads));

    let decoded = heic_rs::decode(bytes, &heic_opts).map_err(map_err)?;
    let (width, height) = (decoded.width, decoded.height);
    opts.limits.check_dimensions(width, height, 1)?;
    let stride = (format.bytes_per_pixel() as u64)
        .checked_mul(width as u64)
        .filter(|s| *s <= u32::MAX as u64)
        .ok_or(Error::LimitExceeded("stride"))? as u32;

    let texture = Texture::from_parts(
        width,
        height,
        stride,
        format,
        decoded.data.into_boxed_slice(),
    )
    .ok_or_else(|| Error::Decoder {
        format: "heif",
        message: "texture construction failed".into(),
    })?;

    let mut image = Image::from_parts("heif", width, height, vec![Frame::new(texture, None)]);
    // A file whose EXIF item is split across extents cannot be borrowed
    // by `heic-rs`; metadata is optional, so drop it rather than fail
    // the whole decode.
    if let Ok(Some(exif)) = ctx.exif(primary) {
        image.set_exif(exif.to_vec());
    }
    // HEIF prefers an ICC profile over `nclx`. The pixels are already
    // full-range RGB, so the matrix and range describe the output, not
    // the coded YCbCr.
    if let Some(profile) = ctx.icc(&props) {
        image.set_icc_profile(profile.to_vec());
    } else if let Some(nclx) = props.nclx {
        image.set_cicp([
            nclx.primaries.min(u16::from(u8::MAX)) as u8,
            nclx.transfer.min(u16::from(u8::MAX)) as u8,
            0,
            1,
        ]);
    }

    let _ = (opts.apply_transformations, opts.render_size_hint);
    Ok(image)
}

fn map_err(e: heic_rs::Error) -> Error {
    use heic_rs::Error as E;
    match e {
        E::Truncated(what) => Error::Truncated(what),
        E::Malformed(what) => Error::Malformed(what.to_string()),
        E::PixelLimit { .. } => Error::LimitExceeded("max_pixels"),
        E::Io(kind) => Error::Io(std::io::Error::from(kind)),
        other => Error::Decoder {
            format: "heif",
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;

    fn opts() -> DecodeOptions {
        DecodeOptions {
            limits: Limits::default(),
            apply_transformations: true,
            render_size_hint: None,
        }
    }

    #[test]
    fn rejects_garbage() {
        let err = decode(b"\0\0\0\x18ftypheicgarbage", &opts()).unwrap_err();
        assert!(matches!(
            err,
            Error::Malformed(_) | Error::Truncated(_) | Error::Decoder { .. }
        ));
    }

    #[test]
    fn rejects_empty() {
        let err = decode(b"", &opts()).unwrap_err();
        assert!(matches!(
            err,
            Error::Malformed(_) | Error::Truncated(_) | Error::Decoder { .. }
        ));
    }
}
