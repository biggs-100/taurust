//! Tests for the `read` tool.
//!
//! Ports the `read` contract from tau's `tests/test_coding_tools.py`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_coding::tools::image_processing::DEFAULT_MAX_SOURCE_IMAGE_BYTES;
use tau_coding::tools::read::{
    ImageSupportState, ReadOperations, ReadTool, create_read_tool, create_read_tool_spec,
};
use tau_protocol::{AgentToolResult, JsonObject};

fn object(value: serde_json::Value) -> JsonObject {
    value.as_object().expect("a JSON object").clone()
}

async fn run(tool: &ReadTool, arguments: serde_json::Value) -> Result<AgentToolResult, ToolError> {
    tool.execute("test-call", &object(arguments), &ToolUpdates::new())
        .await
}

fn image_bytes(format: image::ImageFormat, size: (u32, u32)) -> Vec<u8> {
    let buffer = image::RgbImage::from_pixel(size.0, size.1, image::Rgb([0, 0, 128]));
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(buffer)
        .write_to(&mut cursor, format)
        .expect("the test encodes an image");
    cursor.into_inner()
}

fn png_bytes() -> Vec<u8> {
    image_bytes(image::ImageFormat::Png, (8, 6))
}

fn animated_png_bytes() -> Vec<u8> {
    let png = png_bytes();
    let idat = find(&png, b"IDAT").expect("an IDAT chunk") - 4;
    let mut animated = Vec::new();
    animated.extend_from_slice(&png[..idat]);
    animated.extend_from_slice(b"\x00\x00\x00\x08acTL\x00\x00\x00\x02\x00\x00\x00\x00");
    animated.extend_from_slice(&[0u8; 4]);
    animated.extend_from_slice(&png[idat..]);
    animated
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Decode standard base64 so the attached bytes can be compared literally.
fn base64_decode(data: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in data.bytes() {
        if byte == b'=' {
            break;
        }
        let value = lookup[byte as usize];
        assert_ne!(value, 255, "the tool emitted a valid base64 character");
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
        }
    }
    output
}

struct PanickingReadOperations;

impl ReadOperations for PanickingReadOperations {
    fn validate_path(&self, _path: &Path) -> Result<(), ToolError> {
        Ok(())
    }

    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        panic!("unexpected full read: {}", path.display());
    }

    fn size_bytes(&self, _path: &Path) -> Option<std::io::Result<u64>> {
        Some(Ok((DEFAULT_MAX_SOURCE_IMAGE_BYTES + 1) as u64))
    }

    fn read_prefix(&self, _path: &Path, limit: usize) -> Option<std::io::Result<Vec<u8>>> {
        let bytes = png_bytes();
        Some(Ok(bytes[..limit.min(bytes.len())].to_vec()))
    }
}

struct LargeTextOperations;

impl ReadOperations for LargeTextOperations {
    fn validate_path(&self, _path: &Path) -> Result<(), ToolError> {
        Ok(())
    }

    fn read_bytes(&self, _path: &Path) -> std::io::Result<Vec<u8>> {
        Ok(b"large text".to_vec())
    }

    fn size_bytes(&self, _path: &Path) -> Option<std::io::Result<u64>> {
        Some(Ok((DEFAULT_MAX_SOURCE_IMAGE_BYTES + 1) as u64))
    }

    fn read_prefix(&self, _path: &Path, limit: usize) -> Option<std::io::Result<Vec<u8>>> {
        Some(Ok(b"large text"[..limit.min(10)].to_vec()))
    }
}

#[derive(Default)]
struct RecordingReadOperations {
    reads: Mutex<Vec<PathBuf>>,
}

impl ReadOperations for RecordingReadOperations {
    fn validate_path(&self, _path: &Path) -> Result<(), ToolError> {
        Ok(())
    }

    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.reads
            .lock()
            .expect("the recording lock is not poisoned")
            .push(path.to_path_buf());
        Ok(b"remote text".to_vec())
    }
}

#[tokio::test]
async fn read_tool_reads_file_with_offset_and_limit() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("notes.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(
        &tool,
        serde_json::json!({"path": "notes.txt", "offset": 2, "limit": 1}),
    )
    .await
    .expect("the read succeeds");

    assert_eq!(
        result.text(),
        "two\n\n[2 more lines in file. Use offset=3 to continue.]"
    );
    let details = result.details.expect("details");
    assert_eq!(details["path"], path.display().to_string());
    assert!(details["truncation"].is_object());
}

#[tokio::test]
async fn read_tool_returns_images_as_model_content() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let image_data = png_bytes();
    let path = directory.path().join("diagram.png");
    std::fs::write(&path, &image_data).expect("the fixture image");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "diagram.png"}))
        .await
        .expect("the read succeeds");

    assert_eq!(result.text(), "Read image file [image/png]");
    let image = result.images().into_iter().next().expect("an image block");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(base64_decode(&image.data), image_data);
    let details = result.details.expect("details");
    assert_eq!(details["path"], path.display().to_string());
    assert_eq!(details["source_mime_type"], "image/png");
    assert_eq!(details["mime_type"], "image/png");
    assert_eq!(details["bytes"], image_data.len());
    assert_eq!(details["processed_bytes"], image_data.len());
    assert_eq!(details["width"], 8);
    assert_eq!(details["height"], 6);
}

