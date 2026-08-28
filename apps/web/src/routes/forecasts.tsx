import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { api, type ApiError } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import { SCENARIO_TYPES, isoToday, parseOptionalNumber } from "@/lib/finance";
import type { FinancialItem, ForecastScenario } from "@/types/api";

export const Route = createFileRoute("/forecasts")({ component: ForecastsPage });

function ForecastsPage() {
  const qc = useQueryClient();
  const forecasts = useQuery({
    queryKey: ["forecasts"],
    queryFn: () => api.get<{ forecasts: ForecastScenario[] }>("/api/forecasts"),
  });
  const items = useQuery({
    queryKey: ["financial-items"],
    queryFn: () => api.get<{ items: { item: FinancialItem }[] }>("/api/financial-items"),
  });

  const list = forecasts.data?.forecasts ?? [];
  const byType = Object.fromEntries(
    SCENARIO_TYPES.map((s) => [s.value, list.filter((f) => f.scenario_type === s.value)])
  ) as Record<string, ForecastScenario[]>;

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1100px]">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">Planning</p>
        <h1 className="text-4xl md:text-5xl mb-2">
          <em>Forecasts</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          Conservative, base, and optimistic scenarios. Projected values are not actual net worth
          and do not change valuations. Target returns are your inputs, not advice.
        </p>
      </header>

      <div className="grid md:grid-cols-3 gap-4">
        {SCENARIO_TYPES.map((s) => (
          <section key={s.value} className="card p-5">
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">{s.label}</p>
            {(byType[s.value] ?? []).length === 0 && (
              <p className="text-sm text-mid">No {s.label.toLowerCase()} scenario yet.</p>
            )}
            {(byType[s.value] ?? []).map((f) => (
              <div key={f.id} className="border-b border-thin last:border-0 py-3">
                <p className="font-semibold">{f.name}</p>
                <p className="text-xs text-mid">
                  {f.forecast_start} → {f.forecast_end}
                </p>
                <p className="mono text-sm mt-1">
                  {formatGbp(f.starting_gbp_value)}
                  <span className="text-mid"> → </span>
                  <span className="text-green">
                    {f.ending_projected_value != null
                      ? formatGbp(f.ending_projected_value)
                      : "—"}
                  </span>
                </p>
                <p className="text-[11px] text-mid mt-1">{f.assumptions}</p>
              </div>
            ))}
          </section>
        ))}
      </div>

      <ForecastForm
        items={(items.data?.items ?? []).map((r) => r.item)}
        onSaved={() => qc.invalidateQueries({ queryKey: ["forecasts"] })}
      />

      <section className="card overflow-x-auto">
        <table className="w-full text-sm min-w-[800px]">
          <thead className="bg-cream">
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left px-4 py-2.5">Name</th>
              <th className="text-left px-4 py-2.5">Scenario</th>
              <th className="text-left px-4 py-2.5">Period</th>
              <th className="text-right px-4 py-2.5">Start</th>
              <th className="text-right px-4 py-2.5">Projected end</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {list.map((f) => (
              <tr key={f.id} className="border-b border-thin">
                <td className="px-4 py-2.5 font-medium">{f.name}</td>
                <td className="px-4 py-2.5">{f.scenario_type}</td>
                <td className="px-4 py-2.5 mono text-xs">
                  {f.forecast_start} – {f.forecast_end}
                </td>
                <td className="px-4 py-2.5 text-right mono">{formatGbp(f.starting_gbp_value)}</td>
                <td className="px-4 py-2.5 text-right mono text-green">
                  {f.ending_projected_value != null ? formatGbp(f.ending_projected_value) : "—"}
                </td>
                <td className="px-4 py-2.5">
                  <button
                    className="text-xs text-danger"
                    onClick={async () => {
                      await api.delete(`/api/forecasts/${f.id}`);
                      qc.invalidateQueries({ queryKey: ["forecasts"] });
                    }}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
      <p className="text-xs text-mid">
        Actuals live on <Link to="/valuations" className="text-green">assets</Link> and{" "}
        <Link to="/net-worth" className="text-green">net worth</Link>.
      </p>
    </div>
  );
}

function ForecastForm({
  items,
  onSaved,
}: {
  items: FinancialItem[];
  onSaved: () => void;
}) {
  const [name, setName] = useState("");
  const [type, setType] = useState("base");
  const [itemId, setItemId] = useState("");
  const [start, setStart] = useState(isoToday());
  const [end, setEnd] = useState("");
  const [starting, setStarting] = useState("");
  const [target, setTarget] = useState("");
  const [growth, setGrowth] = useState("");
  const [contrib, setContrib] = useState("");
  const [withd, setWithd] = useState("");
  const [assumptions, setAssumptions] = useState("");
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/forecasts", {
        name,
        scenario_type: type,
        financial_item_id: itemId ? Number(itemId) : null,
        forecast_start: start,
        forecast_end: end,
        starting_gbp_value: Number(starting),
        target_return: parseOptionalNumber(target),
        property_growth_rate: parseOptionalNumber(growth),
        contributions: parseOptionalNumber(contrib),
        withdrawals: parseOptionalNumber(withd),
        assumptions,
      }),
    onSuccess: () => {
      setName("");
      setStarting("");
      setAssumptions("");
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
        create.mutate();
      }}
    >
      <h2 className="md:col-span-3 text-lg font-semibold">New scenario</h2>
      <input className="input" placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} required />
      <select className="input" value={type} onChange={(e) => setType(e.target.value)}>
        {SCENARIO_TYPES.map((s) => (
          <option key={s.value} value={s.value}>{s.label}</option>
        ))}
      </select>
      <select className="input" value={itemId} onChange={(e) => setItemId(e.target.value)}>
        <option value="">Whole plan</option>
        {items.map((i) => (
          <option key={i.id} value={i.id}>{i.name}</option>
        ))}
      </select>
      <input className="input" type="date" value={start} onChange={(e) => setStart(e.target.value)} />
      <input className="input" type="date" value={end} onChange={(e) => setEnd(e.target.value)} required />
      <input className="input" type="number" step="0.01" placeholder="Starting GBP" value={starting} onChange={(e) => setStarting(e.target.value)} required />
      <input className="input" type="number" step="0.0001" placeholder="Target return (e.g. 0.05)" value={target} onChange={(e) => setTarget(e.target.value)} />
      <input className="input" type="number" step="0.0001" placeholder="Property growth rate" value={growth} onChange={(e) => setGrowth(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Contributions" value={contrib} onChange={(e) => setContrib(e.target.value)} />
      <input className="input" type="number" step="0.01" placeholder="Withdrawals" value={withd} onChange={(e) => setWithd(e.target.value)} />
      <textarea className="input md:col-span-3" rows={2} placeholder="Assumptions (visible, editable)" value={assumptions} onChange={(e) => setAssumptions(e.target.value)} />
      {error && <p className="md:col-span-3 text-sm text-danger">{error}</p>}
      <button className="btn-primary">Save forecast</button>
    </form>
  );
}
