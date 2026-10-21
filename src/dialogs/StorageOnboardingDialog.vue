<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { PhFolderOpen, PhShieldCheck, PhX } from '@phosphor-icons/vue'
import { useMessage } from 'naive-ui'
import { commands } from '../bindings.ts'
import { useStore } from '../store.ts'
import { formatCommandError, getErrorMessage } from '../utils/errors.ts'
import { canWritePublicDownloads, downloadsPath, hasAllFilesAccess, onPermissionResult, requestAllFilesAccess, supportsPublicStorage } from '../utils/androidStorage.ts'
import { useBackHandler } from '../composables/useAndroidBack.ts'

const show = defineModel<boolean>('show', { required: true })
const store = useStore()
const message = useMessage()
const initializing = ref(false)
// Android 10 没有可用的公共存储授权，直接隐藏该选项，避免用户点进死路
const publicStorageAvailable = supportsPublicStorage()

useBackHandler(() => {
  if (!show.value) return false
  show.value = false
  return true
}, 60)

/** 点击「授权」后注册：从系统设置回到应用（或收到运行时权限结果）时复查权限，通过则完成公共下载初始化 */
let stopWatchingAuthorization: (() => void) | undefined

function clearAuthorizationWatcher() {
  stopWatchingAuthorization?.()
  stopWatchingAuthorization = undefined
}

async function initializeWith(path: string, successText: string): Promise<boolean> {
  if (initializing.value) return false
  initializing.value = true
  try {
    const result = await commands.initializeAndroidStorage(path)
    if (result.status === 'error') {
      message.error(formatCommandError(result.error))
      return false
    }
    store.applyConfig(result.data)
    message.success(successText)
    clearAuthorizationWatcher()
    show.value = false
    return true
  } catch (error) {
    message.error(getErrorMessage(error))
    return false
  } finally {
    initializing.value = false
  }
}

function authorizePublicDownloads() {
  if (initializing.value) return
  clearAuthorizationWatcher()
  const tryInitialize = () => {
    if (!show.value || !hasAllFilesAccess()) return
    // 权限已授予但公共目录不可写：留在引导里提示用户改用应用专用目录
    if (!canWritePublicDownloads()) {
      message.warning('公共下载目录当前不可写，请改用应用专用目录')
      return
    }
    void initializeWith(downloadsPath(), '漫画将保存到公共下载')
  }
  // 系统设置页返回：WebView 可见性变化；API 29 以下的运行时权限：原生回传 jm-permission-result。
  // 两条路径都必须显式移除监听，否则每次回到前台都会重复初始化并改写配置。
  const onVisibilityChange = () => {
    if (document.visibilityState === 'visible') tryInitialize()
  }
  const removePermissionListener = onPermissionResult((granted) => {
    if (granted) tryInitialize()
  })
  document.addEventListener('visibilitychange', onVisibilityChange)
  stopWatchingAuthorization = () => {
    document.removeEventListener('visibilitychange', onVisibilityChange)
    removePermissionListener()
  }
  requestAllFilesAccess()
  message.info('请在系统设置中授予「所有文件访问」，返回应用后自动完成')
}

function useAppStorage() {
  void initializeWith(window.JmAndroid?.libraryPath() ?? '', '漫画将保存到应用专用目录')
}

onBeforeUnmount(clearAuthorizationWatcher)
</script>

