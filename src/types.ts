import type { DownloadTaskEvent } from './bindings.ts'

export type CurrentTabName = 'search' | 'favorite' | 'weekly' | 'downloaded' | 'chapter' | 'account' | 'settings'

export type ProgressesPaneTabName = 'uncompleted' | 'completed'

export type ProgressData = Extract<DownloadTaskEvent, { event: 'Create' }>['data'] & {
  percentage: number
  indicator: string
}
