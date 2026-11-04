use std::{collections::VecDeque, io::{Cursor, Read}, path::{Path, PathBuf}, sync::Arc};
use gpui_kit::RenderImage;
use image::{ImageFormat, ImageReader, Limits};
use lumakit_ai::chat::{ChatSession, ImageRef};

const MAX_INPUT: usize = 4 * 1024 * 1024;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
struct Preview {
    id: String, key: String, bytes: u64,
    image: Result<Arc<RenderImage>, &'static str>, texture_bytes: usize,
}
#[derive(Default)]
pub(crate) struct MediaPreview {
    scope: PathBuf,
    entries: VecDeque<Preview>,
    bytes: usize,
}
impl MediaPreview {
    pub fn get(&mut self, session: &ChatSession, image: &ImageRef) -> Result<Arc<RenderImage>, &'static str> {
        if self.scope != session.media_scope() {
            self.entries.clear(); self.bytes = 0;
            self.scope = session.media_scope().to_path_buf();
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.id == image.id && entry.key == image.storage_key && entry.bytes == image.bytes) {
            let entry = self.entries.remove(index).unwrap();
            let result = entry.image.clone();
            self.entries.push_back(entry);
            return result;
        }
        // ponytail: bounded first-use decoding on the UI thread; move to a worker
        // only if real-window scrolling shows stalls (4 MiB input, 64 MiB decode).
        let decoded = session.media_path(&image.id).map_err(|_| "图片不可用：文件缺失或校验失败。")
            .and_then(|path| read_thumbnail(&path));
        let texture_bytes = decoded.as_ref().map_or(0, |image| image.as_raw().len());
        while self.entries.len() >= 32 || self.bytes + texture_bytes > MAX_TEXTURE_BYTES {
            let Some(old) = self.entries.pop_front() else { break; };
            self.bytes -= old.texture_bytes;
        }
        let result = decoded.map(|mut rgba| {
            // RenderImage expects BGRA, unlike image's RGBA decoder output.
            for pixel in rgba.pixels_mut() { pixel.0.swap(0, 2); }
            Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]))
        });
        self.bytes += texture_bytes;
        self.entries.push_back(Preview { id: image.id.clone(), key: image.storage_key.clone(), bytes: image.bytes, image: result.clone(), texture_bytes });
        result
    }
}
fn read_thumbnail(path: &Path) -> Result<image::RgbaImage, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "图片不可用：无法读取文件。")?;
    if !file.metadata().map_err(|_| "图片不可用：无法读取文件信息。")?.is_file() { return Err("图片不可用：不是普通文件。"); }
    let mut bytes = Vec::new();
    file.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes).map_err(|_| "图片不可用：读取失败。")?;
    decode_thumbnail(&bytes)
}
fn decode_thumbnail(bytes: &[u8]) -> Result<image::RgbaImage, &'static str> {
    if bytes.len() > MAX_INPUT { return Err("图片不可预览：文件超过 4 MiB。"); }
    let format = image::guess_format(bytes).map_err(|_| "图片不可预览：文件损坏或格式不支持。")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP) {
        return Err("图片不可预览：仅支持 PNG、JPEG、GIF 和 WebP。");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| "图片不可预览：文件损坏或超过解码限制。")?;
    Ok(if decoded.width() > 560 || decoded.height() > 440 { decoded.thumbnail(560, 440) } else { decoded }.into_rgba8())
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    #[test]
    fn decoder_enforces_format_size_and_dimensions() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8).unwrap();
        assert_eq!(decode_thumbnail(&png).unwrap().dimensions(), (1, 1));
        assert!(decode_thumbnail(b"not an image").is_err());
        assert!(decode_thumbnail(&vec![0; MAX_INPUT + 1]).is_err());
        let mut wide = Vec::new();
        image::codecs::png::PngEncoder::new(&mut wide).write_image(&vec![0; 8193 * 4], 8193, 1, image::ExtendedColorType::Rgba8).unwrap();
        assert!(decode_thumbnail(&wide).is_err());
    }
}
