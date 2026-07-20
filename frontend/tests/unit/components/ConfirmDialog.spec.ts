import { describe, expect, it, beforeEach, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";

const vuetify = createVuetify({ components, directives });

// Vuetify's VOverlay reads visualViewport which happy-dom doesn't provide.
beforeEach(() => {
  if (!globalThis.visualViewport) {
    vi.stubGlobal("visualViewport", {
      width: 1024,
      height: 768,
      offsetLeft: 0,
      offsetTop: 0,
      scale: 1,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
  }
});

function mountDialog(props: Partial<InstanceType<typeof ConfirmDialog>["$props"]> = {}) {
  return mount(ConfirmDialog, {
    props: {
      modelValue: true,
      title: "Delete Item",
      message: "Are you sure you want to delete this?",
      ...props,
    },
    global: { plugins: [vuetify] },
    attachTo: document.body,
  });
}

describe("ConfirmDialog", () => {
  it("accepts title, message, confirmText, and confirmColor props", () => {
    const wrapper = mountDialog({
      title: "Custom Title",
      message: "Custom message",
      confirmText: "Yes, do it",
      confirmColor: "warning",
    });

    // The component mounts without error with all props accepted
    expect(wrapper.exists()).toBe(true);
    wrapper.unmount();
  });

  it("emits confirm when the confirm button is clicked", async () => {
    const wrapper = mountDialog();

    const buttons = wrapper.findAllComponents(components.VBtn);
    const confirmBtn = buttons.find((b) => b.text() === "Confirm");
    expect(confirmBtn).toBeTruthy();

    await confirmBtn!.trigger("click");
    expect(wrapper.emitted("confirm")).toBeTruthy();
    expect(wrapper.emitted("confirm")).toHaveLength(1);
    wrapper.unmount();
  });

  it("emits update:modelValue with false when cancel is clicked", async () => {
    const wrapper = mountDialog();

    const buttons = wrapper.findAllComponents(components.VBtn);
    const cancelBtn = buttons.find((b) => b.text() === "Cancel");
    expect(cancelBtn).toBeTruthy();

    await cancelBtn!.trigger("click");
    expect(wrapper.emitted("update:modelValue")).toBeTruthy();
    expect(wrapper.emitted("update:modelValue")![0]).toEqual([false]);
    wrapper.unmount();
  });

  it("uses custom confirmText when provided", () => {
    const wrapper = mountDialog({ confirmText: "Delete Forever" });

    const buttons = wrapper.findAllComponents(components.VBtn);
    const confirmBtn = buttons.find((b) => b.text() === "Delete Forever");
    expect(confirmBtn).toBeTruthy();
    wrapper.unmount();
  });

  it("renders a text field when confirmInput prop is set", () => {
    const wrapper = mountDialog({ confirmInput: "my-deployment" });

    const textField = wrapper.findComponent(components.VTextField);
    expect(textField.exists()).toBe(true);
    wrapper.unmount();
  });

  it("does not render a text field when confirmInput is not set", () => {
    const wrapper = mountDialog();

    const textField = wrapper.findComponent(components.VTextField);
    expect(textField.exists()).toBe(false);
    wrapper.unmount();
  });

  it("disables confirm button when confirmInput is set but not matched", () => {
    const wrapper = mountDialog({ confirmInput: "my-deployment" });

    const buttons = wrapper.findAllComponents(components.VBtn);
    const confirmBtn = buttons.find((b) => b.text() === "Confirm");
    expect(confirmBtn).toBeTruthy();
    expect(confirmBtn!.props("disabled")).toBe(true);
    wrapper.unmount();
  });

  it("enables confirm button when typed text matches confirmInput", async () => {
    const wrapper = mountDialog({ confirmInput: "my-deployment" });

    const textField = wrapper.findComponent(components.VTextField);
    await textField.setValue("my-deployment");

    const buttons = wrapper.findAllComponents(components.VBtn);
    const confirmBtn = buttons.find((b) => b.text() === "Confirm");
    expect(confirmBtn!.props("disabled")).toBe(false);
    wrapper.unmount();
  });
});
