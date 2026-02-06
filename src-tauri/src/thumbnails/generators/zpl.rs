use super::super::{ThumbnailGenerationResult, ThumbnailRequest};
use super::ThumbnailGenerator;
use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;

/// Mutex to serialize ZPL renders — the underlying Go library is not thread-safe.
/// Recovery from poisoning is safe since the lock guards no shared state.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

/// Maximum ZPL file size we'll attempt to render (5 MB).
/// Labels with embedded bitmaps are typically under 1 MB.
const MAX_ZPL_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum rendered pixel count (width * height) to prevent OOM.
/// 50 megapixels is generous for label thumbnails (e.g., 7000x7000).
const MAX_RENDER_PIXELS: u32 = 50_000_000;

pub struct ZplGenerator;

impl ZplGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let path = Path::new(&request.path);

        // Guard against unreasonably large or non-regular files
        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Failed to read ZPL metadata: {e}"))?;
        if !metadata.is_file() {
            return Err("ZPL path is not a regular file".to_string());
        }
        if metadata.len() > MAX_ZPL_SIZE {
            return Err(format!(
                "ZPL file too large ({} bytes, max {})",
                metadata.len(),
                MAX_ZPL_SIZE
            ));
        }

        // Read ZPL content as raw bytes — ZPL can contain binary graphic data
        let zpl_bytes =
            std::fs::read(path).map_err(|e| format!("Failed to read ZPL file: {e}"))?;

        // Render to PNG (serialized due to Go runtime thread safety)
        let png_bytes = {
            let _lock = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            zpl_rs::render_bytes(&zpl_bytes).map_err(|e| format!("Failed to render ZPL: {e}"))?
        };

        // Read dimensions from PNG header *before* full decode to avoid OOM
        // on pathologically large renders from small ZPL input.
        let reader = image::ImageReader::new(Cursor::new(&png_bytes))
            .with_guessed_format()
            .map_err(|e| format!("Failed to guess rendered ZPL format: {e}"))?;
        let (label_width, label_height) = reader
            .into_dimensions()
            .map_err(|e| format!("Failed to read rendered ZPL dimensions: {e}"))?;

        if label_width.saturating_mul(label_height) > MAX_RENDER_PIXELS {
            return Err(format!(
                "Rendered ZPL too large ({}x{} = {} pixels, max {})",
                label_width,
                label_height,
                label_width as u64 * label_height as u64,
                MAX_RENDER_PIXELS
            ));
        }

        // Now do full decode (dimensions verified safe)
        let image = image::load_from_memory(&png_bytes)
            .map_err(|e| format!("Failed to decode rendered ZPL: {e}"))?;

        // Resize and encode as thumbnail
        let resized = ThumbnailGenerator::resize_image(image, request.size, request.quality)?;

        let data_url = ThumbnailGenerator::encode_to_data_url(
            &resized,
            request.format,
            request.quality,
        )?;

        Ok(ThumbnailGenerationResult {
            data_url,
            has_transparency: false,
            image_width: Some(label_width),
            image_height: Some(label_height),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbnails::{ThumbnailFormat, ThumbnailQuality, ThumbnailRequest};
    use std::io::Write;

    #[test]
    fn test_zpl_render_basic() {
        let zpl = "^XA^FO50,50^A0N,30,30^FDHello!^FS^XZ";
        let png = zpl_rs::render(zpl).expect("zpl_rs::render failed");
        assert!(png.len() > 8, "PNG too small: {} bytes", png.len());
        assert_eq!(&png[0..4], &[0x89, b'P', b'N', b'G'], "Not a valid PNG");
    }

    #[test]
    fn test_zpl_render_bytes() {
        // render_bytes should handle the same content as render
        let zpl = b"^XA^FO50,50^A0N,30,30^FDHello!^FS^XZ";
        let png = zpl_rs::render_bytes(zpl).expect("zpl_rs::render_bytes failed");
        assert!(png.len() > 8, "PNG too small: {} bytes", png.len());
        assert_eq!(&png[0..4], &[0x89, b'P', b'N', b'G'], "Not a valid PNG");
    }

    #[test]
    fn test_zpl_generate_thumbnail() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let zpl_path = dir.path().join("label.zpl");
        {
            let mut f = std::fs::File::create(&zpl_path).expect("create file");
            f.write_all(b"^XA^FO50,50^A0N,30,30^FDTest^FS^XZ")
                .expect("write");
        }

        let request = ThumbnailRequest {
            id: "test".to_string(),
            path: zpl_path.to_string_lossy().to_string(),
            size: 256,
            quality: ThumbnailQuality::Medium,
            format: ThumbnailFormat::PNG,
            priority: crate::thumbnails::ThumbnailPriority::High,
            accent: None,
        };

        let result = ZplGenerator::generate(&request).expect("generate failed");
        assert!(result.data_url.starts_with("data:image/png;base64,"));
        assert!(result.image_width.unwrap() > 0);
        assert!(result.image_height.unwrap() > 0);
    }

    #[test]
    fn test_zpl_rejects_oversized_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let zpl_path = dir.path().join("huge.zpl");
        // Create a sparse file that exceeds MAX_ZPL_SIZE
        let f = std::fs::File::create(&zpl_path).expect("create file");
        f.set_len(MAX_ZPL_SIZE + 1).expect("set_len");
        drop(f);

        let request = ThumbnailRequest {
            id: "test".to_string(),
            path: zpl_path.to_string_lossy().to_string(),
            size: 256,
            quality: ThumbnailQuality::Medium,
            format: ThumbnailFormat::PNG,
            priority: crate::thumbnails::ThumbnailPriority::High,
            accent: None,
        };
        let err = ZplGenerator::generate(&request).unwrap_err();
        assert!(err.contains("too large"), "expected 'too large' error, got: {err}");
    }

    #[test]
    fn test_zpl_rejects_nonexistent_file() {
        let request = ThumbnailRequest {
            id: "test".to_string(),
            path: "/nonexistent/label.zpl".to_string(),
            size: 256,
            quality: ThumbnailQuality::Medium,
            format: ThumbnailFormat::PNG,
            priority: crate::thumbnails::ThumbnailPriority::High,
            accent: None,
        };
        let err = ZplGenerator::generate(&request).unwrap_err();
        assert!(err.contains("metadata"), "expected metadata error, got: {err}");
    }
}
