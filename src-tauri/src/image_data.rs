use anyhow::Context;
use std::fmt;
use std::io::{Cursor, Read};
use std::path::Path;

pub const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;
pub const MAX_IMAGE_DIMENSION: u32 = 65_535;
pub const MAX_DECODE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageDataError {
    FileTooLarge {
        limit_bytes: u64,
    },
    PixelsTooLarge {
        width: u32,
        height: u32,
        max_pixels: u64,
    },
    InvalidImage(String),
}

impl fmt::Display for ImageDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooLarge { limit_bytes } => {
                if *limit_bytes >= 1024 * 1024 {
                    write!(f, "文件超过大小上限（{} MB）", limit_bytes / 1024 / 1024)
                } else {
                    write!(f, "文件超过大小上限（{limit_bytes} 字节）")
                }
            }
            Self::PixelsTooLarge {
                width,
                height,
                max_pixels,
            } => write!(
                f,
                "图片像素过大（{width}×{height}），请使用较小的图片（上限 {} 万像素）",
                max_pixels / 10_000
            ),
            Self::InvalidImage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ImageDataError {}

pub fn image_decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    limits
}

pub fn read_limited(reader: impl Read, declared_size: u64, limit: u64) -> anyhow::Result<Vec<u8>> {
    if declared_size > limit {
        return Err(anyhow::Error::new(ImageDataError::FileTooLarge {
            limit_bytes: limit,
        }));
    }
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context("读取文件失败")?;
    if bytes.len() as u64 > limit {
        return Err(anyhow::Error::new(ImageDataError::FileTooLarge {
            limit_bytes: limit,
        }));
    }
    Ok(bytes)
}

pub fn read_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let bytes = read_limited(&file, file.metadata()?.len(), MAX_PAGE_BYTES)?;
    validate_image(&bytes)?;
    Ok(bytes)
}

pub fn validate_image(bytes: &[u8]) -> anyhow::Result<()> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| anyhow::Error::new(ImageDataError::InvalidImage("无法识别图片内容".into())))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| anyhow::Error::new(ImageDataError::InvalidImage("无法识别图片内容".into())))?;
    if width == 0 || height == 0 {
        return Err(anyhow::Error::new(ImageDataError::InvalidImage(
            "图片尺寸无效".into(),
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(anyhow::Error::new(ImageDataError::PixelsTooLarge {
            width,
            height,
            max_pixels: MAX_IMAGE_PIXELS,
        }));
    }
    Ok(())
}

pub fn decode_image_limited(bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    validate_image(bytes)?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| anyhow::Error::new(ImageDataError::InvalidImage("无法识别图片内容".into())))?;
    reader.limits(image_decode_limits());
    reader.decode().context("解码图片失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_png_header(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc: u32 = 0xffff_ffff;
            for &b in bytes {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[0, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    #[test]
    fn actual_read_limit_is_enforced_even_when_declared_size_is_small() {
        let small_overrun = read_limited(&b"12345"[..], 1, 4).unwrap_err();
        assert_eq!(
            small_overrun.downcast_ref::<ImageDataError>(),
            Some(&ImageDataError::FileTooLarge { limit_bytes: 4 })
        );
        assert_eq!(read_limited(&b"1234"[..], 4, 4).unwrap(), b"1234");
        let declared_overrun = read_limited(&b"x"[..], u64::MAX, 4).unwrap_err();
        assert_eq!(
            declared_overrun.downcast_ref::<ImageDataError>(),
            Some(&ImageDataError::FileTooLarge { limit_bytes: 4 })
        );
    }

    #[test]
    fn pixel_limit_allows_64mp_and_rejects_larger_or_zero_dimensions() {
        assert_eq!(MAX_IMAGE_PIXELS, 64_000_000);
        // 8000 x 8000 = 64_000_000 (放宽后的精确上限)
        assert!(validate_image(&minimal_png_header(8000, 8000)).is_ok());
        // 长条漫 1000 x 60000 = 60_000_000 也应放行
        assert!(validate_image(&minimal_png_header(1000, 60_000)).is_ok());

        let err = validate_image(&minimal_png_header(8001, 8000)).unwrap_err();
        assert_eq!(
            err.downcast_ref::<ImageDataError>(),
            Some(&ImageDataError::PixelsTooLarge {
                width: 8001,
                height: 8000,
                max_pixels: MAX_IMAGE_PIXELS,
            })
        );

        let zero_dim = validate_image(&minimal_png_header(0, 800)).unwrap_err();
        assert!(matches!(
            zero_dim.downcast_ref::<ImageDataError>(),
            Some(ImageDataError::InvalidImage(_))
        ));

        let garbage = validate_image(b"not an image").unwrap_err();
        assert!(matches!(
            garbage.downcast_ref::<ImageDataError>(),
            Some(ImageDataError::InvalidImage(_))
        ));
    }
}
