/-
# Dai, Deng, Li, Peng: throughput-optimal scheduling for LLM inference

The claims of `examples/papers/dai_sarathi.sq` and
`examples/papers/dai_fastertransformer.sq` (arXiv 2504.07347, the basic LLM
queueing model of §3.1 at the operating point of §6.2, time in 10 µs),
proved about the executable semantics of those programs.

* `token_rate` (both programs), Theorem 2(a): before every iteration the
  engine has served at most `b_max / t_{b_max}` tokens per unit of time,
  whatever the arrivals. It is `Exec.served_rate` with the staircase cost:
  a batch of `b ≤ b_max` tokens lasts `c + a ⌈b / b₀⌉`, and
  `b / (c + a ⌈b / b₀⌉) ≤ b_max / (c + a b_max / b₀)` because `b₀` divides
  `b_max` (`staircase_rate`).
* `not_work_conserving` (FasterTransformer, §4): two requests 467.5 ms
  apart; while the first decodes, the second's 290 prefill tokens wait and
  the batch holds one token.
-/
import Serq.Claims
import Serq.Work

namespace SerqLang
namespace Papers

open Exec

/-- The staircase cost's rate: with `b₀ ∣ b_max`, a batch of `b ≤ b_max`
tokens serves at most `b_max / t_{b_max}` tokens per unit of its time. -/
theorem staircase_rate (c a b0 k b : ℕ) (hb0 : 0 < b0) (hb : b ≤ k * b0) :
    b * (c + a * k) ≤ k * b0 * max 1 (c + a * ((b + b0 - 1) / b0)) := by
  have hq : b ≤ b0 * ((b + b0 - 1) / b0) := by
    have := Nat.div_add_mod (b + b0 - 1) b0
    have := Nat.mod_lt (b + b0 - 1) hb0
    rcases Nat.eq_zero_or_pos b with rfl | hpos
    · simp
    · nlinarith [Nat.sub_add_cancel (show 1 ≤ b + b0 by omega)]
  have h1 : b * c ≤ k * b0 * c := Nat.mul_le_mul_right _ hb
  have h2 : b * (a * k) ≤ k * b0 * (a * ((b + b0 - 1) / b0)) := by
    calc b * (a * k) = a * k * b := by ring
      _ ≤ a * k * (b0 * ((b + b0 - 1) / b0)) := Nat.mul_le_mul_left _ hq
      _ = k * b0 * (a * ((b + b0 - 1) / b0)) := by ring
  calc b * (c + a * k) = b * c + b * (a * k) := by ring
    _ ≤ k * b0 * c + k * b0 * (a * ((b + b0 - 1) / b0)) := Nat.add_le_add h1 h2
    _ = k * b0 * (c + a * ((b + b0 - 1) / b0)) := by ring
    _ ≤ k * b0 * max 1 (c + a * ((b + b0 - 1) / b0)) := Nat.mul_le_mul_left _ (le_max_right _ _)

namespace DaiSarathi

theorem token_rate : Claims.DaiSarathi.token_rate :=
  served_rate _ (fun st h => by
    exact staircase_rate 1128 3547 128 1 st.tokens (by norm_num) (by have : st.tokens ≤ 128 := h; omega)) _ _

/-- (8)-(9) for Sarathi: the engine serves every resident in admission order
and nothing grows, so each batch is `min b_max demand` (`Exec.work_conserving`). -/
theorem work_conserving : Claims.DaiSarathi.work_conserving := by
  intro w hw m hr hs
  have := Exec.work_conserving Claims.DaiSarathi.deployment rfl (fun p => by
      simp [pdef, Claims.DaiSarathi.deployment]) Claims.DaiSarathi.family_work_conserving
    Claims.DaiSarathi.prog (by decide) w hw m hr hs
  change m.last.stats.tokens = min 128 m.last.demand at this
  omega

end DaiSarathi

namespace DaiFastertransformer

theorem token_rate : Claims.DaiFastertransformer.token_rate :=
  served_rate _ (fun st h => by
    exact staircase_rate 1128 3547 128 1 st.tokens (by norm_num) (by have : st.tokens ≤ 128 := h; omega)) _ _

/-- Two requests, the second arriving 467.5 ms after the first. -/
def witness : Workload := ⟨[[(10, 46750)], [(10, 93500)]], [], none, 0, some 8⟩

theorem reach_iterate (D : Deployment) (w : Workload) (P : Prog) :
    ∀ k, Reach D w P ((step D)^[k] (Exec.start D w.init.length w.attr P w))
  | 0 => Reach.start
  | k + 1 => by
    rw [Function.iterate_succ_apply']
    exact Reach.step (reach_iterate D w P k)

/-- The machine after `k` events of the witness. -/
def at_ (k : ℕ) : Machine :=
  (step Claims.DaiFastertransformer.deployment)^[k]
    (Exec.start Claims.DaiFastertransformer.deployment witness.init.length witness.attr
      Claims.DaiFastertransformer.prog witness)

theorem not_work_conserving : Claims.DaiFastertransformer.not_work_conserving := by
  refine ⟨witness, ?_, at_ 12, reach_iterate _ _ _ 12, ?_⟩
  · refine ⟨by decide, rfl, rfl, rfl, ?_⟩
    intro i hi
    simp only [witness, List.length_cons, List.length_nil] at hi
    interval_cases i <;> decide
  · have : (at_ 12).iterEnd.isSome = true ∧ (at_ 12).last.demand ≥ 128 ∧ (at_ 12).last.stats.tokens < 128 := by
      decide +kernel
    exact ⟨this.1, this.2.1, this.2.2⟩

end DaiFastertransformer

end Papers
end SerqLang
