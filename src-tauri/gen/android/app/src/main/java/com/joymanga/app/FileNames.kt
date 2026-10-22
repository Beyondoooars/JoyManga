package com.joymanga.app

/**
 * 文件名的跨平台安全化处理。
 *
 * 导入/导出与更新包下载都要把外部给的名字落到本机文件系统上：去掉非法字符、
 * 限制长度（按 UTF-8 字节，避免超长文件名在某些 ROM 上写入失败）。
 */
internal fun safeFileName(name: String): String {
    val clean = name.replace(Regex("""[\\/:*?"<>|]"""), "_").trim().ifBlank { "file" }
    if (clean == "." || clean == "..") return "file"
    val extension = clean.substringAfterLast('.', "").lowercase()
    val suffix = if (extension in listOf("cbz", "json", "jpg", "jpeg", "png", "webp")) ".$extension" else ""
    val stem = if (suffix.isEmpty()) clean else clean.dropLast(extension.length + 1)
    val result = StringBuilder()
    var bytes = 0
    var index = 0
    val limit = 180 - suffix.toByteArray(Charsets.UTF_8).size
    while (index < stem.length) {
        val point = stem.codePointAt(index)
        val piece = String(Character.toChars(point))
        val count = piece.toByteArray(Charsets.UTF_8).size
        if (bytes + count > limit) break
        result.append(piece); bytes += count; index += Character.charCount(point)
    }
    return result.toString().ifBlank { "file" } + suffix
}
