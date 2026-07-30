/**
 * Recursively patches `target` with values from `source`, only touching
 * fields that actually changed. Arrays are replaced wholesale (pod lists
 * change identity frequently -- deep-diffing arrays of objects is fragile).
 * Returns true if anything changed.
 */
export function deepPatch(target: any, source: any): boolean {
  if (target === source) return false;
  if (
    typeof target !== "object" ||
    typeof source !== "object" ||
    target === null ||
    source === null
  ) {
    return true; // caller should replace
  }
  if (Array.isArray(target) || Array.isArray(source)) {
    return true; // arrays: always replace (caller handles)
  }

  let changed = false;

  // Patch existing keys and add new ones
  for (const key of Object.keys(source)) {
    if (!(key in target)) {
      target[key] = source[key];
      changed = true;
    } else if (Array.isArray(source[key])) {
      if (JSON.stringify(target[key]) !== JSON.stringify(source[key])) {
        target[key] = source[key];
        changed = true;
      }
    } else if (
      typeof source[key] === "object" &&
      source[key] !== null &&
      typeof target[key] === "object" &&
      target[key] !== null
    ) {
      if (deepPatch(target[key], source[key])) {
        changed = true;
      }
    } else if (target[key] !== source[key]) {
      target[key] = source[key];
      changed = true;
    }
  }

  // Remove keys not in source
  for (const key of Object.keys(target)) {
    if (!(key in source)) {
      delete target[key];
      changed = true;
    }
  }

  return changed;
}

/**
 * Patches an array by matching items on a key field (default: 'name'),
 * patching existing items in-place and only adding/removing when the
 * list membership changes. Preserves Vue reactivity on unchanged items.
 */
export function patchArray<T extends Record<string, any>>(
  target: T[],
  source: T[],
  keyField: string = "name",
): boolean {
  const sourceMap = new Map(source.map((item) => [item[keyField], item]));
  const targetMap = new Map(target.map((item) => [item[keyField], item]));
  let changed = false;

  // Remove items not in source
  for (let i = target.length - 1; i >= 0; i--) {
    if (!sourceMap.has(target[i][keyField])) {
      target.splice(i, 1);
      changed = true;
    }
  }

  // Update existing items and add new ones
  for (const [key, sourceItem] of sourceMap) {
    const existing = targetMap.get(key);
    if (existing) {
      if (deepPatch(existing, sourceItem)) {
        changed = true;
      }
    } else {
      target.push(sourceItem);
      changed = true;
    }
  }

  return changed;
}
