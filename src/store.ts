import { defineStore } from 'pinia'
import type { CurrentTabName, ProgressData, ProgressesPaneTabName } from './types.ts'
import {
  commands,
  events,
  type ArchiveStage,
  type Comic,
  type ComicArchiveEvent,
  type LoginRespData,
  type SessionChangedEvent,
  type ApiDomainPreset,
  type DownloadTaskEvent,
  type Config,
  type GetFavoriteResult,
  type GetUserProfileRespData,
  type GetWeeklyResult,
  type LogEvent,
  type SearchResult,
  type TaskFailure,
} from './bindings.ts'
import { computed, nextTick, ref, toRaw } from 'vue'
import { mergeServerConfig, hasConfigChanges } from './utils/configState.ts'
import { applyDownloadEvent } from './utils/downloadProgress.ts'
import { resolveImageDomain } from './utils/imageDomain.ts'
import { emptyReadingState, parseReadingState, recordProgress, sanitizeReadingStateValue, toggleBookmark,
  READING_STORAGE_KEY, type ReadingProgress, type ReadingState } from './utils/readingState.ts'

export type LogRecord = LogEvent & { id: number; formatedLog: string }

function formatLogEvent(logEvent: LogEvent): string {
  const { timestamp, level, fields, target, filename, line_number } = logEvent
  const fields_str = Object.entries(fields)
    .sort(([key1], [key2]) => key1.localeCompare(key2))
    .map(([key, value]) => `${key}=${value}`)
    .join(' ')
  return `${timestamp} ${level} ${target}: ${filename}:${line_number} ${fields_str}`
}

