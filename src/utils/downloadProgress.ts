import type {
  DownloadTaskEvent,
  DownloadTaskState,
  FailureKind,
  TaskFailure,
  TaskStage,
} from '../bindings.ts'
import type { ProgressData } from '../types.ts'

const stateLabels: Record<DownloadTaskState, string> = {
  Pending: '排队中',
  Downloading: '下载中',
  Paused: '已暂停',
  Cancelled: '已取消',
  Completed: '下载完成',
  Failed: '下载失败',
}

/** 失败原因的中文标签；界面与诊断文案共用同一份映射。 */
export const failureKindLabels: Record<FailureKind, string> = {
  network: '网络问题',
  upstream: '站点异常',
  parse: '解析失败',
  storage: '存储失败',
  business: '资源不可用',
  unknown: '未知错误',
}

const failureStageLabels: Record<TaskStage, string> = {
  queued: '排队阶段',
  fetchMeta: '获取信息阶段',
  downloading: '下载阶段',
  verifying: '校验阶段',
  archiving: '归档阶段',
  done: '完成阶段',
}

function countSuffix(totalImgCount: number, downloadedImgCount: number): string {
  return totalImgCount === 0 ? '' : ` ${downloadedImgCount}/${totalImgCount}`
}

function indicatorFor(progress: ProgressData): string {
  const { state, stage, totalImgCount, downloadedImgCount } = progress
  // 阶段只在「下载中」覆盖状态文案；排队/暂停/失败仍以任务状态为准。
  if (state === 'Downloading') {
    if (stage === 'fetchMeta') return '获取章节信息'
    if (stage === 'verifying') return '校验中' + countSuffix(totalImgCount, downloadedImgCount)
    if (stage === 'archiving') return '整理 CBZ 中'
  }
  if (state === 'Failed' && progress.failure) {
    return `${stateLabels.Failed} · ${failureKindLabels[progress.failure.kind]}`
  }
  return stateLabels[state] + countSuffix(totalImgCount, downloadedImgCount)
}

/**
 * 失败详情的单行摘要：类别 · 阶段 · 尝试次数 · 线路。
 * 只用于界面定位，不替代 `failure.message` 的完整错误链。
 */
export function describeFailure(failure: TaskFailure | null | undefined): string {
  if (!failure) return ''
  const parts = [failureKindLabels[failure.kind], failureStageLabels[failure.stage]]
  if (failure.attempts > 1) parts.push(`第 ${failure.attempts} 次尝试`)
  if (failure.line) parts.push(failure.line)
  return parts.join(' · ')
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
  const { totalImgCount, downloadedImgCount } = progress
  progress.percentage = totalImgCount === 0 ? 0 : Math.min(100, downloadedImgCount / totalImgCount * 100)
  progress.indicator = indicatorFor(progress)
  progresses.set(chapterId, progress)
  return progress
}