#[tokio::test]
async fn read_tool_detects_images_by_content_not_extension() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("diagram.txt");
    std::fs::write(&path, png_bytes()).expect("the fixture image");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "diagram.txt"}))
        .await
        .expect("the read succeeds");

    assert!(!result.images().is_empty());
}

#[tokio::test]
async fn read_tool_resizes_over_dimension_images() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("large.png");
    std::fs::write(&path, image_bytes(image::ImageFormat::Png, (2_500, 100)))
        .expect("the fixture image");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "large.png"}))
        .await
        .expect("the read succeeds");

    assert!(
        result
            .text()
            .contains("Image resized from 2500x100 to 2000x80"),
        "{}",
        result.text()
    );
    let image = result.images().into_iter().next().expect("an image block");
    let decoded = base64_decode(&image.data);
    let processed = image::load_from_memory(&decoded).expect("the attached image decodes");
    assert_eq!((processed.width(), processed.height()), (2_000, 80));
}

#[tokio::test]
async fn read_tool_explicitly_omits_images_for_text_only_model() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("galaxy.png");
    std::fs::write(&path, png_bytes()).expect("the fixture image");
    let image_support = ImageSupportState::with_supported(Some(false));
    let tool = create_read_tool(directory.path(), None, Some(image_support.clone()));

    let omitted = run(&tool, serde_json::json!({"path": "galaxy.png"}))
        .await
        .expect("the read succeeds");

    assert!(
        omitted
            .text()
            .contains("current model does not support image input")
    );
    assert!(omitted.text().contains("do not infer or describe"));
    assert!(omitted.text().contains("switch to a vision-capable model"));
    assert!(omitted.images().is_empty());

    image_support.set_supported(Some(true));
    let attached = run(&tool, serde_json::json!({"path": "galaxy.png"}))
        .await
        .expect("the read succeeds");

    assert!(!attached.images().is_empty());
}

#[tokio::test]
async fn read_tool_converts_bmp_to_png() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("legacy.bmp");
    std::fs::write(&path, image_bytes(image::ImageFormat::Bmp, (8, 6))).expect("the fixture image");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "legacy.bmp"}))
        .await
        .expect("the read succeeds");

    assert!(
        result.text().contains("Read image file [image/png]"),
        "{}",
        result.text()
    );
    assert!(
        result
            .text()
            .contains("Image converted from image/bmp to image/png"),
        "{}",
        result.text()
    );
    let image = result.images().into_iter().next().expect("an image block");
    assert_eq!(image.mime_type, "image/png");
    assert!(
        image.data.starts_with("iVBORw0KGgo"),
        "{}",
        &image.data[..12]
    );
    assert!(base64_decode(&image.data).starts_with(b"\x89PNG\r\n\x1a\n"));
    let details = result.details.expect("details");
    assert_eq!(details["source_mime_type"], "image/bmp");
}

#[tokio::test]
async fn read_tool_reports_decode_failure_without_attachment() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let malformed = [
        b"\x89PNG\r\n\x1a\n".as_slice(),
        b"\x00\x00\x00\x0dIHDR".as_slice(),
        &[0u8; 17],
        b"\x00\x00\x00\x00IDAT".as_slice(),
        &[0u8; 4],
    ]
    .concat();
    std::fs::write(directory.path().join("broken.png"), &malformed).expect("the fixture image");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "broken.png"}))
        .await
        .expect("the read succeeds");

    assert!(
        result
            .text()
            .contains("Image omitted: could not decode a valid image"),
        "{}",
        result.text()
    );
    assert!(result.images().is_empty());
}

#[tokio::test]
async fn read_tool_reports_known_unsupported_image_variants() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let cases: [(&str, Vec<u8>, &str); 2] = [
        (
            "animated.png",
            animated_png_bytes(),
            "animated PNG images are not supported",
        ),
        (
            "image.jxl",
            b"\xff\xd8\xff\xf7not-jpeg".to_vec(),
            "JPEG XL images are not supported",
        ),
    ];
    for (filename, data, reason) in cases {
        std::fs::write(directory.path().join(filename), &data).expect("the fixture image");
        let tool = create_read_tool(directory.path(), None, None);

        let result = run(&tool, serde_json::json!({"path": filename}))
            .await
            .expect("the read succeeds");

        assert!(result.text().contains(reason), "{}", result.text());
        assert!(result.images().is_empty());
    }
}

