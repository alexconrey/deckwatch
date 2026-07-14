<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { deploymentsUxApi } from "@/api/deployments_additions";
import type { RevisionSummary } from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  namespace: string;
  name: string;
  revisions: RevisionSummary[];
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
}>();

// Two independent selectors. `left` defaults to the current/newest revision
// (revisions[] is sorted newest-first by the backend), `right` to the one
// before it — the diff an operator almost always wants to see first.
const leftRevision = ref<number | null>(null);
const rightRevision = ref<number | null>(null);

// Cache YAML per revision so flipping the dropdown back to a previously
// fetched revision is instant. Cleared on close.
const yamlCache = ref<Map<number, string>>(new Map());
const leftYaml = ref<string>("");
const rightYaml = ref<string>("");
const loadingLeft = ref(false);
const loadingRight = ref(false);
const error = ref<string | null>(null);

const revisionOptions = computed(() =>
  props.revisions.map((r) => ({
    title: `#${r.revision}${r.is_current ? " (current)" : ""} — ${r.image || "no image"}`,
    value: r.revision,
  })),
);

const fetchYaml = async (revision: number): Promise<string> => {
  const cached = yamlCache.value.get(revision);
  if (cached !== undefined) return cached;
  const yaml = await deploymentsUxApi.revisionYaml(
    props.namespace,
    props.name,
    revision,
  );
  yamlCache.value.set(revision, yaml);
  return yaml;
};

const loadSide = async (side: "left" | "right", revision: number | null) => {
  if (revision === null) return;
  const loadingRef = side === "left" ? loadingLeft : loadingRight;
  const yamlRef = side === "left" ? leftYaml : rightYaml;
  loadingRef.value = true;
  error.value = null;
  try {
    yamlRef.value = await fetchYaml(revision);
  } catch (e) {
    error.value = e instanceof Error ? e.message : "Failed to load revision YAML";
    yamlRef.value = "";
  } finally {
    loadingRef.value = false;
  }
};

watch(
  () => props.modelValue,
  (open) => {
    if (!open) {
      // Free cache so a subsequent open re-fetches (revisions may have
      // been added by an in-flight rollout the operator wants to see).
      yamlCache.value = new Map();
      leftYaml.value = "";
      rightYaml.value = "";
      error.value = null;
      return;
    }
    if (props.revisions.length === 0) {
      error.value = "No revisions available to compare";
      return;
    }
    // Default: current vs previous. If there is only one revision, both
    // sides point at it and the diff renders as "no differences".
    leftRevision.value = props.revisions[0].revision;
    rightRevision.value =
      props.revisions[1]?.revision ?? props.revisions[0].revision;
    void loadSide("left", leftRevision.value);
    void loadSide("right", rightRevision.value);
  },
);

watch(leftRevision, (rev) => {
  void loadSide("left", rev);
});
watch(rightRevision, (rev) => {
  void loadSide("right", rev);
});

// Line-by-line diff. Rendered as two parallel arrays so the two <pre>
// panes stay aligned even when lines differ in length. Marker meaning:
//   "same"    — bytes match at this index
//   "changed" — both sides have a line at this index but differ
//   "added"   — only present on this side (other side is padded blank)
//   "removed" — only present on the other side (this side is blank)
type LineMarker = "same" | "changed" | "added" | "removed";
interface DiffLine {
  text: string;
  marker: LineMarker;
}

const diff = computed<{ left: DiffLine[]; right: DiffLine[] }>(() => {
  const leftLines = leftYaml.value.split("\n");
  const rightLines = rightYaml.value.split("\n");
  const max = Math.max(leftLines.length, rightLines.length);
  const left: DiffLine[] = [];
  const right: DiffLine[] = [];
  for (let i = 0; i < max; i++) {
    const l = leftLines[i];
    const r = rightLines[i];
    if (l === undefined) {
      left.push({ text: "", marker: "removed" });
      right.push({ text: r ?? "", marker: "added" });
    } else if (r === undefined) {
      left.push({ text: l, marker: "added" });
      right.push({ text: "", marker: "removed" });
    } else if (l === r) {
      left.push({ text: l, marker: "same" });
      right.push({ text: r, marker: "same" });
    } else {
      left.push({ text: l, marker: "changed" });
      right.push({ text: r, marker: "changed" });
    }
  }
  return { left, right };
});

