import type { Config } from '../bindings.ts'

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)
export function mergeServerConfig(local: Config | undefined, base: Config | undefined, server: Config): Config {
  const merged = { ...server }
  if (local && base) {
    for (const key of Object.keys(local) as Array<keyof Config>) {
      if (key === 'revision' || key === 'schemaVersion' || key === 'androidStorageInitialized') continue
      if (!same(local[key], base[key])) Object.assign(merged, { [key]: local[key] })
    }
  }
  if (typeof merged.downloadDir === 'string') merged.exportDir = merged.downloadDir
  return merged
}
export function hasConfigChanges(local: Config, base: Config): boolean {
  return (Object.keys(local) as Array<keyof Config>).some((key) => key !== 'revision' && key !== 'schemaVersion' && !same(local[key], base[key]))
}
