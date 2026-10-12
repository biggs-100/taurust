//! Tests for bounded image detection and normalization.
//!
//! Ported from tau's `tests/test_image_processing.py`. The reference builds its
//! fixtures with Pillow; here the same fixtures are built with the `image`
//! crate, which is the Rust replacement for the same operations.

use std::io::Cursor;

use image::codecs::bmp::BmpEncoder;
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType, Frame, ImageBuffer, ImageFormat, Rgb, Rgba};
use tau_coding::tools::image_processing::{
    ImageLimits, detect_supported_image_mime_type, process_image, unsupported_image_reason,
};

/// Pillow's `"teal"` is RGB(0, 128, 128).
const TEAL: Rgb<u8> = Rgb([0, 128, 128]);

/// A solid-colour image, the reference's `Image.new("RGB", size, "teal")`.
fn solid(size: (u32, u32)) -> DynamicImage {
    DynamicImage::ImageRgb8(ImageBuffer::from_pixel(size.0, size.1, TEAL))
}

/// Encode an image the way the reference test helper encodes each format.
fn encoded(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut output = Vec::new();
    match format {
        ImageFormat::Jpeg => {
            let rgb = image.to_rgb8();
            JpegEncoder::new_with_quality(&mut output, 75)
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .expect("the fixture encodes as JPEG");
        }
        ImageFormat::Png => {
            image
                .write_to(Cursor::new(&mut output), ImageFormat::Png)
                .expect("the fixture encodes as PNG");
        }
        ImageFormat::Gif => {
            let rgba = image.to_rgba8();
            GifEncoder::new(&mut output)
                .encode(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    ExtendedColorType::Rgba8,
                )
                .expect("the fixture encodes as GIF");
        }
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            WebPEncoder::new_lossless(&mut output)
                .encode(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    ExtendedColorType::Rgba8,
                )
                .expect("the fixture encodes as WebP");
        }
        ImageFormat::Bmp => {
            let rgb = image.to_rgb8();
            BmpEncoder::new(&mut output)
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .expect("the fixture encodes as BMP");
        }
        other => panic!("the fixture helper does not encode {other:?}"),
    }
    output
}

/// The reference's `image_bytes(format_name, size=(16, 12))`.
fn image_bytes(format: ImageFormat, size: (u32, u32)) -> Vec<u8> {
    encoded(&solid(size), format)
}

/// Random-looking, incompressible pixels, the reference's `Image.effect_noise`.
fn noise_png(size: u32) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut image = ImageBuffer::new(size, size);
    for pixel in image.pixels_mut() {
        *pixel = Rgb([
            next_byte(&mut state),
            next_byte(&mut state),
            next_byte(&mut state),
        ]);
    }
    encoded(&DynamicImage::ImageRgb8(image), ImageFormat::Png)
}

fn next_byte(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state & 0xff) as u8
}

#[test]
fn detects_supported_images_by_content() {
    let cases = [
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Gif, "image/gif"),
        (ImageFormat::WebP, "image/webp"),
        (ImageFormat::Bmp, "image/bmp"),
    ];

    for (format, mime_type) in cases {
        assert_eq!(
            detect_supported_image_mime_type(&image_bytes(format, (16, 12))),
            Some(mime_type),
            "{format:?} should be detected as {mime_type}"
        );
    }
}

#[test]
fn rejects_jpeg_xl_and_malformed_headers() {
    assert_eq!(
        detect_supported_image_mime_type(b"\xff\xd8\xff\xf7not-jpeg"),
        None
    );
    assert_eq!(
        detect_supported_image_mime_type(b"\x89PNG\r\n\x1a\nnot-a-png"),
        None
    );

    let mut zeroed_bmp = b"BM".to_vec();
    zeroed_bmp.extend_from_slice(&[0_u8; 40]);
    assert_eq!(detect_supported_image_mime_type(&zeroed_bmp), None);
}

#[test]
fn rejects_animated_png_before_image_data() {
    let png = image_bytes(ImageFormat::Png, (16, 12));
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

    assert_eq!(detect_supported_image_mime_type(&animated), None);
    assert_eq!(
        unsupported_image_reason(&animated).as_deref(),
        Some("animated PNG images are not supported")
    );
}

#[test]
fn jpeg_xl_has_explicit_unsupported_reason() {
    let data = b"\xff\xd8\xff\xf7not-jpeg";

    assert_eq!(detect_supported_image_mime_type(data), None);
    assert_eq!(
        unsupported_image_reason(data).as_deref(),
        Some("JPEG XL images are not supported")
    );
}

