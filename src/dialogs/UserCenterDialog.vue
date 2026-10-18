<script setup lang="ts">
import { ref, watch, onBeforeUnmount } from 'vue'
import { commands } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import FloatLabelInput from '../components/FloatLabelInput.vue'
import FavoritePane from '../panes/FavoritePane.vue'
import { useStore } from '../store.ts'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { PhSignOut, PhUser, PhX } from '@phosphor-icons/vue'
import { useBackHandler } from '../composables/useAndroidBack.ts'

const store = useStore()
const message = useMessage()

const showing = defineModel<boolean>('showing', { required: true })

useBackHandler(() => {
  if (showing.value) {
    showing.value = false
    return true
  }
  return false
}, 50)

const username = ref<string>(store.config?.username ?? '')
const password = ref<string>(store.config?.password ?? '')
const remember = ref<boolean>(username.value !== '' && password.value !== '')
const loggingIn = ref(false)
const loggingOut = ref(false)
let disposed = false
onBeforeUnmount(() => { disposed = true; if (loggingIn.value) void commands.cancelLogin().catch(console.error) })

// Close user center dialog when a comic detail is opened
watch(
  () => store.chapterDetailShowing,
  (val) => {
    if (val) {
      showing.value = false
    }
  },
)

async function onLogin() {
  if (store.config === undefined || loggingIn.value) {
    return
  }
  const cleanUsername = username.value.trim()
  const cleanPassword = password.value
  if (cleanUsername === '') {
    message.error('请输入用户名')
    return
  }
  if (cleanPassword === '') {
    message.error('请输入密码')
    return
  }

  loggingIn.value = true
  try {
    const result = await commands.login(cleanUsername, cleanPassword)
    if (disposed || (result.status === 'ok' && result.data.sessionEpoch < store.sessionEpoch)) return
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      return
    }
    const switched = store.applyLoginResult(result.data)
    const route = store.apiDomainPresets.find((preset) => preset.mode === switched)
    message.success(switched ? `登录成功，已切换至${route?.label ?? switched}` : '登录成功')
    if (remember.value) {
      store.config.username = cleanUsername
      store.config.password = cleanPassword
      store.config.autoLogin = true
    } else {
      store.config.username = ''
      store.config.password = ''
      store.config.autoLogin = false
    }
  } catch (error) {
    console.error(error)
    message.error(`登录失败：${getErrorMessage(error)}`)
  } finally {
    loggingIn.value = false
  }
}

async function handleLogout() {
  if (loggingOut.value || loggingIn.value) return
  loggingOut.value = true
  try {
    const result = await store.runConfigMaintenance(async () => {
      if (store.config) { store.config.username = ''; store.config.password = ''; store.config.autoLogin = false }
      const result = await commands.logout()
      store.applyConfig(await commands.getConfig(), true)
      store.applySession(await commands.getSession())
      return result
    })
    if (result.status === 'error') message.error(result.error.err_message)
    else { username.value = ''; password.value = ''; remember.value = false; message.info('已退出登录并清除记住密码') }
  } catch (error) { message.error(`退出登录失败：${getErrorMessage(error)}`) }
  finally { loggingOut.value = false }
}

async function cancelLogin() { await commands.cancelLogin() }

async function clearUsernameAndPasswordInConfig() {
  if (store.config === undefined) return
  store.config.username = ''
  store.config.password = ''
  store.config.autoLogin = false
  try { await store.saveConfigNow(); message.success('已清除配置文件中的记住密码') }
  catch (error) { message.error(`清除记住密码未能保存：${getErrorMessage(error)}`) }
}
</script>

<template>
  <n-modal v-model:show="showing" class="user-center-modal">
    <div class="user-center-container">
      <!-- Modal Header -->
      <div class="user-center-header">
        <div v-if="store.userProfile !== undefined" class="user-profile-bar">
          <n-avatar
            round
            :size="44"
            :src="store.userProfile.photo"
            fallback-src="https://cdn-msp.18comic.vip/templates/frontend/airav/img/title-png/more-ms-jm.webp?v=2"
            class="user-avatar" />
          <div class="user-info">
            <div class="user-name-row">
              <span class="user-name">{{ store.userProfile.username }}</span>
              <span
                v-if="store.userProfile.level_name || store.userProfile.level"
                class="vip-badge">
                {{ store.userProfile.level_name || `Lv.${store.userProfile.level}` }}
              </span>
            </div>
            <div class="user-sub">
              <span>UID: {{ store.userProfile.uid }}</span>
              <span class="dot-sep">•</span>
              <span class="text-neutral-500">云端收藏</span>
            </div>
          </div>
          <div class="header-right-actions">
            <button
              type="button"
              class="logout-btn"
              title="退出登录"
              :disabled="loggingIn || loggingOut" @click="handleLogout">
              <PhSignOut :size="16" />
              <span>退出登录</span>
            </button>
            <button
              type="button"
              class="close-modal-btn"
              title="关闭"
              @click="showing = false">
              <PhX :size="18" />
            </button>
          </div>
        </div>

        <div v-else class="not-logged-in-header">
          <div class="flex items-center gap-2">
            <div class="login-icon-box">
              <PhUser :size="20" weight="bold" />
            </div>
            <h3 class="login-title">登录 JoyManga 账号</h3>
          </div>
          <div class="header-right-actions">
            <button
              type="button"
              class="close-modal-btn"
              title="关闭"
              @click="showing = false">
              <PhX :size="18" />
            </button>
          </div>
        </div>
      </div>

      <!-- Modal Body -->
      <div class="user-center-body">
        <!-- Logged in state: Show embedded FavoritePane -->
        <div v-if="store.userProfile !== undefined" class="favorites-wrapper">
          <div class="favorites-header-title">
            <span class="pink-bar"></span>
            <h4>我的在线收藏</h4>
            <span class="sub-hint">点击任意漫画直接展开章节详情</span>
          </div>
          <div class="favorites-content">
            <FavoritePane />
          </div>
        </div>

        <!-- Not logged in state: Show login form -->
        <div v-else class="login-form-wrapper">
          <div class="login-card">
            <n-button v-if="loggingIn" size="small" @click="cancelLogin">取消登录</n-button>
            <p class="login-tip">
              登录后可同步收藏夹，并一键收藏。
            </p>
            <div class="form-fields">
              <FloatLabelInput label="用户名" v-model:value="username" @keydown.enter="onLogin" />
              <FloatLabelInput label="密码" v-model:value="password" type="password" @keydown.enter="onLogin" />
            </div>
            <div class="login-options">
              <n-tooltip>
                账密以明文保存在本机配置文件中
                <template #trigger>
                  <n-checkbox v-model:checked="remember">记住我</n-checkbox>
                </template>
              </n-tooltip>
              <button
                type="button"
                class="clear-cred-btn"
                @click="clearUsernameAndPasswordInConfig">
                清除保存的账密
              </button>
            </div>
            <n-button
              type="primary"
              class="submit-login-btn"
              :loading="loggingIn"
              :disabled="loggingIn"
              @click="onLogin">
              立即登录
            </n-button>
          </div>
        </div>
      </div>
    </div>
  </n-modal>
