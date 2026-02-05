use base64::Engine as _;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::io::Cursor;
use std::path::Path;
use std::sync::OnceLock;

use super::{ThumbnailFormat, ThumbnailGenerationResult, ThumbnailQuality, ThumbnailRequest};

#[cfg(target_os = "macos")]
use crate::macos_security;
#[cfg(target_os = "macos")]
pub mod apps;
pub mod images;
pub mod pdf;
pub mod psd;
pub mod stl;
pub mod svg;
pub mod video;
pub mod fonts;
pub mod zpl;

pub mod smb;

/// Shared runtime for async thumbnail operations (archive extraction, Google Drive download, etc.)
/// Using OnceLock ensures we create it only once and reuse it
static ASYNC_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn get_async_runtime() -> &'static tokio::runtime::Runtime {
    ASYNC_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to create thumbnail async runtime")
    })
}

pub struct ThumbnailGenerator;

impl ThumbnailGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        if request.path.starts_with("archive://") {
            let archive_path = request.path.clone();
            let runtime = get_async_runtime();
            let temp_path = runtime.block_on(
                crate::locations::archive::extract_archive_entry_to_temp(&archive_path),
            )?;

            let mut temp_request = request.clone();
            temp_request.path = temp_path.to_string_lossy().to_string();
            return Self::generate_local(&temp_request);
        }

        // Download Google Drive files to a local temp path before generating thumbnails
        if request.path.starts_with("gdrive://") {
            return Self::generate_gdrive(request);
        }

        // Check for SMB paths and handle them specially
        if smb::is_smb_path(&request.path) {
            return smb::generate_smb_thumbnail(request);
        }

        // Handle local files
        Self::generate_local(request)
    }

    /// Download a Google Drive file to temp and generate a thumbnail from the local copy.
    fn generate_gdrive(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let (email, path) = parse_gdrive_path(&request.path)?;
        let file_name = std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file");

        let runtime = get_async_runtime();
        let file_id = runtime.block_on(
            crate::locations::gdrive::provider::get_file_id_by_path(&email, &path),
        )?;

        let temp_path = runtime.block_on(
            crate::locations::gdrive::provider::download_file_to_temp(&email, &file_id, file_name),
        )?;

        let mut temp_request = request.clone();
        temp_request.path = temp_path.clone();
        let result = Self::generate_local(&temp_request);

        // Clean up temp file regardless of outcome
        let _ = std::fs::remove_file(&temp_path);

        result
    }

    /// Generate a thumbnail from a local file path.
    /// This is called directly for local files, or after downloading remote files.
    pub fn generate_local(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let path = Path::new(&request.path);

        if !path.exists() {
            return Err("File does not exist".to_string());
        }

        #[cfg(target_os = "macos")]
        let _scope_guard = macos_security::retain_access(path)?;

        #[cfg(target_os = "macos")]
        macos_security::persist_bookmark(path, "preparing thumbnail");

        // Determine generator based on file type
        #[cfg(target_os = "macos")]
        {
            // Check for macOS special file types that have system icons
            if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
                let ext = extension.to_lowercase();
                if matches!(ext.as_str(), "app" | "dmg" | "pkg") {
                    return apps::generate(request);
                }
            }

            // Also check .app directories (bundles)
            if path.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("app") {
                return apps::generate(request);
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if path.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("app") {
                return Err("App thumbnails only supported on macOS".to_string());
            }
        }

        // Check if it's a PSD file (handle before regular images)
        if Self::is_psd_file(path) {
            return psd::PsdGenerator::generate(request);
        }

        // Check if it's an image file
        if Self::is_image_file(path) {
            return images::ImageGenerator::generate(request);
        }

        // Check if it's an SVG file
        if Self::is_svg_file(path) {
            return svg::SvgGenerator::generate(request);
        }

        // Check if it's a PDF file (includes AI and EPS)
        if Self::is_pdf_file(path) {
            return pdf::PdfGenerator::generate(request);
        }

        // Check if it's an STL 3D model
        if Self::is_stl_file(path) {
            return stl::StlGenerator::generate(request);
        }

        if Self::is_video_file(path) {
            return video::VideoGenerator::generate(request);
        }

        // Check if it's a font file
        if Self::is_font_file(path) {
            return fonts::FontGenerator::generate(request);
        }

        // Check if it's a ZPL label file
        if Self::is_zpl_file(path) {
            return zpl::ZplGenerator::generate(request);
        }

        // TODO: Add support for documents
        Err("Unsupported file type for thumbnail generation".to_string())
    }

    fn is_image_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            matches!(
                extension.to_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tga" | "ico"
            )
        } else {
            false
        }
    }

    fn is_psd_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            // Note: PSB (Large Document Format) is not supported by the psd crate
            matches!(extension.to_lowercase().as_str(), "psd")
        } else {
            false
        }
    }

    fn is_pdf_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            matches!(
                extension.to_lowercase().as_str(),
                "pdf" | "ai" | "eps" // PDF and Adobe Illustrator/EPS files
            )
        } else {
            false
        }
    }

    fn is_svg_file(path: &Path) -> bool {
        match path.extension().and_then(|s| s.to_str()) {
            Some(ext) => ext.eq_ignore_ascii_case("svg"),
            None => false,
        }
    }

    fn is_stl_file(path: &Path) -> bool {
        match path.extension().and_then(|s| s.to_str()) {
            Some(ext) => ext.eq_ignore_ascii_case("stl"),
            None => false,
        }
    }

    fn is_video_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            matches!(
                extension.to_lowercase().as_str(),
                "mp4"
                    | "m4v"
                    | "mov"
                    | "mkv"
                    | "webm"
                    | "avi"
                    | "flv"
                    | "wmv"
                    | "mpg"
                    | "mpeg"
                    | "m2ts"
                    | "mts"
                    | "3gp"
                    | "ogv"
            )
        } else {
            false
        }
    }

    fn is_font_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            // Only TTF and OTF are supported - ab_glyph doesn't reliably parse WOFF/WOFF2
            matches!(extension.to_lowercase().as_str(), "ttf" | "otf")
        } else {
            false
        }
    }

    fn is_zpl_file(path: &Path) -> bool {
        match path.extension().and_then(|s| s.to_str()) {
            Some(ext) => ext.eq_ignore_ascii_case("zpl"),
            None => false,
        }
    }

    pub fn resize_image(
        image: DynamicImage,
        target_size: u32,
        quality: ThumbnailQuality,
    ) -> Result<DynamicImage, String> {
        let (width, height) = image.dimensions();

        if width == 0 || height == 0 {
            return Err("Invalid image dimensions".to_string());
        }

        // Calculate new dimensions maintaining aspect ratio
        let (new_width, new_height) = if width > height {
            let new_height = (height * target_size) / width;
            (target_size, new_height.max(1))
        } else {
            let new_width = (width * target_size) / height;
            (new_width.max(1), target_size)
        };

        // Choose resize algorithm based on quality
        let resized = match quality {
            ThumbnailQuality::Low => {
                // Fast nearest neighbor for speed
                image.resize(new_width, new_height, image::imageops::FilterType::Nearest)
            }
            ThumbnailQuality::Medium => {
                // Balanced triangle filter
                image.resize(new_width, new_height, image::imageops::FilterType::Triangle)
            }
            ThumbnailQuality::High => {
                // High quality Lanczos3 filter
                image.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
            }
        };

        Ok(resized)
    }

    pub fn encode_to_data_url(
        image: &DynamicImage,
        format: ThumbnailFormat,
        quality: ThumbnailQuality,
    ) -> Result<String, String> {
        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);

        match format {
            ThumbnailFormat::PNG => {
                image
                    .write_to(&mut cursor, ImageFormat::Png)
                    .map_err(|e| format!("Failed to encode PNG: {}", e))?;
                Ok(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&buffer)
                ))
            }
            ThumbnailFormat::JPEG => {
                let quality_value = match quality {
                    ThumbnailQuality::Low => 60,
                    ThumbnailQuality::Medium => 80,
                    ThumbnailQuality::High => 95,
                };

                // Convert to RGB if it has alpha channel
                let rgb_image = if image.color().has_alpha() {
                    DynamicImage::ImageRgb8(image.to_rgb8())
                } else {
                    image.clone()
                };

                let mut jpeg_cursor = Cursor::new(&mut buffer);
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut jpeg_cursor,
                    quality_value,
                );
                rgb_image
                    .write_with_encoder(encoder)
                    .map_err(|e| format!("Failed to encode JPEG: {}", e))?;

                Ok(format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&buffer)
                ))
            }
            ThumbnailFormat::WebP => {
                #[cfg(feature = "webp")]
                {
                    // TODO: Implement WebP encoding when webp crate is available
                    // For now, fall back to PNG
                    Self::encode_to_data_url(image, ThumbnailFormat::PNG, quality)
                }
                #[cfg(not(feature = "webp"))]
                {
                    // Fall back to PNG if WebP is not available
                    Self::encode_to_data_url(image, ThumbnailFormat::PNG, quality)
                }
            }
        }
    }
}

