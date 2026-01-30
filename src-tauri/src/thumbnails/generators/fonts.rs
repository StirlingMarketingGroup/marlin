use super::super::{ThumbnailGenerationResult, ThumbnailRequest};
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{DynamicImage, Rgba, RgbaImage};
use std::fs;

pub struct FontGenerator;

impl FontGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let font_data =
            fs::read(&request.path).map_err(|e| format!("Failed to read font file: {}", e))?;

        let font = FontRef::try_from_slice(&font_data)
            .map_err(|e| format!("Failed to parse font: {}", e))?;

        let size = request.size;

        // Create a white background image
        let mut img = RgbaImage::from_pixel(size, size, Rgba([255u8, 255u8, 255u8, 255u8]));

        // Sample text that shows character variety (uppercase, lowercase, descender)
        let sample_text = "Ag";

        // Calculate font size to fit nicely in the thumbnail (use ~60% of the size)
        let target_height = size as f32 * 0.55;

        // Get font metrics to determine proper scaling
        let scale = Self::calculate_scale_for_height(&font, target_height);
        let scaled_font = font.as_scaled(scale);

        // Calculate text width for centering
        let mut text_width = 0.0f32;
        let mut prev_glyph: Option<ab_glyph::GlyphId> = None;

        for c in sample_text.chars() {
            let glyph_id = scaled_font.glyph_id(c);
            if let Some(prev) = prev_glyph {
                text_width += scaled_font.kern(prev, glyph_id);
            }
            text_width += scaled_font.h_advance(glyph_id);
            prev_glyph = Some(glyph_id);
        }

        // Calculate baseline position for vertical centering
        let ascent = scaled_font.ascent();
        let descent = scaled_font.descent();
        let text_height = ascent - descent;

        // Center horizontally and vertically
        let start_x = (size as f32 - text_width) / 2.0;
        let baseline_y = (size as f32 + text_height) / 2.0 - descent.abs();

        // Draw each character
        let mut cursor_x = start_x;
        let mut prev_glyph: Option<ab_glyph::GlyphId> = None;

        for c in sample_text.chars() {
            let glyph_id = scaled_font.glyph_id(c);

            // Apply kerning
            if let Some(prev) = prev_glyph {
                cursor_x += scaled_font.kern(prev, glyph_id);
            }

            // Position the glyph
            let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));

            // Draw the glyph
            if let Some(outlined) = scaled_font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|x, y, coverage| {
                    let px = (bounds.min.x as i32 + x as i32) as u32;
                    let py = (bounds.min.y as i32 + y as i32) as u32;

                    if px < size && py < size {
                        // Anti-aliased black text on white background
                        let alpha = (coverage * 255.0) as u8;
                        if alpha > 0 {
                            let pixel = img.get_pixel_mut(px, py);
                            // Blend black text onto white background
                            let bg = 255u8;
                            let fg = 0u8;
                            let blend = |bg: u8, fg: u8, a: u8| -> u8 {
                                let a_f = a as f32 / 255.0;
                                ((fg as f32 * a_f) + (bg as f32 * (1.0 - a_f))) as u8
                            };
                            *pixel = Rgba([
                                blend(bg, fg, alpha),
                                blend(bg, fg, alpha),
                                blend(bg, fg, alpha),
                                255,
                            ]);
                        }
                    }
                });
            }

            cursor_x += scaled_font.h_advance(glyph_id);
            prev_glyph = Some(glyph_id);
        }

        let di = DynamicImage::ImageRgba8(img);
        let data_url =
            super::ThumbnailGenerator::encode_to_data_url(&di, request.format, request.quality)?;

        Ok(ThumbnailGenerationResult {
            data_url,
            has_transparency: false,
            image_width: Some(size),
            image_height: Some(size),
        })
    }

    /// Calculate the scale needed to achieve approximately the target height
    fn calculate_scale_for_height(font: &FontRef, target_height: f32) -> PxScale {
        // Start with a reference scale
        let ref_scale = PxScale::from(100.0);
        let scaled = font.as_scaled(ref_scale);

        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let actual_height = ascent - descent;

        // Calculate the scale needed for target height
        let scale_factor = target_height / actual_height * 100.0;
        PxScale::from(scale_factor)
    }
}
