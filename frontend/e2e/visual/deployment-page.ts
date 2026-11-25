// Shared by the visual specs that capture a deployment page.
import type { App } from "../support/app";
import { expect } from "./visual-test";

export const WEB = "/deployments/default/web";

export const POD_METRICS = "/namespaces/:ns/pods/metrics";

/** Requests every deployment page makes on load. */
export const ON_LOAD = [
  "/namespaces/:ns/deployments/:name",
  "/namespaces/:ns/deployments/:name/hpa",
  "/namespaces/:ns/deployments/:name/gitops",
  "/namespaces/:ns/deployments/:name/monitor",
  "/namespaces/:ns/deployments/:name/history",
  "/namespaces/:ns/events",
  "/namespaces/:ns/pods/:pod/logs/history",
  POD_METRICS,
];

/**
 * Open a deployment, wait for every request it makes on load (a quiet
 * network alone is not proof under load), then fire the 10 s metrics poll
 * once so the CPU/memory sparklines hold two identical samples and draw a
 * flat line instead of a ramp from zero. The poll's fetch can leave the page
 * after `runFor` returns, so wait for the request itself. Later polls,
 * however slow the machine, only add identical samples, which draw the same
 * line.
 */
export async function openDeployment(app: App, path = WEB): Promise<void> {
  await app.goto(path);
  for (const p of ON_LOAD) await app.api.waitForRequest("GET", p);
  await app.settle();
  const polls = app.api.requests("GET", POD_METRICS).length;
  await app.page.clock.runFor(10_000);
  await expect.poll(() => app.api.requests("GET", POD_METRICS).length).toBeGreaterThan(polls);
  await app.settle();
}