/// Parse a `gdrive://user@domain/path` URI into (email, decoded_path).
fn parse_gdrive_path(raw: &str) -> Result<(String, String), String> {
    // gdrive://brian@smg.gg/My Drive/file.zpl
    // The url crate treats this as scheme=gdrive, username=brian, host=smg.gg,
    // path=/My%20Drive/file.zpl (percent-encoded).
    let url =
        url::Url::parse(raw).map_err(|e| format!("Invalid Google Drive path: {e}"))?;

    let host = url
        .host_str()
        .ok_or_else(|| "Google Drive path missing account".to_string())?;

    let email = if url.username().is_empty() {
        host.to_string()
    } else {
        format!("{}@{}", url.username(), host)
    };

    // url.path() returns percent-encoded; decode it back to the original path
    let path = urlencoding::decode(url.path())
        .map_err(|e| format!("Invalid UTF-8 in Google Drive path: {e}"))?
        .into_owned();

    Ok((email, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gdrive_path_basic() {
        let (email, path) =
            parse_gdrive_path("gdrive://brian@smg.gg/My Drive/file.zpl").unwrap();
        assert_eq!(email, "brian@smg.gg");
        assert_eq!(path, "/My Drive/file.zpl");
    }

    #[test]
    fn test_parse_gdrive_path_spaces() {
        let (email, path) = parse_gdrive_path(
            "gdrive://brian@smg.gg/My Drive/shipping-label-fedex real test.zpl",
        )
        .unwrap();
        assert_eq!(email, "brian@smg.gg");
        assert_eq!(path, "/My Drive/shipping-label-fedex real test.zpl");
    }

    #[test]
    fn test_parse_gdrive_path_shared() {
        let (email, path) =
            parse_gdrive_path("gdrive://user@example.com/Shared with me/doc.pdf").unwrap();
        assert_eq!(email, "user@example.com");
        assert_eq!(path, "/Shared with me/doc.pdf");
    }

    #[test]
    fn test_parse_gdrive_path_nested() {
        let (email, path) = parse_gdrive_path(
            "gdrive://a@b.co/My Drive/folder/sub folder/file name.png",
        )
        .unwrap();
        assert_eq!(email, "a@b.co");
        assert_eq!(path, "/My Drive/folder/sub folder/file name.png");
    }

    #[test]
    fn test_parse_gdrive_path_invalid() {
        assert!(parse_gdrive_path("not-a-url").is_err());
        assert!(parse_gdrive_path("gdrive:///no-host").is_err());
    }
}
