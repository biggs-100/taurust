//! Bounded image detection and normalization for coding-tool attachments.
//!
//! Ported from tau's `tau_coding/image_processing.py`. Detection reads magic
//! bytes only; normalization decodes, resizes and re-encodes within
//! deterministic limits so a provider-safe attachment always fits.
//!
//! Pillow's role is played by the `image` crate: `Image.open` and `load` become
//! an [`ImageReader`] dimensions probe plus a full decode, `Image.verify` becomes
//! the decoder's own header validation, and `Image.Resampling.LANCZOS` becomes
//! [`FilterType::Lanczos3`].

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageFormat, ImageReader};

/// The inline attachment ceiling.
pub const DEFAULT_MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// The largest source file the module will even look at.
pub const DEFAULT_MAX_SOURCE_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// The largest width or height an attachment may keep.
pub const DEFAULT_MAX_IMAGE_DIMENSION: u32 = 2_000;

/// The largest source pixel count the module will decode.
pub const DEFAULT_MAX_SOURCE_PIXELS: u64 = 40_000_000;

/// How many times the encoder may shrink the image before giving up.
pub const MAX_RESIZE_ATTEMPTS: usize = 12;

/// The PNG file signature.
pub const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The reference's `PngKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PngKind {
    Static,
    Animated,
    Invalid,
}

/// A provider-safe encoded image and the transformations applied to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedImage {
    /// The encoded bytes to attach.
    pub data: Vec<u8>,
    /// The media type of `data`.
    pub mime_type: String,
    /// The width of the encoded image.
    pub width: u32,
    /// The height of the encoded image.
    pub height: u32,
    /// Human-readable notes about the transformations applied.
    pub notes: Vec<String>,
}

/// The safe user-facing reason an image attachment was omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProcessingFailure {
    /// The reason, quoted verbatim into the tool result.
    pub message: String,
}

/// The limits one normalization pass runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    /// The inline attachment ceiling.
    pub max_bytes: usize,
    /// The largest width or height an attachment may keep.
    pub max_dimension: u32,
    /// The largest source file to look at.
    pub max_source_bytes: usize,
    /// The largest source pixel count to decode.
    pub max_source_pixels: u64,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_dimension: DEFAULT_MAX_IMAGE_DIMENSION,
            max_source_bytes: DEFAULT_MAX_SOURCE_IMAGE_BYTES,
            max_source_pixels: DEFAULT_MAX_SOURCE_PIXELS,
        }
    }
}

/// Identify a known image family from the minimum available magic bytes.
pub fn detect_image_family_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xff\xd8\xff\xf7") {
        return Some("image/jxl");
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if data.starts_with(PNG_SIGNATURE) {
        return Some("image/png");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if data.starts_with(b"BM") {
        return Some("image/bmp");
    }
    None
}

/// Explain a recognized image variant Tau intentionally cannot attach.
pub fn unsupported_image_reason(data: &[u8]) -> Option<String> {
    if data.starts_with(b"\xff\xd8\xff\xf7") {
        return Some("JPEG XL images are not supported".to_string());
    }
    if data.starts_with(PNG_SIGNATURE) && classify_png(data) == PngKind::Animated {
        return Some("animated PNG images are not supported".to_string());
    }
    None
}

/// Detect a supported image from its bytes while rejecting unsafe variants.
pub fn detect_supported_image_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xff\xd8\xff") {
        return if data.starts_with(b"\xff\xd8\xff\xf7") {
            None
        } else {
            Some("image/jpeg")
        };
    }
    if data.starts_with(PNG_SIGNATURE) {
        return if classify_png(data) == PngKind::Static {
            Some("image/png")
        } else {
            None
        };
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.starts_with(b"RIFF") && data.len() >= 16 && &data[8..12] == b"WEBP" {
        let variant = &data[12..16];
        if variant == b"VP8 ".as_slice()
            || variant == b"VP8L".as_slice()
            || variant == b"VP8X".as_slice()
        {
            return Some("image/webp");
        }
    }
    if data.starts_with(b"BM") && is_valid_bmp_header(data) {
        return Some("image/bmp");
    }
    None
}

/// Validate and, when needed, normalize an image within deterministic limits.
pub fn process_image(
    data: &[u8],
    mime_type: &str,
    limits: ImageLimits,
) -> Result<ProcessedImage, ImageProcessingFailure> {
    normalize_image(data, mime_type, limits, encode_image)
}

