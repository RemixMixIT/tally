import { groupCategories } from "@/lib/categories";
import type { Category } from "@/types/api";

export function CategorySelect({
  categories,
  value,
  onChange,
  className,
  allowEmpty = true,
  emptyLabel = "Uncategorised",
  id,
}: {
  categories: Category[];
  value: number | null | "";
  onChange: (id: number | null) => void;
  className?: string;
  allowEmpty?: boolean;
  emptyLabel?: string;
  id?: string;
}) {
  const { roots, childrenOf } = groupCategories(categories);
  return (
    <select
      id={id}
      className={className}
      value={value ?? ""}
      onChange={(e) =>
        onChange(e.target.value ? Number(e.target.value) : null)
      }
    >
      {allowEmpty && <option value="">{emptyLabel}</option>}
      {roots.map((parent) => {
        const kids = childrenOf.get(parent.id) ?? [];
        if (kids.length === 0) {
          return (
            <option key={parent.id} value={parent.id}>
              {parent.name}
            </option>
          );
        }
        return (
          <optgroup key={parent.id} label={parent.name}>
            <option value={parent.id}>{parent.name}</option>
            {kids.map((child) => (
              <option key={child.id} value={child.id}>
                {child.name}
              </option>
            ))}
          </optgroup>
        );
      })}
    </select>
  );
}
