<script setup lang="ts">
import { ref, watch } from "vue";
import type { NodeAffinityConfig, NodeSelectorRequirement, NodeSelectorTerm, PreferredNodeTerm } from "@/types/api";

const props = defineProps<{
  modelValue: boolean;
  nodeSelector: Record<string, string>;
  nodeAffinity: NodeAffinityConfig | null;
  loading?: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  save: [payload: { node_selector: Record<string, string>; node_affinity: NodeAffinityConfig }];
}>();

const OPERATORS = ["In", "NotIn", "Exists", "DoesNotExist"] as const;
const OPERATOR_ITEMS = OPERATORS.map((o) => ({ title: o, value: o }));

// ── local state ───────────────────────────────────────────────────────────────

type KVPair = { key: string; value: string };
type LocalExpr = { key: string; operator: string; values: string };
type LocalTerm = { match_expressions: LocalExpr[] };
type LocalPreferred = { weight: number; match_expressions: LocalExpr[] };

const nodeSelector = ref<KVPair[]>([]);
const requiredTerms = ref<LocalTerm[]>([]);
const preferredTerms = ref<LocalPreferred[]>([]);

function reset() {
  nodeSelector.value = Object.entries(props.nodeSelector).map(([key, value]) => ({ key, value }));

  requiredTerms.value = (props.nodeAffinity?.required ?? []).map((t) => ({
    match_expressions: t.match_expressions.map((e) => ({
      key: e.key,
      operator: e.operator,
      values: e.values.join(", "),
    })),
  }));

  preferredTerms.value = (props.nodeAffinity?.preferred ?? []).map((t) => ({
    weight: t.weight,
    match_expressions: t.match_expressions.map((e) => ({
      key: e.key,
      operator: e.operator,
      values: e.values.join(", "),
    })),
  }));
}

watch(() => props.modelValue, (open) => { if (open) reset(); }, { immediate: true });

// ── node selector helpers ─────────────────────────────────────────────────────

function addNsRow() { nodeSelector.value.push({ key: "", value: "" }); }
function removeNsRow(i: number) { nodeSelector.value.splice(i, 1); }

// ── required affinity helpers ─────────────────────────────────────────────────

function addRequiredTerm() {
  requiredTerms.value.push({ match_expressions: [{ key: "", operator: "In", values: "" }] });
}
function removeRequiredTerm(i: number) { requiredTerms.value.splice(i, 1); }
function addRequiredExpr(term: LocalTerm) {
  term.match_expressions.push({ key: "", operator: "In", values: "" });
}
function removeRequiredExpr(term: LocalTerm, i: number) { term.match_expressions.splice(i, 1); }

// ── preferred affinity helpers ────────────────────────────────────────────────

function addPreferredTerm() {
  preferredTerms.value.push({ weight: 1, match_expressions: [{ key: "", operator: "In", values: "" }] });
}
function removePreferredTerm(i: number) { preferredTerms.value.splice(i, 1); }
function addPreferredExpr(term: LocalPreferred) {
  term.match_expressions.push({ key: "", operator: "In", values: "" });
}
function removePreferredExpr(term: LocalPreferred, i: number) { term.match_expressions.splice(i, 1); }

// ── save ──────────────────────────────────────────────────────────────────────

function operatorHasValues(op: string) {
  return op === "In" || op === "NotIn";
}

function parseExpr(e: LocalExpr): NodeSelectorRequirement {
  return {
    key: e.key,
    operator: e.operator as NodeSelectorRequirement["operator"],
    values: operatorHasValues(e.operator)
      ? e.values.split(",").map((v) => v.trim()).filter(Boolean)
      : [],
  };
}

