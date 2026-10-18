import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import 'virtual:uno.css'
import './styles.css'

const pinia = createPinia()
const app = createApp(App)

app.config.errorHandler = (err, _instance, info) => {
  console.error('[Vue Global Error]:', err, info)
}

app.use(pinia).mount('#app')
