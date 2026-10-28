// 仅通过 /tests/ui.html 加载；所有 IPC 都在内存中模拟，不读写真实账号或下载文件。
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import App from '../src/App.vue'
import { useStore } from '../src/store.ts'
import 'virtual:uno.css'
import '../src/styles.css'

// 在线封面由 Service Worker 就地生成（见 tests/mock-sw.js）：测试页没有真实图床，
// 不拦截的话发现页/收藏页的封面墙只会显示占位图标，无法核对版面。
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/tests/mock-sw.js').catch((error) => console.error('封面 mock 注册失败', error))
}

const config = {
  username: '', password: '', downloadDir: 'D:/模拟下载', exportDir: 'D:/模拟导出',
  proxyMode: 'NoProxy', proxyHost: '127.0.0.1', proxyPort: 7890,
  chapterConcurrency: 3, imgConcurrency: 8,
  apiDomainMode: 'Custom', customApiDomain: 'fixture.invalid',
  imageDomainMode: 'Custom', customImageDomain: 'fixture.invalid',
}
try {
  Object.assign(config, JSON.parse(sessionStorage.getItem('jm-ui-fixture-config') ?? '{}'))
} catch {
  // Ignore malformed session storage in UI fixtures
}
const removedComics = new Set(JSON.parse(sessionStorage.getItem('jm-ui-fixture-removed') ?? '[]'))
const comics = Array.from({ length: 25 }, (_, index) => ({
  id: 1000 + index, name: index === 0 ? '山间旅行' : `城市日记 ${index}`,
  author: index === 0 ? ['小林'] : ['小张'], tags: index === 0 ? ['Adventure', '风景'] : ['生活'],
  addtime: '', description: '界面测试数据', total_views: '0', likes: '0', series_id: '', comment_total: '0',
  chapterInfos: [
    { chapterId: 2000 + index, chapterTitle: '第一章', order: 1, isDownloaded: true, chapterDownloadDir: `D:/模拟下载/${1000 + index}/第一章` },
    { chapterId: 3000 + index, chapterTitle: '第二章', order: 2, isDownloaded: index % 3 === 0, chapterDownloadDir: index % 3 === 0 ? `D:/模拟下载/${1000 + index}/第二章` : null },
  ],
  works: [], actors: [], related_list: [], liked: false, is_favorite: false, is_aids: false,
  isDownloaded: true, comicDownloadDir: `D:/模拟下载/${1000 + index}`,
}))
const coverColors = ['#a491bc', '#8aaba6', '#bfa093', '#87a0bd', '#b393a1']
function mockImage(title, index, page = false) {
  const color = coverColors[index % coverColors.length]
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${page ? 720 : 240}" height="${page ? 1000 : 320}" viewBox="0 0 240 320"><rect width="240" height="320" fill="${color}"/><circle cx="190" cy="65" r="90" fill="white" opacity=".13"/><path d="M-20 235L100 100L270 300H-20Z" fill="white" opacity=".2"/><path d="M-20 290L130 165L270 340H-20Z" fill="#333" opacity=".12"/><text x="22" y="50" fill="white" font-size="12" font-family="sans-serif">LOCAL READING</text><text x="22" y="252" fill="white" font-size="22" font-family="sans-serif">${title}</text><text x="22" y="281" fill="white" opacity=".65" font-size="11" font-family="sans-serif">一段值得留住的故事</text></svg>`
  return `data:image/svg+xml;base64,${btoa(unescape(encodeURIComponent(svg)))}`
}
comics.forEach((comic, index) => { comic.localCoverData = mockImage(comic.name, index) })
let revision = 3
const tasks = [
  {
    taskId: 'failed', revision: 1, state: 'Failed', comic: comics[0], chapterInfo: comics[0].chapterInfos[0],
    downloadedImgCount: 2, totalImgCount: 5, stage: 'downloading', attempts: 3,
    failure: {
      kind: 'network', message: '总共有 5 张图片，但只下载了 2 张。第 3 张图片读取超时，请检查网络或线路后重试。',
      retryable: true, stage: 'downloading', attempts: 3, line: 'img.fixture.invalid', at: 1,
    },
    imageFailures: [{ kind: 'network', count: 3 }],
    errorMessage: '总共有 5 张图片，但只下载了 2 张。第 3 张图片读取超时，请检查网络或线路后重试。',
  },
  {
    taskId: 'paused', revision: 2, state: 'Paused', comic: comics[1], chapterInfo: comics[1].chapterInfos[0],
    downloadedImgCount: 0, totalImgCount: 0, stage: 'queued', attempts: 1, failure: null,
    imageFailures: [], errorMessage: null,
  },
]
// ?signed-in=0 可查看未登录的「我的」页；默认展示已登录形态
const signedIn = new URLSearchParams(location.search).get('signed-in') !== '0'
const fixtureProfile = {
  uid: '245871', username: '山间读者', email: '', emailverified: '', photo: mockImage('读者', 1, true),
  fname: '', gender: '', message: null, coin: 12840, album_favorites: 36, s: '', level_name: 'Lv.6',
  level: 6, nextLevelExp: 4200, exp: '2860', expPercent: 68, album_favorites_max: 500,
  ad_free: true, charge: '', jar: '', invitation_qrcode: '', invitation_url: '', invited_cnt: '0',
}
// 当月日历：1 号前补空位，1–9 号已签 7 天、漏签 2 天，之后为未结算
const fixtureCheckIn = {
  dailyId: 1, eventName: '每日签到', currentProgress: '42.86%',
  threeDaysCoin: '30', threeDaysExp: '15', sevenDaysCoin: '80', sevenDaysExp: '40',
  record: [[
    ...Array.from({ length: 8 }, (_, index) => ({ date: String(index + 1), signed: index === 3 || index === 6 ? false : true, bonus: index === 2 || index === 6 })),
    ...Array.from({ length: 14 }, (_, index) => ({ date: String(index + 9), signed: null, bonus: index === 4 })),
  ]],
  signedDays: 6, elapsedDays: 8,
}

