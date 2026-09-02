import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { AlertTriangle, Plus, Landmark } from "lucide-react";
import { api, type ApiError } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import {
  ITEM_CLASSES,
  ITEM_TYPES,
  OWNER_ENTITIES,
  classLabel,
  typeLabel,
  confidenceLabel,
  isoToday,
  matchesItemFilters,
  type ItemClass,
} from "@/lib/finance";
import type { FinancialItemRow } from "@/types/api";

export const Route = createFileRoute("/valuations")({ component: AssetsPage });

function AssetsPage() {
  const qc = useQueryClient();
  const list = useQuery({
    queryKey: ["financial-items"],
    queryFn: () =>
      api.get<{ items: FinancialItemRow[] }>("/api/financial-items"),
  });

  const [classFilter, setClassFilter] = useState("");
  const [ownerFilter, setOwnerFilter] = useState("");
  const [sideFilter, setSideFilter] = useState<"all" | "asset" | "liability">(
    "all"
  );
  const [activeFilter, setActiveFilter] = useState<"active" | "all" | "inactive">(
    "active"
  );
  const [showAdd, setShowAdd] = useState(false);

  const rows = useMemo(() => {
    return (list.data?.items ?? []).filter((row) =>
      matchesItemFilters(row.item, {
        classFilter,
        ownerFilter,
        sideFilter,
        activeFilter,
      })
    );
  }, [list.data, classFilter, ownerFilter, sideFilter, activeFilter]);

  const owners = Array.from(
    new Set((list.data?.items ?? []).map((r) => r.item.owner_entity))
  );

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1280px]">
      <header className="fade-in flex flex-col md:flex-row md:items-end justify-between gap-4">
        <div>
          <p className="text-xs uppercase tracking-widest text-mid mb-3">
            Assets &amp; liabilities
          </p>
          <h1 className="text-4xl md:text-5xl mb-2">
            Manual <em>valuations</em>
          </h1>
          <p className="text-mid text-sm max-w-2xl">
            Property, pensions, mortgages, tax, receivables, and other items that
            are not bank feeds. Forecasts stay on a separate screen and never
            mix into actuals.
          </p>
        </div>
        <div className="flex gap-2 shrink-0">
          <Link to="/net-worth" className="btn-secondary">
            Net worth
          </Link>
          <Link to="/forecasts" className="btn-ghost">
            Forecasts
          </Link>
          <button className="btn-primary" onClick={() => setShowAdd(!showAdd)}>
            <Plus className="size-4" /> Add item
          </button>
        </div>
      </header>

      {showAdd && (
        <AddItemForm
          onDone={() => {
            setShowAdd(false);
            qc.invalidateQueries({ queryKey: ["financial-items"] });
          }}
        />
      )}

      <section className="card p-4 flex flex-wrap gap-3 items-end fade-in-1">
        <label className="text-xs text-mid">
          Class
          <select
            className="input mt-1"
            value={classFilter}
            onChange={(e) => setClassFilter(e.target.value)}
          >
            <option value="">All</option>
            {ITEM_CLASSES.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
        <label className="text-xs text-mid">
          Owner
          <select
            className="input mt-1"
            value={ownerFilter}
            onChange={(e) => setOwnerFilter(e.target.value)}
          >
            <option value="">All</option>
            {owners.map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        </label>
        <label className="text-xs text-mid">
          Side
          <select
            className="input mt-1"
            value={sideFilter}
            onChange={(e) =>
              setSideFilter(e.target.value as typeof sideFilter)
            }
          >
            <option value="all">All</option>
            <option value="asset">Assets</option>
            <option value="liability">Liabilities</option>
          </select>
        </label>
        <label className="text-xs text-mid">
          Status
          <select
            className="input mt-1"
            value={activeFilter}
            onChange={(e) =>
              setActiveFilter(e.target.value as typeof activeFilter)
            }
          >
            <option value="active">Active</option>
            <option value="inactive">Inactive</option>
            <option value="all">All</option>
          </select>
        </label>
      </section>

      <section className="card overflow-x-auto fade-in-2">
        <table className="w-full text-sm min-w-[720px]">
          <thead className="bg-cream">
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left px-4 py-2.5">Name</th>
              <th className="text-left px-4 py-2.5">Class</th>
              <th className="text-left px-4 py-2.5">Owner</th>
              <th className="text-right px-4 py-2.5">GBP</th>
              <th className="text-left px-4 py-2.5">Valued</th>
              <th className="text-left px-4 py-2.5">Confidence</th>
              <th className="px-4 py-2.5"></th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const i = row.item;
              const gbp = row.counted_gbp ?? row.latest_valuation?.gbp_value;
              return (
                <tr
                  key={i.id}
                  className="border-b border-thin last:border-0 hover:bg-cream/60"
                >
                  <td className="px-4 py-3">
                    <Link
                      to="/valuations/$id"
                      params={{ id: String(i.id) }}
                      className="font-semibold hover:text-green"
                    >
                      {i.name}
                    </Link>
                    {i.is_active !== 1 && (
                      <span className="pill-grey ml-2">Inactive</span>
                    )}
                    {row.unreconciled && (
                      <span className="pill-orange ml-2 inline-flex items-center gap-1">
                        <AlertTriangle className="size-3" /> Unreconciled
                      </span>
                    )}
                  </td>
                  <td className="px-4 py-3 text-mid">
                    {i.is_liability === 1
                      ? typeLabel(i.class, i.item_type)
                      : classLabel(i.class)}
                    {i.is_liability === 1 ? (
                      <span className="block text-[10px] uppercase tracking-widest">
                        liability
                      </span>
                    ) : null}
                  </td>
                  <td className="px-4 py-3 text-mid">{i.owner_entity}</td>
                  <td
                    className={`px-4 py-3 text-right mono font-semibold ${
                      i.is_liability === 1 ? "text-danger" : "text-ink"
                    }`}
                  >
                    {gbp != null ? formatGbp(gbp) : "—"}
                  </td>
                  <td className="px-4 py-3 mono text-xs text-mid">
                    {row.latest_valuation?.valuation_date ?? "—"}
                  </td>
                  <td className="px-4 py-3">
                    {row.latest_valuation ? (
                      <span className="pill-warm">
                        {confidenceLabel(row.latest_valuation.confidence)}
                      </span>
                    ) : (
                      <span className="text-mid text-xs">no snapshot</span>
                    )}
                  </td>
                  <td className="px-4 py-3 text-right">
                    {i.class === "property" && i.is_liability !== 1 && (
                      <Link
                        to="/properties/$id"
                        params={{ id: String(i.id) }}
                        className="text-xs text-green mr-2"
                      >
                        Property
                      </Link>
                    )}
                    <Link
                      to="/valuations/$id"
                      params={{ id: String(i.id) }}
                      className="text-xs text-mid hover:text-ink"
                    >
                      History
                    </Link>
                  </td>
                </tr>
              );
            })}
            {!rows.length && (
              <tr>
                <td colSpan={7} className="px-4 py-10 text-center text-mid">
                  <Landmark className="size-6 mx-auto mb-3 text-mid" />
                  No matching items. Add a manual asset or liability above.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>
    </div>
  );
}