export const useStore = defineStore('store', () => {
  const config = ref<Config>()
  const authoritativeConfig = ref<Config>()
  const apiDomainPresets = ref<ApiDomainPreset[]>([])
  const sessionEpoch = ref(0)
  /** 会话或本地记住的账密变化时递增，供账号页等界面重新同步表单 */
  const accountFormRevision = ref(0)
  const favoriteStatuses = ref(new Map<number, boolean | undefined>())
  const favoritePending = ref(new Set<number>())
  const favoriteRevision = ref(0)
  const favoriteMutationRevisions = new Map<number, number>()
  const pickedComicFavoriteEpoch = ref<number>()
  const searchResultEpoch = ref<number>()
  const weeklyResultEpoch = ref<number>()
  const favoriteResultEpoch = ref<number>()

  function applyConfig(server: Config, discardLocal = false) {
    if (authoritativeConfig.value && (server.revision ?? 0) < (authoritativeConfig.value.revision ?? 0)) return
    const old = authoritativeConfig.value
    const previousCredentials = config.value === undefined ? undefined : `${config.value.username}\u0000${config.value.password}`
    config.value = discardLocal ? structuredClone(toRaw(server)) : mergeServerConfig(config.value, old, server)
    authoritativeConfig.value = structuredClone(toRaw(server))
    // 记住的账密被服务端配置改写（重置/恢复/退出登录）时，账号页表单要重新同步
    if (previousCredentials !== undefined && previousCredentials !== `${config.value.username}\u0000${config.value.password}`) {
      accountFormRevision.value++
    }
  }

  function configIsDirty() { return !!config.value && !!authoritativeConfig.value && hasConfigChanges(config.value, authoritativeConfig.value) }

  async function persistConfig(snapshot = config.value, base = authoritativeConfig.value) {
    if (!snapshot || !base) throw new Error('配置尚未加载')
    const result = await commands.saveConfig(structuredClone(toRaw(snapshot)), structuredClone(toRaw(base)))
    if (result.status === 'error') throw new Error(`${result.error.err_title}：${result.error.err_message}`)
    applyConfig(result.data)
  }

  function applySession(data: SessionChangedEvent) {
    if (data.sessionEpoch < sessionEpoch.value) return
    if (data.sessionEpoch !== sessionEpoch.value) {
      favoriteStatuses.value.clear()
      favoritePending.value.clear()
      favoriteMutationRevisions.clear()
      favoriteRevision.value++
      getFavoriteResult.value = undefined
      searchResult.value = undefined
      getWeeklyResult.value = undefined
      pickedComicFavoriteEpoch.value = undefined
      pickedComicRequestGeneration++
    }
    sessionEpoch.value = data.sessionEpoch
    userProfile.value = data.profile ?? undefined
    // 会话切换（登录/退出/过期）后，账号页表单需按最新状态重建
    accountFormRevision.value++
  }

  function applyLoginResult(data: LoginRespData) {
    applySession({ sessionEpoch: data.sessionEpoch, profile: data.profile })
    return data.switchedApiDomainMode
  }

  function recordFavorite(comicId: number, value: boolean, epoch: number, observedRevision = favoriteRevision.value) {
    if (epoch !== sessionEpoch.value || !userProfile.value) return
    if (observedRevision < (favoriteMutationRevisions.get(comicId) ?? 0)) return
    favoriteStatuses.value.set(comicId, value)
  }
  function applyFavoriteMutation(comicId: number, value: boolean, epoch: number) {
    if (epoch !== sessionEpoch.value || !userProfile.value) return
    favoriteMutationRevisions.set(comicId, ++favoriteRevision.value)
    favoriteStatuses.value.set(comicId, value)
  }

  async function setFavorite(comicId: number, desired: boolean) {
    // 未登录时跳到「我的」页登录（原先是弹窗，现在是与发现/书库同级的页面）
    if (!userProfile.value) { openAccount(); return }
    if (favoritePending.value.has(comicId)) return
    const epoch = sessionEpoch.value
    favoritePending.value.add(comicId)
    try {
      const result = await commands.setFavoriteComic(comicId, desired, epoch)
      if (epoch !== sessionEpoch.value) return
      if (result.status === 'error') {
        favoriteStatuses.value.set(comicId, undefined)
        throw new Error(`${result.error.err_title}：${result.error.err_message}`)
      }
      applyFavoriteMutation(comicId, result.data.isFavorite, epoch)
      return result.data.isFavorite
    } finally {
      if (epoch === sessionEpoch.value) favoritePending.value.delete(comicId)
    }
  }

  const stateUnlisteners: Array<() => void> = []
  let stateReady: Promise<void> | undefined
  function initStateListeners(): Promise<void> {
    if (stateReady) return stateReady
    stateReady = (async () => {
      stateUnlisteners.push(await events.configChangedEvent.listen(({ payload }) => applyConfig(payload.config)))
      stateUnlisteners.push(await events.sessionChangedEvent.listen(({ payload }) => applySession(payload)))
      stateUnlisteners.push(await events.comicArchiveEvent.listen(({ payload }) => handleArchiveEvent(payload)))
      // 导出进度监听常驻：书库页发起导出时传输页可能尚未挂载
      await initExportListener().catch((error) => {
        console.error('注册导出进度监听失败', error)
      })
      stateUnlisteners.push(await events.favoriteChangedEvent.listen(({ payload }) => {
        applyFavoriteMutation(payload.comicId, payload.isFavorite, payload.sessionEpoch)
        if (payload.sessionEpoch === sessionEpoch.value && getFavoriteResult.value) {
          if (!payload.isFavorite) {
            const previous = getFavoriteResult.value.list.length
            getFavoriteResult.value.list = getFavoriteResult.value.list.filter((comic) => comic.id !== payload.comicId)
            getFavoriteResult.value.total -= previous - getFavoriteResult.value.list.length
          }
        }
      }))
      applySession(await commands.getSession())
      apiDomainPresets.value = await commands.getApiDomainPresets()
    })().catch((error) => {
      stateUnlisteners.splice(0).forEach((unlisten) => unlisten())
      stateReady = undefined
      throw error
    })
    return stateReady
  }

  // 正在执行 CBZ 自动整合（合并/命名整理）的漫画：下载完成前不可阅读打开的窗口期。
  // runId 用来隔离重试：旧运行迟到的进度/结束事件不能覆盖新运行。
  const archivingComics = ref(
    new Map<number, { runId: number; stage: ArchiveStage; current: number; total: number }>(),
  )
  // 最近一次整合失败按漫画保留，成功或下一次开始后清除；只在本次会话内有效。
  const archiveFailures = ref(new Map<number, TaskFailure>())

  function isArchiving(comicId: number) {
    return archivingComics.value.has(comicId)
  }

  function archiveFailure(comicId: number) {
    return archiveFailures.value.get(comicId)
  }

  function handleArchiveEvent(payload: ComicArchiveEvent) {
    const { comicId } = payload.data
    switch (payload.event) {
      case 'Start':
        archiveFailures.value.delete(comicId)
        archivingComics.value.set(comicId, {
          runId: payload.data.runId,
          stage: 'collecting',
          current: 0,
          total: payload.data.total,
        })
        break
      case 'Progress': {
        const state = archivingComics.value.get(comicId)
        if (!state || state.runId !== payload.data.runId) break
        state.stage = payload.data.stage
        state.current = payload.data.current
        state.total = payload.data.total
        break
      }
      case 'End': {
        const state = archivingComics.value.get(comicId)
        if (state && state.runId !== payload.data.runId) break
        archivingComics.value.delete(comicId)
        archiveFailures.value.delete(comicId)
        if (pickedComic.value?.id === comicId) {
          pickedComic.value.isDownloaded = true
        }
        // 整合会重写书库文件（合并成整本 CBZ 或改名），列表必须重新扫描；
        // 不在书库页时只做陈旧标记，等用户进来再拉。
        downloadedComicsStale.value = true
        if (currentTabName.value === 'downloaded') {
          void refreshDownloadedComics().catch((error) => {
            console.error('整合完成后刷新本地库存失败', error)
          })
        }
        break
      }
      case 'Error': {
        const state = archivingComics.value.get(comicId)
        if (state && state.runId !== payload.data.runId) break
        archivingComics.value.delete(comicId)
        archiveFailures.value.set(comicId, payload.data.failure)
        console.error('整合失败', payload.data.message, payload.data.failure)
        break
      }
    }
  }

  const readingPersistenceError = ref('')
  const readingState = ref<ReadingState>(emptyReadingState())
  try {
    if (typeof localStorage !== 'undefined') readingState.value = parseReadingState(localStorage.getItem(READING_STORAGE_KEY))
  } catch { readingPersistenceError.value = '无法读取阅读记录，当前记录仅保存在本次会话' }

  function persistReadingState(next: ReadingState) {
    readingState.value = next
    try {
      if (typeof localStorage !== 'undefined') localStorage.setItem(READING_STORAGE_KEY, JSON.stringify(next))
      readingPersistenceError.value = ''
    } catch { readingPersistenceError.value = '阅读记录保存失败，请检查磁盘空间' }
  }

  /** 恢复备份时整批导入：先成功写入本地存储，再替换内存状态，失败时保留原记录。 */
  function importReadingStateSnapshot(value: unknown): 'applied' | 'invalid' | 'write-failed' {
    const next = sanitizeReadingStateValue(value)
    if (!next) return 'invalid'
    try {
      if (typeof localStorage !== 'undefined') localStorage.setItem(READING_STORAGE_KEY, JSON.stringify(next))
      readingState.value = next
      readingPersistenceError.value = ''
      return 'applied'
    } catch {
      readingPersistenceError.value = '阅读记录保存失败，请检查磁盘空间'
      return 'write-failed'
    }
  }

  function saveReadingProgress(progress: ReadingProgress) {
    persistReadingState(recordProgress(readingState.value, progress))
  }

  function toggleReadingBookmark(progress: ReadingProgress) {
    persistReadingState(toggleBookmark(readingState.value, progress))
  }

  /// 删除库存后同步清理该漫画的阅读进度与书签（否则书签列表仍能点到已删除的漫画）
  function forgetComicReadingState(comicId: number) {
    const restProgress = { ...readingState.value.progress }
    delete restProgress[String(comicId)]
    const bookmarks = readingState.value.bookmarks.filter((item) => item.comicId !== comicId)
    persistReadingState({ ...readingState.value, progress: restProgress, bookmarks })
  }

  function removeReadingBookmark(id: string) {
    persistReadingState({ ...readingState.value, bookmarks: readingState.value.bookmarks.filter((item) => item.id !== id) })
  }

  function continueReading(comic: Comic) {
    const progress = readingState.value.progress[String(comic.id)]
    const chapter = comic.chapterInfos.find((item) => item.chapterId === progress?.chapterId && item.isDownloaded)
      ?? comic.chapterInfos.find((item) => item.isDownloaded)
    if (!chapter) return
    openReader(comic.id, chapter.chapterId, comic.name, chapter.chapterId === progress?.chapterId ? progress.page : 1)
  }
  const configMaintenanceInProgress = ref(false)
  let waitForConfigSaves = async () => {}

  function registerConfigSaveWaiter(waiter: () => Promise<void>) {
    waitForConfigSaves = waiter
  }

  async function runConfigMaintenance<T>(action: () => Promise<T>): Promise<T> {
    if (configMaintenanceInProgress.value) throw new Error('配置维护正在进行，请稍后重试')
    configMaintenanceInProgress.value = true
    try {
      await waitForConfigSaves()
      return await action()
    } finally {
      await nextTick()
      configMaintenanceInProgress.value = false
    }
  }
  async function saveConfigNow() {
    await runConfigMaintenance(async () => {
      if (!config.value) throw new Error('配置尚未加载')
      await persistConfig()
    })
  }
  const userProfile = ref<GetUserProfileRespData>()
  // 用户手动登录进行中：冷启动自动登录的重试必须让路，否则会用新的登录请求
  // 顶掉正在进行的会话（后端每次登录都会递增 auth_generation）。
  const manualLoginInProgress = ref(false)
  const pickedComic = ref<Comic>()
  const currentTabName = ref<CurrentTabName>('search')
  /** 打开「我的」前所在的标签页，返回键/关闭时回到这里 */
  const accountReturnTab = ref<CurrentTabName>('search')
  const chapterDetailShowing = ref<boolean>(false)
  const progresses = ref<Map<number, ProgressData>>(new Map())
  const getFavoriteResult = ref<GetFavoriteResult>()
  const searchResult = ref<SearchResult>()
  const progressesPaneTabName = ref<ProgressesPaneTabName>('uncompleted')
  const getWeeklyResult = ref<GetWeeklyResult>()
  const downloadedComics = ref<Comic[]>([])
  const downloadedComicsLoaded = ref(false)
  // 磁盘书库已变化（下载完成/删除/整合结束），列表需要重新扫描才能反映。
  // 不在事件里直接扫描：下载可能连续完成多个章节，攒到用户真正打开书库页时再拉一次。
  const downloadedComicsStale = ref(false)
  const downloadedSubTab = ref<'library' | 'transfers'>('library')

  function openTransfers() {
    chapterDetailShowing.value = false
    currentTabName.value = 'downloaded'
    downloadedSubTab.value = 'transfers'
  }

  /** 打开「我的」页（账号/登录/在线收藏），并记住来处以便返回。 */
  function openAccount() {
    if (currentTabName.value !== 'account') {
      accountReturnTab.value = currentTabName.value
    }
    // 走 closeComicDetail 递增请求代数：否则在途的 openComic 返回后会把详情重新盖上来
    closeComicDetail()
    currentTabName.value = 'account'
  }

  /** 从「我的」返回之前的页面（未登录时点收藏进入的那一页）。 */
  function closeAccount() {
    if (currentTabName.value !== 'account') return false
    currentTabName.value = accountReturnTab.value === 'account' ? 'search' : accountReturnTab.value
    return true
  }

  /** 打开设置页，并记住来处以便返回键回到原页面。 */
  const settingsReturnTab = ref<CurrentTabName>('search')
  function openSettings() {
    if (currentTabName.value !== 'settings') {
      // 从「我的」进设置时，来处记成「我的」的来处：返回键应先回到看设置前的页面，
      // 而不是把用户送回「我的」再按一次才能离开。
      settingsReturnTab.value = currentTabName.value === 'account' ? accountReturnTab.value : currentTabName.value
    }
    closeComicDetail()
    currentTabName.value = 'settings'
  }

  /** 从设置返回来处；返回键与页面顶部返回按钮共用。 */
  function closeSettings() {
    if (currentTabName.value !== 'settings') return false
    currentTabName.value = settingsReturnTab.value === 'settings' ? 'search' : settingsReturnTab.value
    return true
  }
  const imageDomain = computed(() =>
    resolveImageDomain(config.value?.imageDomainMode, config.value?.customImageDomain),
  )
  const downloadSpeed = ref<string>('')
  /** 当前正在下载的章节页进度（供 Android 通知进度条使用） */
  const activeDownloadPages = computed(() => {
    for (const progress of progresses.value.values()) {
      if (progress.state !== 'Downloading') continue
      return { current: progress.downloadedImgCount ?? 0, total: progress.totalImgCount ?? 0 }
    }
    return { current: 0, total: 0 }
  })
  let speedUnlisten: (() => void) | undefined

  const activeDownloadCount = computed(() => {
    let count = 0
    for (const progress of progresses.value.values()) {
      if (progress.state === 'Downloading' || progress.state === 'Pending') {
        count++
      }
    }
    return count
  })

  /**
   * 取消当前正在下载的第一章（Android 通知栏"取消当前"按钮触发）。
   * 只挑 Downloading 状态的任务，避免误取消排队中的章节。
   */
  async function cancelCurrentDownload(): Promise<string | null> {
    for (const [chapterId, progress] of progresses.value) {
      if (progress.state !== 'Downloading') continue
      const result = await commands.cancelDownloadTask(chapterId)
      if (result.status === 'error') return `${result.error.err_title}：${result.error.err_message}`
      return progress.chapterInfo.chapterTitle
    }
    return null
  }

  async function initSpeedListener() {
    if (speedUnlisten) return
    speedUnlisten = await events.downloadSpeedEvent.listen(({ payload: { speed } }) => {
      downloadSpeed.value = speed
    })
  }

  let pickedComicRequestGeneration = 0
  let downloadedComicsRequestGeneration = 0

  const pendingSearchKeyword = ref<string>('')
  const activeReaderTarget = ref<{
    comicId: number
    chapterId: number
    comicTitle?: string
    page?: number
  } | null>(null)

  function searchKeyword(keyword: string) {
    const trimmed = keyword.trim()
    if (!trimmed) return
    chapterDetailShowing.value = false
    currentTabName.value = 'search'
    pendingSearchKeyword.value = trimmed
  }

  function openReader(comicId: number, chapterId: number, comicTitle?: string, page?: number) {
    activeReaderTarget.value = { comicId, chapterId, comicTitle, page }
  }

  function closeReader() {
    activeReaderTarget.value = null
  }

  function showComic(comic: Comic, fromNetwork = false) {
    pickedComicRequestGeneration += 1
    pickedComic.value = comic
    pickedComicFavoriteEpoch.value = fromNetwork ? sessionEpoch.value : undefined
    if (fromNetwork) recordFavorite(comic.id, comic.is_favorite, sessionEpoch.value)
    chapterDetailShowing.value = true
  }

  async function openComic(comicId: number) {
    const generation = ++pickedComicRequestGeneration
    const epoch = sessionEpoch.value
    const observedRevision = favoriteRevision.value
    const result = await commands.getComic(comicId)
    if (generation !== pickedComicRequestGeneration || epoch !== sessionEpoch.value) {
      return { status: 'stale' } as const
    }
    if (result.status === 'error') {
      return result
    }

    pickedComic.value = result.data
    pickedComicFavoriteEpoch.value = epoch
    recordFavorite(comicId, result.data.is_favorite, epoch, observedRevision)
    chapterDetailShowing.value = true
    return result
  }

  function closeComicDetail() {
    pickedComicRequestGeneration++
    chapterDetailShowing.value = false
  }

  async function refreshPickedComic() {
    const comicId = pickedComic.value?.id
    if (comicId === undefined) {
      return { status: 'stale' } as const
    }

    const generation = ++pickedComicRequestGeneration
    const epoch = sessionEpoch.value
    const observedRevision = favoriteRevision.value
    const result = await commands.getComic(comicId)
    if (generation !== pickedComicRequestGeneration || pickedComic.value?.id !== comicId) {
      return { status: 'stale' } as const
    }
    if (result.status === 'error') {
      return result
    }

    pickedComic.value = result.data
    pickedComicFavoriteEpoch.value = epoch
    recordFavorite(comicId, result.data.is_favorite, epoch, observedRevision)
    return result
  }

  async function refreshDownloadedComics() {
    const generation = ++downloadedComicsRequestGeneration
    const comics = await commands.getDownloadedComics()
    if (generation !== downloadedComicsRequestGeneration) {
      return false
    }

    downloadedComics.value = comics
    downloadedComicsLoaded.value = true
    downloadedComicsStale.value = false
    return true
  }

  // ── 导出进度（书库 / 传输页共用）──
  // 监听器放在 store 中常驻，避免“从书库发起导出、但传输页未挂载时进度事件全部丢失”。
  interface ExportProgressItem {
    // 与 ExportProgresses.vue 的 ProgressData 保持同形
    uuid: string
    exportType: 'cbz'
    state: 'Processing' | 'Error' | 'End'
    comicTitle: string
    current: number
    total: number
    percentage: number
    indicator: string
    chapterExportDir?: string
  }
  const exportProgresses = ref(new Map<string, ExportProgressItem>())
  let exportListenerPromise: Promise<void> | undefined

  function initExportListener(): Promise<void> {
    if (exportListenerPromise) return exportListenerPromise
    exportListenerPromise = (async () => {
      const cbzUnlisten = await events.exportCbzEvent.listen(({ payload }) => {
        if (payload.event === 'Start') {
          const { uuid, comicTitle, total } = payload.data
          exportProgresses.value.set(uuid, {
            uuid,
            exportType: 'cbz',
            state: 'Processing',
            comicTitle,
            current: 0,
            total,
            percentage: 0,
            indicator: 'CBZ 创建中',
          })
          return
        }
        const progress = exportProgresses.value.get(payload.data.uuid)
        if (!progress) return
        if (payload.event === 'Progress') {
          progress.current = payload.data.current
          progress.percentage = progress.total === 0 ? 0 : (progress.current / progress.total) * 100
          progress.indicator = `CBZ创建中 ${progress.current}/${progress.total}`
        } else if (payload.event === 'Error') {
          progress.state = 'Error'
          progress.indicator = 'CBZ创建失败'
        } else if (payload.event === 'End') {
          progress.state = 'End'
          progress.chapterExportDir = payload.data.chapterExportDir
          progress.indicator = 'CBZ创建完成'
        }
      })
      exportUnlisteners.push(cbzUnlisten)
    })().catch((error) => {
      exportUnlisteners.splice(0).forEach((unlisten) => unlisten())
      exportListenerPromise = undefined
      throw error
    })
    return exportListenerPromise
  }
  const exportUnlisteners: Array<() => void> = []

  const taskLoadError = ref('')
  const loadingTasks = ref(false)
  let taskUnlisten: (() => void) | undefined
  let tasksHydrating = true
  let taskListenerPromise: Promise<void> | undefined
  const bufferedTasks: DownloadTaskEvent[] = []
  function handleTaskEvent(event: DownloadTaskEvent) {
    const previous = progresses.value.get(event.event === 'Create' ? event.data.chapterInfo.chapterId : event.data.chapterId)?.state
    const progress = applyDownloadEvent(progresses.value, event)
    if (progress?.state === 'Completed' && previous !== 'Completed') {
      progress.chapterInfo.isDownloaded = true
      syncCompletedDownload(progress)
    }
    trimProgressHistory()
  }
  let reloadTasksPromise: Promise<void> | undefined
  async function reloadTasks() {
    // 并发调用共享同一个进行中的请求：调用方要么等到最新快照，要么收到同一个错误，
    // 不会出现“清空队列后 reload 被静默跳过、列表空到下一次事件”的情况。
    if (reloadTasksPromise) return reloadTasksPromise
    reloadTasksPromise = (async () => {
      loadingTasks.value = true; tasksHydrating = true; taskLoadError.value = ''
      try {
        const snapshots = await commands.getDownloadTasks()
        for (const snapshot of snapshots) handleTaskEvent({ event: 'Create', data: snapshot })
      } catch (error) {
        taskLoadError.value = String(error)
        throw error
      } finally {
        tasksHydrating = false; loadingTasks.value = false
        for (const event of bufferedTasks.splice(0)) handleTaskEvent(event)
        reloadTasksPromise = undefined
      }
    })()
    return reloadTasksPromise
  }
  function initDownloadListener(): Promise<void> {
    if (taskListenerPromise) return taskListenerPromise
    taskListenerPromise = (async () => {
      taskUnlisten = await events.downloadTaskEvent.listen(({ payload }) => {
        if (tasksHydrating) bufferedTasks.push(payload)
        else handleTaskEvent(payload)
      })
      await reloadTasks()
    })().catch((error) => {
      taskUnlisten?.(); taskListenerPromise = undefined
      throw error
    })
    return taskListenerPromise
  }

function syncCompletedDownload(progressData: ProgressData) {
  const comicId = progressData.comic.id
  const comicDownloadDir = progressData.comic.comicDownloadDir

  if (pickedComic.value?.id === comicId) {
    pickedComic.value.isDownloaded = true
    if (comicDownloadDir !== undefined && comicDownloadDir !== null) {
      pickedComic.value.comicDownloadDir = comicDownloadDir
    }
    const chapter = pickedComic.value.chapterInfos.find(
      ({ chapterId }) => chapterId === progressData.chapterInfo.chapterId,
    )
    if (chapter !== undefined) {
      Object.assign(chapter, progressData.chapterInfo, { isDownloaded: true })
    }
  }

  const summaries = [searchResult.value?.content, getFavoriteResult.value?.list]
  for (const list of summaries) {
    const comic = list?.find(({ id }) => id === comicId)
    if (comic !== undefined) {
      comic.isDownloaded = true
      if (comicDownloadDir !== undefined && comicDownloadDir !== null) {
        comic.comicDownloadDir = comicDownloadDir
      }
    }
  }

  const weeklyComic = getWeeklyResult.value?.list.find(({ id }) => id === comicId)
  if (weeklyComic !== undefined) {
    weeklyComic.is_downloaded = true
    if (comicDownloadDir !== undefined && comicDownloadDir !== null) {
      weeklyComic.comic_download_dir = comicDownloadDir
    }
  }

  // 无论在哪个页面下载，书库内容都已改变：标记陈旧，用户下次进入书库页时重新扫描。
  // 正在看书库时立刻刷新，避免停在旧列表上。
  downloadedComicsStale.value = true
  if (currentTabName.value === 'downloaded') {
    void refreshDownloadedComics().catch((error) => {
      console.error('下载完成后刷新本地库存失败', error)
    })
  }
}

function trimProgressHistory() {
  const maxHistorySize = 500
  if (progresses.value.size <= maxHistorySize) return

  for (const [chapterId, progress] of progresses.value) {
    if (progresses.value.size <= maxHistorySize) break
    if (progress.state === 'Completed' || progress.state === 'Cancelled') {
      progresses.value.delete(chapterId)
    }
  }
}


  const logRecords = ref<LogRecord[]>([])
  let nextLogRecordId = 1
  let logEventUnlisten: (() => void) | undefined

  async function initLogListener(notification?: {
    error: (options: { title?: string; description?: string; duration?: number }) => void
  }) {
    if (logEventUnlisten) return
    logEventUnlisten = await events.logEvent.listen(({ payload: logEvent }) => {
      const record: LogRecord = {
        ...logEvent,
        id: nextLogRecordId++,
        formatedLog: formatLogEvent(logEvent),
      }
      logRecords.value.push(record)
      if (logRecords.value.length > 5000) {
        logRecords.value.splice(0, logRecords.value.length - 5000)
      }

      if (logEvent.level === 'ERROR' && notification) {
        notification.error({
          title: (logEvent.fields['err_title'] as string) || '系统错误',
          description: (logEvent.fields['message'] as string) || '',
          duration: 0,
        })
      }
    })
  }

  function clearLogRecords() {
    logRecords.value = []
    nextLogRecordId = 1
  }

  return {
    config,
    authoritativeConfig, apiDomainPresets, applyConfig, persistConfig, configIsDirty,
    openAccount, closeAccount, openSettings, closeSettings, sessionEpoch, accountFormRevision, favoriteStatuses, favoritePending, pickedComicFavoriteEpoch,
    searchResultEpoch, weeklyResultEpoch, favoriteResultEpoch,
    applySession, applyLoginResult, recordFavorite, applyFavoriteMutation, favoriteRevision, setFavorite, initStateListeners,
    archivingComics, isArchiving, archiveFailures, archiveFailure,
    exportProgresses, initExportListener,
    initDownloadListener, reloadTasks, taskLoadError, loadingTasks,
    readingState,
    readingPersistenceError,
    saveReadingProgress,
    importReadingStateSnapshot,
    toggleReadingBookmark,
    removeReadingBookmark,
    forgetComicReadingState,
    continueReading,
    configMaintenanceInProgress,
    registerConfigSaveWaiter,
    runConfigMaintenance,
    saveConfigNow,
    userProfile,
    manualLoginInProgress,
    pickedComic,
    currentTabName,
    chapterDetailShowing,
    closeComicDetail,
    progresses,
    getFavoriteResult,
    searchResult,
    progressesPaneTabName,
    getWeeklyResult,
    downloadedComics,
    downloadedComicsLoaded,
    downloadedComicsStale,
    downloadedSubTab,
    openTransfers,
    imageDomain,
    pendingSearchKeyword,
    searchKeyword,
    activeReaderTarget,
    openReader,
    closeReader,
    showComic,
    openComic,
    refreshPickedComic,
    refreshDownloadedComics,
    logRecords,
    initLogListener,
    clearLogRecords,
    downloadSpeed,
    activeDownloadPages,
    cancelCurrentDownload,
    activeDownloadCount,
    initSpeedListener,
  }
})
