use anyhow::{ensure, Context};
use std::io::{Cursor, Read};
use std::path::Path;

pub const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 16_000_000;

pub fn read_limited(reader: impl Read, declared_size: u64, limit: u64) -> anyhow::Result<Vec<u8>> {
    ensure!(
        declared_size <= limit,
        "文件超过大小上限（{} MB）",
        limit / 1024 / 1024
    );
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context("读取文件失败")?;
    ensure!(bytes.len() as u64 <= limit, "解压内容超过大小上限");
    Ok(bytes)
}

pub fn read_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let bytes = read_limited(&file, file.metadata()?.len(), MAX_PAGE_BYTES)?;
    validate_image(&bytes)?;
    Ok(bytes)
}

pub fn validate_image(bytes: &[u8]) -> anyhow::Result<()> {
    let (width, height) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()
        .context("无法识别图片内容")?;
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS,
        "图片像素过大，请使用较小的图片（上限 {} 万像素）",
        MAX_IMAGE_PIXELS / 10_000
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_read_limit_is_enforced_even_when_declared_size_is_small() {
        assert!(read_limited(&b"12345"[..], 1, 4).is_err());
        assert_eq!(read_limited(&b"1234"[..], 4, 4).unwrap(), b"1234");
        assert!(read_limited(&b"x"[..], u64::MAX, 4).is_err());
    }
}
