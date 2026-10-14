//! HEIF / HEIC decoder backed by the pure-Rust `heic-rs` crate.
//!
//! `heic-rs` decodes the HEVC still pictures an iPhone or macOS writes,
//! including grid tiles and the `clap` / `irot` / `imir` transforms.
//! EXIF orientation is left to the loader, which bakes it in when the
//! caller asks for it.

use heic_rs::context::Context;
use heic_rs::{DecodeOptions as HeicOptions, PixelLayout};

use crate::{Error, Frame, Image, MemoryFormat, Result, Texture};

use super::DecodeOptions;

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Image> {
    let ctx = Context::open(bytes).map_err(map_err)?;
    let primary = ctx.meta.primary;
    let props = ctx.props(primary).map_err(map_err)?;

    let (coded_width, coded_height) = ctx.coded_size(primary, &props).map_err(map_err)?;
    let (width, height) =
        heic_rs::transform::transformed_size(coded_width, coded_height, &props.transforms)
            .map_err(map_err)?;
    opts.limits.check_dimensions(width, height, 1)?;

    let has_alpha = ctx.alpha_item(primary).map_err(map_err)?.is_some();
    let layout = if has_alpha {
        PixelLayout::Rgba8
    } else {
        PixelLayout::Rgb8
    };
    let heic_opts = HeicOptions::default()
        .with_layout(layout)
        .with_max_pixels(Some(opts.limits.max_pixels))
        .with_alpha(has_alpha)
        // The container's `clap`, `irot` and `imir` are structural and
        // always apply; EXIF orientation is not baked in here, the loader
        // applies it through `metadata::apply_orientation_if_present`.
        .with_transforms(true);

    let decoded = heic_rs::decode(bytes, &heic_opts).map_err(map_err)?;
    let format = if has_alpha {
        MemoryFormat::R8g8b8a8
    } else {
        MemoryFormat::R8g8b8
    };
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
    if let Some(profile) = ctx.icc(&props) {
        image.set_icc_profile(profile.to_vec());
    }
    // A file whose EXIF item is split across extents cannot be borrowed
    // by `heic-rs`; metadata is optional, so drop it rather than fail
    // the whole decode.
    if let Ok(Some(exif)) = ctx.exif(primary) {
        image.set_exif(exif.to_vec());
    }
    // Report CICP only when the file carries an `nclx`, matching the other
    // decoders. `heic-rs` still falls back to BT.709 limited for its own
    // YCbCr conversion when the file has no `colr`.
    if let Some(nclx) = props.nclx {
        image.set_cicp([
            nclx.primaries.min(u16::from(u8::MAX)) as u8,
            nclx.transfer.min(u16::from(u8::MAX)) as u8,
            nclx.matrix_code.min(u16::from(u8::MAX)) as u8,
            matches!(nclx.range, heic_rs::Range::Full) as u8,
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
