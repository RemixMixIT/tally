import { CLAIM_FLAGS, isDualClaim, normalizeClaimFlags } from "@/lib/claims";

export function ClaimFlagToggles({
  flags,
  disabled,
  onChange,
}: {
  flags: readonly string[];
  disabled?: boolean;
  onChange: (next: string[]) => void;
}) {
  const selected = new Set(flags);
  function toggle(flag: string) {
    const next = new Set(selected);
    if (next.has(flag)) next.delete(flag);
    else next.add(flag);
    onChange(normalizeClaimFlags([...next]));
  }

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-3">
        {CLAIM_FLAGS.map((flag) => (
          <label key={flag} className="inline-flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="accent-green"
              checked={selected.has(flag)}
              disabled={disabled}
              onChange={() => toggle(flag)}
            />
            {flag}
          </label>
        ))}
      </div>
      {isDualClaim(flags) && (
        <p className="text-xs text-mid">
          Dual claim. Both flags are set and this item is ready for two packs.
        </p>
      )}
    </div>
  );
}
