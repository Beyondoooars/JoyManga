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
 * 页面标识是绝对路径（`/storage/...`）或 `归档路径|条目名`，只保留
 * RFC 3986 的非保留字符，其余按 UTF-8 字节做百分号编码，避免中文/空格/`|`
 * 被 URL 解析改写。`retryToken` 只用来在失败重试时换一个 URL 触发重新请求。
 */
export function imageUrlFor(filePath: string, retryToken = 0): string {
  const encoded = new TextEncoder().encode(filePath)
    .reduce((text, byte) => {
      const character = String.fromCharCode(byte)
      return /[A-Za-z0-9\-._~]/.test(character)
        ? text + character
        : `${text}%${byte.toString(16).toUpperCase().padStart(2, '0')}`
    }, '')
  return `${IMAGE_HOST}/${encoded}${retryToken > 0 ? `?retry=${retryToken}` : ''}`
}
