use super::super::ThumbnailRequest;

pub struct MacAppGenerator;

#[cfg(target_os = "macos")]
impl MacAppGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<(String, bool), String> {
        // Use the existing macOS app icon implementation
        let data_url = crate::macos_icons::app_icon_png_base64(&request.path, request.size)?;
        // App icons typically don't have transparency backgrounds
        Ok((data_url, false))
    }
}

#[cfg(not(target_os = "macos"))]
impl MacAppGenerator {
    pub fn generate(_request: &ThumbnailRequest) -> Result<(String, bool), String> {
        Err("macOS app icons only supported on macOS".to_string())
    }
}
