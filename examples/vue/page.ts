import { createApp } from 'vue'
import App from './App.vue'

Object.assign(globalThis, { __VUE_OPTIONS_API__: true, __VUE_PROD_DEVTOOLS__: false, __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: false })
createApp(App).mount('#app')
