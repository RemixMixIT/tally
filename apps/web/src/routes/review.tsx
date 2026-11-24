import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { ArrowUpRight, Check, Landmark, ListFilter } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate, formatMoney } from "@/lib/format";
import { TransferAwareCategorySelect } from "@/components/TransferAwareCategorySelect";
import type { Account, Category, FinancialItemRow, Transaction } from "@/types/api";

export const Route = createFileRoute("/review")({
  component: ReviewPage,
});

interface ReviewItem {
  transaction: Transaction;
  reason: "transfer" | "mixed" | "no_history" | "ready";
  signature: string;
  payee: string;
}

interface ReviewResponse {
  items: ReviewItem[];
  total: number;
}

const REASON_LABEL: Record<ReviewItem["reason"], string> = {
  transfer: "Transfer-like",
  mixed: "Mixed history",
  no_history: "New payee",
  ready: "Ready from history",
};

function accountLabel(a: Account | undefined): string {
  if (!a) return "Unknown account";
  const name = a.custom_display_name || a.display_name;
  if (a.card_last4) return `${name} ····${a.card_last4}`;
  return name;
}

function ReviewPage() {
  const qc = useQueryClient();
  const [reasonFilter, setReasonFilter] = useState<string>("");
  const [accountFilter, setAccountFilter] = useState<number | "">("");
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const trading = useQuery({
    queryKey: ["financial-items", "trading"],
    queryFn: () =>
      api.get<{ items: FinancialItemRow[] }>(
        "/api/financial-items?class=trading"
      ),
  });
  const review = useQuery({
    queryKey: ["transactions-review"],
    queryFn: () =>
      api.get<ReviewResponse>("/api/transactions/review?limit=500"),
  });

  const apply = useMutation({
    mutationFn: () =>
      api.post<{
        stats: {
          scanned: number;
          applied_rules: number;
          applied_learned: number;
          skipped_mixed: number;
          skipped_transfer: number;
          skipped_no_history: number;
        };
      }>("/api/transactions/apply-learned"),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
      qc.invalidateQueries({ queryKey: ["transactions"] });
    },
  });

  const setCategory = useMutation({
    mutationFn: ({ id, cat }: { id: number; cat: number | null }) =>
      api.patch(`/api/transactions/${id}`, { category_id: cat }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
      qc.invalidateQueries({ queryKey: ["transactions"] });
    },
  });

  const amap = useMemo(() => {
    const m = new Map<number, Account>();
    accounts.data?.accounts.forEach((a) => m.set(a.id, a));
    return m;
  }, [accounts.data]);

  const tradingItems = useMemo(
    () =>
      (trading.data?.items ?? [])
        .map((r) => r.item)
        .filter((i) => i.is_active === 1),
    [trading.data]
  );

  const accountOptions = useMemo(() => {
    const counts = new Map<number, number>();
    for (const it of review.data?.items ?? []) {
      counts.set(it.transaction.account_id, (counts.get(it.transaction.account_id) ?? 0) + 1);
    }
    return [...counts.entries()]
      .map(([id, count]) => ({
        id,
        count,
        label: accountLabel(amap.get(id)),
      }))
      .sort((a, b) => a.label.localeCompare(b.label));
  }, [review.data, amap]);

  const items = (review.data?.items ?? []).filter((it) => {
    if (reasonFilter && it.reason !== reasonFilter) return false;
    if (accountFilter !== "" && it.transaction.account_id !== accountFilter) {
      return false;
    }
    return true;
  });
  const stats = apply.data?.stats;

  return (
    <div className="p-8 md:p-12 max-w-6xl space-y-6">
      <header className="flex flex-wrap items-end justify-between gap-4 fade-in">
        <div>
          <h1 className="text-4xl mb-2">
            <em>Review</em>
          </h1>
          <p className="text-sm text-mid">
            {review.data?.total ?? 0} need a look · mixed and new payees stay
            here until you pick a category. Transfer asks which account the
            other leg is on, then pairs it or waits for that feed.
          </p>
        </div>
        <button
          className="btn-primary"
          onClick={() => apply.mutate()}
          disabled={apply.isPending}
        >
          <Check className="size-4" />
          Apply from history
        </button>
      </header>

      {stats && (
        <p className="text-sm text-green fade-in">
          Scanned {stats.scanned}: {stats.applied_learned} from history,{" "}
          {stats.applied_rules} from rules. Left in queue: {stats.skipped_mixed}{" "}
          mixed, {stats.skipped_no_history} new, {stats.skipped_transfer}{" "}
          transfer-like.
        </p>
      )}

      <div className="flex flex-wrap items-center gap-2 fade-in-1">
        <ListFilter className="size-4 text-mid" />
        <select
          className="input text-sm py-1.5 min-w-[180px]"
          value={accountFilter}
          onChange={(e) =>
            setAccountFilter(e.target.value ? Number(e.target.value) : "")
          }
          aria-label="Filter by account"
        >
          <option value="">All accounts</option>
          {accountOptions.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label} ({a.count})
            </option>
          ))}
        </select>
        <select
          className="input text-sm py-1.5 min-w-[180px]"
          value={reasonFilter}
          onChange={(e) => setReasonFilter(e.target.value)}
          aria-label="Filter by reason"
        >
          <option value="">All reasons</option>
          <option value="ready">Ready from history</option>
          <option value="no_history">New payee</option>
          <option value="mixed">Mixed history</option>
          <option value="transfer">Transfer-like</option>
        </select>
        {(accountFilter !== "" || reasonFilter) && (
          <button
            className="btn-ghost text-xs"
            onClick={() => {
              setAccountFilter("");
              setReasonFilter("");
            }}
          >
            Clear filters
          </button>
        )}
      </div>

      <div className="card overflow-x-auto fade-in-2">
        <div className="min-w-[860px]">
          <div className="grid grid-cols-[minmax(180px,1.5fr)_110px_100px_minmax(140px,1fr)_110px_minmax(180px,1fr)_28px] gap-3 px-5 py-2.5 border-b border-thin text-[10px] uppercase tracking-widest text-mid font-semibold">
            <span>Payee</span>
            <span>Why</span>
            <span>Date</span>
            <span>Account</span>
            <span className="text-right">Amount</span>
            <span>Category</span>
            <span />
          </div>
          {items.length === 0 ? (
            <p className="px-5 py-8 text-sm text-mid text-center">
              Nothing to review.
            </p>
          ) : (
            <ul className="divide-y divide-thin">
              {items.map((it) => {
                const t = it.transaction;
                const acct = amap.get(t.account_id);
                return (
                  <li
                    key={t.id}
                    className="grid grid-cols-[minmax(180px,1.5fr)_110px_100px_minmax(140px,1fr)_110px_minmax(180px,1fr)_28px] gap-3 px-5 py-3.5 items-center text-sm"
                  >
                    <div className="min-w-0">
                      <p className="font-medium truncate">
                        {it.payee || it.signature || t.merchant_name || t.description}
                      </p>
                      {t.description !== it.payee &&
                        t.description !== t.merchant_name && (
                          <p className="text-[11px] text-mid truncate">
                            {t.description}
                          </p>
                        )}
                    </div>
                    <span className="text-[10px] uppercase tracking-widest text-mid">
                      {REASON_LABEL[it.reason]}
                    </span>
                    <span className="text-xs text-mid mono">
                      {formatDate(t.timestamp)}
                    </span>
                    <span
                      className="inline-flex items-center gap-1.5 min-w-0 font-medium"
                      title={accountLabel(acct)}
                    >
                      <Landmark className="size-3.5 shrink-0 text-green" />
                      <span className="truncate">{accountLabel(acct)}</span>
                    </span>
                    <span
                      className={`mono font-semibold text-right ${
                        t.is_credit ? "text-green" : "text-ink"
                      }`}
                    >
                      {t.is_credit ? "+" : "−"}
                      {formatMoney(t.amount_cents, t.currency)}
                    </span>
                    <TransferAwareCategorySelect
                      className="text-xs bg-transparent border border-thin px-1.5 py-0.5 focus:outline-none focus:border-green w-full"
                      txn={t}
                      categories={categories.data?.categories ?? []}
                      accounts={accounts.data?.accounts ?? []}
                      items={tradingItems}
                      value={t.category_id}
                      onChange={(cat) =>
                        setCategory.mutate({ id: t.id, cat })
                      }
                    />
                    <Link
                      to="/transactions"
                      className="btn-ghost !p-1"
                      title="Open in transactions"
                    >
                      <ArrowUpRight className="size-3.5" />
                    </Link>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