#[test]
fn bmp_is_converted_to_png() {
    let data = image_bytes(ImageFormat::Bmp, (16, 12));

    let result = process_image(&data, "image/bmp", ImageLimits::default())
        .expect("a BMP under the limits converts to PNG");

    assert_eq!(result.mime_type, "image/png");
    assert!(result.data.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        result
            .notes
            .contains(&"Image converted from image/bmp to image/png.".to_string())
    );
}

#[test]
fn large_dimensions_are_resized_without_changing_aspect_ratio() {
    let data = image_bytes(ImageFormat::Png, (2_500, 1_000));

    let result = process_image(&data, "image/png", ImageLimits::default())
        .expect("an oversized PNG is resized in place");

    assert_eq!((result.width, result.height), (2_000, 800));
    assert!(
        result
            .notes
            .contains(&"Image resized from 2500x1000 to 2000x800.".to_string())
    );
}

#[test]
fn small_images_are_not_upscaled_or_reencoded() {
    let data = image_bytes(ImageFormat::Png, (20, 10));

    let result = process_image(&data, "image/png", ImageLimits::default())
        .expect("a small image is attached untouched");

    assert_eq!((result.width, result.height), (20, 10));
    assert_eq!(result.data, data);
    assert_eq!(result.notes, Vec::<String>::new());
}

#[test]
fn encoded_size_limit_triggers_bounded_downscaling() {
    let data = noise_png(300);

    let limits = ImageLimits {
        max_bytes: 3_000,
        ..ImageLimits::default()
    };
    let result = process_image(&data, "image/png", limits)
        .expect("the encoder shrinks the image until it fits");

    assert!(
        result.data.len() <= 3_000,
        "encoded image is {} bytes",
        result.data.len()
    );
    assert!(result.width < 300);
    assert!(result.height < 300);
}

#[test]
fn source_byte_limit_omits_image_before_decoding() {
    let data = image_bytes(ImageFormat::Png, (16, 12));

    let limits = ImageLimits {
        max_source_bytes: data.len() - 1,
        ..ImageLimits::default()
    };
    let failure = process_image(&data, "image/png", limits)
        .expect_err("the source is over the processing limit");

    assert!(failure.message.contains("processing limit"));
}

#[test]
fn source_pixel_limit_omits_image_before_loading_pixels() {
    let data = image_bytes(ImageFormat::Png, (4, 4));

    let limits = ImageLimits {
        max_source_pixels: 15,
        ..ImageLimits::default()
    };
    let failure =
        process_image(&data, "image/png", limits).expect_err("the source has too many pixels");

    assert!(failure.message.contains("16 pixels"));
    assert!(failure.message.contains("15-pixel processing limit"));
}

#[test]
fn decode_failure_returns_safe_failure() {
    let mut malformed = Vec::new();
    malformed.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    malformed.extend_from_slice(b"\x00\x00\x00\x0dIHDR");
    malformed.extend_from_slice(&[0_u8; 17]);
    malformed.extend_from_slice(b"\x00\x00\x00\x00IDAT");
    malformed.extend_from_slice(&[0_u8; 4]);

    assert_eq!(
        detect_supported_image_mime_type(&malformed),
        Some("image/png")
    );

    let failure = process_image(&malformed, "image/png", ImageLimits::default())
        .expect_err("the header is structurally invalid");

    assert!(failure.message.contains("could not decode a valid image"));
}

#[test]
fn animated_gif_that_needs_resizing_is_omitted() {
    let frame = || {
        Frame::new(ImageBuffer::from_pixel(
            2_500,
            1_000,
            Rgba([0, 128, 128, 255]),
        ))
    };
    let mut output = Vec::new();
    GifEncoder::new(&mut output)
        .encode_frames([frame(), frame()])
        .expect("the fixture encodes as an animated GIF");

    let failure = process_image(&output, "image/gif", ImageLimits::default())
        .expect_err("an animated image that must be resized is omitted");

    assert_eq!(
        failure.message,
        "animated image exceeds inline limits and cannot be resized without losing animation"
    );
}

#[test]
fn animated_gif_within_the_limits_is_attached_untouched() {
    let frame = || Frame::new(ImageBuffer::from_pixel(16, 12, Rgba([0, 128, 128, 255])));
    let mut output = Vec::new();
    GifEncoder::new(&mut output)
        .encode_frames([frame(), frame()])
        .expect("the fixture encodes as an animated GIF");

    let result = process_image(&output, "image/gif", ImageLimits::default())
        .expect("an animated image within the limits is attached untouched");

    assert_eq!((result.width, result.height), (16, 12));
    assert_eq!(result.data, output);
    assert_eq!(result.notes, Vec::<String>::new());
}
