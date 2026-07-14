import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import StatusChip from "@/components/common/StatusChip.vue";
import type { DeploymentPhase } from "@/types/api";

const vuetify = createVuetify({ components, directives });

function mountChip(status: DeploymentPhase) {
  return mount(StatusChip, {
    props: { status },
    global: { plugins: [vuetify] },
  });
}

describe("StatusChip", () => {
  const cases: {
    status: DeploymentPhase;
    color: string;
    icon: string;
    text: string;
  }[] = [
    {
      status: "available",
      color: "success",
      icon: "mdi-check-circle",
      text: "Available",
    },
    {
      status: "progressing",
      color: "info",
      icon: "mdi-progress-clock",
      text: "Progressing",
    },
    {
      status: "degraded",
      color: "warning",
      icon: "mdi-alert",
      text: "Degraded",
    },
    {
      status: "failed",
      color: "error",
      icon: "mdi-close-circle",
      text: "Failed",
    },
    {
      status: "scaled_to_zero",
      color: "grey",
      icon: "mdi-pause-circle",
      text: "Scaled to 0",
    },
  ];

  it.each(cases)(
    'maps "$status" to $color color and $icon icon',
    ({ status, color, icon, text }) => {
      const wrapper = mountChip(status);
      const chip = wrapper.findComponent(components.VChip);

      expect(chip.props("color")).toBe(color);
      expect(chip.text()).toContain(text);

      const icons = wrapper.findAllComponents(components.VIcon);
      const statusIcon = icons.find((i) => i.props("icon") === icon);
      expect(statusIcon).toBeTruthy();
    },
  );

  it("shows clickable cursor for failed and degraded statuses", () => {
    for (const status of ["failed", "degraded"] as DeploymentPhase[]) {
      const wrapper = mountChip(status);
      const chip = wrapper.findComponent(components.VChip);
      expect(chip.attributes("style")).toContain("cursor: pointer");
    }
  });

  it("does not show clickable cursor for non-error statuses", () => {
    for (const status of [
      "available",
      "progressing",
      "scaled_to_zero",
    ] as DeploymentPhase[]) {
      const wrapper = mountChip(status);
      const chip = wrapper.findComponent(components.VChip);
      const style = chip.attributes("style") ?? "";
      expect(style).not.toContain("cursor: pointer");
    }
  });

  it("emits click when a clickable chip is clicked", async () => {
    const wrapper = mountChip("failed");
    const chip = wrapper.findComponent(components.VChip);
    await chip.trigger("click");
    expect(wrapper.emitted("click")).toBeTruthy();
  });

  it("shows info icon only for clickable statuses", () => {
    const clickable = mountChip("degraded");
    const infoIcons = clickable
      .findAllComponents(components.VIcon)
      .filter((i) => i.props("icon") === "mdi-information-outline");
    expect(infoIcons).toHaveLength(1);

    const nonClickable = mountChip("available");
    const noInfoIcons = nonClickable
      .findAllComponents(components.VIcon)
      .filter((i) => i.props("icon") === "mdi-information-outline");
    expect(noInfoIcons).toHaveLength(0);
  });
});
