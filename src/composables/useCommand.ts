import { useMessage } from 'naive-ui'
import type { CommandError } from '../bindings.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'

type CommandResult<T> = { status: 'ok'; data: T } | { status: 'error'; error: CommandError }

export function useCommand() {
  const message = useMessage()

  async function execute<T>(
    commandFn: () => Promise<CommandResult<T>>,
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
