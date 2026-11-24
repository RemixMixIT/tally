import type { Category } from "@/types/api";

export function groupCategories(categories: Category[]): {
  roots: Category[];
  childrenOf: Map<number, Category[]>;
} {
  const byParent = new Map<number | null, Category[]>();
  const ids = new Set(categories.map((c) => c.id));
  for (const c of categories) {
    const parent =
      c.parent_id != null && ids.has(c.parent_id) ? c.parent_id : null;
    const list = byParent.get(parent) ?? [];
    list.push(c);
    byParent.set(parent, list);
  }
  for (const list of byParent.values()) {
    list.sort((a, b) => a.name.localeCompare(b.name));
  }
  const childrenOf = new Map<number, Category[]>();
  for (const [k, v] of byParent) {
    if (k != null) childrenOf.set(k, v);
  }
  return {
    roots: byParent.get(null) ?? [],
    childrenOf,
  };
}

export function transferCategoryIds(categories: Category[]): Set<number> {
  const ids = new Set<number>();
  for (const c of categories) {
    if (c.name.toLowerCase() === "transfer") ids.add(c.id);
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const c of categories) {
      if (c.parent_id != null && ids.has(c.parent_id) && !ids.has(c.id)) {
        ids.add(c.id);
        changed = true;
      }
    }
  }
  return ids;
}

export function facetCategoryLabel(
  c: Category,
  byId: Map<number, Category>
): string {
  if (c.parent_id == null) return c.name;
  const parent = byId.get(c.parent_id);
  return parent ? `${parent.name} / ${c.name}` : c.name;
}
