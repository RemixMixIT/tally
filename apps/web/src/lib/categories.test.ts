import { describe, expect, it } from "vitest";
import type { Category } from "@/types/api";
import {
  facetCategoryLabel,
  groupCategories,
  transferCategoryIds,
} from "./categories";

function cat(
  id: number,
  name: string,
  parent_id: number | null = null
): Category {
  return { id, name, parent_id, icon: null, colour: null };
}

describe("category tree", () => {
  const cats = [
    cat(1, "Groceries"),
    cat(3, "Supermarket", 1),
    cat(2, "Bills"),
    cat(4, "Council tax", 2),
    cat(5, "Transfer"),
    cat(6, "Internal", 5),
  ];

  it("nests children under parents and sorts by name", () => {
    const { roots, childrenOf } = groupCategories(cats);
    expect(roots.map((c) => c.name)).toEqual(["Bills", "Groceries", "Transfer"]);
    expect(childrenOf.get(1)?.map((c) => c.name)).toEqual(["Supermarket"]);
    expect(childrenOf.get(2)?.map((c) => c.name)).toEqual(["Council tax"]);
  });

  it("collects Transfer parent and leaves", () => {
    const ids = transferCategoryIds(cats);
    expect([...ids].sort()).toEqual([5, 6]);
  });

  it("labels facets as parent / child", () => {
    const byId = new Map(cats.map((c) => [c.id, c]));
    expect(facetCategoryLabel(cats[1], byId)).toBe("Groceries / Supermarket");
    expect(facetCategoryLabel(cats[0], byId)).toBe("Groceries");
  });
});
