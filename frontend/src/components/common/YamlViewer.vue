<script setup lang="ts">
import { ref } from "vue";

defineProps<{
  yaml: string;
}>();

const copied = ref(false);
let copyResetTimer: ReturnType<typeof setTimeout> | null = null;

async function copyToClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    copied.value = true;
    if (copyResetTimer) clearTimeout(copyResetTimer);
    copyResetTimer = setTimeout(() => {
      copied.value = false;
    }, 1500);
  } catch {
    copied.value = false;
  }
}
</script>

<template>
  <v-card variant="outlined">
    <v-card-title class="d-flex align-center py-2">
      <v-icon icon="mdi-file-code-outline" class="mr-2" size="small" />
      <span class="text-body-2">YAML</span>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        :prepend-icon="copied ? 'mdi-check' : 'mdi-content-copy'"
        :color="copied ? 'success' : undefined"
        @click="copyToClipboard(yaml)"
      >
        {{ copied ? "Copied" : "Copy" }}
      </v-btn>
    </v-card-title>
    <div class="yaml-output">
      <pre><code>{{ yaml }}</code></pre>
    </div>
  </v-card>
</template>

<style scoped>
.yaml-output {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117;
  color: #c9d1d9;
  max-height: 600px;
  overflow: auto;
}

.yaml-output pre {
  margin: 0;
  padding: 12px;
}

.yaml-output code {
  font-family: inherit;
  background: none;
  color: inherit;
  white-space: pre;
}
</style>
