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
  <n-modal
    v-model:show="showUpdateDialog"
    :mask-closable="false"
    class="max-w-[92vw] sm:max-w-[440px] w-full"
  >
    <div class="bg-white rounded-2xl shadow-2xl border border-gray-100 overflow-hidden flex flex-col max-h-[80vh]">
      <!-- 标题栏：新版本号直接给出，不再单独做版本对照卡片 -->
      <div class="flex items-center gap-2 px-4 py-3 border-b border-gray-100">
        <h3 class="m-0 text-sm font-bold text-gray-800 flex-shrink-0">发现新版本</h3>
        <span class="text-xs font-semibold text-pink-600 bg-pink-50 rounded-md px-1.5 py-0.5 flex-shrink-0">
          v{{ latestVersion }}
        </span>
        <span v-if="currentVersion" class="text-[11px] text-gray-400 truncate">当前 v{{ currentVersion }}</span>
        <button
          type="button"
          class="ml-auto text-gray-400 hover:text-gray-600 p-1 rounded-lg hover:bg-gray-100 transition-colors bg-transparent border-none cursor-pointer"
          aria-label="关闭"
          @click="showUpdateDialog = false"
        >
          <PhX :size="16" />
        </button>
      </div>

      <!-- 更新日志：弹窗的主体内容 -->
      <div class="flex-1 min-h-[96px] max-h-[300px] overflow-y-auto px-4 py-3 text-xs text-gray-600 whitespace-pre-wrap leading-relaxed">
        {{ notes || '本次更新没有附带说明。' }}
      </div>

      <!-- 操作区：一个主按钮 + 跳过；关闭即稍后提醒，不再重复一个链接 -->
      <div class="px-4 py-3 border-t border-gray-100 flex flex-col gap-2">
        <div v-if="installing || installMessage" class="flex flex-col gap-1">
          <n-progress v-if="installing" type="line" :percentage="installProgress" :processing="installProgress < 100" />
          <span v-if="installMessage" class="text-[11px] text-gray-500">{{ installMessage }}</span>
        </div>
        <n-button
          type="primary"
          block
          class="font-semibold"
          :loading="installing"
          :disabled="installing"
          @click="downloadUpdate"
        >
          <template #icon>
            <PhDownloadSimple :size="16" />
          </template>
          {{ isAndroid ? (installing ? `下载中 ${installProgress}%` : '下载并更新 APK') : '下载新版本' }}
        </n-button>
        <button
          type="button"
          class="text-[11px] text-gray-400 hover:text-gray-600 bg-transparent border-none p-0 cursor-pointer self-center"
          @click="ignoreCurrentUpdate"
        >
          跳过此版本
        </button>
      </div>
    </div>
  </n-modal>
</template>