function AddItemForm({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState("");
  const [cls, setCls] = useState<ItemClass>("cash");
  const [itemType, setItemType] = useState(ITEM_TYPES.cash[0].value);
  const [owner, setOwner] = useState("personal");
  const [provider, setProvider] = useState("");
  const [currency, setCurrency] = useState("GBP");
  const [notes, setNotes] = useState("");
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: async () => {
      const created = await api.post<{ item: { id: number; currency: string } }>(
        "/api/financial-items",
        {
          name: name.trim(),
          class: cls,
          item_type: itemType,
          owner_entity: owner,
          provider: provider.trim() || null,
          currency: currency.trim().toUpperCase(),
          notes: notes.trim() || null,
        }
      );
      const n = Number(value);
      if (value.trim() !== "" && Number.isFinite(n)) {
        await api.post(`/api/financial-items/${created.item.id}/valuations`, {
          valuation_date: isoToday(),
          native_value: n,
          currency: currency.trim().toUpperCase(),
          source_type: "manual_estimate",
          confidence: "estimated",
        });
      }
    },
    onSuccess: onDone,
    onError: (e: ApiError) => setError(e.message),
  });

  return (
    <form
      className="card p-6 grid md:grid-cols-2 gap-3 fade-in"
      onSubmit={(e) => {
        e.preventDefault();
        setError(null);
        create.mutate();
      }}
    >
      <h2 className="md:col-span-2 text-lg font-semibold">Add manual item</h2>
      <label className="text-xs text-mid md:col-span-2">
        Name
        <input
          className="input mt-1"
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
        />
      </label>
      <label className="text-xs text-mid">
        Class
        <select
          className="input mt-1"
          value={cls}
          onChange={(e) => {
            const next = e.target.value as ItemClass;
            setCls(next);
            setItemType(ITEM_TYPES[next][0].value);
          }}
        >
          {ITEM_CLASSES.map((c) => (
            <option key={c.value} value={c.value}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
      <label className="text-xs text-mid">
        Type
        <select
          className="input mt-1"
          value={itemType}
          onChange={(e) => setItemType(e.target.value)}
        >
          {ITEM_TYPES[cls].map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
      </label>
      <label className="text-xs text-mid">
        Owner / entity
        <select
          className="input mt-1"
          value={owner}
          onChange={(e) => setOwner(e.target.value)}
        >
          {OWNER_ENTITIES.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>
      <label className="text-xs text-mid">
        Provider
        <input
          className="input mt-1"
          value={provider}
          onChange={(e) => setProvider(e.target.value)}
        />
      </label>
      <label className="text-xs text-mid">
        Currency
        <input
          className="input mt-1"
          value={currency}
          onChange={(e) => setCurrency(e.target.value.toUpperCase())}
          maxLength={3}
        />
      </label>
      <label className="text-xs text-mid">
        Opening native value (optional)
        <input
          className="input mt-1"
          type="number"
          step="0.01"
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
      </label>
      <label className="text-xs text-mid md:col-span-2">
        Notes
        <textarea
          className="input mt-1"
          rows={2}
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
        />
      </label>
      {error && <p className="md:col-span-2 text-sm text-danger">{error}</p>}
      <div className="md:col-span-2 flex gap-2">
        <button className="btn-primary" disabled={create.isPending}>
          Save
        </button>
        <button type="button" className="btn-ghost" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
