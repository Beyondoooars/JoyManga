<script setup lang="ts">
import { computed } from 'vue'
import { NModal, NButton } from 'naive-ui'
import { PhDownloadSimple, PhX } from '@phosphor-icons/vue'
import { useAppUpdate } from '../composables/useAppUpdate.ts'
import { isAndroid } from '../composables/useIsMobile.ts'
import { useAndroidBack } from '../composables/useAndroidBack.ts'

const {
  showUpdateDialog,
  currentVersion,
  latestVersion,
  releaseNotes,
  downloadUpdate,
  ignoreCurrentUpdate,
  installing,
  installProgress,
  installMessage,
} = useAppUpdate()

// Release 正文由 scripts/release-notes.mjs 生成，是「### 新功能 / * 条目」的纯列表。
// 这里把 markdown 记号（`###`、`*`、`**`）转成纯文本；同时兼容旧版本正文里的
// 打包说明、对比链接与 `by @xxx in #1`，用户在旧版应用里看到的历史说明也不会露出记号。
const notes = computed(() =>
  releaseNotes.value
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => !/^自动编译并校验签名的 Android 安装包已就绪。?$/.test(line))
    .filter((line) => !/^(\*\*)?Full Changelog(\*\*)?:/.test(line))
    .filter((line, index) => !(index === 0 && /^#{1,6}\s*JoyManga\s+v/i.test(line)))
    .map((line) =>
      line
        .replace(/^#{1,6}\s*/, '')
        .replace(/^\*\s+/, '· ')
        .replace(/\s+by\s+@\S+(\s+in\s+\S+)?$/i, '')
        .replace(/\*\*/g, ''),
    )
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim(),
)

useAndroidBack(() => {
  if (showUpdateDialog.value) {
    showUpdateDialog.value = false
    return true
  }
  return false
}, 60)
</script>

<template>
  <n-modal v-model:show="showUpdateDialog" :mask-closable="false">
    <section class="update-shell" role="dialog" aria-modal="true" aria-labelledby="update-title">
      <!-- 标题栏：新版本号直接给出，不再单独做版本对照卡片 -->
      <header class="update-header">
        <h3 id="update-title">发现新版本</h3>
        <span class="update-version">v{{ latestVersion }}</span>
        <span v-if="currentVersion" class="update-current">当前 v{{ currentVersion }}</span>
        <button type="button" class="update-close" aria-label="关闭" @click="showUpdateDialog = false">
          <PhX :size="16" weight="bold" aria-hidden="true" />
        </button>
      </header>

      <!-- 更新日志：弹窗的主体内容 -->
      <div class="update-notes">{{ notes || '本次更新没有附带说明。' }}</div>

      <!-- 操作区：一个主按钮 + 跳过；关闭即稍后提醒，不再重复一个链接 -->
      <footer class="update-footer">
        <div v-if="installing || installMessage" class="update-progress">
          <n-progress v-if="installing" type="line" :percentage="installProgress" :processing="installProgress < 100" />
          <span v-if="installMessage">{{ installMessage }}</span>
        </div>
        <n-button type="primary" block class="update-action" :loading="installing" :disabled="installing" @click="downloadUpdate">
          <template #icon>
            <PhDownloadSimple :size="16" aria-hidden="true" />
          </template>
          {{ isAndroid ? (installing ? `下载中 ${installProgress}%` : '下载并更新 APK') : '下载新版本' }}
        </n-button>
        <button type="button" class="update-skip" @click="ignoreCurrentUpdate">跳过此版本</button>
      </footer>
    </section>
  </n-modal>
</template>

<style scoped>
.update-shell {
  display: flex;
  max-height: min(560px, calc(100dvh - 48px));
  width: min(440px, calc(100vw - 32px));
  flex-direction: column;
  overflow: hidden;
  color: var(--jm-text);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-xl);
  background: var(--jm-surface);
  box-shadow: 0 16px 56px rgb(23 24 28 / 16%);
}

.update-header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--jm-line);
}

.update-header h3 {
  flex-shrink: 0;
  margin: 0;
  color: var(--jm-text);
  font-size: var(--jm-fs-body);
  font-weight: 800;
}

.update-version {
  flex-shrink: 0;
  padding: 2px 7px;
  color: var(--jm-brand-press);
  border-radius: var(--jm-r-xs);
  background: var(--jm-brand-soft);
  font-size: var(--jm-fs-sm);
  font-weight: 700;
}

.update-current {
  overflow: hidden;
  min-width: 0;
  color: var(--jm-text-4);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.update-close {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  margin-left: auto;
  place-items: center;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-sm);
  background: transparent;
  cursor: pointer;
}

.update-close:hover {
  color: var(--jm-text);
  background: var(--jm-sunken);
}

.update-close:focus-visible {
  outline: 2px solid var(--jm-brand);
  outline-offset: -2px;
}

.update-notes {
  flex: 1 1 auto;
  min-height: 96px;
  max-height: 300px;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 12px 16px;
  color: var(--jm-text-2);
  font-size: var(--jm-fs-sm);
  line-height: 1.7;
  white-space: pre-wrap;
}

.update-footer {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  gap: 8px;
  padding: 12px 16px calc(12px + env(safe-area-inset-bottom, 0px));
  border-top: 1px solid var(--jm-line);
}

.update-progress {
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--jm-text-3);
  font-size: 11px;
}

.update-action {
  font-weight: 700;
}

.update-skip {
  align-self: center;
  padding: 4px 8px;
  color: var(--jm-text-3);
  border: 0;
  background: transparent;
  font-size: 11px;
  cursor: pointer;
}

.update-skip:hover {
  color: var(--jm-text-2);
}
</style>
