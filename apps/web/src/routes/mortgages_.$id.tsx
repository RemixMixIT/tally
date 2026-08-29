import { createFileRoute, Link, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ArrowLeft } from "lucide-react";
import { api, type ApiError } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import { PAYMENT_FREQUENCIES, RATE_TYPES, REPAYMENT_TYPES, parseOptionalNumber } from "@/lib/finance";
import type { FinancialItem, MortgageDetails, ValuationSnapshot } from "@/types/api";

export const Route = createFileRoute("/mortgages_/$id")({
  component: MortgagePage,
});

type MortgageResponse = {
  item: FinancialItem;
  mortgage: MortgageDetails | null;
  latest_valuation: ValuationSnapshot | null;
  property: FinancialItem | null;
  property_valuation: ValuationSnapshot | null;
  counted_balance_gbp: number;
  equity_impact_gbp: number | null;
};

function MortgagePage() {
  const { id } = useParams({ from: "/mortgages_/$id" });
  const itemId = Number(id);
  const qc = useQueryClient();
  const data = useQuery({
    queryKey: ["mortgage", itemId],
    queryFn: () => api.get<MortgageResponse>(`/api/mortgages/${itemId}`),
  });
  const properties = useQuery({
    queryKey: ["financial-items", "property"],
    queryFn: () =>
      api.get<{ items: { item: FinancialItem }[] }>("/api/financial-items?class=property"),
  });

  if (!data.data) {
    return <p className="p-12 text-mid text-sm">Loading</p>;
  }
  const d = data.data;
  const m = d.mortgage;

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[860px]">
      <Link to="/valuations/$id" params={{ id: String(itemId) }} className="text-sm text-mid inline-flex items-center gap-1">
        <ArrowLeft className="size-4" /> Item
      </Link>
      <header>
        <p className="text-xs uppercase tracking-widest text-mid mb-2">Mortgage</p>
        <h1 className="text-4xl mb-2">{d.item.name}</h1>
        <p className="text-mid text-sm">
          Counted once as a liability ({formatGbp(d.counted_balance_gbp)}). Interest is not a second liability.
        </p>
      </header>

      <section className="card p-6 grid sm:grid-cols-2 gap-4">
        <Stat label="Property" value={d.property?.name ?? "Unlinked"} />
        <Stat label="Repayment" value={m?.repayment_type.replace(/_/g, " ") ?? "—"} />
        <Stat label="Balance" value={formatGbp(d.counted_balance_gbp)} />
        <Stat label="Rate" value={m?.interest_rate != null ? `${(m.interest_rate * 100).toFixed(2)}%` : "—"} />
        <Stat label="Rate type" value={m?.rate_type ?? "—"} />
        <Stat label="Fixed until" value={m?.fixed_period_expiry ?? "—"} />
        <Stat label="Payment" value={m?.payment_amount != null ? formatGbp(m.payment_amount) : "—"} />
        <Stat label="Frequency" value={m?.payment_frequency ?? "—"} />
        <Stat label="Term end" value={m?.term_end_date ?? "—"} />
        <Stat label="Overpayments" value={m?.overpayment_total != null ? formatGbp(m.overpayment_total) : "—"} />
        <Stat label="Projected balance" value={m?.projected_balance != null ? formatGbp(m.projected_balance) : "—"} />
        <Stat
          label="Equity impact"
          value={d.equity_impact_gbp != null ? formatGbp(d.equity_impact_gbp) : "—"}
        />
        {d.property && (
          <Link
            to="/properties/$id"
            params={{ id: String(d.property.id) }}
            className="sm:col-span-2 text-green font-semibold"
          >
            Open {d.property.name}
          </Link>
        )}
      </section>

      <MortgageForm
        itemId={itemId}
        existing={m}
        properties={(properties.data?.items ?? []).map((r) => r.item)}
        onSaved={() => {
          qc.invalidateQueries({ queryKey: ["mortgage", itemId] });
          qc.invalidateQueries({ queryKey: ["property"] });
          qc.invalidateQueries({ queryKey: ["financial-items"] });
          qc.invalidateQueries({ queryKey: ["net-worth"] });
        }}
      />
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-mid mb-1">{label}</p>
      <p className="font-semibold">{value}</p>
    </div>
  );
}

