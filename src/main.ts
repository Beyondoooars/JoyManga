import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles.css'
import { recordFrontendError } from './utils/frontendErrors.ts'

const pinia = createPinia()
const app = createApp(App)

app.config.errorHandler = (err, _instance, info) => {
  console.error('[Vue Global Error]:', err, info)
  recordFrontendError(`vue:${info}`, err)
}

window.addEventListener('error', (event) => {
  recordFrontendError('window:error', event.error ?? event.message)
})

window.addEventListener('unhandledrejection', (event) => {
  recordFrontendError('window:unhandledrejection', event.reason)
})

app.use(pinia).mount('#app')
