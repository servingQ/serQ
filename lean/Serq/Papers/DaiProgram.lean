/-
# Dai et al., Theorem 2(b), on the program's own chain

The machine chain of `dai_sarathi.sq` (`DaiStable.kernel`) never revisits a
state: it keeps the clock and every ended session. What recurs is the
engine being empty, `σ m = []`, and every machine with an empty engine
behaves alike: its job list follows `DaiChain`'s chain from `[]`
(`DaiSim.simulation`). Below capacity, on the machine chain itself:

* the engine empties in bounded expected time from every state
  (`hit_idle_le`);
* it empties again in expected time bounded by one constant from every
  machine whose engine is empty (`return_idle`): the set of empty machines
  is a positive recurrent atom.
-/
import Serq.Papers.DaiSim
import Serq.Papers.DaiRecurrent

namespace SerqLang
namespace Papers
namespace DaiProgram

open Foster DaiStable

/-- The engine of the machine holds no job. -/
def Idle {K : ℕ} (x : State K) : Prop := DaiSim.σ x.1 = []

instance {K : ℕ} : DecidablePred (Idle (K := K)) := fun x => by unfold Idle; infer_instance

/-! ### What the machine chain inherits from the job lists -/

/-- The machine's backlog is its job list's. -/
theorem backlog_σ (m : Exec.Machine) : backlog m = DaiChain.backlog (DaiSim.σ m) := by
  simp [backlog, DaiChain.backlog, DaiSim.σ, Function.comp_def]

/-- Every reached machine's job list is one the list chain can hold. -/
theorem good_σ {K : ℕ} (hK : K ≤ 10000) : ∀ {m : Exec.Machine}, Reach K m → DaiRecurrent.Good (DaiSim.σ m)
  | _, .empty => by rw [DaiSim.σ_empty]; intro j hj; simp at hj
  | _, .slot k hk h => by
    rw [DaiSim.simulation hK ⟨_, h⟩ k hk]
    exact DaiRecurrent.good_absSlot k (good_σ hK h)

section Chain

variable {K : ℕ} (A : Arrivals K)

/-- The state after a slot with `k ≤ K` arrivals. -/
def nxt (x : State K) (k : ℕ) (hk : k ≤ K) : State K := ⟨slot k x.1, Reach.slot k hk x.2⟩

theorem apply_kernel (f : State K → ℝ) (x : State K) :
    (kernel A).apply f x = ∑ k ∈ Finset.range (K + 1),
      A.p k * f (if hk : k ≤ K then nxt x k hk else x) :=
  Kernel.apply_ofOutcomes _ _ _ _ _ _ _

/-- One term of the expectation bounds it from below. -/
theorem le_apply (f : State K → ℝ) (hf : ∀ y, 0 ≤ f y) (x : State K) {k : ℕ} (hk : k ≤ K) :
    A.p k * f (nxt x k hk) ≤ (kernel A).apply f x := by
  have := Kernel.le_apply_ofOutcomes K A.p A.nonneg A.sum_one
    (fun x k => if hk : k ≤ K then (⟨slot k x.1, Reach.slot k hk x.2⟩ : State K) else x) f hf x hk
  have e : (if hk : k ≤ K then (⟨slot k x.1, Reach.slot k hk x.2⟩ : State K) else x) = nxt x k hk :=
    dif_pos hk
  rw [← e]
  exact this

include A in
/-- A slot adds at most `1280 K` tokens. -/
theorem backlog_next_le (x : State K) (k : ℕ) :
    (backlog (if hk : k ≤ K then nxt x k hk else x).1 : ℝ) ≤ backlog x.1 + 1280 * K := by
  split
  · rename_i hk
    have h1 : backlog (nxt x k hk).1 ≤ backlog x.1 + 1280 * K := by
      show backlog (slot k x.1) ≤ _
      rw [backlog_σ, backlog_σ, DaiSim.simulation A.small x k hk]
      by_cases h0 : DaiSim.σ x.1 = []
      · rw [h0, DaiRecurrent.backlog_absSlot_nil]
        simp [DaiChain.backlog]; omega
      · have := DaiRecurrent.backlog_absSlot k h0; omega
    exact_mod_cast h1
  · have : (0 : ℝ) ≤ 1280 * K := by positivity
    linarith

theorem applyN_le : ∀ (n : ℕ) (x : State K),
    (kernel A).applyN n (fun y => (backlog y.1 : ℝ)) x ≤ backlog x.1 + 1280 * K * n
  | 0, x => by simp [Kernel.applyN]
  | n + 1, x => by
    simp only [Kernel.applyN]
    have h1 := (kernel A).apply_mono (applyN_le n) x
    rw [Kernel.apply_add, Kernel.apply_const] at h1
    have h2 : (kernel A).apply (fun y => (backlog y.1 : ℝ)) x ≤ backlog x.1 + 1280 * K := by
      rw [apply_kernel]
      calc ∑ k ∈ Finset.range (K + 1), A.p k * (backlog (if hk : k ≤ K then nxt x k hk else x).1 : ℝ)
          ≤ ∑ k ∈ Finset.range (K + 1), A.p k * (backlog x.1 + 1280 * K) :=
            Finset.sum_le_sum fun k _ => mul_le_mul_of_nonneg_left (backlog_next_le A x k) (A.nonneg k)
        _ = backlog x.1 + 1280 * K := by rw [← Finset.sum_mul, A.sum_one, one_mul]
    push_cast; nlinarith

