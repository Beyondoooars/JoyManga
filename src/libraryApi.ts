import { invoke } from '@tauri-apps/api/core'
import { onOpenFile } from './utils/androidStorage.ts'

async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args) }
  catch (error) {
    const detail = error as { err_title?: string; err_message?: string } | null
    if (detail?.err_message) throw new Error(`${detail.err_title ?? '操作失败'}：${detail.err_message}`)
    throw error
  }
}

export interface ImportReport { imported: number; skipped: number; errors: string[] }

export const libraryApi = {
  importComics: (paths: string[]) => call<ImportReport>('import_local_comics', { paths }),
  removeComic: (comicId: number, deleteFiles = false) => call<void>('remove_library_comic', { comicId, deleteFiles }),
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
        handler.onError(error instanceof Error ? error.message : String(error))
      } finally {
        // 导入会把文件复制进书库，暂存副本没有保留价值
        if (id) window.JmAndroid?.discardImports(id)
      }
    })
  })
}
