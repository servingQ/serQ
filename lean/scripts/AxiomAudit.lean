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
