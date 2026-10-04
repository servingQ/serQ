/-
# A serQ program as a Markov chain

The executable semantics (`Serq/Exec.lean`) runs a workload fixed in
advance. Here the workload arrives over time at random: in each *slot*,
which is one iteration of the engine, `k` new sessions arrive with
probability `p k`, `k ≤ K`. This gives a Markov kernel on the machines the
program reaches (`Foster.Kernel`), and Foster's criterion
(`Serq/Foster.lean`) then applies to the program itself.

* `Exec.inject`: a session arrives now: it is appended, ready to run its
  program. Nothing else changes, so `Exec` itself is untouched.
* `Exec.slot`: `k` sessions arrive; on a busy engine the next event is then
  handled (`step`), on an idle one an iteration starts if anything is ready
  (`afterEvent`). For a program without delays the next event is the running
  iteration's end, so the slot's machine is again at an iteration's start, or
  idle; with delays a slot may end at a delay instead.
* `Exec.slotL`: a slot whose arrivals each have their own attributes (drawn
  lengths); `slot` is `slotL` with `k` copies of one (`slot_eq_slotL`).
* `Foster.Kernel.ofOutcomes`: the kernel of a finite random choice
  `k ∈ {0, …, K}` with probabilities `p`, and its expectation
  (`apply_ofOutcomes`).
-/
import Serq.Exec
import Serq.Foster

namespace SerqLang

namespace Exec

variable (D : Deployment)

/-- A session arrives now, with attributes `a`, ready to run `prog`. On a
busy engine "now" is the running iteration's start, so a session that
arrives during an iteration is stamped (`now`) with its start: a response
time read on this chain is up to one iteration longer. -/
def inject (prog : Prog) (a : ℕ → ℕ) (m : Machine) : Machine :=
  { m with sess := m.sess.push ⟨m.sess.size, ⟨a, []⟩, 0, prog, [], .ready, 0, 0⟩
           ready := m.ready ++ [m.sess.size] }

/-- One slot: `k` sessions arrive, then the next event is handled (on a busy
engine; for a program without delays, the iteration's end and the next
iteration's start), or an iteration starts (on an idle one). -/
def slot (prog : Prog) (a : ℕ → ℕ) (k : ℕ) (m : Machine) : Machine :=
  let m' := afterEvent D ((inject prog a)^[k] m)
  if m.iterEnd.isSome then step D m' else m'

/-- One slot whose arrivals have attributes `as`, in order. -/
def slotL (prog : Prog) (as : List (ℕ → ℕ)) (m : Machine) : Machine :=
  let m' := afterEvent D (as.foldl (fun m a => inject prog a m) m)
  if m.iterEnd.isSome then step D m' else m'

/-- `slot` is `slotL` with `k` copies of the same attributes. -/
theorem slot_eq_slotL (prog : Prog) (a : ℕ → ℕ) (k : ℕ) (m : Machine) :
    slot D prog a k m = slotL D prog (List.replicate k a) m := by
  have key : ∀ k (m : Machine), (inject prog a)^[k] m =
      (List.replicate k a).foldl (fun m a => inject prog a m) m := by
    intro k
    induction k with
    | zero => intro m; rfl
    | succ k ih => intro m; rw [Function.iterate_succ_apply, ih, List.replicate_succ, List.foldl_cons]
  unfold slot slotL
  rw [key]

end Exec

namespace Foster

open Classical in
/-- The kernel of a random choice among `f x 0, …, f x K`, with probabilities
`p 0, …, p K`. -/
noncomputable def Kernel.ofOutcomes {α : Type*} (K : ℕ) (p : ℕ → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k ∈ Finset.range (K + 1), p k = 1) (f : α → ℕ → α) : Kernel α where
  supp x := (Finset.range (K + 1)).image (f x)
  P x y := ∑ k ∈ (Finset.range (K + 1)).filter (fun k => f x k = y), p k
  nonneg _ _ := Finset.sum_nonneg fun k _ => hp0 k
  sum_one x := by
    rw [Finset.sum_fiberwise_of_maps_to (fun k hk => Finset.mem_image_of_mem (f x) hk)]
    exact hp1

/-- The expectation under `ofOutcomes` is the weighted sum over the choices. -/
theorem Kernel.apply_ofOutcomes {α : Type*} (K : ℕ) (p : ℕ → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k ∈ Finset.range (K + 1), p k = 1) (f : α → ℕ → α) (V : α → ℝ) (x : α) :
    (Kernel.ofOutcomes K p hp0 hp1 f).apply V x = ∑ k ∈ Finset.range (K + 1), p k * V (f x k) := by
  classical
  unfold Kernel.apply Kernel.ofOutcomes
  simp only
  rw [← Finset.sum_fiberwise_of_maps_to (g := f x) (t := (Finset.range (K + 1)).image (f x))
    (fun k hk => Finset.mem_image_of_mem (f x) hk)]
  refine Finset.sum_congr rfl fun y _ => ?_
  rw [Finset.sum_mul]
  refine Finset.sum_congr ?_ fun k hk => ?_
  · ext k; simp
  · rw [(Finset.mem_filter.1 hk).2]

/-- One outcome's term bounds the expectation of a nonnegative function from
below. -/
theorem Kernel.le_apply_ofOutcomes {α : Type*} (K : ℕ) (p : ℕ → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k ∈ Finset.range (K + 1), p k = 1) (f : α → ℕ → α) (V : α → ℝ) (hV : ∀ y, 0 ≤ V y)
    (x : α) {k : ℕ} (hk : k ≤ K) :
    p k * V (f x k) ≤ (Kernel.ofOutcomes K p hp0 hp1 f).apply V x := by
  rw [Kernel.apply_ofOutcomes]
  exact Finset.single_le_sum (f := fun k => p k * V (f x k)) (fun k _ => mul_nonneg (hp0 k) (hV _))
    (Finset.mem_range.mpr (Nat.lt_succ_of_le hk))

end Foster

end SerqLang
