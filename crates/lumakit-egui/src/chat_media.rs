//! 仅预览会话已登记的本地图片，不安装 Markdown 能调用的文件或网络加载器。
use std::{collections::VecDeque, io::{Cursor, Read}, path::{Path, PathBuf}};

use egui::{ColorImage, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use image::{ImageFormat, ImageReader, Limits};
use lumakit_ai::chat::{ChatSession, ImageRef};
use lumakit_ui::chat::ChatAction;

const MAX_INPUT: usize = 4 * 1024 * 1024;
const MAX_DECODE: u64 = 64 * 1024 * 1024;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_DIMENSION: u32 = 8192;

#[derive(Default)]
pub(super) struct MediaPreview {
    scope: PathBuf,
    entries: VecDeque<Preview>,
    texture_bytes: usize,
}

struct Preview {
    id: String,
    storage_key: String,
    bytes: u64,
    texture: Result<TextureHandle, &'static str>,
    texture_bytes: usize,
}

impl MediaPreview {
    pub(super) fn show(&mut self, ui: &mut Ui, session: &ChatSession, images: &[ImageRef], actions: &mut Vec<ChatAction>) {
        if images.is_empty() { return; }
        if self.scope != session.media_scope() {
            self.entries.clear();
            self.texture_bytes = 0;
            self.scope = session.media_scope().to_path_buf();
        }
        let gap = ui.spacing().item_spacing.x;
        let width = ui.available_width().max(1.0);
        let columns = ((width + gap) / (200.0 + gap)).floor().max(1.0) as usize;
        let tile_width = ((width - gap * (columns - 1) as f32) / columns as f32).min(280.0).max(1.0);
        for row in images.chunks(columns) {
            ui.horizontal_top(|ui| {
                for image in row {
                    ui.vertical(|ui| {
                        ui.set_width(tile_width);
                        let visible = ui.is_rect_visible(Rect::from_min_size(ui.next_widget_position(), Vec2::new(tile_width, 220.0)));
                        let index = self.entries.iter().position(|entry| entry.id == image.id && entry.storage_key == image.storage_key && entry.bytes == image.bytes);
                        if visible {
                            if let Some(index) = index {
                                if let Some(entry) = self.entries.remove(index) { self.entries.push_back(entry); }
                            } else {
                                self.load(ui, session, image);
                            }
                        }
                        let entry = if visible { self.entries.back() } else { index.and_then(|index| self.entries.get(index)) };
                        match entry.map(|entry| &entry.texture) {
                            Some(Ok(texture)) => {
                                let original = texture.size_vec2();
                                let size = original * (tile_width / original.x).min(220.0 / original.y).min(1.0);
                                if ui.add(egui::Image::new((texture.id(), size)).sense(Sense::click()).corner_radius(6)
                                    .alt_text("图片，点击打开")).on_hover_text("打开图片").clicked() {
                                    actions.push(ChatAction::OpenMedia(image.id.clone()));
                                }
                            }
                            Some(Err(error)) => { ui.label(egui::RichText::new(*error).weak()); }
                            None => { ui.allocate_space(Vec2::new(tile_width, 180.0)); }
                        }
                    });
                }
            });
        }
        ui.add_space(8.0);
    }

    // ponytail: 首次可见时同步解码，输入上限 4 MiB；若滚动实测卡顿，再移到后台。
    fn load(&mut self, ui: &Ui, session: &ChatSession, image: &ImageRef) {
        let decoded = session.media_path(&image.id).map_err(|_| "图片不可用：文件缺失或校验失败。")
            .and_then(|path| read_thumbnail(&path));
        let texture_bytes = decoded.as_ref().map_or(0, |image| image.pixels.len() * 4);
        while self.entries.len() >= MAX_ENTRIES || self.texture_bytes + texture_bytes > MAX_TEXTURE_BYTES {
            let Some(entry) = self.entries.pop_front() else { break; };
            self.texture_bytes -= entry.texture_bytes;
        }
        let texture = decoded.map(|image| ui.ctx().load_texture("chat-local-preview", image, TextureOptions::LINEAR));
        self.texture_bytes += texture_bytes;
        self.entries.push_back(Preview { id: image.id.clone(), storage_key: image.storage_key.clone(), bytes: image.bytes, texture, texture_bytes });
    }
}

fn read_thumbnail(path: &Path) -> Result<ColorImage, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "图片不可用：无法读取文件。")?;
    if !file.metadata().map_err(|_| "图片不可用：无法读取文件信息。")?.is_file() { return Err("图片不可用：不是普通文件。"); }
    let mut bytes = Vec::new();
    file.take((MAX_INPUT + 1) as u64).read_to_end(&mut bytes).map_err(|_| "图片不可用：读取失败。")?;
    decode_thumbnail(&bytes)
}

fn decode_thumbnail(bytes: &[u8]) -> Result<ColorImage, &'static str> {
    if bytes.len() > MAX_INPUT { return Err("图片不可预览：文件超过 4 MiB。"); }
    let format = image::guess_format(bytes).map_err(|_| "图片不可预览：文件损坏或格式不支持。")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP) {
        return Err("图片不可预览：仅支持 PNG、JPEG、GIF 和 WebP。");
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    // 全尺寸纹理会随历史增长挤满内存；只缓存有界缩略图，原图由显式打开动作处理。
    let decoded = reader.decode().map_err(|_| "图片不可预览：文件损坏或超过解码限制。")?;
    let thumbnail = if decoded.width() > 560 || decoded.height() > 440 { decoded.thumbnail(560, 440) } else { decoded }.into_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied([thumbnail.width() as usize, thumbnail.height() as usize], thumbnail.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn preview_decode_boundary() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&[255, 0, 0, 255, 0, 255, 0, 255], 2, 1, image::ExtendedColorType::Rgba8).unwrap();
        let preview = decode_thumbnail(&png).unwrap();
        assert_eq!(preview.size, [2, 1]);
        assert_eq!(preview.pixels[0], egui::Color32::RED);
        assert!(decode_thumbnail(b"not an image").is_err());
        assert!(decode_thumbnail(&vec![0; MAX_INPUT + 1]).is_err());
        let mut wide = Vec::new();
        image::codecs::png::PngEncoder::new(&mut wide).write_image(&vec![0; (MAX_DIMENSION as usize + 1) * 4], MAX_DIMENSION + 1, 1, image::ExtendedColorType::Rgba8).unwrap();
        assert!(decode_thumbnail(&wide).is_err());
        let temp = tempfile::tempdir().unwrap();
        assert!(read_thumbnail(&temp.path().join("missing.png")).is_err());
        assert!(read_thumbnail(temp.path()).is_err());
    }
}
