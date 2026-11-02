<script setup lang="ts">
import { ref } from 'vue'
import Counter from './Counter.vue'

const name = ref('Vue')
</script>

<template>
  <main>
    <p class="label">VUE SINGLE-FILE COMPONENTS</p>
    <h1>Hello, {{ name }}!</h1>
    <p class="muted">A package.json loader compiles .vue files with vue/compiler-sfc.</p>
    <input v-model="name" placeholder="Your name">
    <Counter :step="2" />
  </main>
</template>

<style>
body {
  margin: 0;
  background: #edf2ed;
  color: #172c29;
  font-family: system-ui, sans-serif;
}
</style>

<style scoped>
main {
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-width: 640px;
  margin: 0 auto;
  padding: 48px 24px;
}
h1 {
  margin: 0;
  font-size: 44px;
}
.label {
  margin: 0;
  font-size: 12px;
  font-weight: 700;
}
.muted {
  margin: 0;
  color: #5e7069;
}
input {
  padding: 10px 12px;
  border: 1px solid #c9d5cd;
  border-radius: 8px;
  font-size: 16px;
}
</style>
