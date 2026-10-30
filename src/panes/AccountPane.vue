<script setup lang="ts">
import { computed, onActivated, onBeforeUnmount, onDeactivated, ref, watch } from 'vue'
import { commands } from '../bindings.ts'
import { useMessage } from 'naive-ui'
import FloatLabelInput from '../components/FloatLabelInput.vue'
import DailyCheckInCard from '../components/DailyCheckInCard.vue'
import { useStore } from '../store.ts'
import { getErrorMessage, formatCommandError } from '../utils/errors.ts'
import { compactNumber, expProgress } from '../utils/dailyCheckIn.ts'
import {
  PhCaretRight,
  PhCoin,
  PhGearSix,
  PhHeart,
  PhSignIn,
  PhSignOut,
  PhTrendUp,
  PhUser,
} from '@phosphor-icons/vue'

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
  void commands
    .getUserProfile()
    .then((result) => {
      if (result.status === 'error') console.error(result.error)
    })
    .catch(console.error)
})

const username = ref<string>(store.config?.username ?? '')
const password = ref<string>(store.config?.password ?? '')
const remember = ref<boolean>(username.value !== '' && password.value !== '')
const loggingIn = ref(false)
const loggingOut = ref(false)
// 会话失效或用户主动重新登录时，改显示登录表单
const showLoginForm = ref(false)
let disposed = false
onBeforeUnmount(() => {
  disposed = true
  store.manualLoginInProgress = false
  if (loggingIn.value) void commands.cancelLogin().catch(console.error)
})

