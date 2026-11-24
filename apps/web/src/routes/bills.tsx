import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Plus, Trash2, Pencil, Check, X } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate, formatMoney } from "@/lib/format";
import type {
  Account,
  AmountException,
  Bill,
  FundingForecast,
  FundingLine,
} from "@/types/api";

export const Route = createFileRoute("/bills")({ component: BillsPage });

function accountLabel(a: Account): string {
  return a.custom_display_name || a.display_name;
}

function pickDefaultAccountId(accounts: Account[]): number | "" {
  const smile = accounts.find((a) =>
    accountLabel(a).toLowerCase().includes("smile"),
  );
  return (smile ?? accounts[0])?.id ?? "";
}

function toDateInput(ts: number | null): string {
  if (!ts) return "";
  const d = new Date(ts * 1000);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function fromDateInput(s: string): number | null {
  if (!s) return null;
  const [y, m, d] = s.split("-").map(Number);
  if (!y || !m || !d) return null;
  return Math.floor(new Date(y, m - 1, d, 12, 0, 0).getTime() / 1000);
}

function FundingLines({
  lines,
  empty,
}: {
  lines: FundingLine[];
  empty: string;
}) {
  if (!lines.length) {
    return empty ? <p className="text-sm text-mid">{empty}</p> : null;
  }
  return (
    <ul className="space-y-2">
      {lines.map((l) => (
        <li
          key={`${l.bill_id}-${l.expected_date}`}
          className="flex items-center justify-between text-sm pb-2 border-b border-thin last:border-0 last:pb-0"
        >
          <div className="min-w-0">
            <p className="font-medium truncate">{l.name}</p>
            <p className="text-[11px] text-mid">
              {formatDate(l.expected_date)}
              {l.is_inflow ? " · inflow" : ""}
            </p>
          </div>
          <span className="mono font-semibold shrink-0 ml-2">
            {l.is_inflow ? "+" : "−"}
            {formatMoney(l.expected_amount_cents)}
          </span>
        </li>
      ))}
    </ul>
  );
}

function BillsPage() {
  const qc = useQueryClient();
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
  });
  const bills = useQuery({
    queryKey: ["bills"],
    queryFn: () => api.get<{ bills: Bill[] }>("/api/bills"),
  });
  const exceptions = useQuery({
    queryKey: ["bills", "exceptions"],
    queryFn: () =>
      api.get<{ exceptions: AmountException[] }>("/api/bills/amount-exceptions"),
  });

  const accountList = accounts.data?.accounts ?? [];
  const [accountId, setAccountId] = useState<number | "">("");
  useEffect(() => {
    if (accountId === "" && accountList.length) {
      setAccountId(pickDefaultAccountId(accountList));
    }
  }, [accountList, accountId]);

  const selected = accountList.find((a) => a.id === accountId);
  const funding = useQuery({
    queryKey: ["bills", "funding", accountId],
    queryFn: () =>
      api.get<FundingForecast>(`/api/bills/funding?account_id=${accountId}`),
    enabled: typeof accountId === "number",
  });

  const [name, setName] = useState("");
  const [amount, setAmount] = useState("");
  const [regex, setRegex] = useState("");
  const [expectedDate, setExpectedDate] = useState("");
  const [isInflow, setIsInflow] = useState(false);
  const [freq, setFreq] = useState<
    "monthly" | "weekly" | "yearly" | "fortnightly"
  >("monthly");
  const [slack, setSlack] = useState("");

  useEffect(() => {
    if (selected) {
      setSlack(((selected.funding_slack_cents ?? 0) / 100).toFixed(2));
    }
  }, [selected?.id, selected?.funding_slack_cents]);

  const create = useMutation({
    mutationFn: () => {
      const cents = amount ? Math.round(parseFloat(amount) * 100) : 0;
      const next = fromDateInput(expectedDate);
      return api.post("/api/bills", {
        name,
        account_id: accountId,
        next_expected_date: next,
        expected_amount_min_cents: cents ? Math.round(cents * 0.95) : 0,
        expected_amount_max_cents: cents ? Math.round(cents * 1.05) : 0,
        repeat_freq: freq,
        match_description_regex: regex || null,
        is_inflow: isInflow,
      });
    },
    onSuccess: () => {
      setName("");
      setAmount("");
      setRegex("");
      setExpectedDate("");
      setIsInflow(false);
      qc.invalidateQueries({ queryKey: ["bills"] });
    },
  });

  const saveSlack = useMutation({
    mutationFn: () => {
      const n = parseFloat(slack);
      return api.patch(`/api/accounts/${accountId}`, {
        funding_slack: Number.isFinite(n) ? n : 0,
      });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["accounts"] });
      qc.invalidateQueries({ queryKey: ["bills", "funding"] });
    },
  });

  const confirm = useMutation({
    mutationFn: (body: {
      bill_id: number;
      transaction_id: number;
      action: "this_payment_only" | "update_regular";
    }) => api.post("/api/bills/confirm-match", body),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["bills"] });
      qc.invalidateQueries({ queryKey: ["accounts"] });
    },
  });

  const exceptionList = exceptions.data?.exceptions ?? [];
  const accountBills = useMemo(
    () =>
      (bills.data?.bills ?? []).filter(
        (b) => accountId === "" || b.account_id === accountId,
      ),
    [bills.data?.bills, accountId],
  );

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Regular payments</em>
        </h1>
        <p className="text-mid text-sm">
          Recurring inflows and outflows on one account, with an expected date.
          Incoming transactions match by payee and date; an unexpected amount
          asks whether it is this payment only or a new regular amount.
        </p>
      </header>

      <div className="card p-5 space-y-3 fade-in-1">
        <label className="text-[11px] uppercase tracking-widest text-mid">
          Account
        </label>
        <select
          className="input"
          value={accountId}
          onChange={(e) =>
            setAccountId(e.target.value ? Number(e.target.value) : "")
          }
        >
          {accountList.length === 0 && <option value="">No accounts</option>}
          {accountList.map((a) => (
            <option key={a.id} value={a.id}>
              {accountLabel(a)}
            </option>
          ))}
        </select>
        <div className="flex gap-2 items-end">
          <div className="flex-1">
            <label className="text-[11px] uppercase tracking-widest text-mid">
              Buffer / slack
            </label>
            <input
              className="input mt-1"
              inputMode="decimal"
              value={slack}
              onChange={(e) => setSlack(e.target.value)}
              placeholder="0.00"
            />
          </div>
          <button
            className="btn-primary"
            type="button"
            disabled={saveSlack.isPending || typeof accountId !== "number"}
            onClick={() => saveSlack.mutate()}
          >
            Save slack
          </button>
        </div>
      </div>

      {funding.data && (
        <section className="card p-5 space-y-3 fade-in-1">
          <h2 className="text-lg font-semibold">
            Transfer in for{" "}
            {funding.data.next_month_label ?? "next month"}
          </h2>
          <p className="text-sm text-mid">
            About a week before month-end: remaining this month plus next
            month&apos;s regulars, plus slack, minus the current balance.
            Variable amounts (blank expected) use the card statement when it
            is in and unpaid, otherwise the last payment.
          </p>
          <p className="text-sm text-mid">
            Balance {formatMoney(funding.data.current_balance_cents)} · slack{" "}
            {formatMoney(funding.data.slack_cents)}
            {funding.data.this_month_net_cents != null && (
              <>
                {" "}
                · still due this month{" "}
                {formatMoney(funding.data.this_month_net_cents)}
              </>
            )}
            {funding.data.next_month_net_cents != null && (
              <>
                {" "}
                · {funding.data.next_month_label} net{" "}
                {formatMoney(funding.data.next_month_net_cents)}
              </>
            )}
          </p>
          <p className="text-2xl font-extrabold tracking-tight">
            Transfer in {formatMoney(funding.data.transfer_in_cents)}
          </p>
          <FundingLines
            lines={
              funding.data.next_month_lines ?? funding.data.lines ?? []
            }
            empty="No regulars in the next month for this account."
          />
          {(funding.data.this_month_lines ?? []).length > 0 && (
            <>
              <h3 className="text-sm font-semibold pt-2">Still due this month</h3>
              <FundingLines
                lines={funding.data.this_month_lines ?? []}
                empty=""
              />
            </>
          )}
          {funding.data.warnings.map((w) => (
            <p key={w} className="text-xs text-orange">
              {w}
            </p>
          ))}
        </section>
      )}

      {exceptionList.length > 0 && (
        <section className="card p-5 space-y-4 border-orange/40">
          <h2 className="text-lg font-semibold">Unexpected amounts</h2>
          <p className="text-sm text-mid">
            These payments match a regular by payee and date, but the amount is
            outside the expected window.
          </p>
          {exceptionList.map((ex) => (
            <div
              key={`${ex.bill.id}-${ex.transaction.id}`}
              className="border border-thin rounded p-3 space-y-2"
            >
              <p className="font-semibold">{ex.bill.name}</p>
              <p className="text-sm text-mid">
                {ex.transaction.description} ·{" "}
                {formatDate(ex.transaction.timestamp)} · paid{" "}
                {formatMoney(
                  ex.transaction.amount_cents,
                  ex.transaction.currency,
                )}
                {ex.bill.expected_amount_max_cents > 0 && (
                  <>
                    {" "}
                    (expected{" "}
                    {formatMoney(
                      ex.bill.expected_amount_min_cents,
                      ex.bill.currency,
                    )}
                    –
                    {formatMoney(
                      ex.bill.expected_amount_max_cents,
                      ex.bill.currency,
                    )}
                    )
                  </>
                )}
              </p>
              <div className="flex gap-2">
                <button
                  className="btn-primary text-xs"
                  disabled={confirm.isPending}
                  onClick={() =>
                    confirm.mutate({
                      bill_id: ex.bill.id,
                      transaction_id: ex.transaction.id,
                      action: "this_payment_only",
                    })
                  }
                >
                  This payment only
                </button>
                <button
                  className="btn-ghost text-xs"
                  disabled={confirm.isPending}
                  onClick={() =>
                    confirm.mutate({
                      bill_id: ex.bill.id,
                      transaction_id: ex.transaction.id,
                      action: "update_regular",
                    })
                  }
                >
                  Update regular
                </button>
              </div>
            </div>
          ))}
        </section>
      )}

      <form
        className="card p-5 space-y-2 fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name && expectedDate && accountId) create.mutate();
        }}
      >
        <input
          className="input"
          placeholder="Name (e.g. KCOM)"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input"
          type="date"
          value={expectedDate}
          onChange={(e) => setExpectedDate(e.target.value)}
          required
        />
        <input
          className="input"
          placeholder="Expected amount (blank if unknown)"
          inputMode="decimal"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
        />
        <p className="text-[11px] text-mid">
          Leave blank when the amount is the last statement (Vanquis). Matching
          still works; funding uses the statement once the PDF is in, otherwise
          the last Smile payment.
        </p>
        <input
          className="input"
          placeholder="Match regex (optional; defaults to the name)"
          value={regex}
          onChange={(e) => setRegex(e.target.value)}
        />
        <select
          className="input"
          value={freq}
          onChange={(e) => setFreq(e.target.value as typeof freq)}
        >
          <option value="monthly">Monthly</option>
          <option value="weekly">Weekly</option>
          <option value="fortnightly">Fortnightly</option>
          <option value="yearly">Yearly</option>
        </select>
        <label className="flex items-center gap-2 text-sm text-mid">
          <input
            type="checkbox"
            checked={isInflow}
            onChange={(e) => setIsInflow(e.target.checked)}
          />
          Inflow (credit / money in)
        </label>
        <button
          className="btn-primary"
          disabled={create.isPending || !accountId}
        >
          <Plus className="size-4" /> Add regular payment
        </button>
        {create.isError && (
          <p className="text-xs text-danger">
            {(create.error as { message?: string })?.message ?? "Could not save"}
          </p>
        )}
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {accountBills.map((b) => (
          <BillRow
            key={b.id}
            b={b}
            accounts={accountList}
            onChanged={() => {
              qc.invalidateQueries({ queryKey: ["bills"] });
            }}
          />
        ))}
        {accountBills.length === 0 && (
          <li className="px-5 py-6 text-sm text-mid">
            No regulars on this account yet. Add Smile (and other) payments
            here — they are not seeded automatically.
          </li>
        )}
      </ul>
    </div>
  );
}

