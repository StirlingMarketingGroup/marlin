use super::super::{ThumbnailFormat, ThumbnailQuality, ThumbnailRequest};
use super::ThumbnailGenerator;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::download::{download_ffmpeg_package, ffmpeg_download_url, unpack_ffmpeg};
use image::DynamicImage;
use once_cell::sync::OnceCell;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct VideoGenerator;

impl VideoGenerator {
    const ANALYSIS_BATCH: u32 = 120;
    const MIN_SCALE: u32 = 160;
    const MAX_SCALE: u32 = 1920;

    pub fn generate(request: &ThumbnailRequest) -> Result<(String, bool), String> {
        let path = Path::new(&request.path);

        if !path.exists() {
            return Err("Video file does not exist".to_string());
        }

        let ffmpeg_path = Self::ensure_ffmpeg()?;

        let frame_bytes = Self::extract_frame(&ffmpeg_path, path, request)?;
        let frame = image::load_from_memory(&frame_bytes)
            .map_err(|e| format!("Failed to decode FFmpeg output: {e}"))?;

        Self::encode_thumbnail(frame, request)
    }

    fn encode_thumbnail(
        frame: DynamicImage,
        request: &ThumbnailRequest,
    ) -> Result<(String, bool), String> {
        let resized = ThumbnailGenerator::resize_image(frame, request.size, request.quality)?;

        let target_format = match request.format {
            ThumbnailFormat::WebP | ThumbnailFormat::PNG | ThumbnailFormat::JPEG => request.format,
        };

        let data_url =
            ThumbnailGenerator::encode_to_data_url(&resized, target_format, request.quality)?;
        Ok((data_url, false))
    }

    fn extract_frame(
        ffmpeg_path: &Path,
        video_path: &Path,
        request: &ThumbnailRequest,
    ) -> Result<Vec<u8>, String> {
        let filter = Self::build_filter(request.size, request.quality);

        let mut command = FfmpegCommand::new_with_path(ffmpeg_path);
        command.hide_banner();
        command.args(["-loglevel", "error"]);
        command.args(["-nostdin"]);

        // Input video
        command.args(["-i", Self::path_as_str(video_path)?]);

        // Filter selection and frame extraction
        command.args(["-vf", &filter]);
        command.args(["-frames:v", "1"]);

        // Output single image to stdout as PNG for lossless decode
        command.args(["-f", "image2pipe", "-vcodec", "png"]);
        command.pipe_stdout();

        let output = command
            .as_inner_mut()
            .output()
            .map_err(|e| format!("Failed to execute FFmpeg: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "FFmpeg exited with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        let buffer = output.stdout;

        if buffer.is_empty() {
            return Err("FFmpeg returned no data".to_string());
        }

        Ok(buffer)
    }

    fn build_filter(size: u32, quality: ThumbnailQuality) -> String {
        let batch = Self::ANALYSIS_BATCH;
        let scale_width = Self::scale_width(size, quality);
        format!("thumbnail={batch},scale={scale_width}:-1:force_original_aspect_ratio=decrease")
    }

    fn scale_width(size: u32, quality: ThumbnailQuality) -> u32 {
        let multiplier = match quality {
            ThumbnailQuality::Low => 1,
            ThumbnailQuality::Medium => 2,
            ThumbnailQuality::High => 3,
        };
        let widened = size.saturating_mul(multiplier).max(Self::MIN_SCALE);
        widened.min(Self::MAX_SCALE)
    }

    fn ensure_ffmpeg() -> Result<PathBuf, String> {
        static INIT: OnceCell<PathBuf> = OnceCell::new();
        INIT.get_or_try_init(|| Self::prepare_ffmpeg_binary())
            .cloned()
    }

    fn prepare_ffmpeg_binary() -> Result<PathBuf, String> {
        let target_dir = Self::ffmpeg_store_dir()?;
        fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Failed to create FFmpeg directory: {e}"))?;

        let binary_path = target_dir.join(Self::ffmpeg_binary_name());
        if binary_path.exists() {
            return Ok(binary_path);
        }

        let download_url = ffmpeg_download_url()
            .map_err(|e| format!("Unable to determine FFmpeg download URL: {e}"))?;
        let archive_path = download_ffmpeg_package(download_url, &target_dir)
            .map_err(|e| format!("Failed to download FFmpeg: {e}"))?;
        unpack_ffmpeg(&archive_path, &target_dir)
            .map_err(|e| format!("Failed to unpack FFmpeg: {e}"))?;

        if !binary_path.exists() {
            return Err("FFmpeg binary missing after download".to_string());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&binary_path)
                .map_err(|e| format!("Failed to read FFmpeg permissions: {e}"))?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&binary_path, perms)
                .map_err(|e| format!("Failed to set FFmpeg permissions: {e}"))?;
        }

        Ok(binary_path)
    }

    fn ffmpeg_store_dir() -> Result<PathBuf, String> {
        let base = dirs::cache_dir()
            .or_else(|| dirs::data_dir())
            .or_else(|| dirs::home_dir())
            .ok_or_else(|| "Unable to determine a writable cache directory".to_string())?;
        Ok(base.join("marlin").join("ffmpeg"))
    }

    fn ffmpeg_binary_name() -> &'static str {
        if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        }
    }

    fn path_as_str(path: &Path) -> Result<&str, String> {
        path.to_str().ok_or_else(|| {
            format!(
                "Video path contains invalid UTF-8: {}",
                Self::display_path(path)
            )
        })
    }

    fn display_path(path: &Path) -> String {
        path.to_string_lossy().chars().take(200).collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::VideoGenerator;
    use crate::thumbnails::ThumbnailQuality;

    #[test]
    fn scale_width_bounds() {
        use ThumbnailQuality::{High, Low, Medium};
        assert_eq!(VideoGenerator::scale_width(64, Low), 160);
        assert_eq!(VideoGenerator::scale_width(120, Medium), 240);
        assert_eq!(VideoGenerator::scale_width(800, High), 1920);
    }
}
