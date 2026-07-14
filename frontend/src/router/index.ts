import { createRouter, createWebHistory } from "vue-router";
import { settingsApi } from "@/api/settings";
import { useAuth } from "@/composables/useAuth";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/auth/callback",
      name: "AuthCallback",
      component: () => import("@/components/pages/AuthCallbackPage.vue"),
      meta: { public: true },
    },
    {
      path: "/",
      component: () => import("@/layouts/AppLayout.vue"),
      children: [
        {
          path: "",
          redirect: { name: "Applications" },
        },
        {
          path: "applications",
          name: "Applications",
          component: () => import("@/components/pages/ApplicationsPage.vue"),
        },
        {
          path: "applications/create",
          name: "CreateApplication",
          component: () => import("@/components/pages/CreateApplicationWizard.vue"),
        },
        {
          path: "applications/:namespace/:name",
          name: "ApplicationDetail",
          component: () =>
            import("@/components/pages/ApplicationDetailPage.vue"),
          props: true,
        },
        {
          path: "deployments",
          name: "Deployments",
          component: () => import("@/components/pages/DeploymentsPage.vue"),
        },
        {
          path: "deployments/templates",
          name: "TemplatePicker",
          component: () =>
            import("@/components/pages/TemplatePickerPage.vue"),
        },
        {
          path: "deployments/create",
          name: "CreateDeployment",
          component: () =>
            import("@/components/pages/CreateDeploymentPage.vue"),
        },
        {
          path: "deployments/:namespace/:name",
          name: "DeploymentDetail",
          component: () =>
            import("@/components/pages/DeploymentDetailPage.vue"),
          props: true,
        },
        {
          path: "pods/:namespace/:podName",
          name: "PodDetail",
          component: () => import("@/components/pages/PodDetailPage.vue"),
          props: true,
        },
        {
          path: "registry",
          name: "Registry",
          component: () =>
            import("@/components/pages/RegistryPage.vue"),
        },
        {
          // Legacy path — Secrets and ConfigMaps live under the Resources
          // page now, addressed by ?tab= query. Kept as a redirect so any
          // bookmarks or external links still land on the right tab.
          path: "secrets",
          redirect: { name: "Deployments", query: { tab: "secrets" } },
        },
        {
          path: "docs/:slug?",
          name: "Docs",
          component: () => import("@/components/pages/DocsPage.vue"),
          props: true,
        },
        {
          path: "settings",
          name: "Settings",
          component: () =>
            import("@/components/pages/SettingsPage.vue"),
        },
        {
          path: "cluster",
          name: "ClusterOverview",
          component: () =>
            import("@/components/pages/ClusterOverviewPage.vue"),
        },
        {
          path: "audit",
          name: "AuditLog",
          component: () =>
            import("@/components/pages/AuditLogPage.vue"),
        },
      ],
    },
    {
      path: "/:pathMatch(.*)*",
      name: "NotFound",
      component: () => import("@/components/pages/NotFoundPage.vue"),
    },
  ],
});

// One-shot bootstrap: pull `auth` from the settings API on the very first
// navigation and hand it to the composable. Cached forever after — settings
// changes require a page reload to take effect (matches the backend, which
// reads auth at startup only).
let bootstrapped = false;
let bootstrapPromise: Promise<void> | null = null;

async function bootstrapAuth(): Promise<void> {
  if (bootstrapped) return;
  if (bootstrapPromise) return bootstrapPromise;

  bootstrapPromise = (async () => {
    try {
      const settings = await settingsApi.get();
      useAuth().setAuthSettings(settings.auth ?? null);
    } catch {
      // Settings endpoint failed — leave auth disabled (composable default)
      // rather than blocking every navigation on a transient error.
    } finally {
      bootstrapped = true;
    }
  })();
  return bootstrapPromise;
}

router.beforeEach(async (to) => {
  await bootstrapAuth();
  const auth = useAuth();

  // Public routes (callback page) never require auth.
  if (to.meta.public) return true;

  // Auth disabled → let everything through.
  if (!auth.isEnabled()) return true;

  // Auth enabled + already have a token → let everything through.
  if (auth.isAuthenticated.value) return true;

  // Auth enabled + no token → kick to Entra, remembering where they wanted
  // to go so `handleCallback` can return them here after sign-in.
  await auth.login(to.fullPath);
  // Login navigates the window away; block this in-app navigation so we
  // don't render the target view for a frame before Entra takes over.
  return false;
});

export { router };
