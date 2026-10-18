import { useMessage } from 'naive-ui'
import type { Result, CommandError } from '../bindings.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'

export function useCommand() {
  const message = useMessage()

  async function execute<T>(
    commandFn: () => Promise<Result<T, CommandError>>,
    fallbackTitle = '操作失败',
  ): Promise<T | null> {
    try {
      const result = await commandFn()
      if (result.status === 'error') {
        console.error(result.error)
        message.error(formatCommandError(result.error))
        return null
      }
      return result.data
    } catch (error) {
      console.error(error)
      message.error(`${fallbackTitle}：${getErrorMessage(error)}`)
      return null
    }
  }

  return {
    execute,
  }
}
