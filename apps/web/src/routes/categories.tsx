import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import * as LucideIcons from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { api } from "@/lib/api";
import { groupCategories } from "@/lib/categories";
import type { Category } from "@/types/api";

export const Route = createFileRoute("/categories")({
  component: CategoriesPage,
});

function CategoryIcon({ name }: { name: string | null }) {
  if (!name) return null;
  const Icon = (LucideIcons as unknown as Record<string, LucideIcon>)[name];
  if (!Icon) return null;
  return <Icon className="size-4 text-green" />;
}

function CategoryName({
  cat,
  className,
  onRename,
}: {
  cat: Category;
  className?: string;
  onRename: (name: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(cat.name);

  useEffect(() => {
    setDraft(cat.name);
  }, [cat.name]);

  if (editing) {
    const commit = () => {
      const next = draft.trim();
      if (next && next !== cat.name) onRename(next);
      else setDraft(cat.name);
      setEditing(false);
    };
    return (
      <input
        className="input py-1 px-2 text-sm w-full flex-1 min-w-0"
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
          if (e.key === "Escape") {
            setDraft(cat.name);
            setEditing(false);
          }
        }}
        aria-label="Category name"
      />
    );
  }

  return (
    <button
      type="button"
      className={`text-left truncate hover:text-green focus:outline-none focus:text-green ${className ?? ""}`}
      onClick={() => setEditing(true)}
      title="Edit name"
    >
      {cat.name}
    </button>
  );
}

function CategoriesPage() {
  const qc = useQueryClient();
  const q = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });
  const [name, setName] = useState("");
  const [icon, setIcon] = useState("");
  const [parentId, setParentId] = useState<number | "">("");

  const cats = q.data?.categories ?? [];
  const { roots, childrenOf } = groupCategories(cats);

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/categories", {
        name,
        icon: icon || null,
        parent_id: parentId === "" ? null : parentId,
      }),
    onSuccess: () => {
      setName("");
      setIcon("");
      setParentId("");
      qc.invalidateQueries({ queryKey: ["categories"] });
    },
  });
  const rename = useMutation({
    mutationFn: ({ cat, name }: { cat: Category; name: string }) =>
      api.patch(`/api/categories/${cat.id}`, {
        name,
        parent_id: cat.parent_id,
        icon: cat.icon,
        colour: cat.colour,
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["categories"] }),
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/categories/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["categories"] }),
  });

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Categories</em>
        </h1>
        <p className="text-mid text-sm">
          Group transactions for budgets and reporting. Names stay unique.
        </p>
      </header>

      <form
        className="card p-5 flex flex-wrap gap-2 items-center fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) create.mutate();
        }}
      >
        <input
          className="input flex-1 min-w-[180px]"
          placeholder="Category name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <select
          className="input max-w-[220px]"
          value={parentId}
          onChange={(e) =>
            setParentId(e.target.value ? Number(e.target.value) : "")
          }
          aria-label="Parent"
        >
          <option value="">Parent</option>
          {roots.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <input
          className="input max-w-[180px]"
          placeholder="Lucide icon (e.g. ShoppingCart)"
          value={icon}
          onChange={(e) => setIcon(e.target.value)}
        />
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Add
        </button>
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {roots.map((parent) => (
          <li key={parent.id}>
            <div className="flex items-center justify-between gap-3 px-5 py-3.5 text-sm">
              <span className="flex items-center gap-2.5 font-semibold min-w-0 flex-1">
                <CategoryIcon name={parent.icon} />
                <CategoryName
                  cat={parent}
                  className="font-semibold text-ink"
                  onRename={(name) => rename.mutate({ cat: parent, name })}
                />
              </span>
              <button
                className="btn-ghost text-xs shrink-0"
                onClick={() => del.mutate(parent.id)}
              >
                <Trash2 className="size-3.5" /> Delete
              </button>
            </div>
            {(childrenOf.get(parent.id) ?? []).map((child) => (
              <div
                key={child.id}
                className="flex items-center justify-between gap-3 px-5 py-2.5 text-sm pl-12 bg-soft/40"
              >
                <span className="flex items-center gap-2.5 text-mid min-w-0 flex-1">
                  <CategoryIcon name={child.icon} />
                  <CategoryName
                    cat={child}
                    className="text-mid"
                    onRename={(name) => rename.mutate({ cat: child, name })}
                  />
                </span>
                <button
                  className="btn-ghost text-xs shrink-0"
                  onClick={() => del.mutate(child.id)}
                >
                  <Trash2 className="size-3.5" /> Delete
                </button>
              </div>
            ))}
          </li>
        ))}
      </ul>
    </div>
  );
}
