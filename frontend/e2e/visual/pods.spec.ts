import { eachTheme, expect, test } from "./visual-test";

/** Pod detail (/pods/:namespace/:podName). */

eachTheme(() => {
  test("running pod", async ({ app, page }) => {
    await app.goto("/pods/default/web-6c9f7d8b5-4xk2p");
    await expect(page.getByText("Containers (1)")).toBeVisible();
    await app.snap("pods/running", { fullPage: true });
  });

  test("crash-looping pod", async ({ app, page }) => {
    await app.goto("/pods/default/worker-5b8c9d7f6-zx9vb");
    await expect(page.getByText("CrashLoopBackOff")).toBeVisible();
    await app.snap("pods/crash-loop", { fullPage: true });
  });
});
