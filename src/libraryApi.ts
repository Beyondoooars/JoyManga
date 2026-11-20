import { commands, type CommandError, type ImportReport } from './bindings.ts'
import { onOpenFile } from './utils/androidStorage.ts'

type CommandResult<T> = Promise<{ status: 'ok'; data: T } | { status: 'error'; error: CommandError }>

async function call<T>(command: CommandResult<T>): Promise<T> {
  // typedError 对 CommandError 返回 error 对象，但 IPC 自身异常直接抛 Error。
  const result = await command
  if (result.status === 'error') {
    throw new Error(`${result.error.err_title}：${result.error.err_message}`, { cause: result.error })
  }
  return result.data
}

export type { ImportReport }

export const libraryApi = {
  importComics: (paths: string[]) => call<ImportReport>(commands.importLocalComics(paths)),
  removeComic: (comicId: number, deleteFiles = false) => call<null>(commands.removeLibraryComic(comicId, deleteFiles)),
}

/**
 * 监听「用 JoyManga 打开 CBZ」：导入外部文件并清理暂存目录。
 *
 * 连续打开多个文件时串行执行，避免多个批次同时扫描书库、互相把对方刚导入的
 * 记录当成已存在而跳过。返回取消订阅函数。
 */
export function onOpenExternalComic(handler: {
  onImported: (report: ImportReport) => void
  onError: (message: string) => void
}): () => void {
  let queue = Promise.resolve()
  return onOpenFile(({ id, path }) => {
    queue = queue.then(async () => {
      try {
        handler.onImported(await libraryApi.importComics([path]))
      } catch (error) {
        try {
          handler.onError(error instanceof Error ? error.message : String(error))
        } catch (handlerError) {
          console.error('外部漫画导入错误处理失败', handlerError)
        }
      } finally {
        // 导入会把文件复制进书库，暂存副本没有保留价值
        if (id) window.JmAndroid?.discardImports(id)
      }
    })
  })
}
