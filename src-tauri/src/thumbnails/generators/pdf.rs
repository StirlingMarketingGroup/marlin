use super::super::{ThumbnailGenerationResult, ThumbnailRequest};
use crate::thumbnails::generators::ThumbnailGenerator;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};
use image::{DynamicImage, RgbaImage};

pub struct PdfGenerator;

impl PdfGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
        let data = std::fs::read(&request.path).map_err(|e| format!("Failed to read PDF: {e}"))?;
        let pdf = Pdf::new(data).map_err(|e| format!("Failed to open PDF: {:?}", e))?;
        let page = pdf
            .pages()
            .first()
            .ok_or_else(|| "Failed to load first page".to_string())?;

        let (page_width, page_height) = page.render_dimensions();
        let scale = if page_width > page_height {
            request.size as f32 / page_width
        } else {
            request.size as f32 / page_height
        };

        let pixmap = render(
            page,
            &RenderCache::new(),
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );

        // A white background makes every pixel opaque, so premultiplied and straight alpha match.
        let rgba_image = RgbaImage::from_raw(
            u32::from(pixmap.width()),
            u32::from(pixmap.height()),
            pixmap.data_as_u8_slice().to_vec(),
        )
        .ok_or_else(|| "Failed to create image from PDF render data".to_string())?;
        let image = DynamicImage::ImageRgba8(rgba_image);

        let data_url =
            ThumbnailGenerator::encode_to_data_url(&image, request.format, request.quality)?;

        // The caller wants the page size in points, not the thumbnail size.
        Ok(ThumbnailGenerationResult {
            data_url,
            has_transparency: false,
            image_width: Some(page_width.round() as u32),
            image_height: Some(page_height.round() as u32),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbnails::{ThumbnailFormat, ThumbnailQuality, ThumbnailRequest};
    use base64::Engine as _;
    use image::GenericImageView;

    const PAGE_PDF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pdf/page.pdf");
    const LOCKED_PDF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pdf/locked.pdf");

    fn request_for(path: &str) -> ThumbnailRequest {
        ThumbnailRequest {
            id: "test".to_string(),
            path: path.to_string(),
            size: 256,
            quality: ThumbnailQuality::Medium,
            format: ThumbnailFormat::PNG,
            priority: crate::thumbnails::ThumbnailPriority::High,
            accent: None,
        }
    }

    #[test]
    fn renders_the_first_page() {
        let result = PdfGenerator::generate(&request_for(PAGE_PDF)).expect("generate");
        assert!(result.data_url.starts_with("data:image/png;base64,"));
        assert_eq!(result.image_width, Some(200));
        assert_eq!(result.image_height, Some(100));

        let encoded = result
            .data_url
            .strip_prefix("data:image/png;base64,")
            .expect("prefix");
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("decode base64");
        let image = image::load_from_memory(&png).expect("decode png");
        assert_eq!(image.width(), 256);
        assert_eq!(image.height(), 128);

        let center = image.get_pixel(128, 64);
        assert!(center[0] > 200, "red channel {}", center[0]);
        assert!(center[1] < 60, "green channel {}", center[1]);
        assert!(center[2] < 60, "blue channel {}", center[2]);

        let top_left = image.get_pixel(0, 0);
        assert_eq!(top_left.0, [255, 255, 255, 255]);
    }

    #[test]
    fn a_password_protected_pdf_is_an_error() {
        let err = PdfGenerator::generate(&request_for(LOCKED_PDF)).unwrap_err();
        assert!(
            err.starts_with("Failed to open PDF"),
            "expected open error, got: {err}"
        );
    }

    #[test]
    fn a_damaged_or_empty_file_is_an_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let empty = dir.path().join("empty.pdf");
        let garbage = dir.path().join("garbage.pdf");
        std::fs::write(&empty, b"").expect("write empty");
        std::fs::write(&garbage, vec![0xA7; 300]).expect("write garbage");

        assert!(PdfGenerator::generate(&request_for(&empty.to_string_lossy())).is_err());
        assert!(PdfGenerator::generate(&request_for(&garbage.to_string_lossy())).is_err());
    }
}
