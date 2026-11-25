import { expect, test } from "./fixtures";

test.describe("Deployment CRUD", () => {
  test("create posts the form and opens the new deployment", async ({ page, mockApi }) => {
    await page.goto("/deployments/create");
    await page.getByLabel("Deployment Name").fill("api-server");
    await page.getByLabel("Container Image").fill("api-server:v1");
    await page.getByRole("button", { name: "Create Deployment" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments");
    // The settings mock has default resource limits; create mode applies them
    // to the blank resource fields.
    expect(call.body).toEqual({
      name: "api-server",
      image: "api-server:v1",
      replicas: 1,
      resource_requests: { cpu: "100m", memory: "128Mi" },
      resource_limits: { cpu: "500m", memory: "512Mi" },
    });
    await expect(page).toHaveURL(/\/deployments\/default\/api-server$/);
  });

  test("delete asks for the name, and cancel sends nothing", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/web");
    await page.getByRole("button", { name: "Delete" }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("Delete Deployment")).toBeVisible();
    const confirm = dialog.getByRole("button", { name: "Delete" });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("textbox").fill("wrong-name");
    await expect(confirm).toBeDisabled();

    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    expect(mockApi.requests("DELETE", "/namespaces/:ns/deployments/:name")).toHaveLength(0);
  });

  test("delete sends DELETE after the name is typed and returns to the list", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/web");
    await page.getByRole("button", { name: "Delete" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("textbox").fill("web");
    await dialog.getByRole("button", { name: "Delete" }).click();

    await mockApi.waitForRequest("DELETE", "/namespaces/default/deployments/web");
    await expect(page).toHaveURL(/\/deployments$/);
    await expect(page.getByRole("cell", { name: "web", exact: true })).toBeHidden();
  });
});
