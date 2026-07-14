/**
 * Cost overlay helpers.
 *
 * Parses Kubernetes quantity strings for CPU (`100m`, `0.5`, `2`) and memory
 * (`128Mi`, `1Gi`, `512M`, plain bytes) into normalized floats — vCPUs and
 * GiB respectively — then multiplies by the rates configured on
 * `DeckwatchSettings.cost`.
 *
 * All comparisons operate on _requests_, not limits, since requests are what
 * actually reserve capacity on the cluster (and therefore drive real cost).
 * If requests are absent the caller should treat the estimate as unavailable
 * rather than falling back to limits — that would silently overreport.
 */
import type { CostSettings, ResourceSpec } from "@/types/api";

/**
 * Parse a k8s CPU quantity into vCPUs. Returns null when the input is empty
 * or unparseable — the caller decides how to render (usually as "—").
 * Supports `100m` (millicores), `0.5`, `2`.
 */
export function parseCpu(v: string | null | undefined): number | null {
  if (!v) return null;
  const s = v.trim();
  if (!s) return null;
  if (s.endsWith("m")) {
    const n = Number(s.slice(0, -1));
    return Number.isFinite(n) ? n / 1000 : null;
  }
  const n = Number(s);
  return Number.isFinite(n) ? n : null;
}

/**
 * Parse a k8s memory quantity into GiB. Supports SI (`M`, `G`, `T`) and IEC
 * (`Mi`, `Gi`, `Ti`) suffixes, plus raw byte counts. Sub-Ki quantities round
 * to zero — an intentional simplification since sub-KiB requests would never
 * meaningfully change a cost estimate.
 */
export function parseMemoryGiB(v: string | null | undefined): number | null {
  if (!v) return null;
  const s = v.trim();
  if (!s) return null;
  // Longest suffix first so `Mi` is not shadowed by `M`.
  const suffixes: [string, number][] = [
    ["Ki", 1024],
    ["Mi", 1024 ** 2],
    ["Gi", 1024 ** 3],
    ["Ti", 1024 ** 4],
    ["Pi", 1024 ** 5],
    ["K", 1000],
    ["M", 1000 ** 2],
    ["G", 1000 ** 3],
    ["T", 1000 ** 4],
    ["P", 1000 ** 5],
  ];
  for (const [suf, mul] of suffixes) {
    if (s.endsWith(suf)) {
      const n = Number(s.slice(0, -suf.length));
      if (!Number.isFinite(n)) return null;
      return (n * mul) / 1024 ** 3;
    }
  }
  const n = Number(s);
  if (!Number.isFinite(n)) return null;
  return n / 1024 ** 3;
}

export interface HourlyCost {
  /** Per-hour cost for the entire deployment (all replicas). */
  hourly: number;
  /** ~730h/month, matching the AWS convention for monthly billing. */
  monthly: number;
  /** Currency code passed through from settings. */
  currency: string;
}

const HOURS_PER_MONTH = 730;

/**
 * Compute per-hour and per-month cost for a set of resource requests scaled
 * by replica count. Returns null when the cost overlay is unconfigured or
 * neither rate produces a numeric contribution (e.g. no requests set) — the
 * caller should hide the chip in either case rather than render "$0.00".
 */
export function estimateCost(
  requests: ResourceSpec | null | undefined,
  replicas: number,
  cost: CostSettings | null | undefined,
): HourlyCost | null {
  if (!cost) return null;
  const cpuRate = cost.cost_per_cpu_hour ?? null;
  const memRate = cost.cost_per_gb_hour ?? null;
  if (cpuRate === null && memRate === null) return null;

  const cpu = parseCpu(requests?.cpu ?? null);
  const mem = parseMemoryGiB(requests?.memory ?? null);

  let hourly = 0;
  let contributed = false;
  if (cpu !== null && cpuRate !== null) {
    hourly += cpu * cpuRate;
    contributed = true;
  }
  if (mem !== null && memRate !== null) {
    hourly += mem * memRate;
    contributed = true;
  }
  if (!contributed) return null;
  const reps = Math.max(0, Number.isFinite(replicas) ? replicas : 0);
  hourly *= reps;
  return {
    hourly,
    monthly: hourly * HOURS_PER_MONTH,
    currency: cost.currency ?? "USD",
  };
}

const CURRENCY_SYMBOLS: Record<string, string> = {
  USD: "$",
  EUR: "€",
  GBP: "£",
  JPY: "¥",
};

/**
 * Format a numeric cost with the currency's symbol when known, else fall
 * back to a trailing ISO code. Sub-cent values collapse to "<$0.01" so the
 * chip never reads "$0.00/hr" for a real (tiny) request.
 */
export function formatCost(n: number, currency: string): string {
  const sym = CURRENCY_SYMBOLS[currency];
  if (n > 0 && n < 0.01) {
    return sym ? `<${sym}0.01` : `<0.01 ${currency}`;
  }
  const digits = n < 10 ? 2 : n < 1000 ? 2 : 0;
  const body = n.toLocaleString(undefined, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
  return sym ? `${sym}${body}` : `${body} ${currency}`;
}

/**
 * True when `next` is more than `factor` times `prev`. Guards against a
 * zero baseline (any positive next is trivially "infinitely" more expensive
 * and would spam warnings for deployments that had no cost estimate before).
 */
export function isCostIncreaseOverFactor(
  prev: HourlyCost | null,
  next: HourlyCost | null,
  factor: number,
): boolean {
  if (!prev || !next) return false;
  if (prev.hourly <= 0) return false;
  return next.hourly > prev.hourly * factor;
}
