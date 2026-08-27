import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Sparkles,
  ArrowDownRight,
  ArrowUpRight,
  Search,
  X,
  Tag,
  Hash,
  Calendar,
  Building2,
  Receipt,
  StickyNote,
} from "lucide-react";
import { api } from "@/lib/api";
import { formatMoney, formatDate } from "@/lib/format";
import type {
  Account,
  Category,
  Transaction,
  TransactionsListResponse,
} from "@/types/api";

export const Route = createFileRoute("/transactions")({
  component: TransactionsPage,
});

function TransactionsPage() {
  const qc = useQueryClient();
  const [sorting, setSorting] = useState<SortingState>([
    { id: "timestamp", desc: true },
  ]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [accountIds, setAccountIds] = useState<number[]>([]);
  const [categoryIds, setCategoryIds] = useState<number[]>([]);
  const [q, setQ] = useState("");

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });
  const aiSettings = useQuery({
    queryKey: ["ai-settings"],
    queryFn: () =>
      api.get<{ openrouter_configured: boolean }>("/api/ai/settings"),
  });

  const qs = useMemo(() => {
    const p = new URLSearchParams();
    if (accountIds.length) p.set("account_ids", accountIds.join(","));
    if (categoryIds.length) p.set("category_ids", categoryIds.join(","));
    const trimmed = q.trim();
    if (trimmed) p.set("q", trimmed);
    p.set("limit", "500");
    return p.toString();
  }, [accountIds, categoryIds, q]);

  const txns = useQuery({
    queryKey: ["transactions", qs],
    queryFn: () =>
      api.get<TransactionsListResponse>(`/api/transactions?${qs}`),
    placeholderData: (prev) => prev,
  });

  const cmap = useMemo(() => {
    const m = new Map<number, Category>();
    categories.data?.categories.forEach((c) => m.set(c.id, c));
    return m;
  }, [categories.data]);
  const amap = useMemo(() => {
    const m = new Map<number, Account>();
    accounts.data?.accounts.forEach((a) => m.set(a.id, a));
    return m;
  }, [accounts.data]);

  const setCategory = useMutation({
    mutationFn: ({ id, cat }: { id: number; cat: number | null }) =>
      api.patch(`/api/transactions/${id}`, { category_id: cat }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["transactions"] }),
  });

  const saveNotes = useMutation({
    mutationFn: ({ id, notes }: { id: number; notes: string | null }) =>
      api.patch(`/api/transactions/${id}`, { notes }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["transactions"] }),
  });

  const suggestOne = useMutation({
    mutationFn: (id: number) =>
      api.post<{ suggested_category_id: number | null }>(
        `/api/ai/transactions/${id}/suggest`
      ),
    onSuccess: async (r, id) => {
      if (r.suggested_category_id) {
        await setCategory.mutateAsync({ id, cat: r.suggested_category_id });
      }
    },
  });

  const bulkAi = useMutation({
    mutationFn: () =>
      api.post<{ processed: number; applied: number }>(
        "/api/ai/transactions/bulk",
        { limit: 25, only_uncategorised: true, apply: true }
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["transactions"] }),
  });

  // Keep mutate fns in refs so column defs stay referentially stable. Recreating
  // columns every render (useMutation objects are new each time) was making
  // the virtualizer + table fight and freeze the page.
  const setCategoryMutate = setCategory.mutate;
  const suggestOneMutate = suggestOne.mutate;
  const setCategoryRef = useRef(setCategoryMutate);
  const suggestOneRef = useRef(suggestOneMutate);
  setCategoryRef.current = setCategoryMutate;
  suggestOneRef.current = suggestOneMutate;
  const suggestPendingId =
    suggestOne.isPending && suggestOne.variables != null ? suggestOne.variables : null;

  const columns = useMemo<ColumnDef<Transaction>[]>(
    () => [
      {
        id: "direction",
        accessorKey: "is_credit",
        header: "",
        cell: (info) =>
          info.getValue<number>() === 1 ? (
            <ArrowDownRight className="size-4 text-green" />
          ) : (
            <ArrowUpRight className="size-4 text-orange" />
          ),
        size: 28,
      },
      {
        id: "timestamp",
        accessorKey: "timestamp",
        header: "Date",
        cell: (info) => (
          <span className="text-xs text-mid mono whitespace-nowrap">
            {formatDate(info.getValue<number>())}
          </span>
        ),
        size: 100,
      },
      {
        id: "account",
        accessorKey: "account_id",
        header: "Account",
        cell: (info) => {
          const a = amap.get(info.getValue<number>());
          return (
            <span className="text-xs text-mid">
              {a?.custom_display_name || a?.display_name || "—"}
            </span>
          );
        },
        size: 120,
      },
      {
        id: "description",
        accessorFn: (r) => r.merchant_name || r.description,
        header: "Description",
        cell: (info) => (
          <div className="truncate">
            <span className="font-medium">{info.getValue<string>()}</span>
            {info.row.original.is_pending === 1 && (
              <span className="pill-orange ml-2">Pending</span>
            )}
          </div>
        ),
      },
      {
        id: "category",
        accessorKey: "category_id",
        header: "Category",
        cell: (info) => {
          const t = info.row.original;
          const cid = info.getValue<number | null>();
          const cats = categories.data?.categories ?? [];
          return (
            <div className="flex items-center gap-1.5">
              <select
                className="text-xs bg-transparent border border-thin px-1.5 py-0.5 focus:outline-none focus:border-green"
                value={cid ?? ""}
                onChange={(e) =>
                  setCategoryRef.current({
                    id: t.id,
                    cat: e.target.value ? Number(e.target.value) : null,
                  })
                }
              >
                <option value="">Uncategorised</option>
                {cats.map((cc) => (
                  <option key={cc.id} value={cc.id}>
                    {cc.name}
                  </option>
                ))}
              </select>
              {aiSettings.data?.openrouter_configured && !cid && (
                <button
                  className="btn-ghost text-[10px] !px-1.5 !py-0.5"
                  title="Ask AI to categorise"
                  onClick={() => suggestOneRef.current(t.id)}
                  disabled={suggestPendingId === t.id}
                >
                  <Sparkles className="size-3 text-orange" />
                </button>
              )}
            </div>
          );
        },
        size: 200,
      },
      {
        id: "amount",
        accessorKey: "amount_cents",
        header: "Amount",
        cell: (info) => {
          const t = info.row.original;
          return (
            <span
              className={`mono font-semibold ${
                t.is_credit ? "text-green" : "text-ink"
              }`}
            >
              {t.is_credit ? "+" : "−"}
              {formatMoney(t.amount_cents, t.currency)}
            </span>
          );
        },
        size: 120,
      },
    ],
    [amap, categories.data, aiSettings.data, suggestPendingId]
  );

  const rows = txns.data?.transactions ?? [];
  const table = useReactTable({
    data: rows,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getRowId: (row) => String(row.id),
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });
  const tableRows = table.getRowModel().rows;

  const selectedTxn = rows.find((t) => t.id === selectedId) ?? null;

  return (
    <div className="p-8 md:p-12 space-y-5">
      <header className="flex items-end justify-between fade-in">
        <div>
          <h1 className="text-4xl mb-2">
            <em>Transactions</em>
          </h1>
          <p className="text-sm text-mid">
            {txns.data?.total ?? 0} total · showing {rows.length}
          </p>
        </div>
        {aiSettings.data?.openrouter_configured && (
          <button
            className="btn-outlined"
            onClick={() => bulkAi.mutate()}
            disabled={bulkAi.isPending}
            title="Apply AI categorisation to 25 uncategorised transactions"
          >
            <Sparkles
              className={`size-4 ${bulkAi.isPending ? "animate-pulse" : ""}`}
            />
            AI categorise batch
          </button>
        )}
      </header>
      {bulkAi.data && (
        <p className="text-sm text-green">
          AI processed {bulkAi.data.processed}, applied {bulkAi.data.applied}.
        </p>
      )}

      {/* Filter bar — z-20 so the account/category popover paints above the table */}
      <div className="card p-3 flex flex-wrap items-center gap-2 fade-in-1 relative z-20">
        <div className="relative flex-1 min-w-[200px] max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-mid" />
          <input
            className="input pl-9"
            placeholder="Search description or merchant"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <FacetSelect
          label="Account"
          value={accountIds}
          onChange={(ids) => {
            const next = ids ?? [];
            setAccountIds((prev) =>
              prev.length === next.length && prev.every((id) => next.includes(id))
                ? prev
                : next
            );
          }}
          options={
            accounts.data?.accounts.map((a) => ({
              label: a.custom_display_name || a.display_name,
              value: a.id,
            })) ?? []
          }
        />
        <FacetSelect
          label="Category"
          value={categoryIds}
          onChange={(ids) => {
            const next = ids ?? [];
            setCategoryIds((prev) =>
              prev.length === next.length && prev.every((id) => next.includes(id))
                ? prev
                : next
            );
          }}
          options={
            categories.data?.categories.map((c) => ({
              label: c.name,
              value: c.id,
            })) ?? []
          }
        />
        {(accountIds.length > 0 || categoryIds.length > 0 || q) && (
          <button
            className="btn-ghost"
            onClick={() => {
              setAccountIds([]);
              setCategoryIds([]);
              setQ("");
            }}
          >
            <X className="size-4" /> Clear
          </button>
        )}
      </div>

      {/* Virtualized table */}
      <div className="card overflow-hidden fade-in-2">
        <div className="grid grid-cols-[28px_100px_120px_1fr_200px_120px] gap-3 px-4 py-3 border-b border-thin bg-cream text-[11px] font-semibold uppercase tracking-widest text-mid">
          {table.getHeaderGroups().map((hg) =>
            hg.headers.map((h) => (
              <div key={h.id}>
                {flexRender(h.column.columnDef.header, h.getContext())}
              </div>
            ))
          )}
        </div>
        <div
          className="overflow-auto"
          style={{ height: "calc(100vh - 320px)" }}
        >
          {tableRows.map((row) => (
            <div
              key={row.id}
              className={`grid grid-cols-[28px_100px_120px_1fr_200px_120px] gap-3 px-4 items-center border-b border-thin hover:bg-cream/60 cursor-pointer min-h-11 ${
                selectedId === row.original.id ? "bg-green/5" : ""
              }`}
              onClick={(e) => {
                const tag = (e.target as HTMLElement).tagName;
                if (tag === "SELECT" || tag === "BUTTON" || tag === "OPTION") return;
                setSelectedId(row.original.id);
              }}
            >
              {row.getVisibleCells().map((cell) => (
                <div key={cell.id} className="text-sm truncate py-2">
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>

      {selectedTxn && (
        <TransactionDrawer
          txn={selectedTxn}
          account={amap.get(selectedTxn.account_id) ?? null}
          category={selectedTxn.category_id ? cmap.get(selectedTxn.category_id) ?? null : null}
          categories={categories.data?.categories ?? []}
          onClose={() => setSelectedId(null)}
          onSetCategory={(cat) =>
            setCategory.mutate({ id: selectedTxn.id, cat })
          }
          onSetNotes={(notes) =>
            saveNotes.mutate({ id: selectedTxn.id, notes })
          }
        />
      )}
    </div>
  );
}

function TransactionDrawer({
  txn,
  account,
  category,
  categories,
  onClose,
  onSetCategory,
  onSetNotes,
}: {
  txn: Transaction;
  account: Account | null;
  category: Category | null;
  categories: Category[];
  onClose: () => void;
  onSetCategory: (id: number | null) => void;
  onSetNotes: (notes: string | null) => void;
}) {
  const [notesDraft, setNotesDraft] = useState(txn.notes ?? "");
  useEffect(() => {
    setNotesDraft(txn.notes ?? "");
  }, [txn.id, txn.notes]);
  return (
    <>
      <div
        className="fixed inset-0 bg-ink/20 backdrop-blur-sm z-40"
        onClick={onClose}
      />
      <aside className="fixed right-0 top-0 bottom-0 w-full sm:w-[480px] bg-cream border-l border-thin z-50 overflow-y-auto fade-in shadow-2xl">
        <header className="sticky top-0 bg-cream/95 backdrop-blur p-5 border-b border-thin flex items-center justify-between">
          <div className="min-w-0 flex-1">
            <p className="text-[10px] uppercase tracking-widest text-mid mb-1">
              Transaction · #{txn.id}
            </p>
            <h2 className="text-xl font-extrabold truncate">
              {txn.merchant_name || txn.description}
            </h2>
          </div>
          <button
            className="btn-ghost p-1.5 -m-1.5"
            onClick={onClose}
            aria-label="Close"
          >
            <X className="size-5" />
          </button>
        </header>

        <div className="p-5 space-y-6">
          {/* Amount hero */}
          <div className="text-center py-4">
            <p
              className={`mono font-extrabold text-5xl tracking-tight ${
                txn.is_credit ? "text-green" : "text-ink"
              }`}
            >
              {txn.is_credit ? "+" : "−"}
              {formatMoney(txn.amount_cents, txn.currency)}
            </p>
            {txn.is_pending === 1 && (
              <span className="pill-orange mt-2 inline-block">Pending</span>
            )}
          </div>

          {/* Core facts */}
          <dl className="space-y-3">
            <DrawerRow icon={<Calendar className="size-4" />} label="Date">
              <span className="mono text-sm">{formatDate(txn.timestamp)}</span>
              <span className="text-mid text-xs ml-2">
                {new Date(txn.timestamp * 1000).toLocaleTimeString("en-GB")}
              </span>
            </DrawerRow>
            <DrawerRow icon={<Building2 className="size-4" />} label="Account">
              <span className="text-sm font-medium">
                {account?.custom_display_name || account?.display_name || "—"}
              </span>
              {account?.iban && (
                <p className="mono text-[11px] text-mid mt-0.5">{account.iban}</p>
              )}
              {account?.card_last4 && (
                <p className="mono text-[11px] text-mid mt-0.5">•••• {account.card_last4}</p>
              )}
            </DrawerRow>
            <DrawerRow icon={<Receipt className="size-4" />} label="Description">
              <p className="text-sm break-words">{txn.description}</p>
              {txn.merchant_name && txn.merchant_name !== txn.description && (
                <p className="text-xs text-mid mt-1">
                  Merchant: <span className="font-medium">{txn.merchant_name}</span>
                </p>
              )}
            </DrawerRow>
            {(txn.counterparty_name || txn.counterparty_iban) && (
              <DrawerRow icon={<Hash className="size-4" />} label="Counterparty">
                {txn.counterparty_name && (
                  <p className="text-sm">{txn.counterparty_name}</p>
                )}
                {txn.counterparty_iban && (
                  <p className="mono text-[11px] text-mid mt-0.5">
                    {txn.counterparty_iban}
                  </p>
                )}
              </DrawerRow>
            )}
            <DrawerRow icon={<Tag className="size-4" />} label="Category">
              <select
                className="input text-sm py-1.5"
                value={txn.category_id ?? ""}
                onChange={(e) =>
                  onSetCategory(e.target.value ? Number(e.target.value) : null)
                }
              >
                <option value="">Uncategorised</option>
                {categories.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
              {category && (
                <p className="text-[11px] text-mid mt-1">
                  Currently: <span className="font-semibold">{category.name}</span>
                </p>
              )}
            </DrawerRow>
            <DrawerRow icon={<StickyNote className="size-4" />} label="Notes">
              <textarea
                className="input text-sm w-full"
                rows={3}
                value={notesDraft}
                onChange={(e) => setNotesDraft(e.target.value)}
                onBlur={() => {
                  const next = notesDraft.trim() || null;
                  if (next !== (txn.notes ?? null)) onSetNotes(next);
                }}
                placeholder="Add a note (autosaves on blur)"
              />
            </DrawerRow>
          </dl>

          {/* Raw provider data — collapsed by default */}
          <details className="card p-3 bg-ink/[0.02]">
            <summary className="text-[10px] uppercase tracking-widest text-mid cursor-pointer">
              Raw provider payload
            </summary>
            <pre className="text-[10px] mono mt-2 overflow-x-auto whitespace-pre-wrap break-all">
              {JSON.stringify(
                {
                  provider_txn_id: txn.provider_txn_id,
                  timestamp: txn.timestamp,
                  is_credit: txn.is_credit,
                  is_pending: txn.is_pending,
                  amount_cents: txn.amount_cents,
                  currency: txn.currency,
                },
                null,
                2
              )}
            </pre>
          </details>
        </div>
      </aside>
    </>
  );
}

function DrawerRow({
  icon,
  label,
  children,
}: {
  icon: React.ReactNode;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-3">
      <dt className="text-mid shrink-0 w-28 flex items-start gap-1.5 text-[11px] uppercase tracking-widest pt-1">
        {icon}
        {label}
      </dt>
      <dd className="flex-1 min-w-0">{children}</dd>
    </div>
  );
}

function FacetSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: number[] | undefined;
  onChange: (v: number[] | undefined) => void;
  options: { label: string; value: number }[];
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const selected = useMemo(() => new Set(value ?? []), [value]);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  return (
    <div className="relative" ref={rootRef}>
      <button
        type="button"
        className="btn-secondary"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
      >
        {label}
        {selected.size > 0 && (
          <span className="pill-grey ml-1">{selected.size}</span>
        )}
      </button>
      {open && (
        <div className="absolute left-0 top-full z-50 mt-1 card p-2 w-56 max-h-72 overflow-y-auto shadow-lg">
          {options.map((o) => (
            <label
              key={o.value}
              className="flex items-center gap-2 py-1 px-1 text-sm hover:bg-cream rounded cursor-pointer"
            >
              <input
                type="checkbox"
                checked={selected.has(o.value)}
                onChange={(e) => {
                  const next = new Set(selected);
                  if (e.target.checked) next.add(o.value);
                  else next.delete(o.value);
                  onChange(next.size ? Array.from(next) : undefined);
                }}
              />
              <span className="truncate">{o.label}</span>
            </label>
          ))}
          {options.length === 0 && (
            <p className="text-xs text-mid p-2">None yet.</p>
          )}
        </div>
      )}
    </div>
  );
}
