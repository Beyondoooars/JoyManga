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
