import type { CommandError, LoginRespData } from '../bindings.ts'

export type LoginCommandResult =
  | { status: 'ok'; data: LoginRespData }
  | { status: 'error'; error: CommandError }

export interface AutoLoginOptions {
  username: string
  password: string
  login: (username: string, password: string) => Promise<LoginCommandResult>
  /** 用户已手动登录、或表单里的账密已改变时停止重试 */
  shouldStop?: () => boolean
  wait?: (milliseconds: number) => Promise<void>
  attempts?: number
  retryDelayMs?: number
  onRetry?: (attempt: number, error: CommandError | string) => void
}

export type AutoLoginOutcome =
  | { status: 'success'; data: LoginRespData; attempts: number }
  | { status: 'failed'; error: CommandError | string; attempts: number }
  | { status: 'skipped' }

/** 只有网络/超时/未归类错误值得重试；账号密码错误或资源不存在重试只会打扰用户。 */
export function isRetryableAutoLoginError(error: CommandError | string): boolean {
  if (typeof error === 'string') return true
  // 取消和“登录期间网络设置改变”都是用户/配置动作触发的，重试只会再次打断。
  if (error.err_message.includes('登录已取消') || error.err_message.includes('登录期间网络设置已改变')) return false
  return error.kind === 'Network' || error.kind === 'Timeout' || error.kind === 'Unknown'
}

/**
 * 冷启动自动登录：首次失败后短暂等待再试，避开刚启动时网络/域名尚未就绪的窗口。
 * 不改变手动登录路径；用户一旦登录成功或改了表单，立即停止。
 */
export async function runAutoLogin(options: AutoLoginOptions): Promise<AutoLoginOutcome> {
  const attempts = Math.max(1, options.attempts ?? 3)
  const wait = options.wait ?? ((milliseconds: number) => new Promise<void>((resolve) => setTimeout(resolve, milliseconds)))
  let lastError: CommandError | string = '自动登录失败'

  for (let attempt = 1; attempt <= attempts; attempt++) {
    if (options.shouldStop?.()) return { status: 'skipped' }
    try {
      const result = await options.login(options.username, options.password)
      if (result.status === 'ok') return { status: 'success', data: result.data, attempts: attempt }
      lastError = result.error
      // 失败可能正是手动登录/改配置导致的取消：此时静默让路，不再弹自动登录失败。
      if (options.shouldStop?.()) return { status: 'skipped' }
      if (attempt >= attempts || !isRetryableAutoLoginError(result.error)) {
        return { status: 'failed', error: lastError, attempts: attempt }
      }
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error)
      if (options.shouldStop?.()) return { status: 'skipped' }
      if (attempt >= attempts) return { status: 'failed', error: lastError, attempts: attempt }
    }
    options.onRetry?.(attempt, lastError)
    await wait(options.retryDelayMs ?? 1500 * attempt)
  }

  return { status: 'failed', error: lastError, attempts }
}