function MortgageForm({
  itemId,
  existing,
  properties,
  onSaved,
}: {
  itemId: number;
  existing: MortgageDetails | null;
  properties: FinancialItem[];
  onSaved: () => void;
}) {
  const [propertyId, setPropertyId] = useState(
    existing?.property_item_id ? String(existing.property_item_id) : ""
  );
  const [repay, setRepay] = useState(existing?.repayment_type ?? "interest_only");
  const [balance, setBalance] = useState(String(existing?.outstanding_balance ?? ""));
  const [rate, setRate] = useState(existing?.interest_rate != null ? String(existing.interest_rate) : "");
  const [rateType, setRateType] = useState(existing?.rate_type ?? "fixed");
  const [fixedUntil, setFixedUntil] = useState(existing?.fixed_period_expiry ?? "");
  const [payment, setPayment] = useState(existing?.payment_amount != null ? String(existing.payment_amount) : "");
  const [freq, setFreq] = useState(existing?.payment_frequency ?? "monthly");
  const [term, setTerm] = useState(existing?.term_end_date ?? "");
  const [over, setOver] = useState(existing?.overpayment_total != null ? String(existing.overpayment_total) : "");
  const [proj, setProj] = useState(existing?.projected_balance != null ? String(existing.projected_balance) : "");
  const [notes, setNotes] = useState(existing?.notes ?? "");
  const [error, setError] = useState<string | null>(null);
  const selectable = properties.filter(
    (p) => p.is_active === 1 || p.id === existing?.property_item_id
  );

  const save = useMutation({
    mutationFn: () =>
      api.put(`/api/mortgages/${itemId}`, {
        property_item_id: propertyId ? Number(propertyId) : null,
        repayment_type: repay,
        outstanding_balance: Number(balance),
        interest_rate: parseOptionalNumber(rate),
        rate_type: rateType || null,
        fixed_period_expiry: fixedUntil || null,
        payment_amount: parseOptionalNumber(payment),
        payment_frequency: freq || null,
        term_end_date: term || null,
        overpayment_total: parseOptionalNumber(over),
        projected_balance: parseOptionalNumber(proj),
        notes: notes || null,
      }),
    onSuccess: onSaved,
    onError: (e: ApiError) => setError(e.message),
  });

  return (
    <form
      className="card p-6 grid md:grid-cols-2 gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        setError(null);
        save.mutate();
      }}
    >
      <h2 className="md:col-span-2 text-lg font-semibold">Mortgage details</h2>
      <label className="text-xs text-mid md:col-span-2">
        Linked property
        <select className="input mt-1" value={propertyId} onChange={(e) => setPropertyId(e.target.value)}>
          <option value="">None — unlinked</option>
          {selectable.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
          {propertyId && !selectable.some((p) => String(p.id) === propertyId) && (
            <option value={propertyId}>Property #{propertyId} (unavailable)</option>
          )}
        </select>
      </label>
      <select className="input" value={repay} onChange={(e) => setRepay(e.target.value)}>
        {REPAYMENT_TYPES.map((r) => (
          <option key={r.value} value={r.value}>{r.label}</option>
        ))}
      </select>
      <input className="input" type="number" step="0.01" placeholder="Outstanding balance" value={balance} onChange={(e) => setBalance(e.target.value)} required />
      <input className="input" type="number" step="0.0001" placeholder="Interest rate (e.g. 0.045)" value={rate} onChange={(e) => setRate(e.target.value)} />
      <select className="input" value={rateType} onChange={(e) => setRateType(e.target.value)}>
        {RATE_TYPES.map((r) => (
          <option key={r.value} value={r.value}>{r.label}</option>
        ))}
      </select>
      <input className="input" type="date" value={fixedUntil} onChange={(e) => setFixedUntil(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Payment amount" value={payment} onChange={(e) => setPayment(e.target.value)} />
      <select className="input" value={freq} onChange={(e) => setFreq(e.target.value)}>
        {PAYMENT_FREQUENCIES.map((f) => (
          <option key={f.value} value={f.value}>{f.label}</option>
        ))}
      </select>
      <input className="input" type="date" value={term} onChange={(e) => setTerm(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Overpayment total" value={over} onChange={(e) => setOver(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Projected balance (C&I)" value={proj} onChange={(e) => setProj(e.target.value)} />
      <textarea className="input md:col-span-2" rows={2} placeholder="Notes" value={notes} onChange={(e) => setNotes(e.target.value)} />
      {error && <p className="md:col-span-2 text-sm text-danger">{error}</p>}
      <button className="btn-primary">Save mortgage</button>
    </form>
  );
}
