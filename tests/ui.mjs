// 仅通过 /tests/ui.html 加载；所有 IPC 都在内存中模拟，不读写真实账号或下载文件。
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'
import App from '../src/App.vue'
import { useStore } from '../src/store.ts'
import 'virtual:uno.css'
import '../src/styles.css'

const config = {
  username: '', password: '', downloadDir: 'D:/模拟下载', exportDir: 'D:/模拟导出',
  downloadFormat: 'Jpeg', dirFmt: '{comic_title}/{chapter_title}', proxyMode: 'NoProxy',
  proxyHost: '127.0.0.1', proxyPort: 7890, enableFileLogger: false,
  chapterConcurrency: 3, chapterDownloadIntervalSec: 0, imgConcurrency: 8, imgDownloadIntervalSec: 0,
  downloadAllFavoritesIntervalSec: 0, updateDownloadedComicsIntervalSec: 0,
  apiDomainMode: 'Custom', customApiDomain: 'fixture.invalid',
  imageDomainMode: 'Custom', customImageDomain: 'fixture.invalid', shouldDownloadCover: false,
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
  chapterInfos: [{ chapterId: 2000 + index, chapterTitle: '第一章', order: 1, isDownloaded: true }],
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
let mockImageFailed = false
let revision = 3
const tasks = [
  { taskId: 'failed', revision: 1, state: 'Failed', comic: comics[0], chapterInfo: comics[0].chapterInfos[0], downloadedImgCount: 2, totalImgCount: 5, errorMessage: '总共有 5 张图片，但只下载了 2 张。第 3 张图片读取超时，请检查网络或线路后重试。' },
  { taskId: 'paused', revision: 2, state: 'Paused', comic: comics[1], chapterInfo: comics[1].chapterInfos[0], downloadedImgCount: 0, totalImgCount: 0, errorMessage: null },
]
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
  if (command === 'export_cbz' || command === 'export_pdf') return null
  if (command === 'login') return { uid: 'fixture-user', username: '模拟读者', photo: '', level: 1 }
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
  if (command === 'get_local_image_data_url') {
    if (args.filePath === 'fixture-page-3' && !mockImageFailed) {
      mockImageFailed = true
      throw { err_title: '模拟图片读取失败', err_message: '此文件已损坏，请重新下载' }
    }
    return mockImage(`第 ${args.filePath.slice(-1)} 页`, 1, true)
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
    task.revision = ++revision
    emitTask(structuredClone(task))
    return null
  }
  throw new Error(`测试页面未模拟此操作：${command}`)
})
const pinia = createPinia()
useStore(pinia).currentTabName = 'downloaded'
createApp(App).use(pinia).mount('#app')
