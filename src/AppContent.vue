<script setup lang="ts">
import { computed, defineAsyncComponent } from 'vue'
import type { Component } from 'vue'
import { useMessage } from 'naive-ui'
import { PhBooks, PhCompass, PhDownloadSimple, PhHeart, PhUser } from '@phosphor-icons/vue'
import { useStore } from './store.ts'
import icon from './assets/icon.png'
import type { CurrentTabName } from './types.ts'
import { handleRootBackExit, useBackHandler } from './composables/useAndroidBack.ts'
import { useAppBootstrap } from './composables/useAppBootstrap.ts'

const StorageOnboardingDialog = defineAsyncComponent(() => import('./dialogs/StorageOnboardingDialog.vue'))
const ReaderDialog = defineAsyncComponent(() => import('./dialogs/ReaderDialog.vue'))
const UpdateDialog = defineAsyncComponent(() => import('./dialogs/UpdateDialog.vue'))
const ChapterPane = defineAsyncComponent(() => import('./panes/ChapterPane.vue'))

const pageComponents: Record<CurrentTabName, Component> = {
  search: defineAsyncComponent(() => import('./panes/SearchPane.vue')),
  favorite: defineAsyncComponent(() => import('./panes/FavoritePane.vue')),
  downloaded: defineAsyncComponent(() => import('./panes/DownloadedPane/DownloadedPane.vue')),
  account: defineAsyncComponent(() => import('./panes/AccountPane.vue')),
  settings: defineAsyncComponent(() => import('./panes/SettingsPane.vue')),
}

const store = useStore()
const message = useMessage()

/** 底部导航的四个一级页面；其余（详情、设置）是压栈的二级页面 */
const MAIN_TABS: CurrentTabName[] = ['search', 'favorite', 'downloaded', 'account']

const chromeVisible = computed(() => MAIN_TABS.includes(store.currentTabName) && !store.chapterDetailShowing)

function navigateToTab(tab: CurrentTabName) {
  if (tab === 'account') {
    // 走 openAccount：记录来处以便返回，并按最新配置重置登录表单
    store.openAccount()
    return
  }
  store.closeComicDetail()
  if (store.currentTabName === tab && tab === 'downloaded') {
    store.downloadedSubTab = 'library'
  }
  store.currentTabName = tab
}

const currentPaneComponent = computed(() => pageComponents[store.currentTabName])

useBackHandler(() => {
  if (store.activeReaderTarget !== null) {
    store.closeReader()
    return true
  }
  // 设置页自己注册了更高优先级的返回处理（先退二级面板再离开页面），这里只兜底
  if (store.currentTabName === 'settings') {
    return store.closeSettings()
  }
  if (store.chapterDetailShowing) {
    store.closeComicDetail()
    return true
  }
  if (store.currentTabName === 'downloaded' && store.downloadedSubTab === 'transfers') {
    store.downloadedSubTab = 'library'
    return true
  }
  // 「我的」是从其他标签进去的：返回键先回到来处，而不是直接提示退出应用
  if (store.closeAccount()) {
    return true
  }
  // 其余二级标签回到「发现」，避免一次返回就直接退出应用
  if (store.currentTabName === 'favorite' || store.currentTabName === 'downloaded') {
    store.currentTabName = 'search'
    return true
  }
  return handleRootBackExit((msg) => message.info(msg))
}, 1)

const {
  initializing,
  initializationError,
  storageOnboardingShowing,
  appReady,
  initializeApp,
} = useAppBootstrap({ onNavigateToTab: navigateToTab })
</script>

