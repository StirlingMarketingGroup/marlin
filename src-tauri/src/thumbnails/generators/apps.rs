use super::super::ThumbnailRequest;

pub struct MacAppGenerator;

#[cfg(target_os = "macos")]
impl MacAppGenerator {
    pub fn generate(request: &ThumbnailRequest) -> Result<String, String> {
        // Use the existing macOS app icon implementation
        crate::macos_icons::app_icon_png_base64(&request.path, request.size)
    }
}

#[cfg(not(target_os = "macos"))]
impl MacAppGenerator {
    pub fn generate(_request: &ThumbnailRequest) -> Result<String, String> {
        Err("macOS app icons only supported on macOS".to_string())
    }
}