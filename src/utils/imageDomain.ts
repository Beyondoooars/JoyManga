import type { ImageDomainMode } from '../bindings.ts'

// 与 Rust 端 `config.rs` 中的 `IMAGE_DOMAIN_*` 常量保持一致
export const IMAGE_DOMAIN_PRESETS: Record<Exclude<ImageDomainMode, 'Custom'>, string> = {
  Domain1: 'cdn-msp.jmapiproxy1.cc',
  Domain2: 'cdn-msp.jmapiproxy2.cc',
  Domain3: 'cdn-msp2.jmapiproxy2.cc',
  Domain4: 'cdn-msp3.jmapiproxy2.cc',
  Domain5: 'cdn-msp.jmapinodeudzn.net',
  Domain6: 'cdn-msp3.jmapinodeudzn.net',
}

const DEFAULT_IMAGE_DOMAIN = IMAGE_DOMAIN_PRESETS.Domain3

export function resolveImageDomain(
  mode: ImageDomainMode | undefined,
  customDomain: string | undefined,
): string {
  if (mode === undefined) {
    return DEFAULT_IMAGE_DOMAIN
  }
  if (mode === 'Custom') {
    const custom = (customDomain ?? '').trim()
    return custom === '' ? DEFAULT_IMAGE_DOMAIN : custom
  }
  return IMAGE_DOMAIN_PRESETS[mode] ?? DEFAULT_IMAGE_DOMAIN
}
