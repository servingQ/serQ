/-
# Bari et al., Theorem 2, on the program's own chain

The machine chain of `bari_rad.sq` (`BariStable.kernel`) never revisits a
state: it keeps the clock and every ended session. What recurs is the
engine being empty, `σ m = []`, and every machine with an empty engine
behaves alike: its job list follows `BariChain`'s chain from `[]`
(`BariSim.simulation`). Below capacity, on the machine chain itself:

* the engine empties in bounded expected time from every state
  (`hit_idle_le`);
* it empties again in expected time bounded by one constant from every
  machine whose engine is empty (`return_idle`): the set of empty machines
  is a positive recurrent atom.
-/
import Serq.Papers.BariSim
import Serq.Papers.BariRecurrent

namespace SerqLang
namespace Papers
namespace BariProgram

open Foster BariStable

/-- The engine of the machine holds no job. -/
def Idle (x : State) : Prop := BariSim.σ x.1 = []

instance : DecidablePred Idle := fun x => by unfold Idle; infer_instance

/-! ### The machine's job list -/

/-- The state after a slot with the arrivals of outcome `o`. -/
def nxtS {N : ℕ} (A : Arrivals N) (x : State) (o : ℕ) : State :=
  ⟨slot (A.arr o) x.1, Reach.slot _ (A.fits o) x.2⟩

theorem apply_k {N : ℕ} (A : Arrivals N) (V : State → ℝ) (x : State) :
    (kernel A).apply V x = ∑ o ∈ Finset.range (N + 1), A.p o * V (nxtS A x o) :=
  Kernel.apply_ofOutcomes N A.p A.nonneg A.sum_one (nxtS A) V x

/-- One slot on the job list. -/
theorem σ_nxt {N : ℕ} (A : Arrivals N) (x : State) (o : ℕ) :
    BariSim.σ (nxtS A x o).1 = BariChain.absSlot (A.arr o) (BariSim.σ x.1) :=
  BariSim.simulation x (A.arr o) (A.fits o)

/-- The job list of every machine the chain reaches is one `BariChain` can hold. -/
theorem good_σ {m : Exec.Machine} (h : Reach m) : BariRecurrent.Good (BariSim.σ m) := by
  induction h with
  | empty => rw [BariSim.σ_empty]; intro j hj; cases hj
  | @slot m rs hf hm ih =>
    have e : BariSim.σ (slot rs m) = BariChain.absSlot rs (BariSim.σ m) :=
      BariSim.simulation ⟨m, hm⟩ rs hf
    rw [e]
    exact BariRecurrent.good_absSlot rs _ ih hf

/-- The machine's backlog is its job list's. -/
theorem backlog_eq (m : Exec.Machine) : backlog m = BariChain.backlog (BariSim.σ m) := by
  simp [backlog, Slot.backlog, BariChain.backlog, BariSim.σ, List.map_map, Function.comp_def]

/-- A slot changes the backlog by its arrivals less its batch. -/
theorem backlog_nxt {N : ℕ} (A : Arrivals N) (x : State) (o : ℕ) :
    backlog (nxtS A x o).1 + BariRecurrent.sS (BariSim.σ x.1) =
      backlog x.1 + ((A.arr o).map fun r => r.1 + r.2).sum := by
  rw [backlog_eq, backlog_eq, σ_nxt]
  exact BariRecurrent.absSlot_backlog _ _

theorem V_nxt_le {N : ℕ} (A : Arrivals N) (x : State) (o : ℕ) :
    (backlog (nxtS A x o).1 : ℝ) ≤ backlog x.1 + 15360000 := by
  have h1 := backlog_nxt A x o
  have h2 := BariRecurrent.work_le (A.arr o) (A.fits o)
  have h3 := A.small o
  have : backlog (nxtS A x o).1 ≤ backlog x.1 + 15360000 := by omega
  exact_mod_cast this