function BillRow({
  b,
  accounts,
  onChanged,
}: {
  b: Bill;
  accounts: Account[];
  onChanged: () => void;
}) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(b.name);
  const [amount, setAmount] = useState(
    b.expected_amount_max_cents > 0
      ? (b.expected_amount_max_cents / 100).toFixed(2)
      : "",
  );
  const [regex, setRegex] = useState(b.match_description_regex ?? "");
  const [expectedDate, setExpectedDate] = useState(
    toDateInput(b.next_expected_date),
  );
  const [accountId, setAccountId] = useState(b.account_id ?? "");
  const [freq, setFreq] = useState(b.repeat_freq);
  const [isInflow, setIsInflow] = useState(b.is_inflow !== 0);

  const save = useMutation({
    mutationFn: () => {
      const cents = amount ? Math.round(parseFloat(amount) * 100) : 0;
      return api.patch(`/api/bills/${b.id}`, {
        name,
        match_description_regex: regex || null,
        next_expected_date: fromDateInput(expectedDate),
        account_id: accountId === "" ? null : Number(accountId),
        repeat_freq: freq,
        is_inflow: isInflow,
        expected_amount_min_cents: cents ? Math.round(cents * 0.95) : 0,
        expected_amount_max_cents: cents ? Math.round(cents * 1.05) : 0,
      });
    },
    onSuccess: () => {
      setEditing(false);
      onChanged();
    },
  });
  const del = useMutation({
    mutationFn: () => api.delete(`/api/bills/${b.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["bills"] }),
  });

  const acctName = accounts.find((a) => a.id === b.account_id);

  if (editing) {
    return (
      <li className="px-5 py-3 space-y-2">
        <input
          className="input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Name"
        />
        <select
          className="input"
          value={accountId}
          onChange={(e) =>
            setAccountId(e.target.value ? Number(e.target.value) : "")
          }
        >
          <option value="">Account</option>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {accountLabel(a)}
            </option>
          ))}
        </select>
        <input
          className="input"
          type="date"
          value={expectedDate}
          onChange={(e) => setExpectedDate(e.target.value)}
        />
        <div className="grid grid-cols-2 gap-2">
          <input
            className="input"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            placeholder="Amount"
            inputMode="decimal"
          />
          <input
            className="input"
            value={regex}
            onChange={(e) => setRegex(e.target.value)}
            placeholder="Match regex"
          />
        </div>
        <select
          className="input"
          value={freq}
          onChange={(e) => setFreq(e.target.value)}
        >
          <option value="monthly">Monthly</option>
          <option value="weekly">Weekly</option>
          <option value="fortnightly">Fortnightly</option>
          <option value="yearly">Yearly</option>
        </select>
        <label className="flex items-center gap-2 text-sm text-mid">
          <input
            type="checkbox"
            checked={isInflow}
            onChange={(e) => setIsInflow(e.target.checked)}
          />
          Inflow
        </label>
        <div className="flex gap-2">
          <button
            className="btn-primary"
            onClick={() => save.mutate()}
            disabled={save.isPending}
          >
            <Check className="size-4" /> Save
          </button>
          <button
            className="btn-ghost text-xs"
            onClick={() => setEditing(false)}
          >
            <X className="size-4" /> Cancel
          </button>
        </div>
      </li>
    );
  }
  return (
    <li className="px-5 py-3.5 flex items-center justify-between gap-3">
      <div className="min-w-0">
        <p className="font-semibold">{b.name}</p>
        <p className="text-[11px] mono text-mid mt-0.5">
          {b.repeat_freq}
          {b.is_inflow ? " · inflow" : ""}
          {acctName ? ` · ${accountLabel(acctName)}` : ""}
          {b.next_expected_date
            ? ` · due ${formatDate(b.next_expected_date)}`
            : " · no expected date"}
        </p>
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <span className="mono text-orange font-semibold">
          {b.expected_amount_max_cents > 0
            ? formatMoney(b.expected_amount_max_cents, b.currency)
            : "amount unknown"}
        </span>
        <button className="btn-ghost text-xs" onClick={() => setEditing(true)}>
          <Pencil className="size-3.5" />
        </button>
        <button className="btn-ghost text-xs" onClick={() => del.mutate()}>
          <Trash2 className="size-3.5" />
        </button>
      </div>
    </li>
  );
}
