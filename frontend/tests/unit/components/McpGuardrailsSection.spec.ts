import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import McpGuardrailsSection from "@/components/settings/McpGuardrailsSection.vue";
import type { McpGuardrails } from "@/types/api";
import { defaultGuardrails } from "@/utils/mcpGuardrails";

const vuetify = createVuetify({ components, directives });

function mountSection(initial: McpGuardrails = defaultGuardrails()) {
  const wrapper = mount(McpGuardrailsSection, {
    props: {
      modelValue: initial,
      "onUpdate:modelValue": (value: McpGuardrails) => wrapper.setProps({ modelValue: value }),
    },
    global: { plugins: [vuetify] },
  });
  return wrapper;
}

function model(wrapper: ReturnType<typeof mountSection>): McpGuardrails {
  return wrapper.props("modelValue") as McpGuardrails;
}

/** The switch whose label is `label`. */
function switchInput(wrapper: ReturnType<typeof mountSection>, label: string) {
  const sw = wrapper.findAll(".v-switch").find((s) => s.text().includes(label));
  if (!sw) throw new Error(`no switch labelled ${label}`);
  return sw.find("input");
}

describe("McpGuardrailsSection", () => {
  it("shows the restrictive policy with its exceptions and API groups by default", () => {
    const wrapper = mountSection();
    const text = wrapper.text();
    expect(text).toContain("Kubernetes tool guardrails");
    expect(text).toContain("Restrictive (recommended)");
    expect(text).toContain("Exceptions");
    expect(text).toContain("LoadBalancer Services");
    expect(text).toContain("allow_load_balancer");
    expect(text).toContain("Allowed custom resource API groups");
    expect(wrapper.find(".v-alert").exists()).toBe(false);
  });

  it("explains what each switch exposes", () => {
    const text = mountSection().text();
    // apply_manifest bypasses the guardrails, and clients ask before each call.
    expect(text).toContain("It bypasses the workload guardrails above");
    expect(text).toContain("ask the user to approve each call");
    // Secret decoding exposes the values.
    expect(text).toContain("end up in the agent's context");
    expect(text).toContain("never its values");
    expect(text).toContain("system:masters");
    expect(text).toContain("security_context.run_as_non_root: false");
  });

  it("reflects the defaults in the switches", () => {
    const wrapper = mountSection();
    expect((switchInput(wrapper, "apply_manifest").element as HTMLInputElement).checked).toBe(true);
    expect((switchInput(wrapper, "Return Secret values").element as HTMLInputElement).checked).toBe(false);
    expect(
      (switchInput(wrapper, "Approve privileged certificate requests").element as HTMLInputElement).checked,
    ).toBe(false);
    expect((switchInput(wrapper, "Secure defaults").element as HTMLInputElement).checked).toBe(true);
  });

  it("updates the model from the switches and the exceptions", async () => {
    const wrapper = mountSection();
    await switchInput(wrapper, "apply_manifest").setValue(false);
    await switchInput(wrapper, "Return Secret values").setValue(true);
    const lb = wrapper.findAll(".v-checkbox").find((c) => c.text().includes("allow_load_balancer"));
    await lb!.find("input").setValue(true);
    expect(model(wrapper)).toMatchObject({
      apply_manifest_enabled: false,
      secret_decode_enabled: true,
      exceptions: ["allow_load_balancer"],
    });
  });

  it("warns that the permissive policy turns every workload guardrail off", async () => {
    const wrapper = mountSection();
    const permissive = wrapper.findAll("button").find((b) => b.text() === "Permissive");
    await permissive!.trigger("click");
    expect(model(wrapper).workload_policy).toBe("permissive");
    const alert = wrapper.find(".v-alert");
    expect(alert.exists()).toBe(true);
    expect(alert.text()).toContain("Every workload guardrail is off");
    // Exceptions and API groups don't apply in permissive mode.
    expect(wrapper.text()).not.toContain("Exceptions");
    expect(wrapper.text()).not.toContain("Allowed custom resource API groups");
  });
});
