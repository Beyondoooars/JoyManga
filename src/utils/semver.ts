interface ParsedVersion {
  core: number[]
  pre: string[]
}

function parseSemver(version: string): ParsedVersion {
  const trimmed = version.trim().replace(/^v/i, '')
  const buildIdx = trimmed.indexOf('+')
  const withoutBuild = buildIdx >= 0 ? trimmed.slice(0, buildIdx) : trimmed
  const dashIdx = withoutBuild.indexOf('-')
  const corePart = dashIdx >= 0 ? withoutBuild.slice(0, dashIdx) : withoutBuild
  const prePart = dashIdx >= 0 ? withoutBuild.slice(dashIdx + 1) : ''

  const core = corePart
    .split('.')
    .map((segment) => {
      const parsed = Number.parseInt(segment, 10)
      return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0
    })

  const pre = prePart ? prePart.split('.') : []
  return { core, pre }
}

function comparePrerelease(a: string[], b: string[]): number {
  if (a.length === 0 && b.length === 0) return 0
  if (a.length === 0) return 1
  if (b.length === 0) return -1

  const limit = Math.min(a.length, b.length)
  for (let i = 0; i < limit; i++) {
    const left = a[i]
    const right = b[i]
    if (left === right) continue
    const leftNumeric = /^\d+$/.test(left)
    const rightNumeric = /^\d+$/.test(right)
    if (leftNumeric && rightNumeric) {
      const diff = Number(left) - Number(right)
      if (diff !== 0) return diff > 0 ? 1 : -1
      continue
    }
    // SemVer 2.0.0 §11.4.3: numeric identifiers always have lower precedence than non-numeric.
    if (leftNumeric !== rightNumeric) {
      return leftNumeric ? -1 : 1
    }
    return left < right ? -1 : 1
  }

  if (a.length === b.length) return 0
  return a.length > b.length ? 1 : -1
}

/** 按 SemVer 2.0.0 比较两个版本号：v1 > v2 返回 1，v1 < v2 返回 -1，相等返回 0。 */
export function compareVersions(v1: string, v2: string): number {
  const p1 = parseSemver(v1)
  const p2 = parseSemver(v2)
  const maxCore = Math.max(p1.core.length, p2.core.length)
  for (let i = 0; i < maxCore; i++) {
    const num1 = p1.core[i] ?? 0
    const num2 = p2.core[i] ?? 0
    if (num1 > num2) return 1
    if (num1 < num2) return -1
  }
  return comparePrerelease(p1.pre, p2.pre)
}
