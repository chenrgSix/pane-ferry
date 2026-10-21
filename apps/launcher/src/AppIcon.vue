<script setup lang="ts">
import { ref, watch } from "vue";
const props = defineProps<{ icon?: string | null; name: string }>();
const failed = ref(false);
watch(() => props.icon, () => { failed.value = false; });
</script>
<template>
  <span class="app-icon" aria-hidden="true">
    <img v-if="icon && !failed" :src="`data:image/png;base64,${icon}`" alt="" @error="failed = true" />
    <span v-else>{{ Array.from(name)[0]?.toUpperCase() || "▦" }}</span>
  </span>
</template>
