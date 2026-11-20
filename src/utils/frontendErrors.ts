import { getErrorMessage } from './errors.ts'

const MAX_FRONTEND_ERRORS = 200
const MAX_ENTRY_CHARS = 2000

const errorBuffer: string[] = []

function formatFrontendError(source: string, error: unknown): string {
  const timestamp = new Date().toISOString()
  let detail = getErrorMessage(error)
  if (error instanceof Error && error.stack) {
    const firstStack = error.stack
      .split('\n')
      .slice(1, 3)
      .map((line) => line.trim())
      .join(' | ')
    if (firstStack) detail = `${detail} (${firstStack})`
  }
  const line = `${timestamp} [${source}] ${detail}`
  return line.length > MAX_ENTRY_CHARS ? `${line.slice(0, MAX_ENTRY_CHARS)}...(truncated)` : line
}

export function recordFrontendError(source: string, error: unknown): void {
  errorBuffer.push(formatFrontendError(source, error))
  if (errorBuffer.length > MAX_FRONTEND_ERRORS) {
    errorBuffer.splice(0, errorBuffer.length - MAX_FRONTEND_ERRORS)
  }
}

export function getFrontendErrorLogs(): string[] {
  return [...errorBuffer]
}

export function clearFrontendErrorLogs(): void {
  errorBuffer.length = 0
}
