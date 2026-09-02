import { createFileRoute, Link, useNavigate, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { AlertTriangle, ArrowLeft, Plus } from "lucide-react";
import { api, type ApiError } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import {
  CONFIDENCE,
  ITEM_TYPES,
  OWNER_ENTITIES,
  SOURCE_TYPES,
  classLabel,
  confidenceLabel,
  holdingsAllowed,
  isoToday,
  parseOptionalNumber,
  type ItemClass,
} from "@/lib/finance";
import type {
  FinancialItem,
  ItemHolding,
  MortgageDetails,
  PropertyDetails,
  ReceivableDetails,
  TaxDetails,
  ValuationSnapshot,
} from "@/types/api";

export const Route = createFileRoute("/valuations_/$id")({
  component: ItemDetailPage,
});

type DetailResponse = {
  item: FinancialItem;
  valuations: ValuationSnapshot[];
  holdings: ItemHolding[];
  holdings_total_gbp: number;
  unreconciled: { parent_gbp: number; holdings_gbp: number; difference_gbp: number } | null;
  property: PropertyDetails | null;
  mortgage: MortgageDetails | null;
  linked_mortgage: MortgageDetails | null;
  tax: TaxDetails | null;
  receivable: ReceivableDetails | null;
  children: FinancialItem[];
  parent: FinancialItem | null;
};

function ItemDetailPage() {
  const { id } = useParams({ from: "/valuations_/$id" });
  const itemId = Number(id);
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [removeError, setRemoveError] = useState<string | null>(null);
  const detail = useQuery({
    queryKey: ["financial-item", itemId],
    queryFn: () => api.get<DetailResponse>(`/api/financial-items/${itemId}`),
  });

  const remove = useMutation({
    mutationFn: () => api.delete(`/api/financial-items/${itemId}`),
    onSuccess: async () => {
      qc.removeQueries({ queryKey: ["financial-item", itemId] });
      await Promise.all([
        qc.invalidateQueries({ queryKey: ["financial-items"], refetchType: "all" }),
        qc.invalidateQueries({ queryKey: ["net-worth"], refetchType: "all" }),
      ]);
      navigate({ to: "/valuations" });
    },
    onError: (e: ApiError) => setRemoveError(e.message),
  });

  if (detail.isPending) {
    return <p className="p-12 text-mid text-sm">Loading</p>;
  }
  if (!detail.data) {
    return <p className="p-12 text-danger">Item not found.</p>;
  }
  const d = detail.data;
  const latest = d.valuations[0];
  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["financial-item", itemId] });
    qc.invalidateQueries({ queryKey: ["financial-items"] });
    qc.invalidateQueries({ queryKey: ["net-worth"] });
  };

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[980px]">
      <Link
        to="/valuations"
        className="text-sm text-mid hover:text-ink inline-flex items-center gap-1"
      >
        <ArrowLeft className="size-4" /> Assets &amp; liabilities
      </Link>

      <header className="fade-in flex flex-col md:flex-row md:items-start justify-between gap-4">
        <div>
          <p className="text-xs uppercase tracking-widest text-mid mb-2">
            {classLabel(d.item.class)} · {d.item.owner_entity}
            {d.item.is_active !== 1 ? " · inactive" : ""}
          </p>
          <h1 className="text-4xl mb-2">{d.item.name}</h1>
          <p className="text-mid text-sm">
            {d.item.provider ?? "No provider"}
            {d.item.reference ? ` · ${d.item.reference}` : ""}
          </p>
        </div>
        <div className="text-right">
          <p className="text-[10px] uppercase tracking-widest text-mid">
            Latest actual
          </p>
          <p className="font-extrabold text-3xl mono">
            {latest ? formatGbp(latest.gbp_value) : "—"}
          </p>
          {latest && (
            <p className="text-xs text-mid mt-1">
              {latest.valuation_date} · {confidenceLabel(latest.confidence)}
            </p>
          )}
        </div>
      </header>

      {d.unreconciled && (
        <div className="card p-4 border-orange/40 bg-orange/5 flex gap-3 text-sm">
          <AlertTriangle className="size-4 text-orange shrink-0 mt-0.5" />
          Holdings total {formatGbp(d.unreconciled.holdings_gbp)} does not match
          the parent snapshot {formatGbp(d.unreconciled.parent_gbp)}.
        </div>
      )}

      <div className="flex flex-wrap gap-2 text-sm">
        {d.item.class === "property" && (
          <Link
            to="/properties/$id"
            params={{ id: String(itemId) }}
            className="btn-secondary"
          >
            Property
          </Link>
        )}
        {d.item.class === "mortgage" && (
          <Link
            to="/mortgages/$id"
            params={{ id: String(itemId) }}
            className="btn-secondary"
          >
            Mortgage
          </Link>
        )}
        <Link to="/forecasts" className="btn-ghost">
          Forecasts (not actuals)
        </Link>
        <button
          className="btn-ghost text-danger"
          disabled={remove.isPending}
          onClick={() => {
            if (!confirm(`Remove “${d.item.name}”?`)) return;
            setRemoveError(null);
            remove.mutate();
          }}
        >
          {remove.isPending ? "Removing…" : "Remove"}
        </button>
      </div>
      {removeError && <p className="text-sm text-danger">{removeError}</p>}

      <MetadataForm item={d.item} parent={d.parent} onSaved={invalidate} />
      <ValuationForm itemId={itemId} currency={d.item.currency} onSaved={invalidate} />
      <HistoryTable valuations={d.valuations} />

      {holdingsAllowed(d.item.class) && (
        <HoldingsSection
          itemId={itemId}
          holdings={d.holdings}
          total={d.holdings_total_gbp}
          currency={d.item.currency}
          onSaved={invalidate}
        />
      )}

      {d.receivable && (
        <ReceivableForm itemId={itemId} existing={d.receivable} onSaved={invalidate} />
      )}
      {d.item.class === "receivable" && !d.receivable && (
        <ReceivableForm itemId={itemId} existing={null} onSaved={invalidate} />
      )}

      {d.children.length > 0 && (
        <section className="card p-6">
          <h2 className="text-lg font-semibold mb-3">Linked items</h2>
          <ul className="space-y-2 text-sm">
            {d.children.map((c) => (
              <li key={c.id}>
                <Link
                  to="/valuations/$id"
                  params={{ id: String(c.id) }}
                  className="text-green font-semibold"
                >
                  {c.name}
                </Link>
                <span className="text-mid ml-2">{classLabel(c.class)}</span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function MetadataForm({
  item,
  parent,
  onSaved,
}: {
  item: FinancialItem;
  parent: FinancialItem | null;
  onSaved: () => void;
}) {
  const [name, setName] = useState(item.name);
  const [itemType, setItemType] = useState(item.item_type);
  const [owner, setOwner] = useState(item.owner_entity);
  const [provider, setProvider] = useState(item.provider ?? "");
  const [reference, setReference] = useState(item.reference ?? "");
  const [currency, setCurrency] = useState(item.currency);
  const [notes, setNotes] = useState(item.notes ?? "");
  const [active, setActive] = useState(item.is_active === 1);
  const [error, setError] = useState<string | null>(null);
  const types = ITEM_TYPES[item.class as ItemClass] ?? [];

  const save = useMutation({
    mutationFn: () =>
      api.put(`/api/financial-items/${item.id}`, {
        name,
        item_type: itemType,
        owner_entity: owner,
        provider: provider || null,
        reference: reference || null,
        parent_item_id: item.parent_item_id,
        currency: currency.toUpperCase(),
        is_rollup: item.is_rollup === 1,
        is_active: active,
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
      <h2 className="md:col-span-2 text-lg font-semibold">Details</h2>
      {parent && (
        <p className="md:col-span-2 text-sm text-mid">
          Parent:{" "}
          <Link
            to="/valuations/$id"
            params={{ id: String(parent.id) }}
            className="text-green font-semibold"
          >
            {parent.name}
          </Link>
        </p>
      )}
      <label className="text-xs text-mid md:col-span-2">
        Name
        <input className="input mt-1" value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="text-xs text-mid">
        Type
        <select className="input mt-1" value={itemType} onChange={(e) => setItemType(e.target.value)}>
          {types.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
          {!types.some((t) => t.value === itemType) && (
            <option value={itemType}>{itemType}</option>
          )}
        </select>
      </label>
      <label className="text-xs text-mid">
        Owner / entity
        <select className="input mt-1" value={owner} onChange={(e) => setOwner(e.target.value)}>
          {OWNER_ENTITIES.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
          {!OWNER_ENTITIES.some((o) => o.value === owner) && (
            <option value={owner}>{owner}</option>
          )}
        </select>
      </label>
      <label className="text-xs text-mid">
        Provider
        <input className="input mt-1" value={provider} onChange={(e) => setProvider(e.target.value)} />
      </label>
      <label className="text-xs text-mid">
        Reference
        <input className="input mt-1" value={reference} onChange={(e) => setReference(e.target.value)} />
      </label>
      <label className="text-xs text-mid">
        Currency
        <input className="input mt-1" value={currency} maxLength={3} onChange={(e) => setCurrency(e.target.value.toUpperCase())} />
      </label>
      <label className="text-xs text-mid flex items-center gap-2 mt-6">
        <input type="checkbox" checked={active} onChange={(e) => setActive(e.target.checked)} />
        Active
      </label>
      <label className="text-xs text-mid md:col-span-2">
        Notes
        <textarea className="input mt-1" rows={2} value={notes} onChange={(e) => setNotes(e.target.value)} />
      </label>
      {error && <p className="md:col-span-2 text-sm text-danger">{error}</p>}
      <button className="btn-secondary" disabled={save.isPending}>
        Save details
      </button>
    </form>
  );
}

function ValuationForm({
  itemId,
  currency,
  onSaved,
}: {
  itemId: number;
  currency: string;
  onSaved: () => void;
}) {
  const [date, setDate] = useState(isoToday());
  const [native, setNative] = useState("");
  const [gbp, setGbp] = useState("");
  const [fx, setFx] = useState("");
  const [source, setSource] = useState("manual_estimate");
  const [sourceRef, setSourceRef] = useState("");
  const [confidence, setConfidence] = useState("estimated");
  const [notes, setNotes] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () =>
      api.post(`/api/financial-items/${itemId}/valuations`, {
        valuation_date: date,
        native_value: Number(native),
        currency,
        gbp_value: parseOptionalNumber(gbp),
        fx_rate: parseOptionalNumber(fx),
        source_type: source,
        source_reference: sourceRef || null,
        confidence,
        notes: notes || null,
      }),
    onSuccess: () => {
      setNative("");
      setGbp("");
      setFx("");
      setNotes("");
      onSaved();
    },
    onError: (e: ApiError) => setError(e.message),
  });

  return (
    <form
      className="card p-6 grid md:grid-cols-3 gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        setError(null);
        add.mutate();
      }}
    >
      <h2 className="md:col-span-3 text-lg font-semibold">Add valuation</h2>
      <p className="md:col-span-3 text-xs text-mid">
        Creates a new snapshot. History is never overwritten.
      </p>
      <label className="text-xs text-mid">
        Date
        <input className="input mt-1" type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
      </label>
      <label className="text-xs text-mid">
        Native value
        <input className="input mt-1" type="number" step="0.01" value={native} onChange={(e) => setNative(e.target.value)} required />
      </label>
      <label className="text-xs text-mid">
        GBP value (if not GBP)
        <input className="input mt-1" type="number" step="0.01" value={gbp} onChange={(e) => setGbp(e.target.value)} />
      </label>
      <label className="text-xs text-mid">
        FX rate
        <input className="input mt-1" type="number" step="0.0001" value={fx} onChange={(e) => setFx(e.target.value)} />
      </label>
      <label className="text-xs text-mid">
        Source
        <select className="input mt-1" value={source} onChange={(e) => setSource(e.target.value)}>
          {SOURCE_TYPES.map((s) => (
            <option key={s.value} value={s.value}>
              {s.label}
            </option>
          ))}
        </select>
      </label>
      <label className="text-xs text-mid">
        Confidence
        <select className="input mt-1" value={confidence} onChange={(e) => setConfidence(e.target.value)}>
          {CONFIDENCE.map((c) => (
            <option key={c.value} value={c.value}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
      <label className="text-xs text-mid md:col-span-3">
        Source / evidence
        <input className="input mt-1" value={sourceRef} onChange={(e) => setSourceRef(e.target.value)} placeholder="URL, statement name…" />
      </label>
      <label className="text-xs text-mid md:col-span-3">
        Notes
        <input className="input mt-1" value={notes} onChange={(e) => setNotes(e.target.value)} />
      </label>
      {error && <p className="md:col-span-3 text-sm text-danger">{error}</p>}
      <button className="btn-primary" disabled={add.isPending}>
        <Plus className="size-4" /> Add snapshot
      </button>
    </form>
  );
}

function HistoryTable({ valuations }: { valuations: ValuationSnapshot[] }) {
  return (
    <section className="card overflow-x-auto">
      <div className="px-6 pt-5 pb-3">
        <h2 className="text-lg font-semibold">Valuation history</h2>
      </div>
      <table className="w-full text-sm min-w-[640px]">
        <thead className="bg-cream">
          <tr className="text-[11px] uppercase tracking-widest text-mid">
            <th className="text-left px-4 py-2.5">Date</th>
            <th className="text-right px-4 py-2.5">Native</th>
            <th className="text-right px-4 py-2.5">GBP</th>
            <th className="text-left px-4 py-2.5">Source</th>
            <th className="text-left px-4 py-2.5">Confidence</th>
          </tr>
        </thead>
        <tbody>
          {valuations.map((v) => (
            <tr key={v.id} className="border-b border-thin last:border-0">
              <td className="px-4 py-2.5 mono text-xs">{v.valuation_date}</td>
              <td className="px-4 py-2.5 text-right mono">
                {formatGbp(v.native_value, v.currency)}
              </td>
              <td className="px-4 py-2.5 text-right mono font-semibold">
                {formatGbp(v.gbp_value)}
              </td>
              <td className="px-4 py-2.5 text-mid">
                {v.source_type}
                {v.source_reference ? (
                  <span className="block text-[11px] truncate max-w-[220px]">
                    {v.source_reference}
                  </span>
                ) : null}
              </td>
              <td className="px-4 py-2.5">
                <span className="pill-warm">{confidenceLabel(v.confidence)}</span>
              </td>
            </tr>
          ))}
          {!valuations.length && (
            <tr>
              <td colSpan={5} className="px-4 py-6 text-mid text-center">
                No snapshots yet.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

function HoldingsSection({
  itemId,
  holdings,
  total,
  currency,
  onSaved,
}: {
  itemId: number;
  holdings: ItemHolding[];
  total: number;
  currency: string;
  onSaved: () => void;
}) {
  const [name, setName] = useState("");
  const [ticker, setTicker] = useState("");
  const [units, setUnits] = useState("");
  const [price, setPrice] = useState("");
  const [native, setNative] = useState("");
  const [cost, setCost] = useState("");
  const [alloc, setAlloc] = useState("");
  const [ret, setRet] = useState("");
  const [date, setDate] = useState(isoToday());
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () =>
      api.post(`/api/financial-items/${itemId}/holdings`, {
        holding_name: name,
        ticker_or_identifier: ticker || null,
        units: parseOptionalNumber(units),
        price: parseOptionalNumber(price),
        cost_basis: parseOptionalNumber(cost),
        target_allocation: parseOptionalNumber(alloc),
        target_return: parseOptionalNumber(ret),
        valuation_date: date,
        native_value: Number(native),
        currency,
      }),
    onSuccess: () => {
      setName("");
      setTicker("");
      setUnits("");
      setPrice("");
      setNative("");
      setCost("");
      setAlloc("");
      setRet("");
      onSaved();
    },
    onError: (e: ApiError) => setError(e.message),
  });

  const del = useMutation({
    mutationFn: (hid: number) => api.delete(`/api/item-holdings/${hid}`),
    onSuccess: onSaved,
  });

  return (
    <section className="space-y-4">
      <div className="flex items-baseline justify-between">
        <h2 className="text-lg font-semibold">Holdings</h2>
        <p className="mono text-sm">
          Roll-up {formatGbp(total)}
        </p>
      </div>
      <div className="card overflow-x-auto">
        <table className="w-full text-sm min-w-[800px]">
          <thead className="bg-cream">
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left px-3 py-2">Holding</th>
              <th className="text-right px-3 py-2">Units</th>
              <th className="text-right px-3 py-2">Price</th>
              <th className="text-right px-3 py-2">Value</th>
              <th className="text-right px-3 py-2">Cost</th>
              <th className="text-right px-3 py-2">Alloc %</th>
              <th className="text-right px-3 py-2">Target rtn</th>
              <th className="text-left px-3 py-2">Date</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {holdings.map((h) => (
              <tr key={h.id} className="border-b border-thin">
                <td className="px-3 py-2">
                  {h.holding_name}
                  {h.ticker_or_identifier && (
                    <span className="block text-[11px] mono text-mid">
                      {h.ticker_or_identifier}
                    </span>
                  )}
                </td>
                <td className="px-3 py-2 text-right mono">{h.units ?? "—"}</td>
                <td className="px-3 py-2 text-right mono">
                  {h.price != null ? formatGbp(h.price, h.price_currency ?? h.currency) : "—"}
                </td>
                <td className="px-3 py-2 text-right mono font-semibold">
                  {formatGbp(h.gbp_value)}
                </td>
                <td className="px-3 py-2 text-right mono">
                  {h.cost_basis != null ? formatGbp(h.cost_basis) : "—"}
                </td>
                <td className="px-3 py-2 text-right mono">
                  {h.target_allocation != null ? `${(h.target_allocation * 100).toFixed(1)}%` : "—"}
                </td>
                <td className="px-3 py-2 text-right mono">
                  {h.target_return != null ? `${(h.target_return * 100).toFixed(1)}%` : "—"}
                </td>
                <td className="px-3 py-2 mono text-xs">{h.valuation_date}</td>
                <td className="px-3 py-2">
                  <button className="text-xs text-danger" onClick={() => del.mutate(h.id)}>
                    Remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <form
        className="card p-5 grid md:grid-cols-4 gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          setError(null);
          add.mutate();
        }}
      >
        <input className="input" placeholder="Holding name" value={name} onChange={(e) => setName(e.target.value)} required />
        <input className="input" placeholder="Ticker" value={ticker} onChange={(e) => setTicker(e.target.value)} />
        <input className="input" placeholder="Units" type="number" step="any" value={units} onChange={(e) => setUnits(e.target.value)} />
        <input className="input" placeholder="Price" type="number" step="any" value={price} onChange={(e) => setPrice(e.target.value)} />
        <input className="input" placeholder="Native value" type="number" step="any" value={native} onChange={(e) => setNative(e.target.value)} required />
        <input className="input" placeholder="Cost basis" type="number" step="any" value={cost} onChange={(e) => setCost(e.target.value)} />
        <input className="input" placeholder="Target alloc (0–1)" type="number" step="any" value={alloc} onChange={(e) => setAlloc(e.target.value)} />
        <input className="input" placeholder="Target return (0–1)" type="number" step="any" value={ret} onChange={(e) => setRet(e.target.value)} />
        <input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} />
        {error && <p className="md:col-span-4 text-sm text-danger">{error}</p>}
        <button className="btn-secondary">Add holding</button>
      </form>
    </section>
  );
}

function ReceivableForm({
  itemId,
  existing,
  onSaved,
}: {
  itemId: number;
  existing: ReceivableDetails | null;
  onSaved: () => void;
}) {
  const [debtor, setDebtor] = useState(existing?.debtor ?? "");
  const [ref, setRef] = useState(existing?.invoice_reference ?? "");
  const [due, setDue] = useState(existing?.due_date ?? isoToday());
  const [amount, setAmount] = useState(String(existing?.expected_amount ?? ""));
  const [status, setStatus] = useState(existing?.status ?? "open");
  const [error, setError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: () =>
      api.put(`/api/receivables/${itemId}`, {
        debtor,
        invoice_reference: ref,
        due_date: due,
        expected_amount: Number(amount),
        currency: existing?.currency ?? "GBP",
        status,
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
      <h2 className="md:col-span-2 text-lg font-semibold">Receivable</h2>
      <p className="md:col-span-2 text-xs text-mid">
        Counted as an asset, distinct from cash.
      </p>
      <input className="input" placeholder="Debtor" value={debtor} onChange={(e) => setDebtor(e.target.value)} required />
      <input className="input" placeholder="Invoice reference" value={ref} onChange={(e) => setRef(e.target.value)} required />
      <input className="input" type="date" value={due} onChange={(e) => setDue(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Expected amount" value={amount} onChange={(e) => setAmount(e.target.value)} />
      <select className="input" value={status} onChange={(e) => setStatus(e.target.value)}>
        <option value="open">Open</option>
        <option value="part_paid">Part paid</option>
        <option value="overdue">Overdue</option>
        <option value="settled">Settled</option>
        <option value="written_off">Written off</option>
      </select>
      {error && <p className="md:col-span-2 text-sm text-danger">{error}</p>}
      <button className="btn-secondary">Save receivable</button>
    </form>
  );
}