/-- Without arrivals the engine drains: from a backlog `≤ n`, it is empty
within `n` slots with probability at least `p₀ⁿ`. -/
theorem drain (hA : 1280 * A.mean < 128) : ∀ (n : ℕ) (x : State K), backlog x.1 ≤ n →
    A.p 0 ^ n ≤ reach (kernel A) Idle n x
  | 0, x, hb => by
    have hx : Idle x := by
      by_contra h
      have := DaiRecurrent.backlog_pos (good_σ A.small x.2) h
      rw [backlog_σ] at hb; omega
    rw [reach_of_mem (kernel A) Idle hx]; simp
  | n + 1, x, hb => by
    by_cases hx : Idle x
    · rw [reach_of_mem (kernel A) Idle hx]
      exact pow_le_one₀ (A.nonneg 0) (DaiRecurrent.p0_le_one A)
    · have hb' : backlog (nxt x 0 (Nat.zero_le K)).1 ≤ n := by
        show backlog (slot 0 x.1) ≤ n
        rw [backlog_σ, DaiSim.simulation A.small x 0 (Nat.zero_le K)]
        have h1 := DaiRecurrent.backlog_absSlot 0 hx
        have h2 := DaiRecurrent.one_le_shares (good_σ A.small x.2) hx
        rw [backlog_σ] at hb
        omega
      have ih := drain hA n _ hb'
      have h3 := le_apply A (reach (kernel A) Idle n) (reach_nonneg (kernel A) Idle n) x (Nat.zero_le K)
      show A.p 0 ^ (n + 1) ≤ (if Idle x then 1 else (kernel A).apply (reach (kernel A) Idle n) x)
      rw [if_neg hx, pow_succ]
      calc A.p 0 ^ n * A.p 0 ≤ reach (kernel A) Idle n (nxt x 0 (Nat.zero_le K)) * A.p 0 :=
            mul_le_mul_of_nonneg_right ih (A.nonneg 0)
        _ = A.p 0 * reach (kernel A) Idle n (nxt x 0 (Nat.zero_le K)) := mul_comm _ _
        _ ≤ _ := h3

/-- `F`, or the engine empty. -/
def G (x : State K) : Prop := F x ∨ Idle x

instance : DecidablePred (G (K := K)) := fun x => by unfold G; infer_instance

theorem driftG (hA : 1280 * A.mean < 128) :
    Drift (kernel A) G (fun x => (backlog x.1 : ℝ)) (ε A) :=
  let hD := DaiStable.drift A hA
  ⟨hD.nonneg, hD.pos, fun x hx => hD.drift x (fun h => hx (Or.inl h))⟩

include A in
theorem backlog_le_of_G {x : State K} (hx : G x) : backlog x.1 ≤ 128 * 1280 := by
  rcases hx with h | h
  · exact (backlog_lt_of_F A.small x h).le
  · rw [backlog_σ, show DaiSim.σ x.1 = [] from h]; simp [DaiChain.backlog]

/-- The truncated hitting times of `Idle`, bounded. -/
theorem hit_bound (hA : 1280 * A.mean < 128) : ∀ n x,
    hit (kernel A) Idle n x ≤ (backlog x.1 : ℝ) / ε A +
      ((128 * 1280 : ℕ) + (128 * 1280 + 1280 * K * (128 * 1280 : ℕ)) / ε A) / A.p 0 ^ (128 * 1280) :=
  hit_le_of_reach (kernel A) Idle (driftG A hA) (fun _ h => Or.inr h)
    (pow_pos (DaiRecurrent.p0_pos A hA) _)
    (fun x hx => drain A hA _ x (backlog_le_of_G A hx))
    (fun x hx => by
      have h1 := applyN_le A (128 * 1280) x
      have h2 : (backlog x.1 : ℝ) ≤ 128 * 1280 := by exact_mod_cast backlog_le_of_G A hx
      push_cast at h1 ⊢; linarith)

end Chain

/-- From every state the engine empties in bounded expected time. -/
theorem hit_idle_le {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    ∃ W : State K → ℝ, ∀ n x, hit (kernel A) Idle n x ≤ W x :=
  ⟨_, hit_bound A hA⟩

/-- From every machine with an empty engine the expected time until it is
empty again is bounded by one constant. -/
theorem return_idle {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    ∃ C : ℝ, ∀ n x, Idle x → 1 + (kernel A).apply (hit (kernel A) Idle n) x ≤ C := by
  set M : ℝ := ((128 * 1280 : ℕ) + (128 * 1280 + 1280 * K * (128 * 1280 : ℕ)) / ε A) /
    A.p 0 ^ (128 * 1280)
  have hε : 0 < ε A := (DaiStable.drift A hA).pos
  refine ⟨1 + (1 / ε A) * (1280 * K) + M, fun n x hx => ?_⟩
  have h1 : ∀ y, hit (kernel A) Idle n y ≤ (1 / ε A) * (backlog y.1 : ℝ) + M := fun y => by
    have := hit_bound A hA n y
    rw [one_div, ← div_eq_inv_mul]; exact this
  have h2 := (kernel A).apply_mono h1 x
  rw [Kernel.apply_add, Kernel.apply_const, Kernel.apply_const_mul] at h2
  have h3 := applyN_le A 1 x
  simp only [Kernel.applyN] at h3
  have h0 : (backlog x.1 : ℝ) = 0 := by
    rw [backlog_σ, show DaiSim.σ x.1 = [] from hx]; simp [DaiChain.backlog]
  have h4 : (1 / ε A) * (kernel A).apply (fun y => (backlog y.1 : ℝ)) x ≤ (1 / ε A) * (1280 * K) :=
    mul_le_mul_of_nonneg_left (by push_cast at h3; linarith) (by positivity)
  linarith

end DaiProgram
end Papers
end SerqLang
