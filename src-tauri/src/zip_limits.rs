//! CBZ 写入前的体积边界检查。
//!
//! `zip` 2.x 的 `SimpleFileOptions::default()` 里 `large_file = false`，
//! 单个条目超过 4 GiB 时写盘会直接失败（`Large file option has not been set`），
//! 且报错发生在复制到一半时，临时文件已经写了大半。
//!
//! 正常漫画页远达不到这个体积，但超长条图/异常文件存在理论可能。
//! 这里在写条目之前先查文件大小，超限时给出可读错误而不是让整次
//! 合并/导出失败在半路。

/// ZIP32 单条目上限（未启用 ZIP64 时）。
pub const MAX_ENTRY_BYTES: u64 = u32::MAX as u64;

/// 图片条目的写入选项：不压缩（Stored）。
///
/// 漫画页都是 JPEG/PNG/WebP/GIF，本身已是熵编码数据，deflate 收益约 0%
/// 却要消耗整章打包的 CPU。元数据（XML/JSON）仍走默认的 Deflate。
pub fn image_entry_options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

/// 检查待写入条目的体积；超限时返回带文件名与体积的可读错误。
pub fn ensure_entry_size(path: &std::path::Path) -> anyhow::Result<()> {
    let size = std::fs::metadata(path)
        .map_err(|err| anyhow::anyhow!("读取图片`{}`大小失败: {err}", path.display()))?
        .len();
    ensure_size(size, &path.display().to_string())
}

/// 从已有归档复制条目时调用：用归档头里声明的解压后大小判断，
/// 不需要先读完整条数据。
pub fn ensure_archive_entry_size<R: std::io::Read>(
    entry: &zip::read::ZipFile<'_, R>,
    name: &str,
) -> anyhow::Result<()> {
    ensure_size(entry.size(), name)
}

fn ensure_size(size: u64, label: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        size <= MAX_ENTRY_BYTES,
        "图片`{label}`大小为 {:.1} GiB，超过 CBZ 单条目上限，请先压缩或裁切后再归档",
        size as f64 / 1024.0 / 1024.0 / 1024.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_file_passes() {
        let path = std::env::temp_dir().join(format!("jm-size-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"small").unwrap();
        assert!(ensure_entry_size(&path).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reports_readable_error() {
        let path = std::env::temp_dir().join("jm-size-does-not-exist");
        let error = ensure_entry_size(&path).unwrap_err();
        assert!(error.to_string().contains("读取图片"));
    }
}
