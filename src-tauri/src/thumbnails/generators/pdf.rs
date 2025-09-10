use std::path::Path;
use image::{DynamicImage, RgbaImage};
use mupdf::Document;
use super::super::ThumbnailRequest;
use crate::thumbnails::generators::ThumbnailGenerator;

pub struct PdfGenerator;

impl PdfGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<String, String> {
        let path = Path::new(&request.path);
        
        if !path.exists() {
            return Err("PDF file does not exist".to_string());
        }

        // Use MuPDF for cross-platform PDF rendering
        Self::generate_with_mupdf(request)
    }

    fn generate_with_mupdf(request: &ThumbnailRequest) -> Result<String, String> {
        // Open the PDF document with MuPDF
        let doc = Document::open(&request.path)
            .map_err(|e| format!("Failed to open PDF: {:?}", e))?;
        
        // Get the first page
        let page = doc.load_page(0)
            .map_err(|e| format!("Failed to load first page: {:?}", e))?;
        
        // Get page bounds (in points, 72 points = 1 inch)
        let bounds = page.bounds()
            .map_err(|e| format!("Failed to get page bounds: {:?}", e))?;
        
        let page_width = bounds.x1 - bounds.x0;
        let page_height = bounds.y1 - bounds.y0;
        
        // Calculate scale to fit within requested size
        let scale = if page_width > page_height {
            request.size as f32 / page_width
        } else {
            request.size as f32 / page_height
        };
        
        // Create a transformation matrix for scaling
        let matrix = mupdf::Matrix::new_scale(scale, scale);
        
        // Render the page to a pixmap with white background
        // Using RGB format to avoid alpha channel issues
        let pixmap = page.to_pixmap(
            &matrix,
            &mupdf::Colorspace::device_rgb(),
            false, // no alpha channel (opaque background)
            true // interpolate
        ).map_err(|e| format!("Failed to render page: {:?}", e))?;
        
        // Get pixmap dimensions and data
        let width = pixmap.width() as u32;
        let height = pixmap.height() as u32;
        let stride = pixmap.stride() as usize;
        let n = pixmap.n() as usize; // number of components per pixel
        let samples = pixmap.samples();
        
        // Convert MuPDF pixmap to image::RgbaImage
        // MuPDF gives us RGB data, we need to convert to RGBA
        let mut rgba_buffer = Vec::with_capacity((width * height * 4) as usize);
        
        for y in 0..height {
            for x in 0..width {
                let pixel_offset = y as usize * stride + x as usize * n;
                
                if n == 3 {
                    // RGB format
                    rgba_buffer.push(samples[pixel_offset]);     // R
                    rgba_buffer.push(samples[pixel_offset + 1]); // G
                    rgba_buffer.push(samples[pixel_offset + 2]); // B
                    rgba_buffer.push(255);                        // A (fully opaque)
                } else if n == 4 {
                    // RGBA format
                    rgba_buffer.push(samples[pixel_offset]);     // R
                    rgba_buffer.push(samples[pixel_offset + 1]); // G
                    rgba_buffer.push(samples[pixel_offset + 2]); // B
                    rgba_buffer.push(samples[pixel_offset + 3]); // A
                } else if n == 1 {
                    // Grayscale
                    let gray = samples[pixel_offset];
                    rgba_buffer.push(gray); // R
                    rgba_buffer.push(gray); // G
                    rgba_buffer.push(gray); // B
                    rgba_buffer.push(255);   // A
                } else {
                    return Err(format!("Unsupported pixel format with {} components", n));
                }
            }
        }
        
        // Create an RGBA image from the buffer
        let rgba_image = RgbaImage::from_vec(width, height, rgba_buffer)
            .ok_or_else(|| "Failed to create image from PDF render data".to_string())?;
        
        // Convert to DynamicImage
        let dynamic_image = DynamicImage::ImageRgba8(rgba_image);
        
        // Ensure we're at the target size (might need additional resizing)
        let final_image = if width != request.size || height != request.size {
            ThumbnailGenerator::resize_image(dynamic_image, request.size, request.quality)?
        } else {
            dynamic_image
        };
        
        // Encode to data URL
        ThumbnailGenerator::encode_to_data_url(&final_image, request.format, request.quality)
    }
}