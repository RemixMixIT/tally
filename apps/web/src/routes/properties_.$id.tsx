import { createFileRoute, Link, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ArrowLeft } from "lucide-react";
import { api, type ApiError } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import {
  CONFIDENCE,
  VALUATION_CADENCES,
  VALUATION_METHODS,
  confidenceLabel,
  isoToday,
} from "@/lib/finance";
import type {
  FinancialItem,
  LinkedMortgage,
  PropertyDetails,
  ValuationSnapshot,
} from "@/types/api";

export const Route = createFileRoute("/properties_/$id")({
  component: PropertyPage,
});

type PropertyResponse = {
  item: FinancialItem;
  property: PropertyDetails | null;
  latest_valuation: ValuationSnapshot | null;
  linked_mortgages: LinkedMortgage[];
  equity_gbp: number;
};

function PropertyPage() {
  const { id } = useParams({ from: "/properties_/$id" });
  const itemId = Number(id);
  const qc = useQueryClient();
  const data = useQuery({
    queryKey: ["property", itemId],
    queryFn: () => api.get<PropertyResponse>(`/api/properties/${itemId}`),
  });

  if (!data.data) {
    return <p className="p-12 text-mid text-sm">Loading</p>;
  }
  const d = data.data;
  const v = d.latest_valuation;
  const p = d.property;

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[860px]">
      <Link to="/valuations/$id" params={{ id: String(itemId) }} className="text-sm text-mid inline-flex items-center gap-1">
        <ArrowLeft className="size-4" /> Item
      </Link>
      <header>
        <p className="text-xs uppercase tracking-widest text-mid mb-2">Property</p>
        <h1 className="text-4xl mb-2">{d.item.name}</h1>
        <p className="text-mid text-sm">
          Headline {v ? formatGbp(v.gbp_value) : "—"} · Equity {formatGbp(d.equity_gbp)}
        </p>
      </header>

      <section className="card p-6 grid sm:grid-cols-2 gap-4">
        <Stat label="Value" value={v ? formatGbp(v.gbp_value) : "—"} />
        <Stat label="Valuation date" value={v?.valuation_date ?? "—"} />
        <Stat label="Confidence" value={v ? confidenceLabel(v.confidence) : "—"} />
        <Stat label="Method" value={p?.valuation_method ?? "—"} />
        <Stat label="Region" value={p?.region ?? "—"} />
        <Stat label="Postcode area" value={p?.postcode_area ?? "—"} />
        <div className="sm:col-span-2 text-sm">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-1">Saved URL</p>
          {p?.valuation_url ? (
            <a className="text-green break-all" href={p.valuation_url} target="_blank" rel="noreferrer">
              {p.valuation_url}
            </a>
          ) : (
            <p className="text-mid">None saved. Tally does not scrape valuation sites.</p>
          )}
        </div>
        <div className="sm:col-span-2 text-sm">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-1">Comparable sales</p>
          <p>{p?.comparable_sales_notes || "—"}</p>
        </div>
        {d.linked_mortgages.length === 0 && (
          <p className="sm:col-span-2 text-sm text-mid border-t border-thin pt-3">
            No mortgage linked. Open a mortgage and choose this property in the selector.
          </p>
        )}
        {d.linked_mortgages.map((row) => (
          <div key={row.mortgage.financial_item_id} className="sm:col-span-2 border-t border-thin pt-3">
            <p className="text-[10px] uppercase tracking-widest text-mid mb-1">Linked mortgage</p>
            <Link
              to="/mortgages/$id"
              params={{ id: String(row.item.id) }}
              className="text-green font-semibold"
            >
              {row.item.name}
            </Link>
            <p className="mono text-sm mt-1">
              {formatGbp(row.counted_balance_gbp)}{" "}
              {row.mortgage.repayment_type.replace(/_/g, " ")}
            </p>
          </div>
        ))}
      </section>

      <PropertyForm
        itemId={itemId}
        existing={p}
        onSaved={() => qc.invalidateQueries({ queryKey: ["property", itemId] })}
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

function PropertyForm({
  itemId,
  existing,
  onSaved,
}: {
  itemId: number;
  existing: PropertyDetails | null;
  onSaved: () => void;
}) {
  const [region, setRegion] = useState(existing?.region ?? "");
  const [postcode, setPostcode] = useState(existing?.postcode_area ?? "");
  const [method, setMethod] = useState(existing?.valuation_method ?? "portal_estimate");
  const [url, setUrl] = useState(existing?.valuation_url ?? "");
  const [cadence, setCadence] = useState(existing?.valuation_cadence ?? "six_monthly");
  const [comps, setComps] = useState(existing?.comparable_sales_notes ?? "");
  const [conf, setConf] = useState(existing?.valuation_confidence ?? "estimated");
  const [reviewed, setReviewed] = useState(existing?.last_reviewed ?? isoToday());
  const [error, setError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: () =>
      api.put(`/api/properties/${itemId}`, {
        region,
        postcode_area: postcode || null,
        valuation_method: method,
        valuation_url: url || null,
        valuation_cadence: cadence,
        comparable_sales_notes: comps || null,
        valuation_confidence: conf,
        last_reviewed: reviewed || null,
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
      <h2 className="md:col-span-2 text-lg font-semibold">Property details</h2>
      <p className="md:col-span-2 text-xs text-mid">
        Save a postcode-specific valuation URL and six-monthly review notes. No automated scraping.
      </p>
      <input className="input" placeholder="Region" value={region} onChange={(e) => setRegion(e.target.value)} required />
      <input className="input" placeholder="Postcode area (e.g. HU)" value={postcode} onChange={(e) => setPostcode(e.target.value)} />
      <select className="input" value={method} onChange={(e) => setMethod(e.target.value)}>
        {VALUATION_METHODS.map((m) => (
          <option key={m.value} value={m.value}>{m.label}</option>
        ))}
      </select>
      <select className="input" value={cadence} onChange={(e) => setCadence(e.target.value)}>
        {VALUATION_CADENCES.map((c) => (
          <option key={c.value} value={c.value}>{c.label}</option>
        ))}
      </select>
      <input className="input md:col-span-2" placeholder="Valuation URL" value={url} onChange={(e) => setUrl(e.target.value)} />
      <select className="input" value={conf} onChange={(e) => setConf(e.target.value)}>
        {CONFIDENCE.map((c) => (
          <option key={c.value} value={c.value}>{c.label}</option>
        ))}
      </select>
      <input className="input" type="date" value={reviewed} onChange={(e) => setReviewed(e.target.value)} />
      <textarea className="input md:col-span-2" rows={3} placeholder="Comparable-sales notes" value={comps} onChange={(e) => setComps(e.target.value)} />
      {error && <p className="md:col-span-2 text-sm text-danger">{error}</p>}
      <button className="btn-primary">Save property</button>
    </form>
  );
}
