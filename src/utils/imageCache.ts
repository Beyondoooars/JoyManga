/**
 * 阅读器取图协议；与后端 `reader_commands::IMAGE_SCHEME` 保持一致。
 *
 * Android（以及 Windows）上 wry 无法直接加载自定义 scheme 的子资源，
 * 会把它重写成 `http://<scheme>.localhost/<path>`，所以这里直接用改写后的地址，
 * 后端协议处理器收到的则会是还原回的 `jmimg://localhost/<path>`。
 */
const IMAGE_HOST = 'http://jmimg.localhost'

/**
 * 把页面标识编码成阅读器可用的图片地址。
 *
 * 页面标识是绝对路径（`/storage/...`）或 `归档路径|条目名`。`encodeURIComponent`
 * 按 UTF-8 对 `/`、空格、`|`、中文等全部百分号编码，正好避免这些字符被 URL
 * 解析层改写；后端按百分号编码逐字节还原。`retryToken` 只用来在失败重试时
 * 换一个 URL 触发重新请求。
 */
export function imageUrlFor(filePath: string, retryToken = 0): string {
  return `${IMAGE_HOST}/${encodeURIComponent(filePath)}${retryToken > 0 ? `?retry=${retryToken}` : ''}`
}
