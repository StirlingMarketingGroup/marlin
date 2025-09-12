use std::path::Path;
use image::{io::Reader as ImageReader, DynamicImage};
use super::super::{ThumbnailRequest, get_thumbnail_format_from_path};
use super::ThumbnailGenerator;

pub struct ImageGenerator;

impl ImageGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<(String, bool), String> {
        let path = Path::new(&request.path);
        
        // Load the image
        let image = Self::load_image(path)?;
        
        // Check if the original image has transparency
        let has_transparency = Self::has_transparency(&image);
        
        // Resize the image
        let resized = ThumbnailGenerator::resize_image(image, request.size, request.quality)?;
        
        // Determine output format (prefer the format specified in request, or infer from path)
        let format = if request.format != super::super::ThumbnailFormat::WebP {
            request.format
        } else {
            get_thumbnail_format_from_path(path)
        };
        
        // Encode to data URL
        let data_url = ThumbnailGenerator::encode_to_data_url(&resized, format, request.quality)?;
        Ok((data_url, has_transparency))
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

    fn has_transparency(image: &DynamicImage) -> bool {
        use image::Pixel;
        
        // First check if the color type supports alpha
        if !image.color().has_alpha() {
            return false;
        }

        // Check if any pixels actually have transparency (alpha < 255)
        match image {
            DynamicImage::ImageRgba8(img) => {
                img.pixels().any(|pixel| pixel.channels()[3] < 255)
            }
            DynamicImage::ImageRgba16(img) => {
                img.pixels().any(|pixel| pixel.channels()[3] < 65535)
            }
            DynamicImage::ImageRgba32F(img) => {
                img.pixels().any(|pixel| pixel.channels()[3] < 1.0)
            }
            DynamicImage::ImageLumaA8(img) => {
                img.pixels().any(|pixel| pixel.channels()[1] < 255)
            }
            DynamicImage::ImageLumaA16(img) => {
                img.pixels().any(|pixel| pixel.channels()[1] < 65535)
            }
            _ => false, // For formats without alpha channel
        }
    }
}