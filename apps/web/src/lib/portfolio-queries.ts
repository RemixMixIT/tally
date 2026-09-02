import type { QueryClient } from "@tanstack/react-query";

/** Refetch every surface that depends on broker holdings. */
export async function invalidatePortfolio(qc: QueryClient) {
  await Promise.all([
    qc.invalidateQueries({ queryKey: ["holdings"], refetchType: "all" }),
    qc.invalidateQueries({ queryKey: ["holdings-net-worth"], refetchType: "all" }),
  ]);
  // Chart / net-worth hit Yahoo history — don't block the form on them.
  void qc.invalidateQueries({ queryKey: ["portfolio-history"], refetchType: "all" });
  void qc.invalidateQueries({ queryKey: ["net-worth"], refetchType: "all" });
}

