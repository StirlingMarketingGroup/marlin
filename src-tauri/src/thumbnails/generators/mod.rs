use std::path::Path;
use image::{DynamicImage, ImageFormat, GenericImageView};
use std::io::Cursor;
use base64::Engine as _;

use super::{ThumbnailRequest, ThumbnailFormat, ThumbnailQuality};

pub mod images;
pub mod apps;

pub struct ThumbnailGenerator;

impl ThumbnailGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<String, String> {
        let path = Path::new(&request.path);
        
        if !path.exists() {
            return Err("File does not exist".to_string());
        }

        // Determine generator based on file type
        #[cfg(target_os = "macos")]
        {
            // Check for macOS special file types that have system icons
            if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
                let ext = extension.to_lowercase();
                if matches!(ext.as_str(), "app" | "dmg" | "pkg") {
                    return apps::MacAppGenerator::generate(request);
                }
            }
            
            // Also check .app directories (bundles)
            if path.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("app") {
                return apps::MacAppGenerator::generate(request);
            }
        }
        
        #[cfg(not(target_os = "macos"))]
        {
            if path.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("app") {
                return Err("App thumbnails only supported on macOS".to_string());
            }
        }

        // Check if it's an image file
        if Self::is_image_file(path) {
            return images::ImageGenerator::generate(request);
        }

        // TODO: Add support for PDFs, videos, documents
        Err("Unsupported file type for thumbnail generation".to_string())
    }

    fn is_image_file(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            matches!(extension.to_lowercase().as_str(), 
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tga" | "ico"
            )
        } else {
            false
        }
    }

    pub fn resize_image(
        image: DynamicImage, 
        target_size: u32, 
        quality: ThumbnailQuality
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
        quality: ThumbnailQuality
    ) -> Result<String, String> {
        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);

        match format {
            ThumbnailFormat::PNG => {
                image.write_to(&mut cursor, ImageFormat::Png)
                    .map_err(|e| format!("Failed to encode PNG: {}", e))?;
                Ok(format!("data:image/png;base64,{}", 
                   base64::engine::general_purpose::STANDARD.encode(&buffer)))
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
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_cursor, quality_value);
                rgb_image.write_with_encoder(encoder)
                    .map_err(|e| format!("Failed to encode JPEG: {}", e))?;
                
                Ok(format!("data:image/jpeg;base64,{}", 
                   base64::engine::general_purpose::STANDARD.encode(&buffer)))
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