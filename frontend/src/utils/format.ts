/**
 * Human-friendly age formatter for ISO-8601 timestamps.
 *
 * Consolidates several near-duplicate implementations that had drifted across
 * the codebase (page cards, detail views, event feeds). Callers opt into the
 * variants they need via `options`:
 *
 *   - `suffix`         appended to non-empty output (e.g. `" ago"`)
 *   - `includeSeconds` renders `Ns` under a minute instead of collapsing to `0m`
 *   - `guardInvalid`   returns `"-"` if the diff is negative or NaN (clock
 *                      skew from K8s event timestamps that arrive in the
 *                      future); without this, negative diffs render as `0m`
 *
 * The optional `now` parameter is used only by tests for determinism.
 */
export interface FormatAgeOptions {
  suffix?: string;
  includeSeconds?: boolean;
  guardInvalid?: boolean;
}

export function formatAge(
  iso: string | null,
  options: FormatAgeOptions = {},
  now: number = Date.now(),
): string {
  if (!iso) return "-";
  const diff = now - new Date(iso).getTime();
  if (options.guardInvalid && (Number.isNaN(diff) || diff < 0)) return "-";

  const suffix = options.suffix ?? "";
  const seconds = Math.floor(diff / 1000);
  if (options.includeSeconds && seconds < 60) return `${seconds}s${suffix}`;
  const minutes = Math.floor(diff / 60000);
  if (minutes < 60) return `${minutes}m${suffix}`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h${suffix}`;
  const days = Math.floor(hours / 24);
  return `${days}d${suffix}`;
}

/**
 * Wraps `Date.toLocaleString` and returns a dash for null.
 */
export function formatTimestamp(iso: string | null): string {
  if (!iso) return "-";
  return new Date(iso).toLocaleString();
}

/**
 * Human-friendly formatter for Kubernetes memory quantity strings.
 *
 * Handles IEC suffixes (`Ki`, `Mi`, `Gi`, `Ti`), SI suffixes (`K`, `M`, `G`, `T`),
 * plain byte counts, and already-formatted strings (pass-through).
 *
 * Examples:
 *   "8123456Ki"  -> "7.7 GiB"
 *   "256Mi"      -> "256 MiB"
 *   "2Gi"        -> "2.0 GiB"
 *   "1Ti"        -> "1.0 TiB"
 *   "500M"       -> "500 MB"
 *   "1073741824" -> "1.0 GiB"
 *   "128974848"  -> "123 MiB"
 *   null         -> "-"
 */
export function formatMemory(raw: string | null | undefined): string {
  if (!raw) return "-";
  const s = raw.trim();
  if (!s) return "-";

  // IEC binary suffixes (Ki, Mi, Gi, Ti)
  const iecMatch = s.match(/^(\d+(?:\.\d+)?)(Ki|Mi|Gi|Ti)$/);
  if (iecMatch) {
    const val = parseFloat(iecMatch[1]);
    const unit = iecMatch[2];
    if (unit === "Ti") return `${val.toFixed(1)} TiB`;
    if (unit === "Gi") return `${val.toFixed(1)} GiB`;
    if (unit === "Mi") {
      if (val >= 1024) return `${(val / 1024).toFixed(1)} GiB`;
      return `${Math.round(val)} MiB`;
    }
    if (unit === "Ki") {
      const gib = val / (1024 * 1024);
      if (gib >= 1) return `${gib.toFixed(1)} GiB`;
      const mib = val / 1024;
      if (mib >= 1) return `${Math.round(mib)} MiB`;
      return `${Math.round(val)} KiB`;
    }
  }

  // SI decimal suffixes (K, M, G, T — not followed by 'i')
  const siMatch = s.match(/^(\d+(?:\.\d+)?)(K|M|G|T)$/);
  if (siMatch) {
    const val = parseFloat(siMatch[1]);
    const unit = siMatch[2];
    if (unit === "T") return `${val.toFixed(1)} TB`;
    if (unit === "G") return `${val.toFixed(1)} GB`;
    if (unit === "M") return `${Math.round(val)} MB`;
    if (unit === "K") return `${Math.round(val)} KB`;
  }

  // Plain number (bytes)
  const plainMatch = s.match(/^(\d+(?:\.\d+)?)$/);
  if (plainMatch) {
    const bytes = parseFloat(plainMatch[1]);
    const tib = bytes / (1024 ** 4);
    if (tib >= 1) return `${tib.toFixed(1)} TiB`;
    const gib = bytes / (1024 ** 3);
    if (gib >= 1) return `${gib.toFixed(1)} GiB`;
    const mib = bytes / (1024 ** 2);
    if (mib >= 1) return `${Math.round(mib)} MiB`;
    const kib = bytes / 1024;
    if (kib >= 1) return `${Math.round(kib)} KiB`;
    return `${Math.round(bytes)} B`;
  }

  // Already formatted or unrecognized — pass through
  return s;
}