/-- An idle engine is in `F`: no job, so its batch, if any, is empty. -/
theorem F_of_idle (x : State) (hx : Idle x) : F x := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  have hj : x.1.jobs = [] := by unfold Idle BariSim.σ at hx; exact List.map_eq_nil_iff.mp hx
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · exact Or.inl hie
  · right
    have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    obtain ⟨hb1, -⟩ := hB.busy hbusy
    rw [hb1]
    have h0 : Exec.tokSum x.1.iter = 0 := by
      rw [← Exec.sum_shareOf x.1.sess.size x.1.iter hB.iterOwn]
      refine Finset.sum_eq_zero fun i hi => ?_
      have hi' := Finset.mem_range.mp hi
      have hs := hB.share i hi'
      have hnj : Slot.isJob (g.c i) = false := by
        cases h : Slot.isJob (g.c i)
        · rfl
        · obtain ⟨j, hj', -⟩ := hB.jobsP i hi' h
          rw [hj] at hj'; cases hj'
      simp only [hnj, Bool.false_eq_true, ↓reduceIte] at hs
      omega
    omega

theorem term_le {N : ℕ} (A : Arrivals N) (f : State → ℝ) (hf : ∀ y, 0 ≤ f y) (x : State)
    {o : ℕ} (hoN : o ≤ N) : A.p o * f (nxtS A x o) ≤ (kernel A).apply f x :=
  Kernel.le_apply_ofOutcomes N A.p A.nonneg A.sum_one (nxtS A) f hf x hoN

/-- From a machine whose job list carries at most `n` tokens, `n` slots
without arrivals empty the engine. -/
theorem reach_idle {N : ℕ} (A : Arrivals N) {o0 : ℕ} (ho0 : o0 ≤ N ∧ 0 < A.p o0)
    (harr : A.arr o0 = []) :
    ∀ n (x : State), backlog x.1 ≤ n → A.p o0 ^ n ≤ reach (kernel A) Idle n x
  | 0, x, hx => by
    have hx0 : Idle x := by
      by_contra hne
      have := BariRecurrent.backlog_pos _ (good_σ x.2) hne
      rw [backlog_eq] at hx
      omega
    simp [reach, hx0]
  | n + 1, x, hx => by
    have hp1 : A.p o0 ≤ 1 := by
      rw [← A.sum_one]
      exact Finset.single_le_sum (f := A.p) (fun o _ => A.nonneg o)
        (Finset.mem_range.mpr (Nat.lt_succ_of_le ho0.1))
    by_cases hxi : Idle x
    · simp only [reach, hxi, ↓reduceIte]
      exact pow_le_one₀ (A.nonneg o0) hp1
    · simp only [reach, hxi, ↓reduceIte]
      have hb : backlog (nxtS A x o0).1 ≤ n := by
        have h1 := backlog_nxt A x o0
        rw [harr] at h1
        have h2 := (BariRecurrent.sS_cases _ (good_σ x.2) hxi).1
        simp only [List.map_nil, List.sum_nil] at h1
        omega
      have ih := reach_idle A ho0 harr n (nxtS A x o0) hb
      calc A.p o0 ^ (n + 1) = A.p o0 * A.p o0 ^ n := by ring
        _ ≤ A.p o0 * reach (kernel A) Idle n (nxtS A x o0) :=
          mul_le_mul_of_nonneg_left ih (A.nonneg o0)
        _ ≤ _ := term_le A _ (reach_nonneg _ _ n) x ho0.1

