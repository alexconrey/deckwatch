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
