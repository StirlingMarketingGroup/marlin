use super::super::{ThumbnailGenerationResult, ThumbnailRequest};
use super::ThumbnailGenerator;
use image::{DynamicImage, RgbaImage};
use psd::Psd;
use std::panic::AssertUnwindSafe;
use std::path::Path;

/// Maximum file size for PSD thumbnail generation (100 MB)
const MAX_PSD_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum pixel count to prevent OOM (50 megapixels = ~200MB RGBA buffer)
const MAX_PIXEL_COUNT: u64 = 50_000_000;

pub struct PsdGenerator;

impl PsdGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let path = Path::new(&request.path);

        // Check file size before reading to prevent OOM on large files
        let metadata = std::fs::metadata(path)
            .map_err(|e| format!("Failed to read PSD metadata for {}: {}", path.display(), e))?;

        if metadata.len() > MAX_PSD_FILE_SIZE {
            return Err(format!(
                "PSD file too large for thumbnail generation: {} MB (limit: {} MB)",
                metadata.len() / (1024 * 1024),
                MAX_PSD_FILE_SIZE / (1024 * 1024)
            ));
        }

        // Parse PSD in a block so file_data is dropped before rgba() allocation
        let psd = {
            let file_data = std::fs::read(path)
                .map_err(|e| format!("Failed to read PSD file {}: {}", path.display(), e))?;

            Psd::from_bytes(&file_data)
                .map_err(|e| format!("Failed to parse PSD file {}: {}", path.display(), e))?
        };

        // Get dimensions and validate pixel count before allocating RGBA buffer
        let width = psd.width();
        let height = psd.height();
        let pixel_count = (width as u64).saturating_mul(height as u64);

        if pixel_count > MAX_PIXEL_COUNT {
            return Err(format!(
                "PSD dimensions too large for thumbnail generation: {}x{} ({} megapixels, limit: {} megapixels)",
                width,
                height,
                pixel_count / 1_000_000,
                MAX_PIXEL_COUNT / 1_000_000
            ));
        }

        // Get the pre-flattened composite image stored in the PSD
        // This is the merged/flattened image that Photoshop stores for quick preview
        // Wrap in catch_unwind to handle panics from malformed PSD files gracefully
        let rgba_data = std::panic::catch_unwind(AssertUnwindSafe(|| psd.rgba())).map_err(|_| {
            format!(
                "Failed to extract image data from PSD (possibly corrupt): {}",
                path.display()
            )
        })?;

        // Create an RGBA image from the composite data
        let rgba_image = RgbaImage::from_raw(width, height, rgba_data).ok_or_else(|| {
            format!(
                "Failed to create image from PSD data for {}: expected {} bytes for {}x{}",
                path.display(),
                width as u64 * height as u64 * 4,
                width,
                height
            )
        })?;

        let dynamic_image = DynamicImage::ImageRgba8(rgba_image);

        // Check for transparency
        let has_transparency = Self::has_transparency(&dynamic_image);

        // Resize the image
        let resized =
            ThumbnailGenerator::resize_image(dynamic_image, request.size, request.quality)?;

        // Encode to data URL
        let data_url =
            ThumbnailGenerator::encode_to_data_url(&resized, request.format, request.quality)?;

        Ok(ThumbnailGenerationResult {
            data_url,
            has_transparency,
            image_width: Some(width),
            image_height: Some(height),
        })
    }

    fn has_transparency(image: &DynamicImage) -> bool {
        // Check if any pixels actually have transparency (alpha < 255)
        // This generator always produces ImageRgba8, so we only handle that case
        match image {
            DynamicImage::ImageRgba8(img) => img.pixels().any(|pixel| pixel[3] < 255),
            _ => false,
        }
    }
}