<template>
  <n-modal v-model:show="show" :auto-focus="true" :trap-focus="true">
    <section class="onboarding-shell" role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
      <header class="onboarding-header">
        <h2 id="onboarding-title"><PhFolderOpen :size="20" aria-hidden="true" />选择漫画保存位置</h2>
        <button class="onboarding-close" type="button" aria-label="关闭存储位置引导" @click="show = false"><PhX :size="20" aria-hidden="true" /></button>
      </header>
      <div class="onboarding-body">
        <div v-if="publicStorageAvailable" class="onboarding-option">
          <h3><PhShieldCheck :size="18" aria-hidden="true" />公共下载（推荐）</h3>
          <p>漫画存到 <strong>Download/JoyManga</strong>：文件管理器直接可见，卸载应用不会删除漫画文件，方便备份。需要授予「所有文件访问」权限。</p>
        </div>
        <div class="onboarding-option">
          <h3>应用专用目录</h3>
          <p>无需任何权限，但文件管理器不可见，卸载应用时会一并清空；需要长期保留时再导出到系统下载。</p>
        </div>
      </div>
      <footer class="onboarding-footer">
        <n-button v-if="publicStorageAvailable" type="primary" color="#b73963" text-color="#ffffff" :loading="initializing" @click="authorizePublicDownloads">授权并使用公共下载</n-button>
        <n-button :type="publicStorageAvailable ? 'default' : 'primary'" :color="publicStorageAvailable ? undefined : '#b73963'" :text-color="publicStorageAvailable ? undefined : '#ffffff'" :secondary="publicStorageAvailable" :loading="initializing" @click="useAppStorage">使用应用专用目录</n-button>
        <button class="onboarding-skip" type="button" :disabled="initializing" @click="show = false">暂时跳过</button>
      </footer>
    </section>
  </n-modal>
</template>

<style scoped>
.onboarding-shell { --onboarding-text: var(--text-primary, #18191c); --onboarding-muted: #5e6672; --onboarding-border: var(--border-color, #e3e5e7); width: min(520px, calc(100vw - 40px)); display: flex; flex-direction: column; overflow: hidden; color: var(--onboarding-text); background: var(--bg-card, #fff); border: 1px solid var(--onboarding-border); border-radius: var(--radius-panel); box-shadow: 0 16px 56px rgb(0 0 0 / 16%); }
.onboarding-header { flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; padding: 16px 24px; border-bottom: 1px solid var(--onboarding-border); }
.onboarding-header h2 { display: flex; align-items: center; gap: 10px; margin: 0; font-size: 20px; font-weight: 650; }
.onboarding-header h2 svg { color: var(--bili-pink-active); }
.onboarding-close { display: grid; place-items: center; width: 44px; height: 44px; padding: 0; border: 0; border-radius: 10px; background: transparent; color: var(--onboarding-muted); cursor: pointer; }
.onboarding-close:hover { background: var(--bg-subtle); color: var(--onboarding-text); }
.onboarding-close:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
.onboarding-body { display: grid; gap: 12px; padding: 20px 24px; }
.onboarding-option { padding: 12px 14px; border: 1px solid var(--onboarding-border); border-radius: 10px; background: var(--bg-subtle); }
.onboarding-option h3 { display: flex; align-items: center; gap: 8px; margin: 0 0 6px; font-size: 15px; font-weight: 650; }
.onboarding-option h3 svg { color: var(--bili-pink-active); }
.onboarding-option p { margin: 0; color: var(--onboarding-muted); font-size: 14px; line-height: 1.65; overflow-wrap: anywhere; }
.onboarding-option p strong { color: var(--onboarding-text); }
.onboarding-footer { display: grid; gap: 10px; padding: 16px 24px calc(16px + env(safe-area-inset-bottom, 0px)); border-top: 1px solid var(--onboarding-border); }
.onboarding-footer :deep(.n-button) { min-height: 44px; }
.onboarding-skip { min-height: 40px; border: 0; background: transparent; color: var(--onboarding-muted); font-size: 14px; cursor: pointer; }
.onboarding-skip:not(:disabled):hover { color: var(--onboarding-text); }
.onboarding-skip:focus-visible { outline: 2px solid var(--bili-pink-active); outline-offset: -2px; }
@media (max-width: 600px) {
  .onboarding-shell { width: calc(100vw - 24px); }
  .onboarding-header { padding: 12px 16px; }
  .onboarding-body { padding: 14px 16px; }
  .onboarding-footer { padding: 12px 16px calc(12px + env(safe-area-inset-bottom, 0px)); }
}
</style>