function handleSave() {
  const ns: Record<string, string> = {};
  for (const { key, value } of nodeSelector.value) {
    if (key.trim()) ns[key.trim()] = value.trim();
  }

  const required: NodeSelectorTerm[] = requiredTerms.value
    .filter((t) => t.match_expressions.some((e) => e.key.trim()))
    .map((t) => ({ match_expressions: t.match_expressions.map(parseExpr) }));

  const preferred: PreferredNodeTerm[] = preferredTerms.value
    .filter((t) => t.match_expressions.some((e) => e.key.trim()))
    .map((t) => ({ weight: t.weight, match_expressions: t.match_expressions.map(parseExpr) }));

  emit("save", { node_selector: ns, node_affinity: { required, preferred } });
}
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="800"
    scrollable
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card>
      <v-card-title class="d-flex align-center">
        <v-icon icon="mdi-server-network" class="mr-2" />
        Node Affinity &amp; Scheduling
      </v-card-title>

      <v-card-text style="max-height: 70vh">

        <!-- ── Node Selector ─────────────────────────────────────────────── -->
        <div class="text-subtitle-2 mb-2 mt-1">Node Selector</div>
        <div class="text-caption text-medium-emphasis mb-3">
          Simple key=value pairs. The pod is only scheduled on nodes matching
          <em>all</em> entries.
        </div>

        <v-row
          v-for="(row, i) in nodeSelector"
          :key="i"
          dense
          class="mb-1"
        >
          <v-col cols="5">
            <v-text-field
              v-model="row.key"
              label="Key"
              density="compact"
              variant="outlined"
              hide-details
            />
          </v-col>
          <v-col cols="6">
            <v-text-field
              v-model="row.value"
              label="Value"
              density="compact"
              variant="outlined"
              hide-details
            />
          </v-col>
          <v-col cols="1" class="d-flex align-center justify-center">
            <v-btn icon="mdi-close" variant="text" size="small" @click="removeNsRow(i)" />
          </v-col>
        </v-row>

        <v-btn
          variant="text"
          size="small"
          prepend-icon="mdi-plus"
          class="mb-4"
          @click="addNsRow"
        >
          Add selector
        </v-btn>

        <v-divider class="mb-4" />

        <!-- ── Required Node Affinity ────────────────────────────────────── -->
        <div class="text-subtitle-2 mb-1">Required Node Affinity</div>
        <div class="text-caption text-medium-emphasis mb-3">
          Pod will <strong>not</strong> be scheduled on nodes that don't match.
          Each term is an OR — all expressions within a term are AND.
        </div>

        <v-card
          v-for="(term, ti) in requiredTerms"
          :key="ti"
          variant="outlined"
          class="mb-3 pa-3"
        >
          <div class="d-flex justify-space-between align-center mb-2">
            <span class="text-caption text-medium-emphasis">Term {{ ti + 1 }}</span>
            <v-btn icon="mdi-close" variant="text" size="x-small" @click="removeRequiredTerm(ti)" />
          </div>

          <v-row
            v-for="(expr, ei) in term.match_expressions"
            :key="ei"
            dense
            class="mb-1"
          >
            <v-col cols="4">
              <v-text-field
                v-model="expr.key"
                label="Key"
                density="compact"
                variant="outlined"
                hide-details
              />
            </v-col>
            <v-col cols="3">
              <v-select
                v-model="expr.operator"
                :items="OPERATOR_ITEMS"
                label="Operator"
                density="compact"
                variant="outlined"
                hide-details
              />
            </v-col>
            <v-col cols="4">
              <v-text-field
                v-if="operatorHasValues(expr.operator)"
                v-model="expr.values"
                label="Values (comma-separated)"
                density="compact"
                variant="outlined"
                hide-details
              />
              <v-text-field
                v-else
                label="Values"
                density="compact"
                variant="outlined"
                hide-details
                disabled
                placeholder="—"
              />
            </v-col>
            <v-col cols="1" class="d-flex align-center justify-center">
              <v-btn icon="mdi-close" variant="text" size="x-small" @click="removeRequiredExpr(term, ei)" />
            </v-col>
          </v-row>

          <v-btn
            variant="text"
            size="x-small"
            prepend-icon="mdi-plus"
            @click="addRequiredExpr(term)"
          >
            Add expression
          </v-btn>
        </v-card>

        <v-btn
          variant="text"
          size="small"
          prepend-icon="mdi-plus"
          class="mb-4"
          @click="addRequiredTerm"
        >
          Add required term
        </v-btn>

        <v-divider class="mb-4" />

        <!-- ── Preferred Node Affinity ───────────────────────────────────── -->
        <div class="text-subtitle-2 mb-1">Preferred Node Affinity</div>
        <div class="text-caption text-medium-emphasis mb-3">
          Scheduler tries to match these, but will still schedule the pod if
          no node qualifies. Higher weight = stronger preference (1–100).
        </div>

        <v-card
          v-for="(term, ti) in preferredTerms"
          :key="ti"
          variant="outlined"
          class="mb-3 pa-3"
        >
          <div class="d-flex justify-space-between align-center mb-2">
            <v-text-field
              v-model.number="term.weight"
              label="Weight"
              type="number"
              min="1"
              max="100"
              density="compact"
              variant="outlined"
              hide-details
              style="max-width: 100px"
            />
            <v-btn icon="mdi-close" variant="text" size="x-small" @click="removePreferredTerm(ti)" />
          </div>

          <v-row
            v-for="(expr, ei) in term.match_expressions"
            :key="ei"
            dense
            class="mb-1"
          >
            <v-col cols="4">
              <v-text-field
                v-model="expr.key"
                label="Key"
                density="compact"
                variant="outlined"
                hide-details
              />
            </v-col>
            <v-col cols="3">
              <v-select
                v-model="expr.operator"
                :items="OPERATOR_ITEMS"
                label="Operator"
                density="compact"
                variant="outlined"
                hide-details
              />
            </v-col>
            <v-col cols="4">
              <v-text-field
                v-if="operatorHasValues(expr.operator)"
                v-model="expr.values"
                label="Values (comma-separated)"
                density="compact"
                variant="outlined"
                hide-details
              />
              <v-text-field
                v-else
                label="Values"
                density="compact"
                variant="outlined"
                hide-details
                disabled
                placeholder="—"
              />
            </v-col>
            <v-col cols="1" class="d-flex align-center justify-center">
              <v-btn icon="mdi-close" variant="text" size="x-small" @click="removePreferredExpr(term, ei)" />
            </v-col>
          </v-row>

          <v-btn
            variant="text"
            size="x-small"
            prepend-icon="mdi-plus"
            @click="addPreferredExpr(term)"
          >
            Add expression
          </v-btn>
        </v-card>

        <v-btn
          variant="text"
          size="small"
          prepend-icon="mdi-plus"
          @click="addPreferredTerm"
        >
          Add preferred term
        </v-btn>
      </v-card-text>

      <v-card-actions>
        <v-spacer />
        <v-btn variant="text" @click="emit('update:modelValue', false)">Cancel</v-btn>
        <v-btn color="primary" variant="flat" :loading="loading" @click="handleSave">
          Save
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>
