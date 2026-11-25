import { describe, expect, it } from "vitest";
import {
  GUARDRAIL_EXCEPTIONS,
  defaultGuardrails,
  toggleException,
  validateApiGroup,
  validateGuardrails,
  withGuardrailDefaults,
} from "@/utils/mcpGuardrails";

describe("mcpGuardrails", () => {
  it("defaults to secure but keeps apply_manifest", () => {
    expect(defaultGuardrails()).toEqual({
      workload_policy: "restrictive",
      exceptions: [],
      allowed_custom_resource_groups: [],
      allow_privileged_csr: false,
      apply_manifest_enabled: true,
      secret_decode_enabled: false,
      secure_defaults: true,
    });
  });

  it("fills in what an older backend leaves out", () => {
    expect(withGuardrailDefaults(undefined)).toEqual(defaultGuardrails());
    const g = withGuardrailDefaults({ secret_decode_enabled: true, exceptions: ["allow_node_port"] });
    expect(g.secret_decode_enabled).toBe(true);
    expect(g.apply_manifest_enabled).toBe(true);
    expect(g.exceptions).toEqual(["allow_node_port"]);
  });

  it("toggles exceptions in the backend's order and keeps unknown ones", () => {
    let e = toggleException([], "allow_load_balancer", true);
    e = toggleException(e, "allow_privileged", true);
    expect(e).toEqual(["allow_privileged", "allow_load_balancer"]);
    expect(toggleException(e, "allow_privileged", false)).toEqual(["allow_load_balancer"]);
    expect(toggleException(["allow_future_rule"], "allow_host_ports", true)).toEqual([
      "allow_host_ports",
      "allow_future_rule",
    ]);
  });

  it("lists the eleven rules the restrictive policy can relax", () => {
    expect(GUARDRAIL_EXCEPTIONS.map((e) => e.field)).toEqual([
      "allow_privileged",
      "allow_privilege_escalation",
      "allow_host_namespaces",
      "allow_host_path",
      "allow_host_ports",
      "allow_unconfined",
      "allow_node_port",
      "allow_load_balancer",
      "allow_external_services",
      "allow_ingress_snippets",
      "allow_rbac_escalation",
    ]);
  });

  it("accepts API group names and *", () => {
    for (const ok of ["cert-manager.io", "*", "monitoring.coreos.com", " Argoproj.io "]) {
      expect(validateApiGroup(ok)).toBeNull();
    }
    for (const bad of ["cert manager", "-bad.example.com", "a..b", "under_score.io"]) {
      expect(validateApiGroup(bad)).toContain("is not an API group name");
    }
    expect(
      validateGuardrails({ ...defaultGuardrails(), allowed_custom_resource_groups: ["not a group"] }),
    ).toContain("Allowed custom resource API groups");
    expect(validateGuardrails(defaultGuardrails())).toBeNull();
  });
});
