<script setup lang="ts">
import { ref, computed, watch, onActivated, onBeforeUnmount, onDeactivated } from 'vue'
import { commands } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import FloatLabelInput from '../components/FloatLabelInput.vue'
import FavoritePane from './FavoritePane.vue'
import DailyCheckInCard from '../components/DailyCheckInCard.vue'
import { useStore } from '../store.ts'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { compactNumber, expProgress } from '../utils/dailyCheckIn.ts'
import { PhCoin, PhSignOut, PhTrendUp, PhUser } from '@phosphor-icons/vue'

const store = useStore()
const message = useMessage()

/** 金币与等级经验都挂在用户资料上，签到成功后后端会重新拉取资料并经会话事件推回来 */
const exp = computed(() => expProgress(store.userProfile))

// 资料只在登录时写入 store，金币/经验会随时间变化：每次回到本页静默刷新一次。
// 失败不提示——签到卡自己会显示拉取失败，这里再弹一次只是噪音。
let activatedOnce = false
onActivated(() => {
  if (!activatedOnce) {
    activatedOnce = true
    return
  }
  if (store.userProfile === undefined) return
  void commands.getUserProfile().then((result) => {
    if (result.status === 'error') console.error(result.error)
  }).catch(console.error)
})

const username = ref<string>(store.config?.username ?? '')
const password = ref<string>(store.config?.password ?? '')
const remember = ref<boolean>(username.value !== '' && password.value !== '')
const loggingIn = ref(false)
const loggingOut = ref(false)
// 收藏区提示会话失效（或用户主动重新登录）时，即使仍有本地资料也改显示登录表单
const showLoginForm = ref(false)
let disposed = false
onBeforeUnmount(() => {
  disposed = true
  if (loggingIn.value) void commands.cancelLogin().catch(console.error)
})

// 页面被 KeepAlive 缓存：切到别的标签后登录请求仍在跑，成功后会在后台改写配置并切走用户。
// 失活即取消（与旧弹窗关闭时行为一致）；返回本页后再点一次即可。
let cancelledByLeave = false
onDeactivated(() => {
  if (loggingIn.value) {
    loggingIn.value = false
    cancelledByLeave = true
    void commands.cancelLogin().catch(console.error)
  }
})

// 表单初值只在 setup 时取一次，缓存复用会保留过期内容（清除账密后仍可被重新写回、
// 会话过期后旧密码仍显示）。配置或会话变化时按最新状态重建表单。
watch(
  () => [store.accountFormRevision, store.config?.username, store.config?.password] as const,
  () => {
    if (loggingIn.value) return
    username.value = store.config?.username ?? ''
    password.value = store.config?.password ?? ''
    remember.value = username.value !== '' && password.value !== ''
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
  cancelledByLeave = false
  try {
    const result = await commands.login(cleanUsername, cleanPassword)
    // 用户已离开本页（登录被取消）时不要静默落到后台完成并改写配置
    if (disposed || cancelledByLeave || (result.status === 'ok' && result.data.sessionEpoch < store.sessionEpoch)) return
    if (result.status === 'error') {
      console.error(result.error)
      message.error(formatCommandError(result.error))
      return
    }
    const switched = store.applyLoginResult(result.data)
    // 重新登录成功：收起登录表单、回到账号页
    showLoginForm.value = false
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
      if (store.config) {
        store.config.username = ''
        store.config.password = ''
        store.config.autoLogin = false
      }
      const result = await commands.logout()
      store.applyConfig(await commands.getConfig(), true)
      store.applySession(await commands.getSession())
      return result
    })
    if (result.status === 'error') message.error(result.error.err_message)
    else {
      username.value = ''
      password.value = ''
      remember.value = false
      message.info('已退出登录并清除记住密码')
    }
  } catch (error) {
    message.error(`退出登录失败：${getErrorMessage(error)}`)
  } finally {
    loggingOut.value = false
  }
}

