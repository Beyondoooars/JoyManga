import type { Config } from '../bindings.ts'

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

/**
 * 这些字段由后端在保存时派生或维护，前端不应把它们当作"用户改动"提交：
 * - `revision` / `schemaVersion`：后端版本控制
 * - `androidStorageInitialized`：Android 初始化时由后端置位
 * - `exportDir`：后端 `apply_storage_policy` 强制与 `downloadDir` 一致
 */
const SERVER_OWNED_KEYS = new Set<string>(['revision', 'schemaVersion', 'androidStorageInitialized', 'exportDir'])

export function mergeServerConfig(local: Config | undefined, base: Config | undefined, server: Config): Config {
  const merged = { ...server }
  if (local && base) {
    for (const key of Object.keys(local) as Array<keyof Config>) {
      if (SERVER_OWNED_KEYS.has(key)) continue
      if (!same(local[key], base[key])) Object.assign(merged, { [key]: local[key] })
    }
  }
  // 与后端保持一致：导出目录始终跟随书库目录（后端 apply_storage_policy 会再强制一次）。
  if (typeof merged.downloadDir === 'string') merged.exportDir = merged.downloadDir
  return merged
}

export function hasConfigChanges(local: Config, base: Config): boolean {
  return (Object.keys(local) as Array<keyof Config>).some((key) => !SERVER_OWNED_KEYS.has(key) && !same(local[key], base[key]))
}