const listeners = new Map()
let listenerId = 0
function emitTask(snapshot) {
  for (const { event, handler } of listeners.values()) {
    if (event === 'download-task-event') {
      Reflect.get(window, `_${handler}`)({ event, id: 0, payload: { event: 'Create', data: snapshot } })
    }
  }
}
mockWindows('main')
mockIPC(async (command, args) => {
  if (command === 'plugin:event|listen') {
    listeners.set(++listenerId, args)
    return listenerId
  }
  if (command === 'plugin:event|unlisten') return listeners.delete(args.eventId)
  if (command === 'plugin:app|version') return '0.2.1'
  if (command === 'get_config') return structuredClone(config)
  // 初始化会读取会话与域名预设，缺任一项整页都会停在「应用初始化失败」。
  if (command === 'get_session') return { sessionEpoch: 1, profile: signedIn ? fixtureProfile : null }
  if (command === 'get_user_profile') return fixtureProfile
  // 「我的」与签到卡需要这些数据才能渲染出已登录形态
  if (command === 'get_daily_check_in') return fixtureCheckIn
  if (command === 'daily_check_in') return { justSigned: false, message: '今日已签到', state: fixtureCheckIn }
  if (command === 'logout') return null
  if (command === 'get_api_domain_presets') return [{ mode: 'Official', label: '官方', domain: 'api.fixture.invalid' }]
  if (command === 'get_image_domain_presets') return [{ mode: 'Official', label: '官方', domain: 'img.fixture.invalid' }]
  // 这些命令在 bindings 里被包成 Result，IPC 返回值就是 data 本身
  if (command === 'get_api_domain_status') return { label: '域名', state: 'Passed', elapsedMs: 12, message: '线路正常' }
  if (command === 'get_logs_dir_size') return 0
  if (command === 'get_logs_dir_size') return 0
  if (command === 'get_downloaded_comics') return structuredClone(comics.filter((comic) => !removedComics.has(comic.id)))
  if (command === 'get_download_tasks') return structuredClone(tasks)
  if (command === 'save_config') {
    Object.assign(config, structuredClone(args.config))
    sessionStorage.setItem('jm-ui-fixture-config', JSON.stringify(config))
    return null
  }
  if (command === 'get_weekly_info') return { categories: [{ id: '258', time: '2026年第258期' }], type: [{ id: 'hanman', title: '韩漫' }, { id: 'manga', title: '日漫' }] }
  if (command === 'get_weekly') return { total: comics.length, list: comics.map((comic) => ({
    id: comic.id, name: comic.name, author: comic.author.join('、'), description: '', image: '',
    category: { title: '模拟分类' }, category_sub: { title: '日常' }, liked: false, is_favorite: false,
    update_at: 0, is_downloaded: true, comic_download_dir: comic.comicDownloadDir,
  })) }
  if (command === 'remove_library_comic') {
    removedComics.add(args.comicId)
    sessionStorage.setItem('jm-ui-fixture-removed', JSON.stringify([...removedComics]))
    return null
  }
  if (command === 'export_cbz' || command === 'export_merged_cbz') return null
  if (command === 'login') return { profile: fixtureProfile, sessionEpoch: 2, switchedApiDomainMode: null }
  if (command === 'search') return { SearchResult: { total: comics.length, content: comics.map((comic) => ({
    id: comic.id, name: comic.name, author: comic.author.join('、'), description: '', image: '',
    category: { title: '模拟分类' }, categorySub: { title: '日常' }, liked: false, isFavorite: false,
    updateAt: 0, isDownloaded: true, comicDownloadDir: comic.comicDownloadDir,
  })) } }
  if (command === 'diagnose_library') return { kind: 'Empty', message: '书库目录为空，先下载或导入漫画。', skipped: [] }
  if (command === 'get_favorite_folder') return {
    folderList: [], total: 1, count: 20,
    list: [{ id: comics[0].id, name: comics[0].name, author: comics[0].author.join('、'),
      category: { title: '模拟分类' }, categorySub: { title: '' }, isDownloaded: true,
      comicDownloadDir: comics[0].comicDownloadDir }],
  }
  if (command === 'get_chapter_reader_data') return {
    comicId: args.comicId, chapterId: args.chapterId, comicTitle: comics.find((comic) => comic.id === args.comicId)?.name ?? '模拟漫画',
    chapterTitle: '第一章', imagePaths: ['fixture-page-1', 'fixture-page-2', 'fixture-page-3', 'fixture-page-4', 'fixture-page-5'], totalImages: 5,
    prevChapter: null, nextChapter: null,
  }
  if (command === 'plugin:dialog|open') return args.options?.directory ? 'D:/模拟原文件/图片漫画' : ['D:/模拟原文件/旅行.cbz']
  if (command === 'import_local_comics') {
    removedComics.delete(-100)
    comics.unshift({ ...structuredClone(comics[0]), id: -100, name: '导入的旅行漫画', comicDownloadDir: 'D:/模拟下载/本地导入/local-100', chapterInfos: [{ chapterId: -101, chapterTitle: '第一章', order: 1, isDownloaded: true }] })
    return { imported: 1, skipped: 0, errors: [] }
  }
  if (['create_download_task', 'pause_download_task', 'resume_download_task', 'cancel_download_task'].includes(command)) {
    const task = tasks.find((task) => task.chapterInfo.chapterId === args.chapterId)
    if (!task) throw new Error('模拟任务不存在')
    task.state = command === 'cancel_download_task' ? 'Cancelled' : command === 'pause_download_task' ? 'Paused' : 'Pending'
    task.errorMessage = null
    task.stage = 'queued'
    task.failure = null
    task.attempts = 1
    task.imageFailures = []
    task.revision = ++revision
    emitTask(structuredClone(task))
    return null
  }
  throw new Error(`测试页面未模拟此操作：${command}`)
})
const pinia = createPinia()
// 初始页可用 ?tab=settings 之类的查询参数指定，方便逐个页面截图核对
const initialTab = new URLSearchParams(location.search).get('tab')
const store = useStore(pinia)
store.currentTabName = ['search', 'favorite', 'downloaded', 'account', 'settings'].includes(initialTab ?? '') ? initialTab : 'downloaded'
// ?comic=1000 直接打开章节详情页，方便核对详情页头与章节列表
const initialComicId = Number(new URLSearchParams(location.search).get('comic'))
if (Number.isSafeInteger(initialComicId) && initialComicId > 0) {
  const comic = comics.find((item) => item.id === initialComicId) ?? comics[0]
  store.showComic(comic, false)
}
createApp(App).use(pinia).mount('#app')
