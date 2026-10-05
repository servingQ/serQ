/-
Axiom audit. Prints the axioms each theorem of the serQ Lean model depends
on. `scripts/check_lean.sh` fails if anything other than the three standard
axioms (`propext`, `Classical.choice`, `Quot.sound`) appears, in particular
`sorryAx` (an unfinished proof) or a custom axiom.
-/
import Serq

-- syntax and pool semantics (Serq/Core.lean)
#print axioms SerqLang.Step.invariant
#print axioms SerqLang.Step.nonneg
#print axioms SerqLang.total_evictUntil_le
#print axioms SerqLang.sharedRate_sum
#print axioms SerqLang.serialRate_sum
#print axioms SerqLang.admit_guard_units_only
-- executable semantics and the vLLM scheduler scenarios (Serq/Exec.lean, Serq/Oracle.lean, generated)
#print axioms SerqLang.Exec.makeRoom_used
#print axioms SerqLang.Exec.makeRoom_room
#print axioms SerqLang.Exec.evictOne_lt
#print axioms SerqLang.Oracle.vllmRequest_wf
#print axioms SerqLang.Oracle.vllm_chunked
#print axioms SerqLang.Oracle.vllm_hol
#print axioms SerqLang.Oracle.vllm_longchunk
#print axioms SerqLang.Oracle.vllm_mixed
#print axioms SerqLang.Oracle.vllm_preempt
#print axioms SerqLang.Oracle.vllm_seqcap
#print axioms SerqLang.Oracle.vllmTurn_wf
#print axioms SerqLang.Oracle.vllm_cache_trace
-- serving order of a step engine (Serq/Serve.lean)
#print axioms SerqLang.Serve.serve_preserves_shape
#print axioms SerqLang.Serve.serve_eq_decode_first
#print axioms SerqLang.Serve.shape_append_prefill
#print axioms SerqLang.Serve.chunk_cap_breaks_shape
#print axioms SerqLang.Exec.Attrs.get_upd
#print axioms SerqLang.Exec.lru_before
#print axioms SerqLang.Exec.evictOne_evictK
#print axioms SerqLang.Exec.makeRoom_unroll
#print axioms SerqLang.Exec.makeRoomFast_eq
#print axioms SerqLang.Exec.makeRoom_eq_fast
-- regressions against the interpreter (Serq/Regress.lean, generated)
#print axioms SerqLang.Regress.regress_cost_ctx
#print axioms SerqLang.Regress.regress_preempt_delay
-- an iteration of the step engine is the greedy fill, in serving order (Serq/Fill.lean)
#print axioms SerqLang.Exec.fillIter_eq_amounts
#print axioms SerqLang.Exec.fillAmounts_fifo
#print axioms SerqLang.Exec.assign_eq_fillIter
#print axioms SerqLang.Exec.fillAmounts_le_want
#print axioms SerqLang.Exec.fillAmounts_sum
#print axioms SerqLang.Exec.assign_iter_eq_fillIter
-- dead cache entries are irrelevant to the live ones under LRU (Serq/Regen.lean)
#print axioms SerqLang.Exec.makeRoom_fuel
#print axioms SerqLang.Exec.makeRoom_dead_irrelevant
-- claims: what every path keeps, and the paper programs' claims (Serq/Claim.lean, Serq/Inv.lean,
-- Serq/Work.lean, Serq/Papers/)
#print axioms SerqLang.Exec.served_rate
#print axioms SerqLang.Exec.reach_sub
#print axioms SerqLang.Exec.work_conserving
#print axioms SerqLang.Exec.every_iteration_of
#print axioms SerqLang.Papers.DaiSarathi.token_rate
#print axioms SerqLang.Papers.DaiSarathi.work_conserving
#print axioms SerqLang.Papers.DaiSarathi.bounded
#print axioms SerqLang.Papers.DaiFastertransformer.token_rate
#print axioms SerqLang.Papers.DaiFastertransformer.not_work_conserving
#print axioms SerqLang.Papers.BariRad.token_rate
#print axioms SerqLang.Papers.BariRad.optimal_tiling
#print axioms SerqLang.Papers.KongMath.opt_lower_bound
#print axioms SerqLang.Papers.KongSvf.queueing_bound
#print axioms SerqLang.Papers.KongSvf.competitive_ratio
-- Foster's criterion on a kernel of countably many outcomes, without measure theory (Serq/Foster.lean)
#print axioms SerqLang.Foster.drift_bound
#print axioms SerqLang.Foster.hitTime_le
#print axioms SerqLang.Foster.hitTime_eq
#print axioms SerqLang.Foster.returnTime_le_of_drift
#print axioms SerqLang.Foster.walk_hitTime_le
-- a program with random arrivals as a Markov kernel, and Dai et al.'s Theorem 2(b) on it
-- (Serq/Chain.lean, Serq/Papers/DaiStable.lean)
#print axioms SerqLang.Foster.Kernel.apply_ofOutcomes
#print axioms SerqLang.Papers.DaiStable.backlog_slot
#print axioms SerqLang.Papers.DaiStable.backlog_lt_of_F
#print axioms SerqLang.Papers.DaiStable.drift
#print axioms SerqLang.Papers.DaiStable.hitTime_le
#print axioms SerqLang.Papers.DaiStable.returnTime_le
#print axioms SerqLang.Papers.DaiStable.hit_tendsto
-- Bari et al.'s Theorem 2 on the kernel (Serq/Papers/BariStable.lean)
#print axioms SerqLang.Exec.slot_eq_slotL
#print axioms SerqLang.Papers.BariStable.backlog_slot
#print axioms SerqLang.Papers.BariStable.backlog_lt_of_F
#print axioms SerqLang.Papers.BariStable.drift
#print axioms SerqLang.Papers.BariStable.hitTime_le
#print axioms SerqLang.Papers.BariStable.hit_tendsto
#print axioms SerqLang.Papers.BariStable.returnTime_le
-- Bari et al.'s Theorem 2 over g nodes with the random planner (Serq/Papers/BariNodes.lean)
#print axioms SerqLang.Papers.BariNodes.mean_route
#print axioms SerqLang.Papers.BariNodes.apply_V
#print axioms SerqLang.Papers.BariNodes.drift
#print axioms SerqLang.Papers.BariNodes.hitTime_le
#print axioms SerqLang.Papers.BariNodes.hit_tendsto
#print axioms SerqLang.Papers.BariNodes.returnTime_le
#print axioms SerqLang.Papers.BariNodes.drift_sum
#print axioms SerqLang.Papers.BariNodes.load_thin
#print axioms SerqLang.Papers.BariNodes.marginal
#print axioms SerqLang.Papers.BariNodes.positive_recurrent
-- positive recurrence: from a set to a state, and the papers' chains on job lists
-- (Serq/Recurrence.lean, Serq/Papers/{Dai,Bari}{Sim,Recurrent}.lean)
#print axioms SerqLang.Foster.hit_le_of_reach
#print axioms SerqLang.Foster.positiveRecurrent_of_hit
#print axioms SerqLang.Papers.DaiSim.simulation
#print axioms SerqLang.Papers.DaiRecurrent.hit_nil_le
#print axioms SerqLang.Papers.DaiRecurrent.irreducible
#print axioms SerqLang.Papers.DaiRecurrent.positive_recurrent
#print axioms SerqLang.Papers.BariSim.simulation
#print axioms SerqLang.Papers.BariRecurrent.hit_nil_le
#print axioms SerqLang.Papers.BariRecurrent.irreducible
#print axioms SerqLang.Papers.BariRecurrent.positive_recurrent
#print axioms SerqLang.Papers.DaiProgram.hit_idle_le
#print axioms SerqLang.Papers.DaiProgram.return_idle
#print axioms SerqLang.Papers.BariProgram.hit_idle_le
#print axioms SerqLang.Papers.BariProgram.return_idle
-- Poisson arrivals in continuous time (Serq/Poisson.lean, Serq/Slot.lean, Serq/Papers/{Dai,Bari}Poisson.lean)
#print axioms SerqLang.Poisson.hasSum_mul_pois
#print axioms SerqLang.Poisson.hasSum_compound
#print axioms SerqLang.Poisson.hasSum_compound_work
#print axioms SerqLang.Slot.slot_dur
#print axioms SerqLang.Papers.DaiPoisson.dur_full
#print axioms SerqLang.Papers.DaiPoisson.drift
#print axioms SerqLang.Papers.DaiPoisson.hitTime_le
#print axioms SerqLang.Papers.DaiPoisson.hit_tendsto
#print axioms SerqLang.Papers.DaiPoisson.returnTime_le
#print axioms SerqLang.Papers.BariPoisson.dur_full
#print axioms SerqLang.Papers.BariPoisson.drift
#print axioms SerqLang.Papers.BariPoisson.hitTime_le
#print axioms SerqLang.Papers.BariPoisson.hit_tendsto
#print axioms SerqLang.Papers.BariPoisson.returnTime_le