theorem applyN_le {N : ℕ} (A : Arrivals N) : ∀ n (x : State),
    (kernel A).applyN n (fun y => (backlog y.1 : ℝ)) x ≤ backlog x.1 + 15360000 * n
  | 0, x => by simp [Kernel.applyN]
  | n + 1, x => by
    show (kernel A).apply ((kernel A).applyN n _) x ≤ _
    rw [apply_k]
    calc ∑ o ∈ Finset.range (N + 1), A.p o * (kernel A).applyN n (fun y => (backlog y.1 : ℝ)) (nxtS A x o)
        ≤ ∑ o ∈ Finset.range (N + 1), A.p o * ((backlog x.1 : ℝ) + 15360000 * ((n : ℝ) + 1)) := by
          refine Finset.sum_le_sum fun o _ => mul_le_mul_of_nonneg_left ?_ (A.nonneg o)
          have := applyN_le A n (nxtS A x o)
          have := V_nxt_le A x o
          linarith
      _ = backlog x.1 + 15360000 * ((n + 1 : ℕ) : ℝ) := by
          rw [← Finset.sum_mul, A.sum_one]; push_cast; ring

/-- The bound on the hitting time, with its constant named. -/
theorem hit_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    ∃ c : ℝ, ∀ n x, hit (kernel A) Idle n x ≤ backlog x.1 / ε A + c := by
  obtain ⟨o0, ho0, harr⟩ := BariRecurrent.exists_empty A hA
  have hδ : 0 < A.p o0 ^ 65536 := pow_pos ho0.2 _
  refine ⟨_, hit_le_of_reach (kernel A) Idle (fun _ => Kernel.integrable_ofOutcomes _ _ _ _ _ _) (drift A hA) F_of_idle (L := 65536)
    (B := 128 * 512 + 15360000 * 65536) hδ
    (fun x hx => reach_idle A ho0 harr 65536 x (le_trans (backlog_lt_of_F x hx).le (by norm_num)))
    (fun x hx => ?_)⟩
  have h1 := applyN_le A 65536 x
  have h2 : (backlog x.1 : ℝ) ≤ 128 * 512 := by exact_mod_cast (backlog_lt_of_F x hx).le
  push_cast at h1
  linarith

/-! ### The theorems -/

/-- From every state the engine empties in bounded expected time. -/
theorem hit_idle_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    ∃ W : State → ℝ, ∀ n x, hit (kernel A) Idle n x ≤ W x := by
  obtain ⟨c, hc⟩ := hit_le A hA
  exact ⟨_, hc⟩

/-- From every machine with an empty engine the expected time until it is
empty again is bounded by one constant. -/
theorem return_idle {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    ∃ C : ℝ, ∀ n x, Idle x → 1 + (kernel A).apply (hit (kernel A) Idle n) x ≤ C := by
  obtain ⟨c, hc⟩ := hit_le A hA
  refine ⟨1 + (15360000 / ε A + c), fun n x hx => ?_⟩
  have hε : 0 < ε A := by unfold ε; linarith
  have hs : BariRecurrent.sS (BariSim.σ x.1) = 0 := by
    unfold Idle at hx; rw [hx]; rfl
  suffices h : (kernel A).apply (hit (kernel A) Idle n) x ≤ 15360000 / ε A + c by linarith
  rw [apply_k]
  calc ∑ o ∈ Finset.range (N + 1), A.p o * hit (kernel A) Idle n (nxtS A x o)
      ≤ ∑ o ∈ Finset.range (N + 1), A.p o * (15360000 / ε A + c) := by
        refine Finset.sum_le_sum fun o _ => mul_le_mul_of_nonneg_left ?_ (A.nonneg o)
        refine le_trans (hc n _) ?_
        have h1 := backlog_nxt A x o
        rw [hs] at h1
        have h2 : backlog x.1 = 0 := by
          rw [backlog_eq]; unfold Idle at hx; rw [hx]; rfl
        have h3 := BariRecurrent.work_le (A.arr o) (A.fits o)
        have h4 := A.small o
        have h5 : (backlog (nxtS A x o).1 : ℝ) ≤ 15360000 := by
          have : backlog (nxtS A x o).1 ≤ 15360000 := by omega
          exact_mod_cast this
        have := div_le_div_of_nonneg_right h5 hε.le
        linarith
    _ = 15360000 / ε A + c := by rw [← Finset.sum_mul, A.sum_one, one_mul]

end BariProgram
end Papers
end SerqLang
