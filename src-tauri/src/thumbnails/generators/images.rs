use std::path::Path;
use image::{io::Reader as ImageReader, DynamicImage};
use super::super::{ThumbnailRequest, get_thumbnail_format_from_path};
use super::ThumbnailGenerator;

pub struct ImageGenerator;

impl ImageGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<String, String> {
        let path = Path::new(&request.path);
        
        // Load the image
        let image = Self::load_image(path)?;
        
        // Resize the image
        let resized = ThumbnailGenerator::resize_image(image, request.size, request.quality)?;
        
        // Determine output format (prefer the format specified in request, or infer from path)
        let format = if request.format != super::super::ThumbnailFormat::WebP {
            request.format
        } else {
            get_thumbnail_format_from_path(path)
        };
        
        // Encode to data URL
        ThumbnailGenerator::encode_to_data_url(&resized, format, request.quality)
    }

    fn load_image(path: &Path) -> Result<DynamicImage, String> {
        // Use image crate's built-in format detection
        let reader = ImageReader::open(path)
            .map_err(|e| format!("Failed to open image file: {}", e))?;
        
        // Attempt to decode with format detection
        let image = reader
            .with_guessed_format()
            .map_err(|e| format!("Failed to detect image format: {}", e))?
            .decode()
            .map_err(|e| format!("Failed to decode image: {}", e))?;
        
        Ok(image)
    }

}