<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useNamespaceStore } from "@/stores/namespace";
import { applicationsApi } from "@/api/applications";
import { ingressesApi } from "@/api/ingresses";
import type { CreateApplicationRequest, CreateIngressRequest } from "@/types/api";

const router = useRouter();
const ns = useNamespaceStore();

const step = ref(1);
const loading = ref(false);
const error = ref<string | null>(null);

// Step 1
const appName = ref("");
const appDescription = ref("");
// Namespace is prompted explicitly in the wizard rather than inherited from
// the nav bar dropdown, so operators do not accidentally create an app in the
// wrong ns after leaving the dropdown on something unrelated.
const appNamespace = ref<string>("");

// Step 2
const codeSource = ref<"git" | "manual">("manual");
const repoUrl = ref("");
const branch = ref("main");
const tokenSecret = ref("");

// Step 3 - Access / Ingress
const accessMode = ref<"public" | "internal">("public");
const ingressHost = ref("");
const ingressPath = ref("/");
const ingressPort = ref(80);

const DNS_RE = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;
// Fairly permissive hostname check - RFC 1123 with dotted labels.
const HOST_RE = /^([a-z0-9]([-a-z0-9]*[a-z0-9])?)(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$/;

const nameError = computed(() => {
  if (!appName.value) return "";
  if (appName.value.length > 63)
    return "Name must be 63 characters or fewer.";
  if (!DNS_RE.test(appName.value))
    return "Use lowercase letters, digits, and hyphens (no leading/trailing hyphen).";
  return "";
});

const hostError = computed(() => {
  if (accessMode.value !== "public") return "";
  const v = ingressHost.value.trim();
  if (!v) return "Hostname is required for a public URL.";
  if (!HOST_RE.test(v)) return "Enter a valid hostname (e.g. myapp.example.com).";
  return "";
});

const pathError = computed(() => {
  if (accessMode.value !== "public") return "";
  const v = ingressPath.value.trim();
  if (!v) return "Path is required.";
  if (!v.startsWith("/")) return "Path must start with '/'.";
  return "";
});

const portError = computed(() => {
  if (accessMode.value !== "public") return "";
  const p = Number(ingressPort.value);
  if (!Number.isInteger(p) || p < 1 || p > 65535)
    return "Port must be an integer between 1 and 65535.";
  return "";
});

const step1Valid = computed(
  () => appName.value.length > 0 && !nameError.value && !!appNamespace.value,
);

const step2Valid = computed(() => {
  if (codeSource.value === "manual") return true;
  return repoUrl.value.trim().length > 0;
});

const step3Valid = computed(() => {
  if (accessMode.value === "internal") return true;
  return !hostError.value && !pathError.value && !portError.value;
});

const goNext = () => {
  if (step.value === 1 && !step1Valid.value) return;
  if (step.value === 2 && !step2Valid.value) return;
  if (step.value === 3 && !step3Valid.value) return;
  step.value += 1;
};

const goBack = () => {
  if (step.value > 1) step.value -= 1;
};

// Seed the wizard namespace from the nav bar selection; fall back to the
// first available namespace if none is selected yet.
onMounted(async () => {
  if (ns.namespaces.length === 0) {
    await ns.fetchNamespaces();
  }
  if (!appNamespace.value) {
    appNamespace.value = ns.selected || ns.namespaces[0] || "";
  }
});

watch(
  () => ns.selected,
  (v) => {
    if (step.value === 1 && v && !appNamespace.value) {
      appNamespace.value = v;
    }
  },
);

const submit = async () => {
  if (!appNamespace.value) {
    error.value = "Select a namespace first.";
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    const body: CreateApplicationRequest = {
      name: appName.value,
      description: appDescription.value || undefined,
      git:
        codeSource.value === "git"
          ? {
              repo_url: repoUrl.value,
              branch: branch.value || undefined,
              token_secret: tokenSecret.value || undefined,
            }
          : undefined,
      create_deployment: true,
    };
    const detail = await applicationsApi.create(appNamespace.value, body);

    // If the operator asked for a public URL, create the Ingress after the
    // app exists. The backend auto-creates a matching ClusterIP Service
    // (selector app=<name>) if one does not already exist, which lines up
    // with the labels the seed deployment sets on its pods.
    if (accessMode.value === "public") {
      const ingressBody: CreateIngressRequest = {
        name: appName.value,
        host: ingressHost.value.trim(),
        paths: [
          {
            path: ingressPath.value.trim() || "/",
            path_type: "Prefix",
            service_name: appName.value,
            service_port: Number(ingressPort.value) || 80,
          },
        ],
      };
      try {
        await ingressesApi.create(appNamespace.value, ingressBody);
      } catch (ingErr) {
        // Application already created - surface the ingress failure but
        // still send the operator to the app so they can retry from the UI.
        error.value =
          "Application created, but ingress failed: " +
          (ingErr instanceof Error ? ingErr.message : String(ingErr));
      }
    }

    router.push({
      name: "ApplicationDetail",
      params: { namespace: detail.namespace, name: detail.name },
    });
  } catch (e) {
    error.value =
      e instanceof Error ? e.message : "Failed to create application";
  } finally {
    loading.value = false;
  }
};
</script>

<template>
  <div>
    <div class="d-flex align-center mb-4">
      <v-btn
        icon="mdi-arrow-left"
        variant="text"
        @click="router.push({ name: 'Applications' })"
      />
      <h2 class="text-h5 ml-2">New Application</h2>
    </div>

    <v-alert v-if="error" type="error" class="mb-4" closable>
      {{ error }}
    </v-alert>

    <v-card class="pa-4">
      <v-stepper v-model="step" alt-labels flat>
        <v-stepper-header>
          <v-stepper-item :value="1" title="Name it" :complete="step > 1" />
          <v-divider />
          <v-stepper-item :value="2" title="Point to your code" :complete="step > 2" />
          <v-divider />
          <v-stepper-item :value="3" title="Access" :complete="step > 3" />
          <v-divider />
          <v-stepper-item :value="4" title="Review" />
        </v-stepper-header>

        <v-stepper-window>
          <!-- Step 1 -->
          <v-stepper-window-item :value="1">
            <div class="pa-2">
              <h3 class="text-h6 mb-4">What is your app called?</h3>
              <v-text-field
                v-model="appName"
                label="App name"
                placeholder="my-cool-app"
                hint="Lowercase letters, digits, and hyphens. This becomes part of the URL and resource names."
                persistent-hint
                :error-messages="nameError ? [nameError] : []"
                variant="outlined"
                autofocus
                class="mb-4"
              />

              <h3 class="text-h6 mb-2">Where should it live?</h3>
              <v-select
                v-model="appNamespace"
                :items="ns.namespaces"
                label="Namespace"
                :loading="ns.loading"
                :disabled="ns.namespaces.length === 0"
                :no-data-text="ns.loading ? 'Loading namespaces...' : 'No namespaces available'"
                hint="The Kubernetes namespace this app and its resources will be created in."
                persistent-hint
                variant="outlined"
                class="mb-4"
              />

              <h3 class="text-h6 mb-2">What does it do?</h3>
              <v-textarea
                v-model="appDescription"
                label="Description (optional)"
                placeholder="A short summary that will help teammates understand the purpose of this app."
                rows="3"
                variant="outlined"
              />
            </div>
          </v-stepper-window-item>

          <!-- Step 2 -->
          <v-stepper-window-item :value="2">
            <div class="pa-2">
              <h3 class="text-h6 mb-4">Where is your code?</h3>

              <v-radio-group v-model="codeSource" hide-details class="mb-4">
                <v-radio value="git">
                  <template v-slot:label>
                    <div>
                      <div class="text-body-1 font-weight-medium">
                        I have a Git repository
                      </div>
                      <div class="text-caption text-secondary">
                        We will pull your code and keep it in sync.
                      </div>
                    </div>
                  </template>
                </v-radio>
                <v-radio value="manual">
                  <template v-slot:label>
                    <div>
                      <div class="text-body-1 font-weight-medium">
                        I will deploy manually
                      </div>
                      <div class="text-caption text-secondary">
                        You will upload or point to a container image later.
                      </div>
                    </div>
                  </template>
                </v-radio>
              </v-radio-group>

              <div v-if="codeSource === 'git'" class="mt-2">
                <v-text-field
                  v-model="repoUrl"
                  label="Repository URL"
                  placeholder="https://github.com/your-org/your-repo.git"
                  variant="outlined"
                  class="mb-3"
                />
                <v-text-field
                  v-model="branch"
                  label="Branch"
                  placeholder="main"
                  variant="outlined"
                  class="mb-3"
                />
                <v-text-field
                  v-model="tokenSecret"
                  label="Access token secret name"
                  placeholder="my-git-token"
                  hint="Optional. The name of a Kubernetes Secret in this namespace that holds a token to read your repo. Leave blank for public repos."
                  persistent-hint
                  variant="outlined"
                />
              </div>
            </div>
          </v-stepper-window-item>

          <!-- Step 3 - Access / Ingress -->
          <v-stepper-window-item :value="3">
            <div class="pa-2">
              <h3 class="text-h6 mb-4">How will people access your app?</h3>

              <v-radio-group v-model="accessMode" hide-details class="mb-4">
                <v-radio value="public">
                  <template v-slot:label>
                    <div>
                      <div class="text-body-1 font-weight-medium">
                        I need a URL
                      </div>
                      <div class="text-caption text-secondary">
                        We will create an Ingress so this app is reachable at a
                        hostname you pick.
                      </div>
                    </div>
                  </template>
                </v-radio>
                <v-radio value="internal">
                  <template v-slot:label>
                    <div>
                      <div class="text-body-1 font-weight-medium">
                        Internal only (no public URL)
                      </div>
                      <div class="text-caption text-secondary">
                        The app is reachable in-cluster only. You can add an
                        ingress later.
                      </div>
                    </div>
                  </template>
                </v-radio>
              </v-radio-group>

              <div v-if="accessMode === 'public'" class="mt-2">
                <v-text-field
                  v-model="ingressHost"
                  label="Hostname"
                  placeholder="myapp.example.com"
                  hint="The public hostname operators will use to reach this app. Must resolve to your ingress controller."
                  persistent-hint
                  :error-messages="hostError ? [hostError] : []"
                  variant="outlined"
                  class="mb-3"
                />
                <v-row dense>
                  <v-col cols="12" sm="8">
                    <v-text-field
                      v-model="ingressPath"
                      label="Path"
                      placeholder="/"
                      hint="URL path prefix that routes to this app."
                      persistent-hint
                      :error-messages="pathError ? [pathError] : []"
                      variant="outlined"
                    />
                  </v-col>
                  <v-col cols="12" sm="4">
                    <v-text-field
                      v-model.number="ingressPort"
                      label="Service port"
                      type="number"
                      min="1"
                      max="65535"
                      hint="Container port this app listens on."
                      persistent-hint
                      :error-messages="portError ? [portError] : []"
                      variant="outlined"
                    />
                  </v-col>
                </v-row>
              </div>
            </div>
          </v-stepper-window-item>

          <!-- Step 4 - Review -->
          <v-stepper-window-item :value="4">
            <div class="pa-2">
              <h3 class="text-h6 mb-4">Ready to create?</h3>
              <v-card variant="outlined" class="pa-4">
                <div class="mb-3">
                  <div class="text-caption text-secondary">Name</div>
                  <div class="text-body-1 font-weight-medium">
                    {{ appName }}
                  </div>
                </div>
                <div v-if="appDescription" class="mb-3">
                  <div class="text-caption text-secondary">Description</div>
                  <div class="text-body-2">{{ appDescription }}</div>
                </div>
                <div class="mb-3">
                  <div class="text-caption text-secondary">Namespace</div>
                  <div class="text-body-2">{{ appNamespace }}</div>
                </div>
                <div class="mb-3">
                  <div class="text-caption text-secondary">Code source</div>
                  <div v-if="codeSource === 'git'" class="text-body-2">
                    <div>
                      <code>{{ repoUrl }}</code>
                    </div>
                    <div class="text-caption text-secondary">
                      Branch: {{ branch || "main" }}
                      <template v-if="tokenSecret">
                        &middot; Secret: <code>{{ tokenSecret }}</code>
                      </template>
                    </div>
                  </div>
                  <div v-else class="text-body-2">
                    Manual deployment
                  </div>
                </div>
                <div class="mb-3">
                  <div class="text-caption text-secondary">Access</div>
                  <div v-if="accessMode === 'public'" class="text-body-2">
                    <div>
                      <code>http://{{ ingressHost }}{{ ingressPath }}</code>
                    </div>
                    <div class="text-caption text-secondary">
                      Routes to service <code>{{ appName }}</code> on port
                      {{ ingressPort }}.
                    </div>
                  </div>
                  <div v-else class="text-body-2">
                    Internal only (no ingress)
                  </div>
                </div>
                <v-divider class="my-2" />
                <div class="text-caption text-secondary">
                  We will also create a starter deployment for this app so you
                  can wire things up right away.
                </div>
              </v-card>
            </div>
          </v-stepper-window-item>
        </v-stepper-window>

      </v-stepper>

      <div class="d-flex justify-space-between mt-4">
        <v-btn
          variant="text"
          :disabled="step === 1 || loading"
          @click="goBack"
        >
          Back
        </v-btn>
        <v-btn
          v-if="step < 4"
          color="primary"
          variant="flat"
          :disabled="(step === 1 && !step1Valid) || (step === 2 && !step2Valid) || (step === 3 && !step3Valid)"
          @click="goNext"
        >
          Next
        </v-btn>
        <v-btn
          v-else
          color="primary"
          variant="flat"
          size="large"
          prepend-icon="mdi-rocket-launch"
          :loading="loading"
          @click="submit"
        >
          Create Application
        </v-btn>
      </div>
    </v-card>
  </div>
</template>
