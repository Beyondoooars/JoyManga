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

/** 按文件字节限制的LRU；释放淘汰的Blob URL，当前可见页面暂时保留。 */
export class ImageByteCache {
  private entries = new Map<string, { url: string; bytes: number }>()
  private size = 0
  private budget: number
  private revoke: (url: string) => void
  constructor(budget = 64 * 1024 * 1024, revoke = (url: string) => URL.revokeObjectURL(url)) {
    this.budget = budget
    this.revoke = revoke
  }
  get byteSize() { return this.size }
  touch(key: string) {
    const entry = this.entries.get(key)
    if (!entry) return
    this.entries.delete(key)
    this.entries.set(key, entry)
    return entry.url
  }
  put(key: string, url: string, bytes: number, pinned: Set<string>): string[] {
    this.remove(key)
    this.entries.set(key, { url, bytes })
    this.size += bytes
    return this.evict(pinned)
  }
  resize(key: string, bytes: number, pinned: Set<string>): string[] {
    const entry = this.entries.get(key)
    if (entry) { this.size += bytes - entry.bytes; entry.bytes = bytes }
    return this.evict(pinned)
  }
  private evict(pinned: Set<string>): string[] {
    const evicted: string[] = []
    for (const candidate of this.entries.keys()) {
      if (this.size <= this.budget) break
      if (pinned.has(candidate)) continue
      this.remove(candidate)
      evicted.push(candidate)
    }
    return evicted
  }
  remove(key: string) {
    const entry = this.entries.get(key)
    if (!entry) return
    this.entries.delete(key)
    this.size -= entry.bytes
    this.revoke(entry.url)
  }
  clear() {
    for (const key of this.entries.keys()) this.remove(key)
  }
}

export function imageDataToBlob(data: string): Blob {
  const comma = data.indexOf(',')
  const header = data.slice(0, comma)
  if (comma < 0 || !/^data:image\/[a-z0-9.+-]+;base64$/i.test(header)) throw new Error('图片数据格式无效')
  const binary = atob(data.slice(comma + 1))
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0))
  return new Blob([bytes], { type: header.slice(5, -7) })
}
