<script setup lang="ts">
import { computed, ref } from 'vue'

const props = defineProps<{ step: number }>()
const count = ref(0)
const doubled = computed(() => count.value * 2)
</script>

<template>
  <section>
    <h2>Count: {{ count }}</h2>
    <p v-if="count > 0">Doubled: {{ doubled }}</p>
    <div>
      <button @click="count += props.step">Add {{ step }}</button>
      <button class="secondary" @click="count = 0">Reset</button>
    </div>
  </section>
</template>

<style scoped>
section {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 24px;
  border-radius: 12px;
  background: white;
  box-shadow: 0 4px 12px rgb(0 0 0 / 0.08);
}
h2, p {
  margin: 0;
}
div {
  display: flex;
  gap: 12px;
}
button {
  padding: 10px 20px;
  border: 0;
  border-radius: 8px;
  background: oklch(0.72 0.11 178);
  font-size: 16px;
  font-weight: 600;
  cursor: pointer;
}
.secondary {
  background: #edf2ed;
}
</style>
