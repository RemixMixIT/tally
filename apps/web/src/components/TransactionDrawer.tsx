import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  ArrowLeftRight,
  Building2,
  Calendar,
  Hash,
  Flag,
  Receipt,
  StickyNote,
  Tag,
  X,
} from "lucide-react";
import { ClaimFlagToggles } from "@/components/ClaimFlagToggles";
import { TransferAwareCategorySelect } from "@/components/TransferAwareCategorySelect";
import { TransferPairMenu } from "@/components/TransferPairMenu";
import { api } from "@/lib/api";
import { displayPayee, formatDate, formatMoney } from "@/lib/format";
import type {
  Account,
  Category,
  FinancialItem,
  PairSuggestion,
  Transaction,
  TransferPairInfo,
} from "@/types/api";

export function TransactionDrawer({
  txn,
  account,
  accounts,
  items = [],
  category,
  categories,
  onClose,
  onSetCategory,
  onSetNotes,
  onSetClaimFlags,
}: {
  txn: Transaction;
  account: Account | null;
  accounts: Account[];
  items?: FinancialItem[];
  category: Category | null;
  categories: Category[];
  onClose: () => void;
  onSetCategory: (id: number | null) => void;
  onSetNotes: (notes: string | null) => void;
  onSetClaimFlags: (flags: string[]) => void;
}) {
  const qc = useQueryClient();
  const [notesDraft, setNotesDraft] = useState(txn.notes ?? "");
  useEffect(() => {
    setNotesDraft(txn.notes ?? "");
  }, [txn.id, txn.notes]);

  const detail = useQuery({
    queryKey: ["transaction", txn.id],
    queryFn: () =>
      api.get<{
        transaction: Transaction;
        pair: TransferPairInfo | null;
      }>(`/api/transactions/${txn.id}`),
  });
  const suggestions = useQuery({
    queryKey: ["pair-suggestions", txn.id],
    queryFn: () =>
      api.get<{ suggestions: PairSuggestion[] }>(
        `/api/transactions/${txn.id}/pair-suggestions`
      ),
    enabled:
      (detail.data?.transaction.transfer_pair_id ?? txn.transfer_pair_id) ==
        null &&
      !(detail.data?.transaction.transfer_unmatchable ?? txn.transfer_unmatchable),
  });
  const pairMut = useMutation({
    mutationFn: (otherId: number) =>
      api.post(`/api/transactions/${txn.id}/pair`, { other_id: otherId }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions", txn.id] });
      qc.invalidateQueries({ queryKey: ["spend-series"] });
    },
  });
  const unpairMut = useMutation({
    mutationFn: () => api.delete(`/api/transactions/${txn.id}/pair`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions", txn.id] });
      qc.invalidateQueries({ queryKey: ["spend-series"] });
    },
  });

  const paired = detail.data?.pair ?? null;
  const live = detail.data?.transaction ?? txn;
  const unmatchable = (live.transfer_unmatchable ?? 0) !== 0;
  const acctName = (id: number) => {
    const a = accounts.find((x) => x.id === id);
    return a?.custom_display_name || a?.display_name || `Account ${id}`;
  };

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
              {displayPayee(txn)}
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
                <p className="mono text-[11px] text-mid mt-0.5">
                  •••• {account.card_last4}
                </p>
              )}
            </DrawerRow>
            <DrawerRow icon={<Receipt className="size-4" />} label="Payee">
              <p className="text-sm break-words font-medium">{displayPayee(txn)}</p>
              {txn.description !== displayPayee(txn) && (
                <p className="text-xs text-mid mt-1 break-words">
                  Bank line: {txn.description}
                </p>
              )}
              {txn.merchant_name &&
                txn.merchant_name !== txn.description &&
                txn.merchant_name !== displayPayee(txn) && (
                  <p className="text-xs text-mid mt-1">
                    Merchant:{" "}
                    <span className="font-medium">{txn.merchant_name}</span>
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
              <TransferAwareCategorySelect
                className="input text-sm py-1.5"
                txn={live}
                categories={categories}
                accounts={accounts}
                items={items}
                value={txn.category_id}
                onChange={onSetCategory}
              />
              {category && (
                <p className="text-[11px] text-mid mt-1">
                  Currently:{" "}
                  <span className="font-semibold">{category.name}</span>
                </p>
              )}
            </DrawerRow>
            <DrawerRow
              icon={<ArrowLeftRight className="size-4" />}
              label="Transfer"
            >
              {paired ? (
                <div className="space-y-2">
                  <p className="text-sm">
                    Paired with{" "}
                    <span className="font-medium">
                      {displayPayee(paired.other)}
                    </span>
                    {" · "}
                    {acctName(paired.other.account_id)}
                    {" · "}
                    {paired.other.is_credit ? "+" : "−"}
                    {formatMoney(
                      paired.other.amount_cents,
                      paired.other.currency
                    )}
                  </p>
                  {paired.implied_fx_rate != null && (
                    <p className="text-sm mono">
                      Implied FX: 1{" "}
                      {txn.is_credit ? paired.other.currency : txn.currency} ={" "}
                      {paired.implied_fx_rate.toFixed(4)}{" "}
                      {txn.is_credit ? txn.currency : paired.other.currency}
                    </p>
                  )}
                  <button
                    type="button"
                    className="btn-secondary text-xs"
                    onClick={() => unpairMut.mutate()}
                    disabled={unpairMut.isPending}
                  >
                    Unpair
                  </button>
                </div>
              ) : unmatchable ? (
                <TransferPairMenu
                  txn={live}
                  accounts={accounts}
                  items={items}
                  variant="panel"
                />
              ) : (
                <div className="space-y-3">
                  {suggestions.data?.suggestions.length ? (
                    <ul className="space-y-2">
                      {suggestions.data.suggestions.map((s) => (
                        <li
                          key={s.other.id}
                          className="flex items-start justify-between gap-2 text-sm"
                        >
                          <div className="min-w-0">
                            <p className="font-medium truncate">
                              {displayPayee(s.other)}
                            </p>
                            <p className="text-xs text-mid">
                              {acctName(s.other.account_id)} ·{" "}
                              {formatDate(s.other.timestamp)} ·{" "}
                              {s.other.is_credit ? "+" : "−"}
                              {formatMoney(
                                s.other.amount_cents,
                                s.other.currency
                              )}
                              {s.implied_fx_rate != null && (
                                <>
                                  {" · "}
                                  FX {s.implied_fx_rate.toFixed(4)} → {s.leaf}
                                </>
                              )}
                              {s.implied_fx_rate == null && <> · {s.leaf}</>}
                            </p>
                          </div>
                          <button
                            type="button"
                            className="btn-secondary text-xs shrink-0"
                            onClick={() => pairMut.mutate(s.other.id)}
                            disabled={pairMut.isPending}
                          >
                            Pair
                          </button>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="text-sm text-mid">
                      {suggestions.isLoading
                        ? "Looking for a matching leg…"
                        : "No automatic match in ±3 days. Choose the other account."}
                    </p>
                  )}
                  <TransferPairMenu
                    txn={live}
                    accounts={accounts}
                    items={items}
                    variant="panel"
                  />
                </div>
              )}
              {(pairMut.isError || unpairMut.isError) && (
                <p className="text-xs text-orange mt-1">
                  {(pairMut.error as { message?: string } | undefined)
                    ?.message ||
                    (unpairMut.error as { message?: string } | undefined)
                      ?.message ||
                    "Pairing failed"}
                </p>
              )}
            </DrawerRow>
            <DrawerRow icon={<Flag className="size-4" />} label="Claim">
              <ClaimFlagToggles
                flags={live.claim_flags ?? []}
                onChange={onSetClaimFlags}
              />
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