async function cancelLogin() {
  await commands.cancelLogin()
}

async function clearUsernameAndPasswordInConfig() {
  if (store.config === undefined) return
  store.config.username = ''
  store.config.password = ''
  store.config.autoLogin = false
  try {
    await store.saveConfigNow()
    message.success('已清除配置文件中的记住密码')
  } catch (error) {
    message.error(`清除记住密码未能保存：${getErrorMessage(error)}`)
  }
}
</script>

<template>
  <div class="account-page">
    <!-- 已登录：账号信息 + 在线收藏 -->
    <template v-if="store.userProfile !== undefined && !showLoginForm">
      <div class="user-profile-bar">
        <n-avatar
          round
          :size="48"
          :src="store.userProfile.photo"
          fallback-src="https://cdn-msp.18comic.vip/templates/frontend/airav/img/title-png/more-ms-jm.webp?v=2"
          class="user-avatar" />
        <div class="user-info">
          <div class="user-name-row">
            <span class="user-name">{{ store.userProfile.username }}</span>
            <span v-if="store.userProfile.level_name || store.userProfile.level" class="vip-badge">
              {{ store.userProfile.level_name || `Lv.${store.userProfile.level}` }}
            </span>
          </div>
          <div class="user-sub">
            <span>UID: {{ store.userProfile.uid }}</span>
            <span class="dot-sep">•</span>
            <span class="text-neutral-500">云端收藏</span>
          </div>
          <div class="user-stats">
            <span class="user-stat" title="金币">
              <PhCoin :size="14" weight="duotone" class="stat-icon coin" />
              {{ compactNumber(store.userProfile.coin) }}
            </span>
            <span class="user-stat" :title="`经验 ${exp.current}${exp.target > 0 ? ` / ${exp.target}` : ''}`">
              <PhTrendUp :size="14" weight="duotone" class="stat-icon exp" />
              {{ exp.current }}
              <span v-if="exp.target > 0" class="stat-target">/ {{ exp.target }}</span>
            </span>
            <span v-if="exp.target > 0" class="user-exp-bar" role="progressbar" :aria-valuenow="exp.percent" aria-valuemin="0" aria-valuemax="100">
              <span class="user-exp-fill" :style="{ width: `${exp.percent}%` }"></span>
            </span>
          </div>
        </div>
        <button
          type="button"
          class="logout-btn"
          title="退出登录"
          :disabled="loggingIn || loggingOut"
          @click="handleLogout">
          <PhSignOut :size="16" />
          <span>退出登录</span>
        </button>
      </div>

      <DailyCheckInCard />

      <div class="favorites-content">
        <FavoritePane @open-login="showLoginForm = true" />
      </div>
    </template>

    <!-- 未登录（或会话失效需重新登录）：页内登录表单 -->
    <div v-else class="login-form-wrapper">
      <div class="login-card">
        <div class="login-headline">
          <div class="login-icon-box"><PhUser :size="22" weight="bold" /></div>
          <h2>登录 JoyManga 账号</h2>
          <p class="login-tip">登录后可同步收藏夹，并一键收藏。账号信息只保存在本机。</p>
        </div>
        <p v-if="store.userProfile !== undefined" class="relogin-tip">当前会话已失效，重新登录后可继续使用在线收藏。</p>
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
          <button type="button" class="clear-cred-btn" @click="clearUsernameAndPasswordInConfig">
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
        <n-button v-if="loggingIn" size="small" @click="cancelLogin">取消登录</n-button>
        <n-button
          v-if="store.userProfile !== undefined"
          size="small"
          quaternary
          @click="showLoginForm = false">
          返回账号页
        </n-button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.account-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  background: #fff;
}

.user-profile-bar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 14px;
  padding: 16px 20px;
  border-bottom: 1px solid #f1f2f3;
}

