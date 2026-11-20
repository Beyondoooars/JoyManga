import type { Config } from '../bindings.ts'

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

/**
 * 这些字段由后端在保存时派生或维护，前端不应把它们当作"用户改动"提交：
 * - `revision` / `schemaVersion`：后端版本控制
 * - `androidStorageInitialized`：Android 初始化时由后端置位
 */
const SERVER_OWNED_KEYS = new Set<string>(['revision', 'schemaVersion', 'androidStorageInitialized'])

export function mergeServerConfig(local: Config | undefined, base: Config | undefined, server: Config): Config {
  const merged = { ...server }
  if (local && base) {
    for (const key of Object.keys(local) as Array<keyof Config>) {
      if (SERVER_OWNED_KEYS.has(key)) continue
      if (!same(local[key], base[key])) Object.assign(merged, { [key]: local[key] })
    }
  }
  return merged
}

export function hasConfigChanges(local: Config, base: Config): boolean {
  return (Object.keys(local) as Array<keyof Config>).some((key) => !SERVER_OWNED_KEYS.has(key) && !same(local[key], base[key]))
}
