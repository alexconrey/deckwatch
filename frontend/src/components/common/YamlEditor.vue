<script setup lang="ts">
import { ref, watch } from "vue";

const props = defineProps<{
  initialYaml: string;
  loading?: boolean;
}>();

const emit = defineEmits<{
  save: [yaml: string];
  cancel: [];
}>();

const draft = ref(props.initialYaml);

watch(
  () => props.initialYaml,
  (v) => {
    draft.value = v;
  },
);

const isDirty = () => draft.value !== props.initialYaml;

function reset() {
  draft.value = props.initialYaml;
}
</script>

<template>
  <v-card variant="outlined">
    <v-card-title class="d-flex align-center py-2">
      <v-icon icon="mdi-pencil-box-outline" class="mr-2" size="small" />
      <span class="text-body-2">Edit YAML</span>
      <v-spacer />
      <v-btn
        size="small"
        variant="text"
        prepend-icon="mdi-restore"
        :disabled="!isDirty() || loading"
        @click="reset"
      >
        Reset
      </v-btn>
    </v-card-title>

    <textarea
      v-model="draft"
      class="yaml-textarea"
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
    />

    <v-card-actions>
      <v-spacer />
      <v-btn variant="text" :disabled="loading" @click="emit('cancel')">
        Cancel
      </v-btn>
      <v-btn
        color="primary"
        variant="flat"
        :loading="loading"
        @click="emit('save', draft)"
      >
        Save
      </v-btn>
    </v-card-actions>
  </v-card>
</template>

<style scoped>
.yaml-textarea {
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  background: #0d1117;
  color: #c9d1d9;
  padding: 12px;
  width: 100%;
  min-height: 500px;
  max-height: 70vh;
  border: none;
  outline: none;
  resize: vertical;
  white-space: pre;
  tab-size: 2;
}

.yaml-textarea:focus {
  outline: 1px solid rgba(88, 166, 255, 0.6);
}
</style>
