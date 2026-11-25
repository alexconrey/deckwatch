import { describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";

vi.mock("vue-chartjs", () => ({
  Line: {
    name: "Line",
    props: ["data", "options", "plugins"],
    template: "<canvas data-testid='line' />",
  },
}));

import TimeSeriesCard from "@/components/common/TimeSeriesCard.vue";

const vuetify = createVuetify({ components, directives });

const points = [
  { t: Date.parse("2026-11-24T10:00:00Z"), v: 3 },
  { t: Date.parse("2026-11-24T10:05:00Z"), v: 4.5 },
];

function mountCard(props: Partial<{ points: typeof points }> = {}) {
  return mount(TimeSeriesCard, {
    props: { title: "Tool calls per minute", points, format: (v: number) => `${v} /min`, ...props },
    global: { plugins: [vuetify] },
  });
}

describe("TimeSeriesCard", () => {
  it("formats axis ticks with axisFormat when given", () => {
    const w = mount(TimeSeriesCard, {
      props: {
        title: "Tool calls per minute",
        points,
        format: (v: number) => `${v} /min`,
        axisFormat: (v: number) => String(v),
      },
      global: { plugins: [vuetify] },
    });
    const options = w.findComponent({ name: "Line" }).props("options") as {
      scales: { y: { ticks: { callback: (v: number) => string } } };
    };
    expect(options.scales.y.ticks.callback(5)).toBe("5");
  });

  it("labels the latest value and draws one series", () => {
    const w = mountCard();
    expect(w.find("[data-testid='latest-value']").text()).toBe("4.5 /min");
    const line = w.findComponent({ name: "Line" });
    expect(line.exists()).toBe(true);
    const data = line.props("data") as { datasets: { data: number[]; borderWidth: number }[] };
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toEqual([3, 4.5]);
    expect(data.datasets[0].borderWidth).toBe(2);
  });

  it("has a table view of every point, newest first", async () => {
    const w = mountCard();
    const toggle = w.find("[data-testid='toggle-view']");
    expect(toggle.text()).toBe("Table");
    await toggle.trigger("click");
    expect(w.find("[data-testid='toggle-view']").text()).toBe("Chart");
    const table = w.find("[data-testid='series-table']");
    expect(table.exists()).toBe(true);
    const cells = table.findAll("tbody tr").map((tr) => tr.findAll("td")[1].text());
    expect(cells).toEqual(["4.5 /min", "3 /min"]);
  });

  it("says when the window has no data", () => {
    const w = mountCard({ points: [] });
    expect(w.text()).toContain("No data in this window");
    expect(w.findComponent({ name: "Line" }).exists()).toBe(false);
  });
});
