export function getErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  if (typeof error === 'string') {
    return error
  }

  try {
    return JSON.stringify(error)
  } catch {
    return String(error)
  }
}

export type CommandErrorKind = 'Network' | 'Timeout' | 'Auth' | 'NotFound' | 'Cancelled' | 'Storage' | 'Unknown'

interface CommandErrorLike {
  err_title?: string
  err_message?: string
  kind?: CommandErrorKind
}

/** 后端错误类别 → 给用户的可执行建议。Unknown 不给建议，避免噪音。 */
const ACTION_HINTS: Record<CommandErrorKind, string | null> = {
  Network: '建议在设置中更换线路或检查网络/代理',
  Timeout: '可以稍后重试；持续超时请更换线路',
  Auth: '登录已失效，请重新登录',
  NotFound: '内容可能已下架，重试通常无效',
  Cancelled: null,
  Storage: '请检查磁盘空间与目录权限',
  Unknown: null,
}

export function getErrorHint(kind: CommandErrorKind | undefined): string | null {
  if (!kind) return null
  return ACTION_HINTS[kind] ?? null
}

/**
 * 把后端的 CommandError 渲染成一句话：标题 + 信息 +（可选）可执行建议。
 * 所有 `result.status === 'error'` 的地方都可复用，保证提示风格一致。
 */
export function formatCommandError(error: CommandErrorLike): string {
  const base = `${error.err_title ?? '操作失败'}：${error.err_message ?? '未知错误'}`
  const hint = getErrorHint(error.kind)
  return hint ? `${base}（${hint}）` : base
}
