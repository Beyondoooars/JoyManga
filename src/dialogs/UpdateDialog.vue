<script setup lang="ts">
import { NModal, NButton } from 'naive-ui'
import {
  PhRocketLaunch,
  PhDownloadSimple,
  PhArrowSquareOut,
  PhX,
  PhPackage,
} from '@phosphor-icons/vue'
import { useAppUpdate } from '../composables/useAppUpdate.ts'
import { isAndroid } from '../composables/useIsMobile.ts'
import { useAndroidBack } from '../composables/useAndroidBack.ts'

const {
  showUpdateDialog,
  currentVersion,
  latestVersion,
  releaseTitle,
  releaseNotes,
  publishedAt,
  downloadFileName,
  downloadFileSize,
  downloadUpdate,
  openReleasePage,
  ignoreCurrentUpdate,
} = useAppUpdate()

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
    class="max-w-[92vw] sm:max-w-[480px] w-full"
  >
    <div class="bg-white rounded-2xl shadow-2xl border border-gray-100 overflow-hidden flex flex-col max-h-[85vh]">
      <!-- 头部装饰条与标题 -->
      <div class="bg-gradient-to-r from-pink-500 to-rose-500 px-5 py-4 text-white flex items-center justify-between">
        <div class="flex items-center gap-2.5">
          <div class="p-2 bg-white/20 backdrop-blur rounded-xl flex items-center justify-center">
            <PhRocketLaunch :size="22" weight="fill" />
          </div>
          <div>
            <h3 class="m-0 text-base font-bold leading-tight">发现新版本</h3>
            <p class="m-0 text-[11px] text-white/80 mt-0.5">
              {{ releaseTitle || `v${latestVersion}` }} {{ publishedAt ? `· ${publishedAt}` : '' }}
            </p>
          </div>
        </div>
        <button
          type="button"
          class="text-white/80 hover:text-white p-1 rounded-lg hover:bg-white/10 transition-colors"
          @click="showUpdateDialog = false"
        >
          <PhX :size="18" />
        </button>
      </div>

      <!-- 版本对照信息 -->
      <div class="px-5 pt-4 pb-2">
        <div class="flex items-center justify-between bg-gray-50 border border-gray-100 rounded-xl px-4 py-2.5 text-xs">
          <div class="flex flex-col">
            <span class="text-gray-400 text-[11px]">当前版本</span>
            <span class="font-medium text-gray-700">v{{ currentVersion }}</span>
          </div>
          <div class="text-gray-300 font-bold">➜</div>
          <div class="flex flex-col text-right">
            <span class="text-gray-400 text-[11px]">最新版本</span>
            <span class="font-bold text-pink-600 text-sm">v{{ latestVersion }}</span>
          </div>
        </div>
      </div>

      <!-- 更新日志说明卡片 -->
      <div class="px-5 py-2 flex-1 min-h-[120px] max-h-[260px] flex flex-col overflow-hidden">
        <div class="text-xs font-semibold text-gray-700 mb-1.5 flex items-center gap-1">
          <span>更新内容</span>
        </div>
        <div class="flex-1 bg-gray-50/80 border border-gray-100 rounded-xl p-3 overflow-y-auto text-xs text-gray-600 whitespace-pre-wrap leading-relaxed">
          {{ releaseNotes }}
        </div>
      </div>

      <!-- 推荐安装包信息 -->
      <div v-if="downloadFileName" class="px-5 py-1">
        <div class="flex items-center justify-between text-[11px] text-gray-500 bg-pink-50/60 border border-pink-100 rounded-lg px-3 py-1.5">
          <div class="flex items-center gap-1.5 truncate">
            <PhPackage :size="14" class="text-pink-600 flex-shrink-0" />
            <span class="truncate font-mono">{{ downloadFileName }}</span>
          </div>
          <span v-if="downloadFileSize" class="font-medium text-pink-600 flex-shrink-0 ml-2">
            {{ downloadFileSize }}
          </span>
        </div>
      </div>

      <!-- 底部操作按钮 -->
      <div class="px-5 py-4 border-t border-gray-100 flex flex-col gap-2 mt-1 bg-gray-50/50">
        <div class="flex gap-2.5">
          <n-button
            type="primary"
            class="flex-1 font-semibold"
            @click="downloadUpdate"
          >
            <template #icon>
              <PhDownloadSimple :size="16" />
            </template>
            {{ isAndroid ? '下载并更新 APK' : '下载新版本' }}
          </n-button>

          <n-button
            secondary
            @click="openReleasePage"
          >
            <template #icon>
              <PhArrowSquareOut :size="16" />
            </template>
            详情
          </n-button>
        </div>

        <div class="flex items-center justify-between px-1 pt-1">
          <button
            type="button"
            class="text-[11px] text-gray-400 hover:text-gray-600 bg-transparent border-none p-0 cursor-pointer"
            @click="ignoreCurrentUpdate"
          >
            跳过此版本
          </button>
          <button
            type="button"
            class="text-[11px] text-gray-400 hover:text-gray-600 bg-transparent border-none p-0 cursor-pointer"
            @click="showUpdateDialog = false"
          >
            稍后提醒
          </button>
        </div>
      </div>
    </div>
  </n-modal>
</template>