/// The reference's `process_image`, with the encoder left injectable.
///
/// Python's test suite monkeypatches `_encode_image`; a private closure
/// parameter is the Rust equivalent of that seam.
fn normalize_image(
    data: &[u8],
    mime_type: &str,
    limits: ImageLimits,
    encode: impl Fn(&DynamicImage, &str, usize) -> Result<Vec<u8>, String>,
) -> Result<ProcessedImage, ImageProcessingFailure> {
    if data.len() > limits.max_source_bytes {
        return Err(ImageProcessingFailure {
            message: format!(
                "source is {}, exceeding the {} processing limit",
                format_size(data.len()),
                format_size(limits.max_source_bytes)
            ),
        });
    }

    let (width, height, animated) =
        validated_image_metadata(data).map_err(|error| ImageProcessingFailure {
            message: format!("could not decode a valid image ({error})"),
        })?;

    let pixels = u64::from(width) * u64::from(height);
    if pixels > limits.max_source_pixels {
        return Err(ImageProcessingFailure {
            message: format!(
                "image has {} pixels, exceeding the {}-pixel processing limit",
                group_digits(pixels),
                group_digits(limits.max_source_pixels)
            ),
        });
    }

    let requires_conversion = mime_type == "image/bmp";
    let requires_resize = width > limits.max_dimension
        || height > limits.max_dimension
        || data.len() > limits.max_bytes;
    if !requires_conversion && !requires_resize {
        return Ok(ProcessedImage {
            data: data.to_vec(),
            mime_type: mime_type.to_string(),
            width,
            height,
            notes: Vec::new(),
        });
    }

    if animated {
        return Err(ImageProcessingFailure {
            message:
                "animated image exceeds inline limits and cannot be resized without losing animation"
                    .to_string(),
        });
    }

    let mut image = decode_image_pixels(data).map_err(|error| ImageProcessingFailure {
        message: format!("could not decode image pixels ({error})"),
    })?;

    let output_mime_type = if requires_conversion {
        "image/png"
    } else {
        mime_type
    };
    let mut notes: Vec<String> = Vec::new();
    if output_mime_type != mime_type {
        notes.push(format!(
            "Image converted from {mime_type} to {output_mime_type}."
        ));
    }

    let (target_width, target_height) = bounded_dimensions(width, height, limits.max_dimension);
    if (target_width, target_height) != (width, height) {
        image = image.resize_exact(target_width, target_height, FilterType::Lanczos3);
    }

    for attempt in 0..MAX_RESIZE_ATTEMPTS {
        let encoded =
            encode(&image, output_mime_type, attempt).map_err(|error| ImageProcessingFailure {
                message: format!("could not encode processed image ({error})"),
            })?;
        if encoded.len() <= limits.max_bytes {
            let final_width = image.width();
            let final_height = image.height();
            if (final_width, final_height) != (width, height) {
                notes.push(format!(
                    "Image resized from {width}x{height} to {final_width}x{final_height}."
                ));
            }
            return Ok(ProcessedImage {
                data: encoded,
                mime_type: output_mime_type.to_string(),
                width: final_width,
                height: final_height,
                notes,
            });
        }

        let next_width = (((image.width() as f64) * 0.8) as u32).max(1);
        let next_height = (((image.height() as f64) * 0.8) as u32).max(1);
        if (next_width, next_height) == (image.width(), image.height()) {
            break;
        }
        image = image.resize_exact(next_width, next_height, FilterType::Lanczos3);
    }

    Err(ImageProcessingFailure {
        message: format!(
            "could not resize image below the {} attachment limit",
            format_size(limits.max_bytes)
        ),
    })
}

/// Read the dimensions and the animated flag without loading full pixels.
fn validated_image_metadata(data: &[u8]) -> Result<(u32, u32, bool), String> {
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let format = reader
        .format()
        .ok_or_else(|| "cannot identify image file".to_string())?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    if width < 1 || height < 1 {
        return Err("image dimensions must be positive".to_string());
    }

    let animated = match format {
        ImageFormat::Png => classify_png(data) == PngKind::Animated,
        ImageFormat::Gif => gif_is_animated(data),
        ImageFormat::WebP => {
            WebPDecoder::new(Cursor::new(data)).is_ok_and(|decoder| decoder.has_animation())
        }
        _ => false,
    };
    Ok((width, height, animated))
}

/// Decode the full pixel buffer, the reference's `Image.open(...).load()`.
fn decode_image_pixels(data: &[u8]) -> Result<DynamicImage, String> {
    image::load_from_memory(data).map_err(|error| error.to_string())
}

