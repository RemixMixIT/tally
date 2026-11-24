import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { ArrowDownRight, ArrowUpRight } from "lucide-react";
import { TransactionDrawer } from "@/components/TransactionDrawer";
import { TransferPairMenu } from "@/components/TransferPairMenu";
import { api } from "@/lib/api";
import { displayPayee, formatDate, formatMoney } from "@/lib/format";
import type {
  Account,
  Category,
  FinancialItemRow,
  TransfersListResponse,
} from "@/types/api";

export const Route = createFileRoute("/transfers")({
  component: TransfersPage,
});

type StatusFilter = "all" | "unpaired";

function TransfersPage() {
  const qc = useQueryClient();
  const [status, setStatus] = useState<StatusFilter>("unpaired");
  const [selectedId, setSelectedId] = useState<number | null>(null);

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const items = useQuery({
    queryKey: ["financial-items", "trading"],
    queryFn: () =>
      api.get<{ items: FinancialItemRow[] }>(
        "/api/financial-items?class=trading"
      ),
  });
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });
  const transfers = useQuery({
    queryKey: ["transfers", status],
    queryFn: () =>
      api.get<TransfersListResponse>(
        `/api/transactions/transfers?status=${status}&limit=1000`
      ),
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
  const tradingItems = useMemo(
    () =>
      (items.data?.items ?? [])
        .map((r) => r.item)
        .filter((i) => i.is_active === 1),
    [items.data]
  );

  const setCategory = useMutation({
    mutationFn: ({ id, cat }: { id: number; cat: number | null }) =>
      api.patch(`/api/transactions/${id}`, { category_id: cat }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transactions"] });
    },
  });
  const saveNotes = useMutation({
    mutationFn: ({ id, notes }: { id: number; notes: string | null }) =>
      api.patch(`/api/transactions/${id}`, { notes }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["transfers"] }),
  });

  const rows = transfers.data?.transactions ?? [];
  const selectedTxn = rows.find((t) => t.id === selectedId) ?? null;
  const unpairedTotal = transfers.data?.unpaired_total ?? 0;

  return (
    <div className="p-8 md:p-12 space-y-5">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Transfers</em>
        </h1>
        <p className="text-sm text-mid">
          Pair both bank legs of a move between accounts. Named destinations
          (TSB → Smile) pair automatically when the other leg is there. Click
          Unpaired to pick an account, or Won’t pair if it isn’t imported yet.
        </p>
      </header>

      <div
        className="inline-flex rounded-full border border-thin p-0.5 fade-in-1"
        role="tablist"
        aria-label="Transfer status"
      >
        <StatusTab
          active={status === "all"}
          onClick={() => setStatus("all")}
          label="All"
          count={transfers.data?.all_total}
        />
        <StatusTab
          active={status === "unpaired"}
          onClick={() => setStatus("unpaired")}
          label="Unpaired"
          count={unpairedTotal}
        />
      </div>

      <div className="card overflow-hidden fade-in-2">
        <div className="grid grid-cols-[28px_100px_140px_1fr_120px_100px] gap-3 px-4 py-3 border-b border-thin bg-cream text-[11px] font-semibold uppercase tracking-widest text-mid">
          <span />
          <span>Date</span>
          <span>Account</span>
          <span>Payee</span>
          <span className="text-right">Amount</span>
          <span>Status</span>
        </div>
        {transfers.isLoading ? (
          <p className="px-5 py-8 text-sm text-mid text-center">Loading…</p>
        ) : rows.length === 0 ? (
          <p className="px-5 py-8 text-sm text-mid text-center">
            {status === "unpaired"
              ? "No unpaired transfers."
              : "No transfers yet."}
          </p>
        ) : (
          <ul>
            {rows.map((t) => {
              const a = amap.get(t.account_id);
              return (
                <li
                  key={t.id}
                  className={`grid grid-cols-[28px_100px_140px_1fr_120px_100px] gap-3 px-4 items-center border-b border-thin last:border-0 hover:bg-cream/60 cursor-pointer min-h-11 ${
                    selectedId === t.id ? "bg-green/5" : ""
                  }`}
                  onClick={() => setSelectedId(t.id)}
                >
                  <span className="py-2">
                    {t.is_credit ? (
                      <ArrowDownRight className="size-4 text-green" />
                    ) : (
                      <ArrowUpRight className="size-4 text-orange" />
                    )}
                  </span>
                  <span className="text-xs text-mid mono whitespace-nowrap py-2">
                    {formatDate(t.timestamp)}
                  </span>
                  <span className="text-xs text-mid truncate py-2">
                    {a?.custom_display_name || a?.display_name || "—"}
                  </span>
                  <span className="font-medium truncate text-sm py-2">
                    {displayPayee(t)}
                  </span>
                  <span
                    className={`mono font-semibold text-sm text-right py-2 ${
                      t.is_credit ? "text-green" : "text-ink"
                    }`}
                  >
                    {t.is_credit ? "+" : "−"}
                    {formatMoney(t.amount_cents, t.currency)}
                  </span>
                  <span className="py-2" onClick={(e) => e.stopPropagation()}>
                    {t.transfer_pair_id != null ? (
                      <span className="pill-green">Paired</span>
                    ) : (
                      <TransferPairMenu
                        txn={t}
                        accounts={accounts.data?.accounts ?? []}
                        items={tradingItems}
                      />
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {selectedTxn && (
        <TransactionDrawer
          txn={selectedTxn}
          account={amap.get(selectedTxn.account_id) ?? null}
          accounts={accounts.data?.accounts ?? []}
          items={tradingItems}
          category={
            selectedTxn.category_id
              ? cmap.get(selectedTxn.category_id) ?? null
              : null
          }
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

function StatusTab({
  active,
  onClick,
  label,
  count,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count?: number;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      className={`px-4 py-1.5 text-sm font-semibold rounded-full ${
        active ? "bg-ink text-cream" : "text-mid hover:text-ink"
      }`}
      onClick={onClick}
    >
      {label}
      {count != null && (
        <span className={`ml-1.5 tabular-nums ${active ? "opacity-80" : ""}`}>
          {count}
        </span>
      )}
    </button>
  );
}