.user-avatar {
  flex-shrink: 0;
  border: 2px solid #fb7299;
  box-shadow: 0 2px 8px rgba(251, 114, 153, 0.25);
}

.user-info {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 5px;
}

.user-name-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.user-name {
  overflow: hidden;
  color: #18191c;
  font-size: 16px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vip-badge {
  flex-shrink: 0;
  padding: 2px 8px;
  color: #fff;
  border-radius: 6px;
  background: linear-gradient(135deg, #fb7299 0%, #ff85ad 100%);
  box-shadow: 0 2px 6px rgba(251, 114, 153, 0.35);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.5px;
  white-space: nowrap;
}

.user-sub {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
  color: #9499a0;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dot-sep {
  opacity: 0.5;
}

.user-stats {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 12px;
  min-width: 0;
}

.user-stat {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 4px;
  color: #61666d;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.stat-icon.coin {
  color: #f5a623;
}

.stat-icon.exp {
  color: #00aeec;
}

.stat-target {
  color: #9499a0;
  font-weight: 400;
}

.user-exp-bar {
  flex: 0 1 96px;
  height: 5px;
  min-width: 48px;
  overflow: hidden;
  border-radius: 999px;
  background: #f1f2f3;
}

.user-exp-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, #00aeec 0%, #fb7299 100%);
  transition: width 0.3s;
}

.logout-btn {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  color: #61666d;
  border: 1px solid #e3e5e7;
  border-radius: 8px;
  background: #fff;
  font-size: 12px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s;
}

.logout-btn:hover {
  color: #fb7299;
  border-color: #fb7299;
  background: #fff0f5;
}

.favorites-content {
  flex: 1;
  min-height: 0;
}

.login-form-wrapper {
  display: flex;
  flex: 1;
  align-items: center;
  justify-content: center;
  overflow-y: auto;
  padding: 32px 20px;
}

.login-card {
  display: flex;
  width: 100%;
  max-width: 400px;
  flex-direction: column;
  gap: 18px;
}

.login-headline {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  text-align: center;
}

.login-headline h2 {
  margin: 0;
  color: #18191c;
  font-size: 17px;
  font-weight: 700;
}

.login-icon-box {
  display: flex;
  width: 44px;
  height: 44px;
  align-items: center;
  justify-content: center;
  color: #fb7299;
  border-radius: 12px;
  background: rgba(251, 114, 153, 0.12);
}

.login-tip {
  margin: 0;
  color: #61666d;
  font-size: 13px;
  line-height: 1.6;
}

.relogin-tip {
  margin: 0;
  padding: 10px 12px;
  color: #a8552a;
  border: 1px solid #f3d9c4;
  border-radius: 8px;
  background: #fdf3ea;
  font-size: 12px;
  line-height: 1.6;
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
  color: #9499a0;
  border: none;
  background: none;
  font-size: 12px;
  text-decoration: underline;
  cursor: pointer;
  transition: color 0.15s;
}

.clear-cred-btn:hover {
  color: #fb7299;
}

.submit-login-btn {
  width: 100%;
  height: 42px;
  border-radius: 8px;
  box-shadow: 0 4px 14px rgba(251, 114, 153, 0.35);
  font-size: 15px;
  font-weight: 700;
}

@media (max-width: 760px) {
  .user-profile-bar {
    padding: 14px;
    gap: 12px;
  }

  /* 窄屏下退出按钮只留图标，把宽度让给金币/经验 */
  .logout-btn span {
    display: none;
  }

  .logout-btn {
    padding: 8px;
  }

  .user-exp-bar {
    flex-basis: 72px;
  }
}

/* 320~360dp：头像与资料行留给文字的空间更紧张，收一号 */
@media (max-width: 360px) {
  .user-profile-bar {
    padding: 12px 10px;
    gap: 10px;
  }

  .user-name {
    font-size: 15px;
  }

  .user-sub {
    font-size: 11px;
  }
}
</style>
