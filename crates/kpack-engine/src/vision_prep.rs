//! `vision_prep` — the bytes a photo arrives as, to the RGB8 the image encoder
//! takes. Pure Rust and outside the `vision` gate so the desktop suite tests it:
//! a hostile or merely huge file from the webview must be refused before any
//! decoder sees it, and the EXIF-orientation table is exactly the kind of thing
//! a device run would not reveal to be wrong (a rotated photo "works", upside
//! down).
//!
//! Order: byte cap → dimensions from the header (no decode) → pixel cap →
//! decode → EXIF orientation 1–8 → downscale so the longest edge is `max_side`
//! → RGB8. Downscaling to 512 is what keeps the document models' own
//! preprocessor at one encoder pass (Stage 0, `verification-vision-spike.md`).

use std::io::Cursor;

use crate::error::EngineError;

/// Encoded size cap. A phone photo is 2–6 MB; a screenshot under 1 MB.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Decoded size cap, read from the container header before decoding. 50 MP is
/// above any phone camera's still; a 50 MP RGB8 buffer is 150 MB, which the
/// floor device can hold once but not twice.
pub const MAX_IMAGE_PIXELS: u64 = 50_000_000;

/// The encoder's input: tightly packed RGB8, already upright and downscaled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImage {
    pub width: u32,
    pub height: u32,
    /// The EXIF orientation that was applied (1 when absent or unreadable).
    pub orientation: u32,
    pub rgb: Vec<u8>,
}

fn image_err(what: impl std::fmt::Display) -> EngineError {
    EngineError::Backend(format!("image: {what}"))
}

/// Decode, apply the EXIF orientation, downscale so the longest edge is
/// `max_side` (0 = keep the size), return RGB8. Fails closed on either cap.
pub fn prepare_image(bytes: &[u8], max_side: u32) -> Result<PreparedImage, EngineError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(image_err(format!(
            "too large: {} bytes, the cap is {MAX_IMAGE_BYTES}",
            bytes.len()
        )));
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image_err)?;
    let (w, h) = reader.into_dimensions().map_err(image_err)?;
    if w == 0 || h == 0 {
        return Err(image_err("empty image (0 pixels on a side)"));
    }
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return Err(image_err(format!(
            "too many pixels: {w}x{h}, the cap is {MAX_IMAGE_PIXELS}"
        )));
    }

    let mut img = image::load_from_memory(bytes).map_err(image_err)?;
    let orientation = exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .ok()
        .and_then(|e| {
            e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        })
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1);
    img = match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    };
    if max_side > 0 {
        let (w, h) = (img.width(), img.height());
        let longest = w.max(h);
        if longest > max_side {
            let scale = f64::from(max_side) / f64::from(longest);
            let nw = ((f64::from(w) * scale).round() as u32).max(1);
            let nh = ((f64::from(h) * scale).round() as u32).max(1);
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
        }
    }
    let rgb = img.to_rgb8();
    Ok(PreparedImage {
        width: rgb.width(),
        height: rgb.height(),
        orientation,
        rgb: rgb.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = include_bytes!("../tests/fixtures/vision/plain-64x32.png");
    const EXIF6: &[u8] = include_bytes!("../tests/fixtures/vision/exif6-16x8.jpg");

    #[test]
    fn decodes_without_resizing_when_within_max_side() {
        let p = prepare_image(PLAIN, 512).unwrap();
        assert_eq!((p.width, p.height, p.orientation), (64, 32, 1));
        assert_eq!(p.rgb.len(), 64 * 32 * 3);
        // pixel (3,2) is green; (0,0) red; (63,31) blue — exact, no resampling happened
        let px = |x: usize, y: usize| &p.rgb[(y * 64 + x) * 3..(y * 64 + x) * 3 + 3];
        assert_eq!(px(3, 2), &[0, 255, 0]);
        assert_eq!(px(0, 0), &[255, 0, 0]);
        assert_eq!(px(63, 31), &[0, 0, 255]);
    }

    #[test]
    fn downscales_the_longest_edge_and_keeps_the_aspect() {
        let p = prepare_image(PLAIN, 16).unwrap();
        assert_eq!((p.width, p.height), (16, 8));
        assert_eq!(p.rgb.len(), 16 * 8 * 3);
        // Left half still red-dominant, right half blue-dominant after resampling.
        let px = |x: usize, y: usize| &p.rgb[(y * 16 + x) * 3..(y * 16 + x) * 3 + 3];
        assert!(px(1, 4)[0] > 200 && px(1, 4)[2] < 60, "left is red: {:?}", px(1, 4));
        assert!(px(14, 4)[2] > 200 && px(14, 4)[0] < 60, "right is blue: {:?}", px(14, 4));
    }

    #[test]
    fn max_side_zero_means_keep() {
        let p = prepare_image(PLAIN, 0).unwrap();
        assert_eq!((p.width, p.height), (64, 32));
    }

    #[test]
    fn exif_orientation_6_is_rotated_upright() {
        let p = prepare_image(EXIF6, 512).unwrap();
        assert_eq!(p.orientation, 6);
        // Stored 16x8 with the LEFT half black; orientation 6 = rotate 90° CW,
        // so the display is 8x16 with the TOP half black.
        assert_eq!((p.width, p.height), (8, 16));
        let px = |x: usize, y: usize| &p.rgb[(y * 8 + x) * 3..(y * 8 + x) * 3 + 3];
        assert!(px(4, 2)[0] < 40, "top is black: {:?}", px(4, 2));
        assert!(px(4, 13)[0] > 215, "bottom is white: {:?}", px(4, 13));
    }

    #[test]
    fn refuses_more_than_the_byte_cap_before_decoding() {
        let big = vec![0u8; MAX_IMAGE_BYTES + 1];
        let err = prepare_image(&big, 512).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn refuses_more_than_the_pixel_cap_before_decoding() {
        // A PNG header claiming 10000x10000: dimensions are read without decoding.
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&10_000u32.to_be_bytes());
        png.extend_from_slice(&10_000u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&[0x35, 0x2c, 0xf5, 0x70]); // real IHDR crc: the png reader checks it
        // An empty IDAT: the png reader scans up to the first IDAT before it reports dimensions.
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0x35, 0xaf, 0x06, 0x1e]);
        let err = prepare_image(&png, 512).unwrap_err().to_string();
        assert!(err.contains("too many pixels"), "{err}");
    }

    #[test]
    fn a_zero_sized_image_is_refused_as_empty() {
        // (edge case E1) A header claiming 0x0 must not reach the encoder.
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&[0xb4, 0xe9, 0xeb, 0x45]); // real IHDR crc
        // An empty IDAT: the png reader scans up to the first IDAT before it reports dimensions.
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0x35, 0xaf, 0x06, 0x1e]);
        let err = prepare_image(&png, 512).unwrap_err().to_string();
        assert!(err.starts_with("engine backend error: image:"), "{err}");
    }

    #[test]
    fn garbage_is_an_image_error_not_a_panic() {
        let err = prepare_image(b"not an image", 512).unwrap_err().to_string();
        assert!(err.starts_with("engine backend error: image:"), "{err}");
    }
}
