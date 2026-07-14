<script setup lang="ts">
import { ref, watch, computed } from "vue";

const props = defineProps<{
  initialYaml: string;
  loading?: boolean;
}>();

const emit = defineEmits<{
  save: [yaml: string];
  cancel: [];
}>();

const draft = ref(props.initialYaml);
const showDiscardDialog = ref(false);

watch(
  () => props.initialYaml,
  (v) => {
    draft.value = v;
  },
);

const isDirty = computed(() => draft.value !== props.initialYaml);

function reset() {
  draft.value = props.initialYaml;
}

function handleCancel() {
  if (isDirty.value) {
    showDiscardDialog.value = true;
  } else {
    emit("cancel");
  }
}

function confirmDiscard() {
  showDiscardDialog.value = false;
  draft.value = props.initialYaml;
  emit("cancel");
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
        :disabled="!isDirty || loading"
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
      <v-btn variant="text" :disabled="loading" @click="handleCancel">
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

    <v-dialog v-model="showDiscardDialog" max-width="400">
      <v-card>
        <v-card-title>Discard changes?</v-card-title>
        <v-card-text>
          You have unsaved edits. Are you sure you want to discard them?
        </v-card-text>
        <v-card-actions>
          <v-spacer />
          <v-btn variant="text" @click="showDiscardDialog = false">
            Keep editing
          </v-btn>
          <v-btn color="error" variant="flat" @click="confirmDiscard">
            Discard
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
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
