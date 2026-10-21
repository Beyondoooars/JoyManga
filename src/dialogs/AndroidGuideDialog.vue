<script setup lang="ts">
import { useAndroidBack } from '../composables/useAndroidBack'

const showing = defineModel<boolean>('showing', { required: true })

function handleDismiss() {
  localStorage.setItem('joymanga_android_guide_shown', 'true')
  showing.value = false
}

useAndroidBack(() => {
  if (showing.value) {
    handleDismiss()
    return true
  }
  return false
}, 50)
</script>

<template>
  <n-modal v-model:show="showing">
    <n-card
      class="max-w-[92vw] sm:max-w-lg rounded-2xl shadow-xl"
      title="📱 Android 使用指南"
      :bordered="false"
      size="small"
      role="dialog"
      aria-modal="true"
    >
      <div class="flex flex-col gap-y-3.5 text-xs text-gray-600 leading-relaxed max-h-[65vh] overflow-y-auto pr-1">
        <!-- 存储与卸载 -->
        <div class="bg-gray-50 rounded-xl p-3 border border-gray-100">
          <div class="font-bold text-sm text-gray-800 flex items-center gap-1.5 mb-1.5">
            <span class="text-base">📂</span>
            <span>漫画书库与公共导出</span>
          </div>
          <div class="space-y-1 text-gray-600">
            <div>
              • 首次启动可选择把漫画存到公共下载目录（推荐，卸载不删除）或应用专用目录；公共书库位置：
              <div class="font-mono text-pink-600 bg-pink-50 rounded px-2 py-0.5 mt-1 select-all break-all inline-block">
                Download/JoyManga
              </div>
            </div>
            <div>• <strong class="text-gray-800">卸载前先导出</strong>：应用专用书库会随卸载清理，公共书库与导出的文件保留。</div>
            <div>• <strong class="text-gray-800">外部阅读器</strong>：导出的文件可用其他 CBZ 阅读器打开；也能导入 CBZ 与漫画文件夹。</div>
          </div>
        </div>

        <!-- 后台保活 -->
        <div class="bg-gray-50 rounded-xl p-3 border border-gray-100">
          <div class="font-bold text-sm text-gray-800 flex items-center gap-1.5 mb-1.5">
            <span class="text-base">⚡</span>
            <span>后台下载防休眠建议</span>
          </div>
          <div class="space-y-1 text-gray-600">
            <div>• 下载时显示持续通知；系统仍可能终止后台进程，重开应用可恢复未完成任务。</div>
            <div>
              • 建议前往系统【设置 ➔ 应用管理 ➔ JoyManga ➔ 电池 / 耗电策略】，把电池优化设为
              <span class="text-pink-600 font-semibold">“无限制” / “允许后台运行”</span>。
            </div>
          </div>
        </div>

        <!-- 通知与网络 -->
        <div class="bg-gray-50 rounded-xl p-3 border border-gray-100">
          <div class="font-bold text-sm text-gray-800 flex items-center gap-1.5 mb-1.5">
            <span class="text-base">🔔</span>
            <span>通知权限与网络分流</span>
          </div>
          <div class="space-y-1 text-gray-600">
            <div>• Android 13+ 首次下载会请求通知权限；拒绝后仍可在应用内查看任务状态。</div>
            <div>• 图片加载慢或解析失败时，可在【设置】切换分流线路或开启代理。</div>
          </div>
        </div>

        <!-- 数据备份与彻底卸载 -->
        <div class="bg-gray-50 rounded-xl p-3 border border-gray-100">
          <div class="font-bold text-sm text-gray-800 flex items-center gap-1.5 mb-1.5">
            <span class="text-base">🛠️</span>
            <span>数据备份与彻底清理</span>
          </div>
          <div class="space-y-1 text-gray-600">
            <div>• 彻底释放空间：在「设置 ➔ 数据管理」中清理缓存，或直接删除书库目录中的漫画文件夹。</div>
            <div>• 换机或卸载前请分别导出漫画与数据备份；备份不含漫画文件和账号密码。</div>
          </div>
        </div>
      </div>

      <template #footer>
        <div class="flex justify-end gap-2 pt-2">
          <n-button type="primary" class="w-full sm:w-auto" @click="handleDismiss">
            知道了
          </n-button>
        </div>
      </template>
    </n-card>
  </n-modal>
</template>
