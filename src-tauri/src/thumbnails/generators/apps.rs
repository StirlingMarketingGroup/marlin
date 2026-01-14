use super::super::{ThumbnailGenerationResult, ThumbnailRequest};

#[cfg(target_os = "macos")]
pub fn generate(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
    let data_url = crate::macos_icons::app_icon_png_base64(&request.path, request.size)?;
    // App icons don't have inherent dimensions like images
    Ok(ThumbnailGenerationResult {
        data_url,
        has_transparency: false,
        image_width: None,
        image_height: None,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn generate(_request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
    Err("macOS app icons only supported on macOS".to_string())
}
