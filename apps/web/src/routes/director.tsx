import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Tags } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate, formatGbp, formatMoney } from "@/lib/format";

export const Route = createFileRoute("/director")({
  component: DirectorPage,
});

const TYPES = [
  { value: "", label: "Unclassified" },
  { value: "dividend", label: "Dividend" },
  { value: "pension", label: "Pension" },
  { value: "salary", label: "Salary" },
  { value: "expense", label: "Expense" },
] as const;

interface Target {
  name: string;
  amount_gbp: string;
}

interface Payment {
  transaction_id: number;
  account_name: string;
  timestamp: number;
  description: string;
  amount_cents: number;
  currency: string;
  tide_category: string | null;
  tally_category: string | null;
  classification_type: string | null;
  classification_amount_cents: number | null;
  evidence_link: string | null;
  classification_notes: string | null;
}

interface DirectorResponse {
  period: { from: string; to: string; label: string };
  targets: Target[];
  payments: Payment[];
  outside_period_count: number;
}

function DirectorPage() {
  const q = useQuery({
    queryKey: ["director-payments"],
    queryFn: () => api.get<DirectorResponse>("/api/director-payments"),
  });
  const data = q.data;

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1100px]">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">
          Late accounts
        </p>
        <h1 className="text-4xl md:text-5xl mb-2">
          <em>Director payments</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          Tide payments from {data?.period.label ?? "16 Jan 2025 – 31 Jan 2026"}.
          Tag each one as dividend, pension, salary, or expense. Tide categories
          are reference only.
        </p>
      </header>

      <section className="grid gap-4 md:grid-cols-3 fade-in-1">
        {(data?.targets ?? []).map((t) => (
          <div key={t.name} className="card p-5">
            <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
              {t.name} target
            </p>
            <p className="font-extrabold text-3xl mono tracking-tight">
              {formatGbp(Number(t.amount_gbp))}
            </p>
          </div>
        ))}
      </section>

      {q.isError && (
        <p className="text-sm text-danger">Could not load director payments.</p>
      )}

      {data && data.payments.length === 0 && (
        <section className="card p-6 fade-in-2">
          <p className="text-sm">
            No Tide payments fall in {data.period.label}.
          </p>
          {data.outside_period_count > 0 && (
            <p className="text-sm text-mid mt-2">
              Tide payments on file outside this period:{" "}
              {data.outside_period_count}.
            </p>
          )}
        </section>
      )}

      {data && data.payments.length > 0 && (
        <section className="card p-0 overflow-hidden fade-in-2">
          <div className="px-6 py-4 border-b border-thin flex items-center gap-2">
            <Tags className="size-4 text-mid" />
            <h2 className="text-lg font-semibold">
              {data.payments.length} payment
              {data.payments.length === 1 ? "" : "s"}
            </h2>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-[10px] uppercase tracking-widest text-mid">
                <tr>
                  <th className="px-4 py-3 font-medium">Date</th>
                  <th className="px-4 py-3 font-medium">Account</th>
                  <th className="px-4 py-3 font-medium">Description</th>
                  <th className="px-4 py-3 font-medium">Amount</th>
                  <th className="px-4 py-3 font-medium">Tide category</th>
                  <th className="px-4 py-3 font-medium">Classification</th>
                </tr>
              </thead>
              <tbody>
                {data.payments.map((p) => (
                  <PaymentRow key={p.transaction_id} payment={p} />
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  );
}

function PaymentRow({ payment }: { payment: Payment }) {
  const qc = useQueryClient();
  const [kind, setKind] = useState(payment.classification_type ?? "");
  const [notes, setNotes] = useState(payment.classification_notes ?? "");
  const [evidence, setEvidence] = useState(payment.evidence_link ?? "");
  const save = useMutation({
    mutationFn: (next: { kind: string; notes: string; evidence: string }) =>
      api.put("/api/director-payments", {
        transaction_id: payment.transaction_id,
        classification_type: next.kind || null,
        notes: next.notes || null,
        evidence_link: next.evidence || null,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["director-payments"] });
    },
  });

  const recorded =
    payment.classification_amount_cents != null &&
    payment.classification_amount_cents !== payment.amount_cents
      ? payment.classification_amount_cents
      : null;

  return (
    <tr className="border-t border-thin align-top">
      <td className="px-4 py-3 whitespace-nowrap">{formatDate(payment.timestamp)}</td>
      <td className="px-4 py-3">{payment.account_name}</td>
      <td className="px-4 py-3">
        <div>{payment.description}</div>
        {payment.tally_category && (
          <div className="text-xs text-mid mt-1">Tally: {payment.tally_category}</div>
        )}
      </td>
      <td className="px-4 py-3 whitespace-nowrap">
        {formatMoney(payment.amount_cents, payment.currency)}
        {recorded != null && (
          <div className="text-xs text-mid mt-1">
            Recorded {formatMoney(recorded, payment.currency)}
          </div>
        )}
      </td>
      <td className="px-4 py-3 text-mid">{payment.tide_category || "—"}</td>
      <td className="px-4 py-3 min-w-[220px]">
        <select
          className="input text-sm py-1.5 w-full"
          aria-label={`Classification for ${payment.description}`}
          value={kind}
          onChange={(e) => {
            const next = e.target.value;
            setKind(next);
            save.mutate({ kind: next, notes, evidence });
          }}
        >
          {TYPES.map((t) => (
            <option key={t.value || "none"} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
        <input
          className="input text-sm py-1.5 w-full mt-2"
          placeholder="Notes"
          aria-label={`Notes for ${payment.description}`}
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          onBlur={() => {
            if ((payment.classification_notes ?? "") !== notes) {
              save.mutate({ kind, notes, evidence });
            }
          }}
        />
        <input
          className="input text-sm py-1.5 w-full mt-2"
          placeholder="Evidence link"
          aria-label={`Evidence link for ${payment.description}`}
          value={evidence}
          onChange={(e) => setEvidence(e.target.value)}
          onBlur={() => {
            if ((payment.evidence_link ?? "") !== evidence) {
              save.mutate({ kind, notes, evidence });
            }
          }}
        />
        {save.isError && (
          <p className="text-xs text-danger mt-1">Could not save.</p>
        )}
      </td>
    </tr>
  );
}
