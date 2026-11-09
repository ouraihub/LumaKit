use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 一次校验允许的最多图片数。
pub const MAX_IMAGES: usize = 4;
/// 单张图片解码后允许的最大字节数（4 MiB）。
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// 以 MIME 类型和标准 Base64 数据表示的内嵌图片。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagePart {
    /// 声明的图片 MIME 类型，不根据文件内容推断。
    pub mime: String,
    /// 不含 data URL 前缀的标准 Base64 数据。
    pub data: String,
}

/// 图片数量、声明类型、编码、文件签名或解码大小不符合限制。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImageError {
    /// 图片数量超过上限。
    #[error("too many images")]
    Count,
    /// MIME 类型不是 PNG、JPEG、WebP 或 GIF。
    #[error("unsupported image MIME")]
    Mime,
    /// 数据无法按标准 Base64 解码。
    #[error("invalid base64")]
    Encoding,
    /// 单张图片解码后的字节数超过上限。
    #[error("image exceeds size limit")]
    Size,
    /// 图片字节不是声明 MIME 对应的常见文件格式。
    #[error("image bytes do not match MIME")]
    Magic,
}

/// 校验数量、MIME 声明、Base64 编码、文件签名及单图解码大小，返回首个错误。
pub fn validate(images: &[ImagePart]) -> Result<(), ImageError> {
    if images.len() > MAX_IMAGES {
        return Err(ImageError::Count);
    }
    for image in images {
        if !matches!(
            image.mime.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return Err(ImageError::Mime);
        }
        let bytes = STANDARD
            .decode(&image.data)
            .map_err(|_| ImageError::Encoding)?;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::Size);
        }
        let valid = match image.mime.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            _ => false,
        };
        if !valid {
            return Err(ImageError::Magic);
        }
    }
    Ok(())
}

/// 将已验证的图片字节编码为内嵌图片。
pub fn from_bytes(mime: impl Into<String>, bytes: &[u8]) -> Result<ImagePart, ImageError> {
    let image = ImagePart {
        mime: mime.into(),
        data: STANDARD.encode(bytes),
    };
    validate(std::slice::from_ref(&image))?;
    Ok(image)
}

/// 拼接图片 data URL，不执行校验或重新编码。
pub fn data_url(image: &ImagePart) -> String {
    format!("data:{};base64,{}", image.mime, image.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_limits_and_mime() {
        assert!(
            validate(&[ImagePart {
                mime: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            }])
            .is_ok()
        );
        assert_eq!(
            validate(&[ImagePart {
                mime: "text/plain".into(),
                data: "aA==".into()
            }]),
            Err(ImageError::Mime)
        );
    }
}
