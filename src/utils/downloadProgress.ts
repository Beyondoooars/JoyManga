import type { DownloadTaskEvent, DownloadTaskState } from '../bindings.ts'
import type { ProgressData } from '../types.ts'

const stateLabels: Record<DownloadTaskState, string> = {
  Pending: '排队中',
  Downloading: '下载中',
  Paused: '已暂停',
  Cancelled: '已取消',
  Completed: '下载完成',
  Failed: '下载失败',
}

/** 快照与事件共用递增版本，迟到的数据不能覆盖新的任务状态。 */
export function applyDownloadEvent(
  progresses: Map<number, ProgressData>,
  event: DownloadTaskEvent,
): ProgressData | undefined {
  const chapterId = event.event === 'Create' ? event.data.chapterInfo.chapterId : event.data.chapterId
  const previous = progresses.get(chapterId)
  if (previous !== undefined && previous.revision >= event.data.revision) return
  if (event.event === 'Update' && previous === undefined) return

  const progress = event.event === 'Create'
    ? { ...event.data, percentage: 0, indicator: '' }
    : Object.assign(previous!, event.data)
  const { state, totalImgCount, downloadedImgCount } = progress
  progress.percentage = totalImgCount === 0 ? 0 : Math.min(100, downloadedImgCount / totalImgCount * 100)
  progress.indicator = stateLabels[state] + (totalImgCount === 0 ? '' : ` ${downloadedImgCount}/${totalImgCount}`)
  progresses.set(chapterId, progress)
  return progress
}
