/-
Axiom audit. Prints the axioms each theorem of seQ depends on. CI fails if
anything other than the three standard axioms (`propext`,
`Classical.choice`, `Quot.sound`) appears, in particular `sorryAx`.
-/
import SeQ

open SeQ

-- syntax and pool semantics (Route.lean)
#print axioms RouteLang.Step.invariant
#print axioms RouteLang.Step.nonneg
#print axioms RouteLang.total_evictUntil_le
#print axioms RouteLang.sharedRate_sum
#print axioms RouteLang.serialRate_sum
#print axioms RouteLang.admit_guard_units_only

-- executable semantics and the vLLM scheduler scenarios (RouteOracle.lean, generated)
#print axioms RouteLang.Exec.makeRoom_used
#print axioms RouteLang.Exec.makeRoom_room
#print axioms RouteLang.Exec.evictOne_lt
#print axioms RouteLang.Oracle.vllmRequest_wf
#print axioms RouteLang.Oracle.vllm_chunked
#print axioms RouteLang.Oracle.vllm_hol
#print axioms RouteLang.Oracle.vllm_longchunk
#print axioms RouteLang.Oracle.vllm_mixed
#print axioms RouteLang.Oracle.vllm_preempt
#print axioms RouteLang.Oracle.vllm_seqcap
#print axioms RouteLang.Oracle.vllm_cache_trace
-- serving order of a step engine (admission order is decode-first without a chunk cap)
#print axioms RouteLang.Serve.serve_preserves_shape
#print axioms RouteLang.Serve.serve_eq_decode_first
#print axioms RouteLang.Serve.shape_append_prefill
#print axioms RouteLang.Serve.chunk_cap_breaks_shape
