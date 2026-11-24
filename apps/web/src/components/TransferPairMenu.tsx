import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft } from "lucide-react";
import { api } from "@/lib/api";
import { displayPayee, formatDate, formatMoney } from "@/lib/format";
import type {
  Account,
  FinancialItem,
  PairSuggestion,
  PreferredPairAccount,
  Transaction,
} from "@/types/api";

function accountLabel(a: Account) {
  return a.custom_display_name || a.display_name;
}

type PairBody =
  | { other_id: number }
  | { account_id: number }
  | { financial_item_id: number };

export function TransferPairMenu({
  txn,
  accounts,
  items = [],
  variant = "pill",
  autoOpen = false,
}: {
  txn: Transaction;
  accounts: Account[];
  items?: FinancialItem[];
  variant?: "pill" | "panel";
  autoOpen?: boolean;
}) {
  const qc = useQueryClient();
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const [pickedId, setPickedId] = useState<number | null>(null);
  const [candidates, setCandidates] = useState<PairSuggestion[] | null>(null);
  const [preferredIds, setPreferredIds] = useState<number[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const others = accounts.filter(
    (a) =>
      a.id !== txn.account_id &&
      a.enabled !== 0 &&
      !a.truelayer_id.startsWith("item:")
  );
  const tradingItems = items.filter((i) => i.is_active === 1);
  const heading = txn.is_credit ? "From account" : "To account";

  const pairMut = useMutation({
    mutationFn: (body: PairBody) =>
      api.post(`/api/transactions/${txn.id}/pair`, body),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions"] });
      qc.invalidateQueries({ queryKey: ["accounts"] });
      qc.invalidateQueries({ queryKey: ["spend-series"] });
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
      close();
    },
    onError: (e: unknown) => {
      setError((e as { message?: string })?.message || "Pairing failed");
    },
  });
  const leaveMut = useMutation({
    mutationFn: () => api.post(`/api/transactions/${txn.id}/unmatchable`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions"] });
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
      close();
    },
    onError: (e: unknown) => {
      setError((e as { message?: string })?.message || "Could not leave unpaired");
    },
  });
  const restoreMut = useMutation({
    mutationFn: () => api.delete(`/api/transactions/${txn.id}/unmatchable`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions"] });
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
    },
  });
  const cancelWaitMut = useMutation({
    mutationFn: () => api.delete(`/api/transactions/${txn.id}/await`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["transactions"] });
      qc.invalidateQueries({ queryKey: ["transfers"] });
      qc.invalidateQueries({ queryKey: ["transaction", txn.id] });
      qc.invalidateQueries({ queryKey: ["pair-suggestions"] });
      qc.invalidateQueries({ queryKey: ["transactions-review"] });
    },
  });

  function close() {
    setOpen(false);
    setPickedId(null);
    setCandidates(null);
    setError(null);
    setLoading(false);
    setPreferredIds([]);
  }

  function placeMenu() {
    const btn = btnRef.current;
    if (!btn) return;
    const r = btn.getBoundingClientRect();
    const width = 280;
    const left = Math.min(
      Math.max(8, r.right - width),
      window.innerWidth - width - 8
    );
    const below = r.bottom + 6;
    const top = below + 320 > window.innerHeight ? Math.max(8, r.top - 326) : below;
    setPos({ top, left });
  }

  useEffect(() => {
    if (!open) return;
    placeMenu();
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    const onReposition = () => placeMenu();
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onReposition);
    window.addEventListener("scroll", onReposition, true);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onReposition);
      window.removeEventListener("scroll", onReposition, true);
    };
  }, [open]);

  useEffect(() => {
    if (!autoOpen) return;
    void openMenu();
    // Open once when review asks for a destination.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoOpen]);

  async function openMenu() {
    placeMenu();
    setOpen(true);
    setError(null);
    setLoading(true);
    try {
      const data = await api.get<{
        suggestions: PairSuggestion[];
        preferred_accounts?: PreferredPairAccount[];
      }>(`/api/transactions/${txn.id}/pair-suggestions`);
      const preferred = data.preferred_accounts ?? [];
      setPreferredIds(preferred.map((a) => a.id));
      if (data.suggestions.length === 1 && preferred.length !== 1) {
        pairMut.mutate({ other_id: data.suggestions[0].other.id });
        return;
      }
      if (preferred.length === 1) {
        await pickAccount(preferred[0].id);
        return;
      }
    } catch (e: unknown) {
      setError((e as { message?: string })?.message || "Could not load matches");
    } finally {
      setLoading(false);
    }
  }

  async function pickAccount(accountId: number) {
    setPickedId(accountId);
    setCandidates(null);
    setError(null);
    setLoading(true);
    try {
      const data = await api.get<{ suggestions: PairSuggestion[] }>(
        `/api/transactions/${txn.id}/pair-suggestions?account_id=${accountId}`
      );
      if (data.suggestions.length === 1) {
        pairMut.mutate({ other_id: data.suggestions[0].other.id });
        return;
      }
      if (data.suggestions.length === 0) {
        pairMut.mutate({ account_id: accountId });
        return;
      }
      setCandidates(data.suggestions);
    } catch (e: unknown) {
      setError((e as { message?: string })?.message || "Could not load matches");
    } finally {
      setLoading(false);
    }
  }

  const picked = others.find((a) => a.id === pickedId);
  const likely = others.filter((a) => preferredIds.includes(a.id));
  const rest = others.filter((a) => !preferredIds.includes(a.id));
  const emptyList = others.length === 0 && tradingItems.length === 0;
  const menu = open
    ? createPortal(
        <div
          ref={menuRef}
          className="fixed z-[80] card p-2 w-[280px] max-h-80 overflow-y-auto shadow-lg"
          style={{ top: pos.top, left: pos.left }}
          role="menu"
          aria-label={heading}
        >
          {pickedId != null ? (
            <>
              <button
                type="button"
                className="flex items-center gap-1 text-xs text-mid hover:text-ink px-2 py-1 mb-1"
                onClick={() => {
                  setPickedId(null);
                  setCandidates(null);
                  setError(null);
                }}
              >
                <ChevronLeft className="size-3.5" />
                {heading}
              </button>
              <p className="px-2 pb-2 text-xs font-semibold truncate">
                {picked ? accountLabel(picked) : "Account"}
              </p>
              {loading || pairMut.isPending ? (
                <p className="px-2 py-3 text-sm text-mid">Matching…</p>
              ) : (candidates?.length ?? 0) > 0 ? (
                <ul>
                  {(candidates ?? []).map((s) => (
                    <li key={s.other.id}>
                      <button
                        type="button"
                        role="menuitem"
                        className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm"
                        onClick={() =>
                          pairMut.mutate({ other_id: s.other.id })
                        }
                        disabled={pairMut.isPending}
                      >
                        <span className="font-medium truncate block">
                          {displayPayee(s.other)}
                        </span>
                        <span className="text-xs text-mid">
                          {formatDate(s.other.timestamp)} ·{" "}
                          {s.other.is_credit ? "+" : "−"}
                          {formatMoney(s.other.amount_cents, s.other.currency)}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <>
                  <p className="px-2 py-3 text-sm text-mid">
                    No opposite-leg match on this account in ±7 days.
                  </p>
                  <button
                    type="button"
                    role="menuitem"
                    className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm"
                    onClick={() => leaveMut.mutate()}
                    disabled={leaveMut.isPending}
                  >
                    Won’t pair
                    <span className="block text-[11px] text-mid font-normal">
                      Other leg isn’t imported — hide until it is
                    </span>
                  </button>
                </>
              )}
            </>
          ) : (
            <>
              <p className="px-2 py-1 text-[11px] font-semibold uppercase tracking-widest text-mid">
                {heading}
              </p>
              {emptyList ? (
                <p className="px-2 py-3 text-sm text-mid">No other accounts.</p>
              ) : (
                <>
                  {tradingItems.length > 0 && (
                    <>
                      <p className="px-2 pt-1 pb-0.5 text-[10px] uppercase tracking-widest text-mid">
                        Trading
                      </p>
                      <ul>
                        {tradingItems.map((item) => (
                          <li key={`item-${item.id}`}>
                            <button
                              type="button"
                              role="menuitem"
                              className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm font-medium truncate"
                              onClick={() =>
                                pairMut.mutate({ financial_item_id: item.id })
                              }
                              disabled={pairMut.isPending}
                            >
                              {item.name}
                            </button>
                          </li>
                        ))}
                      </ul>
                    </>
                  )}
                  {others.length > 0 && (
                    <>
                      {likely.length > 0 && (
                        <>
                          <p className="px-2 pt-1 pb-0.5 text-[10px] uppercase tracking-widest text-mid">
                            Likely
                          </p>
                          <ul>
                            {likely.map((a) => (
                              <li key={a.id}>
                                <button
                                  type="button"
                                  role="menuitem"
                                  className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm font-medium truncate"
                                  onClick={() => pickAccount(a.id)}
                                >
                                  {accountLabel(a)}
                                </button>
                              </li>
                            ))}
                          </ul>
                        </>
                      )}
                      {rest.length > 0 && (
                        <>
                          {(tradingItems.length > 0 || likely.length > 0) && (
                            <p className="px-2 pt-2 pb-0.5 text-[10px] uppercase tracking-widest text-mid">
                              Banks
                            </p>
                          )}
                          <ul>
                            {rest.map((a) => (
                              <li key={a.id}>
                                <button
                                  type="button"
                                  role="menuitem"
                                  className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm font-medium truncate"
                                  onClick={() => pickAccount(a.id)}
                                >
                                  {accountLabel(a)}
                                </button>
                              </li>
                            ))}
                          </ul>
                        </>
                      )}
                    </>
                  )}
                </>
              )}
              {pickedId == null && (
                <div className="mt-1 pt-1 border-t border-thin">
                  <button
                    type="button"
                    role="menuitem"
                    className="w-full text-left px-2 py-2 rounded hover:bg-cream text-sm"
                    onClick={() => leaveMut.mutate()}
                    disabled={leaveMut.isPending}
                  >
                    Won’t pair
                    <span className="block text-[11px] text-mid font-normal">
                      Other leg isn’t imported — hide until it is
                    </span>
                  </button>
                </div>
              )}
            </>
          )}
          {error && <p className="px-2 pt-1 text-xs text-orange">{error}</p>}
        </div>,
        document.body
      )
    : null;

  const waitName = txn.await_account?.name;
  if (txn.transfer_await_account_id || waitName) {
    const waitBtn = (
      <button
        ref={btnRef}
        type="button"
        className={
          variant === "panel"
            ? "btn-secondary text-xs"
            : "pill-warm hover:bg-ink/10"
        }
        title="Stop waiting and return to Unpaired"
        onClick={(e) => {
          e.stopPropagation();
          cancelWaitMut.mutate();
        }}
        disabled={cancelWaitMut.isPending}
      >
        {variant === "panel"
          ? "Stop waiting"
          : waitName
            ? `Waiting · ${waitName}`
            : "Waiting"}
      </button>
    );
    if (variant === "panel") {
      return (
        <div className="space-y-2">
          <p className="text-sm text-mid">
            Waiting for {waitName || "the other account"} to be imported or
            synced. It will pair when that leg arrives.
          </p>
          {waitBtn}
        </div>
      );
    }
    return waitBtn;
  }

  if (txn.transfer_unmatchable) {
    const restoreBtn = (
      <button
        ref={btnRef}
        type="button"
        className={
          variant === "panel"
            ? "btn-secondary text-xs"
            : "pill-warm hover:bg-ink/10"
        }
        title="Return to the unpaired queue"
        onClick={(e) => {
          e.stopPropagation();
          restoreMut.mutate();
        }}
        disabled={restoreMut.isPending}
      >
        {variant === "panel" ? "Return to unpaired queue" : "Won’t pair"}
      </button>
    );
    if (variant === "panel") {
      return (
        <div className="space-y-2">
          <p className="text-sm text-mid">
            Left unpaired — the matching leg isn’t in Tally yet.
          </p>
          {restoreBtn}
        </div>
      );
    }
    return restoreBtn;
  }

  if (variant === "panel") {
    return (
      <div className="space-y-2">
        <button
          ref={btnRef}
          type="button"
          className="btn-secondary text-xs"
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => {
            if (open) close();
            else void openMenu();
          }}
        >
          Choose {txn.is_credit ? "source" : "destination"} account
        </button>
        <button
          type="button"
          className="btn-ghost text-xs"
          onClick={() => leaveMut.mutate()}
          disabled={leaveMut.isPending}
        >
          Won’t pair — other leg isn’t imported
        </button>
        {error && <p className="text-xs text-orange">{error}</p>}
        {menu}
      </div>
    );
  }

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className="pill-grey hover:bg-ink/10"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          if (open) close();
          else void openMenu();
        }}
      >
        {txn.preferred_account
          ? `Unpaired · ${txn.preferred_account.name}`
          : "Unpaired"}
      </button>
      {menu}
    </>
  );
}
