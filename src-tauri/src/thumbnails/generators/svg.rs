use super::super::{ThumbnailGenerationResult, ThumbnailRequest};
use image::DynamicImage;
use std::fs;

pub struct SvgGenerator;

impl SvgGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let svg_data = fs::read(&request.path).map_err(|e| format!("Failed to read SVG: {}", e))?;

        // Parse SVG using resvg/usvg
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&svg_data, &opt)
            .map_err(|e| format!("SVG parse error: {:?}", e))?;

        // Determine output size while preserving aspect ratio
        let svg_size = tree.size();
        let (w, h) = (svg_size.width().max(1.0), svg_size.height().max(1.0));
        let target = request.size.max(1);
        // Leave a small padding inside the square so logos don't touch the edges
        let padding = (target as f32 * 0.08).round();
        let inner = (target as f32 - 2.0 * padding).max(1.0);
        let scale = (inner / w).min(inner / h);
        let scaled_w = w * scale;
        let scaled_h = h * scale;

        // Always render to a square pixmap
        let mut pixmap = resvg::tiny_skia::Pixmap::new(target, target)
            .ok_or_else(|| "Failed to allocate pixmap".to_string())?;

        // Build transform to scale into our pixmap and center with padding
        let mut ts = resvg::tiny_skia::Transform::from_scale(scale, scale);
        let tx = ((target as f32 - scaled_w) * 0.5).round();
        let ty = ((target as f32 - scaled_h) * 0.5).round();
        ts = ts.post_translate(tx, ty);
        let mut pmut = pixmap.as_mut();
        resvg::render(&tree, ts, &mut pmut);

        // Convert premultiplied BGRA to straight RGBA
        let data = pixmap.data();
        let mut rgba = Vec::with_capacity(data.len());
        for px in data.chunks_exact(4) {
            let r = px[0] as u32;
            let g = px[1] as u32;
            let b = px[2] as u32;
            let a = px[3] as u32;
            if a == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let ur = ((r * 255 + a / 2) / a).min(255) as u8;
                let ug = ((g * 255 + a / 2) / a).min(255) as u8;
                let ub = ((b * 255 + a / 2) / a).min(255) as u8;
                rgba.extend_from_slice(&[ur, ug, ub, a as u8]);
            }
        }

        let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(target, target, rgba.clone())
            .ok_or_else(|| "Failed to create image buffer".to_string())?;
        let di = DynamicImage::ImageRgba8(img);

        // Check for transparency by looking for any alpha values < 255
        let has_transparency = rgba.chunks_exact(4).any(|pixel| pixel[3] < 255);

        let data_url =
            super::ThumbnailGenerator::encode_to_data_url(&di, request.format, request.quality)?;

        // SVG dimensions from the parsed tree (in pixels)
        Ok(ThumbnailGenerationResult {
            data_url,
            has_transparency,
            image_width: Some(w.round() as u32),
            image_height: Some(h.round() as u32),
        })
    }
}