#[tokio::test]
async fn read_tool_rejects_oversized_image_from_prefix_before_full_read() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let operations: Arc<dyn ReadOperations> = Arc::new(PanickingReadOperations);
    let tool = create_read_tool(directory.path(), Some(operations), None);

    let result = run(&tool, serde_json::json!({"path": "huge.png"}))
        .await
        .expect("the read succeeds");

    assert!(
        result
            .text()
            .contains("exceeding the 50.0MB processing limit"),
        "{}",
        result.text()
    );
    assert!(result.images().is_empty());
    let details = result.details.expect("details");
    assert_eq!(details["bytes"], DEFAULT_MAX_SOURCE_IMAGE_BYTES + 1);
}

#[tokio::test]
async fn read_tool_still_reads_large_text_files() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let operations: Arc<dyn ReadOperations> = Arc::new(LargeTextOperations);
    let tool = create_read_tool(directory.path(), Some(operations), None);

    let result = run(&tool, serde_json::json!({"path": "large.txt"}))
        .await
        .expect("the read succeeds");

    assert_eq!(result.text(), "large text");
}

#[tokio::test]
async fn read_tool_uses_pluggable_read_operations() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let recording = Arc::new(RecordingReadOperations::default());
    let operations: Arc<dyn ReadOperations> = recording.clone();
    let tool = create_read_tool(directory.path(), Some(operations), None);

    let result = run(&tool, serde_json::json!({"path": "not-local.txt"}))
        .await
        .expect("the read succeeds");

    assert_eq!(result.text(), "remote text");
    assert_eq!(
        *recording
            .reads
            .lock()
            .expect("the recording lock is not poisoned"),
        vec![directory.path().join("not-local.txt")]
    );
}

#[tokio::test]
async fn read_tool_treats_zero_offset_as_start_of_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("notes.txt"), "one\ntwo\nthree\n")
        .expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(
        &tool,
        serde_json::json!({"path": "notes.txt", "offset": 0, "limit": 1}),
    )
    .await
    .expect("the read succeeds");

    assert_eq!(
        result.text(),
        "one\n\n[3 more lines in file. Use offset=2 to continue.]"
    );
}

#[tokio::test]
async fn read_tool_rejects_an_offset_beyond_the_end_of_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("notes.txt"), "one\ntwo\n").expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let error = run(&tool, serde_json::json!({"path": "notes.txt", "offset": 5}))
        .await
        .expect_err("the offset is rejected");

    assert_eq!(
        error.message(),
        "Offset 5 is beyond end of file (3 lines total)"
    );
}

#[tokio::test]
async fn read_tool_validates_the_offset_before_touching_the_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_read_tool(directory.path(), None, None);

    let error = run(
        &tool,
        serde_json::json!({"path": "missing.txt", "offset": -1}),
    )
    .await
    .expect_err("the offset is rejected");

    assert_eq!(error.message(), "offset must be at least 0");
}

#[tokio::test]
async fn read_tool_normalizes_carriage_returns() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("crlf.txt"), "one\r\ntwo\rthree")
        .expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "crlf.txt"}))
        .await
        .expect("the read succeeds");

    assert_eq!(result.text(), "one\ntwo\nthree");
}

#[tokio::test]
async fn read_tool_reports_a_first_line_over_the_byte_limit() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("wide.txt"), "a".repeat(60_000))
        .expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "wide.txt"}))
        .await
        .expect("the read succeeds");

    assert_eq!(
        result.text(),
        "[Line 1 is 58.6KB, exceeds 50.0KB limit. Use bash: sed -n '1p' wide.txt | head -c 51200]"
    );
}

#[tokio::test]
async fn read_tool_appends_the_truncation_continuation_hint() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lines: Vec<String> = (1..=2_005).map(|index| format!("line {index}")).collect();
    std::fs::write(directory.path().join("long.txt"), lines.join("\n")).expect("the fixture file");
    let tool = create_read_tool(directory.path(), None, None);

    let result = run(&tool, serde_json::json!({"path": "long.txt"}))
        .await
        .expect("the read succeeds");

    assert!(
        result
            .text()
            .contains("[Showing lines 1-2000 of 2005. Use offset=2001 to continue.]"),
        "{}",
        result.text()
    );
}

#[tokio::test]
async fn read_tool_surfaces_invalid_utf8_as_a_tool_error() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("binary.dat"), [0xff, 0xfe, 0xfd]).expect("the fixture");
    let tool = create_read_tool(directory.path(), None, None);

    let error = run(&tool, serde_json::json!({"path": "binary.dat"}))
        .await
        .expect_err("invalid UTF-8 is rejected");

    assert!(!error.message().is_empty());
}

#[test]
fn read_spec_matches_the_reference_declaration() {
    let spec = create_read_tool_spec();

    assert_eq!(spec.name, "read");
    assert_eq!(spec.label, "read");
    assert_eq!(
        spec.description,
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent to vision-capable models as attachments. For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete."
    );
    assert_eq!(spec.prompt_snippet.as_deref(), Some("Read file contents"));
    assert_eq!(
        spec.prompt_guidelines,
        ["Use read to examine files instead of cat or sed."]
    );
    assert_eq!(
        spec.parameters["properties"]["path"]["description"],
        "Path to the file to read"
    );
    assert_eq!(spec.parameters["required"], serde_json::json!(["path"]));
}
