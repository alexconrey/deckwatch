import { describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import DeploymentForm from "@/components/views/deployment/DeploymentForm.vue";

// Mock the settings API so the onMounted fetch does not fire real requests.
vi.mock("@/api/settings", () => ({
  settingsApi: {
    get: vi.fn().mockResolvedValue({}),
  },
}));

const vuetify = createVuetify({ components, directives });

function mountForm(props: Record<string, unknown> = {}) {
  return mount(DeploymentForm, {
    props: { loading: false, ...props },
    global: {
      plugins: [vuetify],
      stubs: {
        // RegistryTagPicker is a complex child that calls its own APIs;
        // stub it out to keep these tests focused on form logic.
        RegistryTagPicker: true,
      },
    },
  });
}

/** Helper: set a v-text-field's value by finding its <input> and triggering
 *  an input event. We locate fields by their label text to stay decoupled
 *  from DOM structure changes. */
async function setFieldByLabel(
  wrapper: ReturnType<typeof mount>,
  label: string,
  value: string,
) {
  const fields = wrapper.findAllComponents(components.VTextField);
  const field = fields.find((f) => f.props("label") === label);
  if (!field) throw new Error(`VTextField with label "${label}" not found`);
  const input = field.find("input");
  await input.setValue(value);
}

/** Helper: read the messages (hint / error) rendered beneath a field. */
function fieldMessages(
  wrapper: ReturnType<typeof mount>,
  label: string,
): string {
  const fields = wrapper.findAllComponents(components.VTextField);
  const field = fields.find((f) => f.props("label") === label);
  if (!field) throw new Error(`VTextField with label "${label}" not found`);
  // Vuetify renders messages inside `.v-messages__message` elements.
  const msgs = field.findAll(".v-messages__message");
  return msgs.map((m) => m.text()).join(" ");
}

describe("DeploymentForm", () => {
  // ------------------------------------------------------------------
  // 1. Required fields — Name and Image must be filled
  // ------------------------------------------------------------------
  it("shows validation errors when required fields are empty", async () => {
    const wrapper = mountForm();

    // Trigger submit without filling anything.
    const form = wrapper.findComponent(components.VForm);
    await form.trigger("submit.prevent");
    await wrapper.vm.$nextTick();

    // The name field should show "Required".
    const nameMessages = fieldMessages(wrapper, "Deployment Name");
    expect(nameMessages).toContain("Required");

    // The image field should show "Required".
    const imageMessages = fieldMessages(wrapper, "Container Image");
    expect(imageMessages).toContain("Required");
  });

  // ------------------------------------------------------------------
  // 2. CPU limit < request warning
  // ------------------------------------------------------------------
  it("shows error when CPU limit is less than CPU request", async () => {
    const wrapper = mountForm();

    await setFieldByLabel(wrapper, "CPU Request", "500m");
    await setFieldByLabel(wrapper, "CPU Limit", "100m");
    await wrapper.vm.$nextTick();

    // Trigger validation by submitting the form.
    const form = wrapper.findComponent(components.VForm);
    await form.trigger("submit.prevent");
    await wrapper.vm.$nextTick();

    const msgs = fieldMessages(wrapper, "CPU Limit");
    expect(msgs).toContain("CPU limit is less than CPU request");
  });

  // ------------------------------------------------------------------
  // 3. Memory limit < request warning
  // ------------------------------------------------------------------
  it("shows error when memory limit is less than memory request", async () => {
    const wrapper = mountForm();

    await setFieldByLabel(wrapper, "Memory Request", "256Mi");
    await setFieldByLabel(wrapper, "Memory Limit", "128Mi");
    await wrapper.vm.$nextTick();

    const form = wrapper.findComponent(components.VForm);
    await form.trigger("submit.prevent");
    await wrapper.vm.$nextTick();

    const msgs = fieldMessages(wrapper, "Memory Limit");
    expect(msgs).toContain("Memory limit is less than memory request");
  });

  // ------------------------------------------------------------------
  // 4. Image tag warning — no tag specified
  // ------------------------------------------------------------------
  it('shows hint when image has no tag', async () => {
    const wrapper = mountForm();

    await setFieldByLabel(wrapper, "Container Image", "nginx");
    await wrapper.vm.$nextTick();

    const msgs = fieldMessages(wrapper, "Container Image");
    expect(msgs).toContain("No tag specified");
  });

  // ------------------------------------------------------------------
  // 5. Replicas = 0 warning
  // ------------------------------------------------------------------
  it("shows hint when replicas is set to 0", async () => {
    const wrapper = mountForm();

    // The replicas field is the VTextField with label "Replicas".
    await setFieldByLabel(wrapper, "Replicas", "0");
    await wrapper.vm.$nextTick();

    const msgs = fieldMessages(wrapper, "Replicas");
    expect(msgs).toContain("stop all instances");
  });

  // ------------------------------------------------------------------
  // 6. Submit emits correct payload
  // ------------------------------------------------------------------
  it("emits submit with correct CreateDeploymentRequest payload", async () => {
    const wrapper = mountForm();

    await setFieldByLabel(wrapper, "Deployment Name", "my-app");
    await setFieldByLabel(wrapper, "Container Image", "nginx:latest");
    await setFieldByLabel(wrapper, "CPU Request", "100m");
    await setFieldByLabel(wrapper, "Memory Request", "128Mi");
    await setFieldByLabel(wrapper, "CPU Limit", "500m");
    await setFieldByLabel(wrapper, "Memory Limit", "256Mi");
    await wrapper.vm.$nextTick();

    const form = wrapper.findComponent(components.VForm);
    await form.trigger("submit.prevent");
    await wrapper.vm.$nextTick();

    const emitted = wrapper.emitted("submit");
    expect(emitted).toBeTruthy();
    expect(emitted!.length).toBeGreaterThanOrEqual(1);

    const payload = emitted![0][0] as Record<string, unknown>;
    expect(payload).toMatchObject({
      name: "my-app",
      image: "nginx:latest",
      replicas: 1,
      resource_requests: { cpu: "100m", memory: "128Mi" },
      resource_limits: { cpu: "500m", memory: "256Mi" },
    });
  });

  // ------------------------------------------------------------------
  // 7. Edit mode disables the name field
  // ------------------------------------------------------------------
  it("disables the name field in edit mode", () => {
    const wrapper = mountForm({
      isEdit: true,
      initialValues: {
        name: "existing-app",
        image: "nginx:1.25",
        replicas: 2,
      },
    });

    const nameFields = wrapper.findAllComponents(components.VTextField);
    const nameField = nameFields.find(
      (f) => f.props("label") === "Deployment Name",
    );
    expect(nameField).toBeTruthy();
    expect(nameField!.props("disabled")).toBe(true);

    // The name should be populated with the initial value.
    const input = nameField!.find("input");
    expect(input.element.value).toBe("existing-app");
  });
});