</template>

<style scoped>
.user-center-container {
  width: min(960px, 94vw);
  max-height: 86vh;
  background-color: #ffffff;
  border: 1px solid #e3e5e7;
  border-radius: 16px;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.12);
}

.user-center-header {
  padding: 16px 20px;
  background-color: #f6f7f8;
  border-bottom: 1px solid #e3e5e7;
}

.user-profile-bar {
  display: flex;
  align-items: center;
  gap: 16px;
  width: 100%;
}

.user-avatar {
  border: 2px solid #fb7299;
  box-shadow: 0 2px 8px rgba(251, 114, 153, 0.25);
}

.user-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  flex: 1;
}

.user-name-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.user-name {
  color: #18191c;
  font-weight: 700;
  font-size: 16px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vip-badge {
  font-size: 11px;
  font-weight: 700;
  color: #ffffff;
  background: linear-gradient(135deg, #fb7299 0%, #ff85ad 100%);
  padding: 2px 8px;
  border-radius: 6px;
  letter-spacing: 0.5px;
  box-shadow: 0 2px 6px rgba(251, 114, 153, 0.35);
  flex-shrink: 0;
  white-space: nowrap;
}

.user-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: #9499a0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dot-sep {
  opacity: 0.5;
}

.header-right-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.logout-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  border-radius: 8px;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  color: #61666d;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
}

.logout-btn:hover {
  background: #fff0f5;
  color: #fb7299;
  border-color: #fb7299;
}

.close-modal-btn {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  background: #ffffff;
  border: 1px solid #e3e5e7;
  color: #61666d;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.2s;
}

.close-modal-btn:hover {
  background: #e3e5e7;
  color: #18191c;
}

.not-logged-in-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.login-icon-box {
  width: 36px;
  height: 36px;
  border-radius: 10px;
  background: rgba(251, 114, 153, 0.12);
  color: #fb7299;
  display: flex;
  align-items: center;
  justify-content: center;
}

.login-title {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #18191c;
}

.user-center-body {
  flex: 1;
  overflow-y: auto;
  min-height: 360px;
  display: flex;
  flex-direction: column;
  background-color: #ffffff;
}

.favorites-wrapper {
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 16px 20px;
  min-height: 480px;
  height: 60vh;
}

.favorites-header-title {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
}

.pink-bar {
  width: 4px;
  height: 16px;
  background-color: #fb7299;
  border-radius: 2px;
}

.favorites-header-title h4 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  color: #18191c;
}

.sub-hint {
  font-size: 12px;
  color: #9499a0;
  margin-left: 6px;
}

.favorites-content {
  flex: 1;
  min-height: 0;
}

.login-form-wrapper {
  padding: 36px 20px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.login-card {
  width: 100%;
  max-width: 400px;
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.login-tip {
  margin: 0;
  font-size: 13px;
  line-height: 1.6;
  color: #61666d;
  text-align: center;
}

.form-fields {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.login-options {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
}

.clear-cred-btn {
  background: none;
  border: none;
  color: #9499a0;
  font-size: 12px;
  cursor: pointer;
  text-decoration: underline;
  transition: color 0.15s;
}

.clear-cred-btn:hover {
  color: #fb7299;
}

.submit-login-btn {
  width: 100%;
  height: 42px;
  font-weight: 700;
  font-size: 15px;
  border-radius: 8px;
  box-shadow: 0 4px 14px rgba(251, 114, 153, 0.35);
}

@media (max-width: 768px) {
  .user-center-container {
    width: 100vw;
    height: 100vh;
    max-height: 100vh;
    border-radius: 0;
    border: none;
    box-shadow: none;
  }

  .user-center-header {
    padding: calc(12px + env(safe-area-inset-top, 0px)) 16px 12px;
  }

  .user-center-body {
    padding-bottom: env(safe-area-inset-bottom, 0px);
  }

  .user-profile-bar {
    gap: 12px;
  }
}
</style>
