use super::{ThumbnailFormat, ThumbnailGenerationResult, ThumbnailQuality, ThumbnailRequest};
use base64::Engine as _;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::io::Cursor;
use std::path::Path;

#[cfg(target_os = "macos")]
use crate::macos_security;
#[cfg(target_os = "macos")]
pub mod apps;
pub mod fonts;
pub mod images;
pub mod pdf;
pub mod psd;
pub mod stl;
pub mod svg;
pub mod video;
pub mod zpl;

pub mod sftp;
pub mod smb;

/// Check if a file extension (without the dot) is a video format.
pub fn is_video_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
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
}

pub struct ThumbnailGenerator;

/// What `prepare` hands to `generate`: the request, and what the generator reads for it.
pub struct PreparedRequest {
    pub request: ThumbnailRequest,
    pub source: Source,
}

/// What the generator reads for a request.
pub enum Source {
    /// The file at `request.path`.
    File,
    /// A local copy of a remote or archived file. The worker deletes it once generation ends.
    Copy(std::path::PathBuf),
    /// A picture of the file that the file's remote made. The file itself is read only when the picture does not decode.
    #[cfg(unix)]
    RemotePicture(crate::mount_helper::RemotePicture),
}

impl ThumbnailGenerator {
    /// Async pre-processing phase: copies a remote or archived file to a local temp file, or fetches the remote's own picture of a file in a Google Drive mount.
    /// This runs in the async context (no thread blocking) and must be called
    /// BEFORE `generate()` which runs in `spawn_blocking`.
    pub async fn prepare(request: ThumbnailRequest) -> Result<PreparedRequest, String> {
        if request.path.starts_with("archive://") {
            let temp_path =
                crate::locations::archive::extract_archive_entry_to_temp(&request.path).await?;
            return Ok(PreparedRequest {
                request,
                source: Source::Copy(temp_path.into()),
            });
        }

        if request.path.starts_with("gdrive://") {
            let (email, path) = parse_gdrive_path(&request.path)?;
            let file_name = path.rsplit('/').next().unwrap_or_default();

            let file_id =
                crate::locations::gdrive::provider::get_file_id_by_path(&email, &path).await?;
            let temp_path = crate::locations::gdrive::provider::download_file_to_temp(
                &email, &file_id, file_name,
            )
            .await?;

            return Ok(PreparedRequest {
                request,
                source: Source::Copy(temp_path.into()),
            });
        }

        if sftp::is_sftp_path(&request.path) {
            let temp_path = sftp::download_sftp_file_async(&request.path).await?;
            return Ok(PreparedRequest {
                request,
                source: Source::Copy(temp_path.into()),
            });
        }

        // A file in a Google Drive mount gets Drive's own picture, so drawing the grid reads none of its bytes through the mount.
        #[cfg(unix)]
        if let Some(picture) =
            crate::mount_helper::remote_thumbnail(Path::new(&request.path), request.size).await
        {
            return Ok(PreparedRequest {
                request,
                source: Source::RemotePicture(picture),
            });
        }

        // SMB and local files: no async pre-processing needed
        Ok(PreparedRequest {
            request,
            source: Source::File,
        })
    }

    /// Synchronous generation phase, run on the blocking pool after `prepare()`.
    /// Reading a file in a mount, or an `smb://` file, can wait on the network.
    pub fn generate(prepared: &PreparedRequest) -> Result<ThumbnailGenerationResult, String> {
        let request = &prepared.request;
        match &prepared.source {
            Source::File if smb::is_smb_path(&request.path) => smb::generate_smb_thumbnail(request),
            Source::File => Self::generate_local(request),
            Source::Copy(copy) => Self::generate_local(&ThumbnailRequest {
                path: copy.to_string_lossy().into_owned(),
                ..request.clone()
            }),
            #[cfg(unix)]
            Source::RemotePicture(picture) => {
                match images::ImageGenerator::generate(&picture.file, request) {
                    Ok(mut made) => {
                        // The UI shows these as the file's size. The picture's own size is not the file's.
                        made.image_width = picture.dimensions.map(|dimensions| dimensions.width);
                        made.image_height = picture.dimensions.map(|dimensions| dimensions.height);
                        Ok(made)
                    }
                    Err(error) => {
                        log::debug!("The remote's picture could not be read, so the file makes the thumbnail: {error}");
                        Self::generate_local(request)
                    }
                }
            }
        }
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
            return images::ImageGenerator::generate(path, request);
        }

        // Check if it's an SVG file
        if Self::is_svg_file(path) {
            return svg::SvgGenerator::generate(request);
        }

        // Check if it's a PDF file (includes AI)
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
                "pdf" | "ai" // PDF and Adobe Illustrator files
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
        path.extension()
            .and_then(|s| s.to_str())
            .map_or(false, is_video_extension)
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
    crate::locations::gdrive::parse_gdrive_uri(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gdrive_path_basic() {
        let (email, path) = parse_gdrive_path("gdrive://brian@smg.gg/My Drive/file.zpl").unwrap();
        assert_eq!(email, "brian@smg.gg");
        assert_eq!(path, "/My Drive/file.zpl");
    }

    #[test]
    fn test_parse_gdrive_path_spaces() {
        let (email, path) =
            parse_gdrive_path("gdrive://brian@smg.gg/My Drive/shipping-label-fedex real test.zpl")
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
        let (email, path) =
            parse_gdrive_path("gdrive://a@b.co/My Drive/folder/sub folder/file name.png").unwrap();
        assert_eq!(email, "a@b.co");
        assert_eq!(path, "/My Drive/folder/sub folder/file name.png");
    }

    #[test]
    fn test_parse_gdrive_path_invalid() {
        assert!(parse_gdrive_path("not-a-url").is_err());
        assert!(parse_gdrive_path("gdrive:///no-host").is_err());
    }

    #[test]
    fn pdf_and_ai_go_to_the_pdf_generator() {
        assert!(!ThumbnailGenerator::is_pdf_file(Path::new("a.eps")));
        assert!(ThumbnailGenerator::is_pdf_file(Path::new("a.pdf")));
        assert!(ThumbnailGenerator::is_pdf_file(Path::new("a.AI")));
    }
}