const changeCount = computed(
  () => diff.value.left.filter((l) => l.marker !== "same").length,
);

const swapSides = () => {
  const tmp = leftRevision.value;
  leftRevision.value = rightRevision.value;
  rightRevision.value = tmp;
};
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="1400"
    scrollable
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <span>Compare Revisions</span>
        <v-chip
          v-if="!loadingLeft && !loadingRight && !error"
          size="small"
          :color="changeCount > 0 ? 'warning' : 'success'"
          variant="tonal"
          class="ml-3"
        >
          {{ changeCount }} line{{ changeCount === 1 ? "" : "s" }} differ
        </v-chip>
        <v-spacer />
        <v-btn
          icon="mdi-close"
          size="small"
          variant="text"
          @click="emit('update:modelValue', false)"
        />
      </v-card-title>

      <v-divider />

      <v-card-text class="pa-3">
        <v-alert
          v-if="error"
          type="error"
          density="compact"
          class="mb-3"
          closable
          @click:close="error = null"
        >
          {{ error }}
        </v-alert>

        <div class="d-flex align-center ga-2 mb-3">
          <v-select
            v-model="leftRevision"
            :items="revisionOptions"
            label="Left"
            density="compact"
            hide-details
            style="flex: 1"
          />
          <v-btn
            icon="mdi-swap-horizontal"
            size="small"
            variant="tonal"
            @click="swapSides"
          />
          <v-select
            v-model="rightRevision"
            :items="revisionOptions"
            label="Right"
            density="compact"
            hide-details
            style="flex: 1"
          />
        </div>

        <div class="diff-grid">
          <div class="diff-pane">
            <div class="diff-pane-header">
              #{{ leftRevision }} <span class="text-secondary">(left)</span>
              <v-progress-circular
                v-if="loadingLeft"
                size="14"
                width="2"
                indeterminate
                class="ml-2"
              />
            </div>
            <pre class="diff-content"><template
              v-for="(line, i) in diff.left"
              :key="`l-${i}`"
            ><span
              :class="['diff-line', `diff-line--${line.marker}`]"
            >{{ line.text }}
</span></template></pre>
          </div>
          <div class="diff-pane">
            <div class="diff-pane-header">
              #{{ rightRevision }} <span class="text-secondary">(right)</span>
              <v-progress-circular
                v-if="loadingRight"
                size="14"
                width="2"
                indeterminate
                class="ml-2"
              />
            </div>
            <pre class="diff-content"><template
              v-for="(line, i) in diff.right"
              :key="`r-${i}`"
            ><span
              :class="['diff-line', `diff-line--${line.marker}`]"
            >{{ line.text }}
</span></template></pre>
          </div>
        </div>
      </v-card-text>
    </v-card>
  </v-dialog>
</template>

<style scoped>
.diff-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  min-height: 400px;
  max-height: 65vh;
}

.diff-pane {
  display: flex;
  flex-direction: column;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 4px;
  overflow: hidden;
  background: #0d1117;
}

.diff-pane-header {
  padding: 6px 10px;
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  color: #c9d1d9;
  background: #161b22;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  align-items: center;
}

.diff-content {
  margin: 0;
  padding: 8px 0;
  font-family: "JetBrains Mono", "Fira Code", "Consolas", monospace;
  font-size: 12px;
  line-height: 1.5;
  color: #c9d1d9;
  flex: 1;
  overflow: auto;
  white-space: pre;
}

/* One line per span so we can color the background of only the changed
 * region and leave surrounding whitespace untouched. Padding-left leaves
 * room for a marker gutter without needing a second grid column. */
.diff-line {
  display: block;
  padding: 0 10px;
  border-left: 3px solid transparent;
}

.diff-line--same {
  /* no marker — dominant case, avoid visual noise */
}

.diff-line--changed {
  background: rgba(245, 158, 11, 0.18);
  border-left-color: rgb(245, 158, 11);
}

.diff-line--added {
  background: rgba(34, 197, 94, 0.18);
  border-left-color: rgb(34, 197, 94);
}

.diff-line--removed {
  background: rgba(239, 68, 68, 0.18);
  border-left-color: rgb(239, 68, 68);
}
</style>