<template>
  <div v-if="appReady && store.config !== undefined" class="app-shell">
    <!-- 顶栏：品牌 + 下载状态 + 设置；二级页面自带导航，不再重复一条 -->
    <header v-if="chromeVisible" class="app-header">
      <button type="button" class="brand-button" title="回到发现" @click="navigateToTab('search')">
        <span class="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 48 48" focusable="false">
            <g stroke="#241C2B" stroke-width="3" stroke-linejoin="round">
              <path d="M3.5 5.5H15C20.5228 5.5 25 9.97715 25 15.5V40.5C25 36.0817 21.4183 32.5 17 32.5H3.5V5.5Z" fill="#FB7299" />
              <path d="M44.5 5.5H33C27.4772 5.5 23 9.97715 23 15.5V40.5C23 36.0817 26.5817 32.5 31 32.5H44.5V5.5Z" fill="#00AEEC" />
            </g>
          </svg>
        </span>
        <span class="brand-title">Joy<span>Manga</span></span>
      </button>

      <div class="header-tools">
        <button
          v-if="store.activeDownloadCount > 0"
          type="button"
          class="download-chip"
          :aria-label="`${store.activeDownloadCount} 个章节正在下载，查看传输`"
          @click="store.openTransfers()">
          <PhDownloadSimple :size="15" weight="bold" />
          <span class="download-chip-count">{{ store.activeDownloadCount }}</span>
          <span v-if="store.downloadSpeed" class="download-chip-speed">{{ store.downloadSpeed }}</span>
        </button>
      </div>
    </header>

    <n-alert
      v-if="store.config?.apiDomainMigrationPending"
      class="shell-alert"
      type="warning"
      closable
      @close="store.config.apiDomainMigrationPending = false">
      已保留升级前的线路设置，可在设置 → 网络中核对。
    </n-alert>

    <main class="app-body">
      <KeepAlive>
        <ChapterPane
          v-if="store.chapterDetailShowing"
          key="chapter-detail"
          @back="store.closeComicDetail" />
        <component
          v-else
          :is="currentPaneComponent"
          :key="store.currentTabName"
          @open-login="store.openAccount({ relogin: true })"
          @back="store.closeSettings"
          @open-transfers="store.openTransfers()" />
      </KeepAlive>
    </main>

    <!-- 底部导航：发现 / 收藏 / 书库 / 我的 -->
    <nav v-if="chromeVisible" class="app-tabbar" aria-label="主导航">
      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'search' }"
        :aria-current="store.currentTabName === 'search' ? 'page' : undefined"
        @click="navigateToTab('search')">
        <span class="tab-icon"><PhCompass :size="23" :weight="store.currentTabName === 'search' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">发现</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'favorite' }"
        :aria-current="store.currentTabName === 'favorite' ? 'page' : undefined"
        @click="navigateToTab('favorite')">
        <span class="tab-icon"><PhHeart :size="23" :weight="store.currentTabName === 'favorite' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">收藏</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'downloaded' }"
        :aria-current="store.currentTabName === 'downloaded' ? 'page' : undefined"
        @click="navigateToTab('downloaded')">
        <span class="tab-icon">
          <PhBooks :size="23" :weight="store.currentTabName === 'downloaded' ? 'fill' : 'regular'" />
          <span v-if="store.activeDownloadCount > 0" class="tab-badge">{{ store.activeDownloadCount > 99 ? '99+' : store.activeDownloadCount }}</span>
        </span>
        <span class="tab-label">书库</span>
      </button>

      <button
        type="button"
        class="tab-item"
        :class="{ 'is-active': store.currentTabName === 'account' }"
        :aria-current="store.currentTabName === 'account' ? 'page' : undefined"
        @click="navigateToTab('account')">
        <span class="tab-icon"><PhUser :size="23" :weight="store.currentTabName === 'account' ? 'fill' : 'regular'" /></span>
        <span class="tab-label">我的</span>
      </button>
    </nav>

    <!-- Modals and Dialogs -->
    <StorageOnboardingDialog v-if="storageOnboardingShowing" v-model:show="storageOnboardingShowing" />
    <ReaderDialog v-if="store.activeReaderTarget" />
    <UpdateDialog />
  </div>

  <div v-else-if="initializationError" class="app-loading" role="alert">
    <img :src="icon" alt="JoyManga" class="loading-logo" />
    <div class="loading-copy">
      <strong>应用初始化失败</strong>
      <span>{{ initializationError }}</span>
    </div>
    <n-button type="primary" :loading="initializing" @click="initializeApp">重试</n-button>
  </div>

  <div v-else class="app-loading" aria-live="polite">
    <img :src="icon" alt="JoyManga" class="loading-logo is-pulsing" />
    <div class="loading-copy">
      <strong>Joy<span>Manga</span></strong>
      <span>正在加载精彩漫库…</span>
    </div>
    <div class="loading-bar"><i></i></div>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  width: 100%;
  height: 100vh;
  height: 100dvh;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-bg);
  color: var(--jm-text);
}

/* 顶栏：52px，随状态栏安全区内缩 */
.app-header {
  display: flex;
  height: calc(var(--jm-header-h) + var(--safe-top));
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: var(--safe-top) calc(var(--jm-gutter) - 4px) 0 calc(var(--jm-gutter) - 4px);
  border-bottom: 1px solid var(--jm-line);
  background: rgba(255, 255, 255, 0.96);
  backdrop-filter: saturate(180%) blur(12px);
}

.brand-button {
  display: flex;
  min-height: 44px;
  align-items: center;
  gap: 9px;
  padding: 4px 6px;
  border: 0;
  border-radius: var(--jm-r-md);
  background: transparent;
  cursor: pointer;
  transition: background-color 160ms ease;
}

