use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_IMAGES: usize = 4;
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagePart {
    pub mime: String,
    pub data: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImageError {
    #[error("too many images")]
    Count,
    #[error("unsupported image MIME")]
    Mime,
    #[error("invalid base64")]
    Encoding,
    #[error("image exceeds size limit")]
    Size,
}

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
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::Size);
        }
    }
    Ok(())
}

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
                data: "aGVsbG8=".into()
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
