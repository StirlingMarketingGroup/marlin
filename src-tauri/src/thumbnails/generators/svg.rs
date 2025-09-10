use super::super::{ThumbnailRequest, ThumbnailFormat, ThumbnailQuality};
use image::{DynamicImage};
use std::fs;

pub struct SvgGenerator;

impl SvgGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<String, String> {
        let svg_data = fs::read(&request.path).map_err(|e| format!("Failed to read SVG: {}", e))?;

        // Parse SVG using resvg/usvg
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&svg_data, &opt)
            .map_err(|e| format!("SVG parse error: {:?}", e))?;

        // Determine output size while preserving aspect ratio
        let svg_size = tree.size();
        let (w, h) = (svg_size.width().max(1.0), svg_size.height().max(1.0));
        let target = request.size.max(1);
        let scale = (target as f32 / w).min(target as f32 / h);
        let out_w = (w * scale).round().max(1.0) as u32;
        let out_h = (h * scale).round().max(1.0) as u32;

        let mut pixmap = resvg::tiny_skia::Pixmap::new(out_w, out_h)
            .ok_or_else(|| "Failed to allocate pixmap".to_string())?;

        // Build transform to scale into our pixmap (top-left origin)
        let ts = resvg::tiny_skia::Transform::from_scale(scale, scale);
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

        let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(out_w, out_h, rgba)
            .ok_or_else(|| "Failed to create image buffer".to_string())?;
        let di = DynamicImage::ImageRgba8(img);

        super::ThumbnailGenerator::encode_to_data_url(
            &di,
            request.format,
            request.quality,
        )
    }
}