.brand-button:active {
  background: var(--jm-sunken);
}

.brand-mark {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 9px;
  background: var(--jm-brand-soft);
}

.brand-mark svg {
  width: 22px;
  height: 22px;
}

.brand-title {
  color: var(--jm-text);
  font-size: 17px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

.brand-title span {
  color: var(--jm-brand);
}

.header-tools {
  display: flex;
  align-items: center;
  gap: 4px;
}

/* 下载中的常驻胶囊：点击直达传输页 */
.download-chip {
  display: inline-flex;
  height: 34px;
  align-items: center;
  gap: 6px;
  padding: 0 10px;
  color: var(--jm-accent);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent-soft);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
  cursor: pointer;
  transition: background-color 160ms ease;
}

.download-chip:active {
  background: rgba(0, 174, 236, 0.18);
}

.download-chip-count {
  font-variant-numeric: tabular-nums;
}

.download-chip-speed {
  max-width: 92px;
  overflow: hidden;
  color: var(--jm-accent-deep);
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10.5px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shell-alert {
  flex: 0 0 auto;
  border-radius: 0;
}

.app-body {
  position: relative;
  display: flex;
  min-height: 0;
  flex: 1 1 auto;
  flex-direction: column;
  overflow: hidden;
  background: var(--jm-surface);
}

.app-body > * {
  height: 100%;
}

/* 底部导航：58px + 安全区，四等分 */
.app-tabbar {
  display: flex;
  height: calc(var(--jm-nav-h) + var(--safe-bottom));
  flex: 0 0 auto;
  align-items: stretch;
  padding: 0 4px var(--safe-bottom);
  border-top: 1px solid var(--jm-line);
  background: rgba(255, 255, 255, 0.98);
  backdrop-filter: saturate(180%) blur(12px);
}

.tab-item {
  display: flex;
  flex: 1 1 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  padding: 6px 0 7px;
  color: var(--jm-text-3);
  border: 0;
  background: transparent;
  cursor: pointer;
  transition: color 160ms ease;
  -webkit-tap-highlight-color: transparent;
}

.tab-icon {
  position: relative;
  display: grid;
  width: 52px;
  height: 30px;
  place-items: center;
  border-radius: var(--jm-r-pill);
  transition:
    background-color 180ms ease,
    transform 180ms ease;
}

.tab-item.is-active {
  color: var(--jm-brand);
}

.tab-item.is-active .tab-icon {
  background: var(--jm-brand-soft);
}

.tab-item:active .tab-icon {
  transform: scale(0.92);
}

.tab-label {
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.01em;
}

.tab-item.is-active .tab-label {
  font-weight: 700;
}

.tab-badge {
  position: absolute;
  top: -3px;
  right: 6px;
  display: flex;
  min-width: 16px;
  height: 16px;
  align-items: center;
  justify-content: center;
  padding: 0 4px;
  color: #fff;
  border: 1.5px solid var(--jm-surface);
  border-radius: var(--jm-r-pill);
  background: var(--jm-accent);
  font-size: 9px;
  font-weight: 800;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

/* 加载 / 错误 */
.app-loading {
  display: flex;
  width: 100%;
  height: 100%;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 18px;
  padding: 32px;
  color: var(--jm-text);
  background: var(--jm-bg);
}

.loading-logo {
  width: 72px;
  height: 72px;
  border-radius: 20px;
  box-shadow: var(--jm-shadow-2);
  object-fit: contain;
}

.loading-logo.is-pulsing {
  animation: loading-breathe 1.8s ease-in-out infinite;
}

.loading-copy {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}

.loading-copy strong {
  font-size: 18px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

.loading-copy strong span {
  color: var(--jm-brand);
}

.loading-copy span {
  max-width: 280px;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-sm);
  text-align: center;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.loading-bar {
  width: 132px;
  height: 3px;
  overflow: hidden;
  border-radius: var(--jm-r-pill);
  background: var(--jm-line);
}

.loading-bar i {
  display: block;
  width: 42%;
  height: 100%;
  border-radius: inherit;
  background: var(--jm-brand);
  animation: loading-slide 1.1s ease-in-out infinite alternate;
}

@keyframes loading-slide {
  from {
    transform: translateX(0);
  }
  to {
    transform: translateX(138%);
  }
}

@keyframes loading-breathe {
  0%,
  100% {
    transform: scale(1);
    opacity: 1;
  }
  50% {
    transform: scale(0.94);
    opacity: 0.82;
  }
}
</style>
