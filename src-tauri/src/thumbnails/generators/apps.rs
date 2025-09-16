use super::super::ThumbnailRequest;

#[cfg(target_os = "macos")]
pub fn generate(request: &ThumbnailRequest) -> Result<(String, bool), String> {
    let data_url = crate::macos_icons::app_icon_png_base64(&request.path, request.size)?;
    Ok((data_url, false))
}

#[cfg(not(target_os = "macos"))]
pub fn generate(_request: &ThumbnailRequest) -> Result<(String, bool), String> {
    Err("macOS app icons only supported on macOS".to_string())
}