/// Encode one attempt of the bounded resize loop, the reference's `_encode_image`.
fn encode_image(image: &DynamicImage, mime_type: &str, attempt: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    match mime_type {
        "image/jpeg" => {
            let rgb = image.to_rgb8();
            JpegEncoder::new_with_quality(&mut output, jpeg_quality(attempt))
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(|error| error.to_string())?;
        }
        "image/webp" => {
            image
                .write_to(Cursor::new(&mut output), ImageFormat::WebP)
                .map_err(|error| error.to_string())?;
        }
        "image/gif" => {
            image
                .write_to(Cursor::new(&mut output), ImageFormat::Gif)
                .map_err(|error| error.to_string())?;
        }
        _ => {
            let encoder = PngEncoder::new_with_quality(
                &mut output,
                CompressionType::Best,
                PngFilterType::Adaptive,
            );
            image
                .write_with_encoder(encoder)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(output)
}

/// The reference's quality ladder, `max(45, 90 - attempt * 8)`.
fn jpeg_quality(attempt: usize) -> u8 {
    let quality = 90 - (attempt as i64) * 8;
    quality.max(45) as u8
}

/// The reference's `_bounded_dimensions`.
fn bounded_dimensions(width: u32, height: u32, maximum: u32) -> (u32, u32) {
    let scale = 1.0_f64
        .min(f64::from(maximum) / f64::from(width))
        .min(f64::from(maximum) / f64::from(height));
    (
        ((f64::from(width) * scale) as u32).max(1),
        ((f64::from(height) * scale) as u32).max(1),
    )
}

/// The reference's `_classify_png`, a pure scan of the chunk list.
fn classify_png(data: &[u8]) -> PngKind {
    if data.len() < 33 || be_u32(data, 8) != 13 || &data[12..16] != b"IHDR" {
        return PngKind::Invalid;
    }

    let mut offset = PNG_SIGNATURE.len();
    while offset + 12 <= data.len() {
        let chunk_length = be_u32(data, offset) as usize;
        let chunk_type = [
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ];
        if chunk_type == *b"acTL" {
            return PngKind::Animated;
        }
        if chunk_type == *b"IDAT" {
            return PngKind::Static;
        }
        let next_offset = offset + 12 + chunk_length;
        if next_offset <= offset || next_offset > data.len() {
            return PngKind::Invalid;
        }
        offset = next_offset;
    }
    PngKind::Invalid
}

/// The reference's `_is_valid_bmp_header`, a pure header validation.
fn is_valid_bmp_header(data: &[u8]) -> bool {
    if data.len() < 30 {
        return false;
    }
    let declared_size = le_u32(data, 2);
    let pixel_offset = le_u32(data, 10);
    let dib_size = le_u32(data, 14);
    if declared_size != 0 && declared_size < 26 {
        return false;
    }
    if u64::from(pixel_offset) < 14 + u64::from(dib_size)
        || (declared_size != 0 && pixel_offset >= declared_size)
    {
        return false;
    }
    let (planes, bits_per_pixel) = if dib_size == 12 {
        (le_u16(data, 22), le_u16(data, 24))
    } else if (40..=124).contains(&dib_size) {
        (le_u16(data, 26), le_u16(data, 28))
    } else {
        return false;
    };
    planes == 1 && matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32)
}

/// Whether a GIF holds more than one frame, Pillow's `is_animated`.
///
/// Pillow seeks every frame to count them; the Rust port counts image
/// descriptors in the block structure instead, so the flag is known before any
/// pixel is decoded.
fn gif_is_animated(data: &[u8]) -> bool {
    if data.len() < 13 {
        return false;
    }
    let mut offset = 13;
    let global_table = data[10];
    if global_table & 0x80 != 0 {
        offset += 3 * (1_usize << (u32::from(global_table & 0x07) + 1));
    }

    let mut frames = 0_usize;
    while offset < data.len() {
        match data[offset] {
            0x2C => {
                frames += 1;
                if frames > 1 {
                    return true;
                }
                if offset + 10 > data.len() {
                    return false;
                }
                let descriptor = data[offset + 9];
                offset += 10;
                if descriptor & 0x80 != 0 {
                    offset += 3 * (1_usize << (u32::from(descriptor & 0x07) + 1));
                }
                offset += 1; // The LZW minimum code size.
                offset = skip_gif_sub_blocks(data, offset);
            }
            0x21 => {
                offset += 2; // Introducer plus label.
                offset = skip_gif_sub_blocks(data, offset);
            }
            0x3B => return false, // The trailer.
            _ => return false,
        }
    }
    false
}

/// Skip a GIF sub-block chain and return the offset after its terminator.
fn skip_gif_sub_blocks(data: &[u8], mut offset: usize) -> usize {
    while offset < data.len() {
        let length = usize::from(data[offset]);
        offset += 1;
        if length == 0 {
            return offset;
        }
        offset = offset.saturating_add(length);
    }
    data.len()
}

/// The reference's private `_format_size`, deliberately not `support::format_size`.
fn format_size(size: usize) -> String {
    format!("{:.1}MB", size as f64 / (1024.0 * 1024.0))
}

/// Python's `{value:,}` thousands grouping.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Read a big-endian `u32`; the caller guarantees the four bytes exist.
fn be_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Read a little-endian `u32`; the caller guarantees the four bytes exist.
fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Read a little-endian `u16`; the caller guarantees the two bytes exist.
fn le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, ImageFormat, Rgb};

    use super::*;

    /// A 16x12 solid PNG, the smallest fixture the private helpers need.
    fn small_png() -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(16, 12, Rgb([0, 128, 128])));
        let mut data = Vec::new();
        image
            .write_to(Cursor::new(&mut data), ImageFormat::Png)
            .expect("the fixture encodes as PNG");
        data
    }

    /// Random-looking, incompressible pixels for the bounded resize loop.
    fn noisy_png(size: u32) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut image = ImageBuffer::new(size, size);
        for pixel in image.pixels_mut() {
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state & 0xff) as u8
            };
            *pixel = Rgb([next(), next(), next()]);
        }
        let mut data = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(Cursor::new(&mut data), ImageFormat::Png)
            .expect("the fixture encodes as PNG");
        data
    }

    #[test]
    fn format_size_always_reports_mebibytes() {
        assert_eq!(format_size(0), "0.0MB");
        assert_eq!(format_size(1024), "0.0MB");
        assert_eq!(format_size(1_048_576), "1.0MB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0MB");
        assert_eq!(format_size(50 * 1024 * 1024), "50.0MB");
    }

    #[test]
    fn format_size_rounds_like_python() {
        // Python's `f"{1.25:.1f}"` is "1.2": a tie rounds to even.
        assert_eq!(format_size(1_310_720), "1.2MB");
        assert_eq!(format_size(1_572_864), "1.5MB");
    }

    #[test]
    fn group_digits_uses_python_thousands_separators() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(16), "16");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(40_000_000), "40,000,000");
    }

    #[test]
    fn bounded_dimensions_scales_down_and_floors_at_one_pixel() {
        assert_eq!(bounded_dimensions(2_500, 1_000, 2_000), (2_000, 800));
        assert_eq!(bounded_dimensions(16, 12, 2_000), (16, 12));
        assert_eq!(bounded_dimensions(4_000, 1, 2_000), (2_000, 1));
    }

    #[test]
    fn jpeg_quality_follows_the_reference_ladder() {
        assert_eq!(jpeg_quality(0), 90);
        assert_eq!(jpeg_quality(1), 82);
        assert_eq!(jpeg_quality(5), 50);
        assert_eq!(jpeg_quality(6), 45);
        assert_eq!(jpeg_quality(11), 45);
    }

    #[test]
    fn classify_png_separates_static_from_animated_and_invalid() {
        let png = small_png();
        assert_eq!(classify_png(&png), PngKind::Static);
        assert_eq!(
            classify_png(b"\x89PNG\r\n\x1a\nnot-a-png"),
            PngKind::Invalid
        );

        let idat = png
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("the fixture has an IDAT chunk")
            - 4;
        let mut animated = Vec::new();
        animated.extend_from_slice(&png[..idat]);
        animated.extend_from_slice(b"\x00\x00\x00\x08acTL\x00\x00\x00\x02\x00\x00\x00\x00");
        animated.extend_from_slice(&[0_u8; 4]);
        animated.extend_from_slice(&png[idat..]);
        assert_eq!(classify_png(&animated), PngKind::Animated);
    }

    #[test]
    fn is_valid_bmp_header_rejects_zeroed_headers() {
        let mut zeroed = b"BM".to_vec();
        zeroed.extend_from_slice(&[0_u8; 40]);
        assert!(!is_valid_bmp_header(&zeroed));
        assert!(!is_valid_bmp_header(b"BM"));
    }

    #[test]
    fn webp_output_uses_the_lossless_vp8l_bitstream() {
        // KNOWN DEVIATION: tau writes lossy WebP through a quality ladder; the
        // `image` crate's WebP encoder is lossless only, so the ladder does not
        // apply here and the bounded loop may shrink the image further to reach
        // the byte ceiling.
        let data = noisy_png(300);
        let limits = ImageLimits {
            max_bytes: 3_000,
            ..ImageLimits::default()
        };

        let result = normalize_image(&data, "image/webp", limits, encode_image)
            .expect("lossless WebP reaches the byte ceiling");

        assert_eq!(&result.data[12..16], b"VP8L");
        assert!(result.data.len() <= 3_000);
    }

    #[test]
    fn encode_failure_returns_safe_failure() {
        // The reference monkeypatches `_encode_image` to raise `OSError`; the
        // Rust port injects a failing encoder through the private seam.
        let data = small_png();
        let limits = ImageLimits {
            max_dimension: 8,
            ..ImageLimits::default()
        };

        let failure = normalize_image(&data, "image/png", limits, |_image, _mime, _attempt| {
            Err("encoder unavailable".to_string())
        })
        .expect_err("the injected encoder fails");

        assert_eq!(
            failure.message,
            "could not encode processed image (encoder unavailable)"
        );
    }
}