// 页面被 KeepAlive 缓存：切到别的标签后登录请求仍在跑，成功后会在后台改写配置并切走用户。
// 失活即取消（与旧弹窗关闭时行为一致）；返回本页后再点一次即可。
let cancelledByLeave = false
onDeactivated(() => {
  store.manualLoginInProgress = false
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
  store.manualLoginInProgress = true
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
    store.manualLoginInProgress = false
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
  <div class="profile-page jm-scroll">
    <!-- 已登录：资料卡 + 快捷入口 + 签到 + 阅读数据 -->
    <template v-if="store.userProfile !== undefined && !showLoginForm">
      <section class="profile-hero">
        <div class="hero-top">
          <n-avatar
            round
            :size="58"
            :src="store.userProfile.photo"
            fallback-src="https://cdn-msp.18comic.vip/templates/frontend/airav/img/title-png/more-ms-jm.webp?v=2"
            class="hero-avatar" />
          <div class="hero-identity">
            <div class="name-row">
              <strong class="hero-name">{{ store.userProfile.username }}</strong>
              <span v-if="store.userProfile.level_name || store.userProfile.level" class="level-badge">
                {{ store.userProfile.level_name || `Lv.${store.userProfile.level}` }}
              </span>
            </div>
            <span class="hero-uid">UID {{ store.userProfile.uid }} · 云端已同步</span>
          </div>
          <button
            type="button"
            class="hero-settings"
            aria-label="打开设置"
            @click="store.openSettings()">
            <PhGearSix :size="19" weight="duotone" />
          </button>
        </div>

        <div class="hero-stats">
          <div class="hero-stat">
            <span class="stat-value">
              <PhCoin :size="15" weight="duotone" class="stat-coin" />
              {{ compactNumber(store.userProfile.coin) }}
            </span>
            <span class="stat-label">金币</span>
          </div>
          <span class="stat-divider" aria-hidden="true"></span>
          <div class="hero-stat">
            <span class="stat-value">
              <PhTrendUp :size="15" weight="duotone" class="stat-exp" />
              {{ exp.current }}
            </span>
            <span class="stat-label">经验{{ exp.target > 0 ? ` / ${exp.target}` : '' }}</span>
          </div>
          <span class="stat-divider" aria-hidden="true"></span>
          <div class="hero-stat">
            <span class="stat-value">
              <PhHeart :size="15" weight="duotone" class="stat-heart" />
              {{ compactNumber(store.userProfile.album_favorites) }}
            </span>
            <span class="stat-label">云端收藏</span>
          </div>
        </div>

        <div class="exp-track" role="progressbar" :aria-valuenow="exp.percent" aria-valuemin="0" aria-valuemax="100" aria-label="等级经验">
          <span class="exp-fill" :style="{ width: `${exp.percent}%` }"></span>
        </div>
      </section>

      <DailyCheckInCard />

      <button type="button" class="logout-row" :disabled="loggingIn || loggingOut" @click="handleLogout">
        <PhSignOut :size="18" />
        <span>退出登录</span>
        <span class="logout-note">同时清除记住的账密</span>
      </button>
    </template>

    <!-- 未登录：欢迎卡 + 登录表单 + 本地入口 -->
    <template v-else>
      <section class="guest-hero">
        <span class="guest-mark" aria-hidden="true">
          <PhUser :size="26" weight="duotone" />
        </span>
        <strong>登录 JoyManga</strong>
        <p>登录后同步云端收藏，在「发现」里一键收藏喜欢的漫画。</p>
        <p v-if="store.userProfile !== undefined" class="relogin-tip">当前会话已失效，重新登录后可继续使用在线收藏。</p>
      </section>

      <section class="login-card">
        <div class="form-fields">
          <FloatLabelInput label="用户名" v-model:value="username" @keydown.enter="onLogin" />
          <FloatLabelInput label="密码" v-model:value="password" type="password" @keydown.enter="onLogin" />
        </div>
        <div class="login-options">
          <n-tooltip>
            账号与密码加密保存在本机，换设备后需重新登录
            <template #trigger>
              <n-checkbox v-model:checked="remember">记住我</n-checkbox>
            </template>
          </n-tooltip>
          <button type="button" class="clear-cred-btn" @click="clearUsernameAndPasswordInConfig">清除保存的账密</button>
        </div>
        <n-button type="primary" class="submit-login-btn" :loading="loggingIn" :disabled="loggingIn" @click="onLogin">
          <template #icon><PhSignIn :size="18" weight="bold" /></template>
          立即登录
        </n-button>
        <n-button v-if="loggingIn" size="small" quaternary @click="cancelLogin">取消登录</n-button>
        <n-button v-if="store.userProfile !== undefined" size="small" quaternary @click="showLoginForm = false">返回我的</n-button>
        <p class="privacy-note">账号信息只保存在本机；退出登录会一并清除记住的账密。</p>
      </section>

      <button type="button" class="settings-row" @click="store.openSettings()">
        <PhGearSix :size="18" />
        <span>设置</span>
        <PhCaretRight :size="16" class="row-chevron" />
      </button>
    </template>
  </div>
</template>

<style scoped>
.profile-page {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding-bottom: 28px;
  background: var(--jm-bg);
}

/* 资料卡 */
.profile-hero {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin: 12px var(--jm-gutter) 0;
  padding: 16px;
  border: 1px solid rgba(251, 114, 153, 0.2);
  border-radius: var(--jm-r-xl);
  background:
    radial-gradient(120% 90% at 100% 0%, rgba(0, 174, 236, 0.1) 0%, transparent 58%),
    linear-gradient(135deg, var(--jm-brand-soft) 0%, var(--jm-surface) 62%);
  box-shadow: var(--jm-shadow-1);
}

.hero-top {
  display: flex;
  align-items: center;
  gap: 13px;
}

.hero-avatar {
  flex: 0 0 auto;
  border: 2px solid var(--jm-surface);
  box-shadow: 0 0 0 2px var(--jm-brand-ring), var(--jm-shadow-1);
}

.hero-identity {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 4px;
}

.name-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 7px;
}

.hero-name {
  overflow: hidden;
  max-width: 64%;
  color: var(--jm-text);
  font-size: 18px;
  font-weight: 800;
  letter-spacing: -0.02em;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.level-badge {
  flex: 0 0 auto;
  padding: 2px 8px;
  color: #fff;
  border-radius: var(--jm-r-pill);
  background: linear-gradient(135deg, var(--jm-brand) 0%, var(--jm-brand-hover) 100%);
  box-shadow: 0 2px 6px var(--jm-brand-ring);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.02em;
  white-space: nowrap;
}

.hero-uid {
  overflow: hidden;
  color: var(--jm-text-3);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hero-stats {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 12px;
  border-radius: var(--jm-r-lg);
  background: rgba(255, 255, 255, 0.76);
}

.hero-stat {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  align-items: center;
  gap: 3px;
}

.stat-value {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--jm-text);
  font-size: 16px;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
  line-height: 1.15;
}

.stat-coin {
  color: var(--jm-coin);
}

.stat-exp {
  color: var(--jm-accent);
}

.stat-heart {
  color: var(--jm-brand);
}

.stat-label {
  overflow: hidden;
  max-width: 100%;
  color: var(--jm-text-3);
  font-size: 10.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.stat-divider {
  width: 1px;
  height: 24px;
  flex: 0 0 auto;
  background: var(--jm-line);
}

.exp-track {
  height: 5px;
  overflow: hidden;
  border-radius: var(--jm-r-pill);
  background: rgba(23, 24, 28, 0.07);
}

.exp-fill {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, var(--jm-accent) 0%, var(--jm-brand) 100%);
  transition: width 320ms ease;
}

/* 设置入口：资料卡右上角 */
.hero-settings {
  display: grid;
  width: 36px;
  height: 36px;
  flex: 0 0 auto;
  margin-left: auto;
  align-self: flex-start;
  padding: 0;
  color: var(--jm-text-2);
  border: 0;
  border-radius: var(--jm-r-pill);
  background: var(--jm-surface);
  box-shadow: var(--jm-shadow-1);
  cursor: pointer;
  place-items: center;
  transition:
    color 160ms ease,
    background 160ms ease;
}

.hero-settings:hover {
  color: var(--jm-brand);
  background: var(--jm-brand-soft);
}

/* 行式入口 */
.logout-row,
.settings-row {
  display: flex;
  min-height: 50px;
  align-items: center;
  gap: 9px;
  margin: 0 var(--jm-gutter);
  padding: 0 16px;
  color: var(--jm-text-2);
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
  font-size: var(--jm-fs-body);
  font-weight: 600;
  text-align: left;
  cursor: pointer;
}

.logout-row {
  color: var(--jm-danger);
}

.logout-row:disabled {
  opacity: 0.55;
  cursor: wait;
}

.logout-note {
  margin-left: auto;
  color: var(--jm-text-4);
  font-size: 10.5px;
  font-weight: 500;
}

.row-chevron {
  margin-left: auto;
  color: var(--jm-text-4);
}

/* 未登录 */
.guest-hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 30px 24px 18px;
  text-align: center;
}

.guest-mark {
  display: grid;
  width: 58px;
  height: 58px;
  color: var(--jm-brand);
  place-items: center;
  border-radius: 20px;
  background: linear-gradient(140deg, var(--jm-brand-soft), var(--jm-brand-tint));
  box-shadow: 0 0 0 6px rgba(251, 114, 153, 0.08);
}

.guest-hero strong {
  margin-top: 4px;
  color: var(--jm-text);
  font-size: 19px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

.guest-hero p {
  max-width: 300px;
  margin: 0;
  color: var(--jm-text-3);
  font-size: var(--jm-fs-sm);
  line-height: 1.6;
}

.guest-hero .relogin-tip {
  margin-top: 4px;
  padding: 8px 12px;
  color: var(--jm-warning);
  border-radius: var(--jm-r-md);
  background: var(--jm-warning-soft);
  font-size: var(--jm-fs-xs);
}

.login-card {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin: 0 var(--jm-gutter);
  padding: 16px;
  border: 1px solid var(--jm-line);
  border-radius: var(--jm-r-lg);
  background: var(--jm-surface);
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
  font-size: var(--jm-fs-xs);
}

.clear-cred-btn {
  color: var(--jm-text-3);
  border: none;
  background: none;
  font-size: var(--jm-fs-xs);
  text-decoration: underline;
  cursor: pointer;
}

.clear-cred-btn:hover {
  color: var(--jm-brand);
}

.submit-login-btn {
  width: 100%;
  height: 46px;
  border-radius: var(--jm-r-md);
  box-shadow: 0 4px 14px rgba(251, 114, 153, 0.3);
  font-size: 15px;
  font-weight: 700;
}

.privacy-note {
  margin: 0;
  color: var(--jm-text-4);
  font-size: 10.5px;
  line-height: 1.5;
  text-align: center;
}
</style>
