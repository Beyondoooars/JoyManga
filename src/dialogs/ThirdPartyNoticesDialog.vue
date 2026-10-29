<script setup lang="ts">
import { PhX } from '@phosphor-icons/vue'
import { useBackHandler } from '../composables/useAndroidBack.ts'
// 许可全文随应用打包，保证离线也能查看；仓库内同一份文件见 third_party/
import converterLicense from '../../third_party/opencc-fmmseg-LICENSE.txt?raw'
import dictionaryLicense from '../../third_party/opencc-dictionaries-LICENSE.txt?raw'

const show = defineModel<boolean>('show', { required: true })

const attribution = `JoyManga 以 MIT License 开源。

本应用包含以下第三方组件：

opencc-fmmseg 0.13.0（搜索关键词简繁转换）
Copyright (c) 2025 Laisuk Lai
MIT License

OpenCC 词典（由 opencc-fmmseg 内嵌）
Copyright © OpenCC contributors
Apache License 2.0

────────────────────────────────────────

${converterLicense}

────────────────────────────────────────

${dictionaryLicense}`

useBackHandler(() => {
  if (!show.value) return false
  show.value = false
  return true
})
</script>

<template>
  <n-modal v-model:show="show" :auto-focus="true" :trap-focus="true">
    <section class="notices-shell" role="dialog" aria-modal="true" aria-labelledby="notices-title">
      <header class="notices-header">
        <h2 id="notices-title">开源许可</h2>
        <button
          type="button"
          class="notices-close"
          aria-label="关闭开源许可"
          @click="show = false">
          <PhX :size="20" aria-hidden="true" />
        </button>
      </header>
      <pre class="notices-body" tabindex="0">{{ attribution }}</pre>
    </section>
  </n-modal>
</template>

<style scoped>
.notices-shell {
  display: flex;
  width: min(92vw, 640px);
  max-height: 84vh;
  flex-direction: column;
  overflow: hidden;
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
  box-shadow: var(--jm-shadow-2);
}

.notices-header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px;
  border-bottom: 1px solid var(--jm-line);
}

.notices-header h2 {
  margin: 0;
  color: var(--jm-text);
  font-size: var(--jm-fs-title);
  font-weight: 700;
}

.notices-close {
  display: grid;
  width: 32px;
  height: 32px;
  padding: 0;
  color: var(--jm-text-3);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: transparent;
  cursor: pointer;
  place-items: center;
}

.notices-close:hover {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

.notices-body {
  margin: 0;
  padding: 14px 16px 18px;
  overflow: auto;
  color: var(--jm-text-2);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  line-height: 1.55;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}
</style>
