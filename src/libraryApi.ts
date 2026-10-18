import { invoke } from '@tauri-apps/api/core'

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
