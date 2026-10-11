import Mathlib.Tactic
import Mathlib.Data.List.Sort
import Mathlib.Algebra.Order.BigOperators.Group.Finset
import Mathlib.Algebra.BigOperators.Intervals

/-!
# The mathematics of Kong et al. §3

Kong, Qi, Ye, Zhou, "Geometry-Aware Online Scheduling for LLM Serving"
(arXiv 2606.22327), §3, stated and proved independently of the serving
system: requests `0, …, n-1` all arrive at time `0`, request `i` has prompt
`s i` and output `o i`, and memory is `M`.

* `prefixTotal_eq`: the sum over `k` of the `k` smallest values is the total
  plus the sum of `min` over unordered pairs (`pmin`);
* `sum_prec`: the same pairwise sum, written as the sum over each request of
  the volumes queued before it in SVF's order (by volume, ties by serial);
* `opt_lower_bound` (Proposition 3.1): every feasible schedule has
  `M · TEL ≥ Σ_k (sum of the k smallest volumes)`;
* `tel_ge_out`, `competitive` (Theorem 3.2, as a composition of its bounds)
  and `vol_bound`, the step from peaks to volumes.
-/

namespace SerqLang
namespace Papers
namespace KongMath

open Finset

/-- `Σ_k Σ_{i ≤ k} v_(i)` of a list sorted ascending. -/
def prefixSums : List ℕ → ℕ
  | [] => 0
  | v :: vs => (v :: vs).sum + prefixSums vs

/-- The values sorted descending `vₙ ≥ … ≥ v₁`, `prefixSums` adds the suffix
sums, which are the sums of the `k` smallest: `Σ_k (v₁ + … + v_k)`.

This differs from the definition in the task statement, which reversed the
sorted list: on an ascending list `prefixSums` is `Σ_k k · v_k`, the sum of
the `k` *largest*, and Proposition 3.1 is false for it (two requests with
`s = 0`, `o = 1, 2`, both started at `0`, `M = 2`: volumes `1, 3`,
`M · TEL = 6`, the reversed form gives `7`). The Rust `Agg::PrefixTotal`
computes this one. -/
def prefixTotal (l : List ℕ) : ℕ := prefixSums (l.mergeSort (fun a b => decide (a ≥ b)))

/-! ### Pairwise minima -/

/-- The sum of `min` over unordered pairs of positions. -/
def pmin : List ℕ → ℕ
  | [] => 0
  | a :: l => (l.map (min a)).sum + pmin l

/-- The pairwise sum does not depend on the order. -/
theorem pmin_perm {l₁ l₂ : List ℕ} (h : l₁.Perm l₂) : pmin l₁ = pmin l₂ := by
  induction h with
  | nil => rfl
  | cons x _ ih => simp only [pmin, ih, (List.Perm.map _ ‹_›).sum_eq]
  | swap x y l =>
    simp only [pmin, List.map_cons, List.sum_cons, min_comm x y]
    omega
  | trans _ _ ih₁ ih₂ => exact ih₁.trans ih₂

/-- On a descending list, `prefixSums` is the total plus the pairwise minima. -/
theorem prefixSums_desc :
    ∀ l : List ℕ, l.Pairwise (fun a b => a ≥ b) → prefixSums l = l.sum + pmin l
  | [], _ => rfl
  | v :: vs, h => by
    rw [List.pairwise_cons] at h
    have hm : vs.map (min v) = vs := by
      conv_rhs => rw [← List.map_id vs]
      exact List.map_congr_left fun w hw => min_eq_right (h.1 w hw)
    simp only [prefixSums, pmin, hm, prefixSums_desc vs h.2, List.sum_cons]

/-- The sum of the `k` smallest values, over `k`, is the total plus the
pairwise minima. -/
theorem prefixTotal_eq (l : List ℕ) : prefixTotal l = l.sum + pmin l := by
  have hp := List.mergeSort_perm l (fun a b => decide (a ≥ b))
  have hs := List.pairwise_mergeSort (le := fun a b : ℕ => decide (a ≥ b))
    (fun a b c hab hbc => by simp only [decide_eq_true_eq] at *; omega)
    (fun a b => by simp only [Bool.or_eq_true, decide_eq_true_eq]; omega) l
  unfold prefixTotal
  rw [prefixSums_desc _ (by simpa using hs), hp.sum_eq, pmin_perm hp]

/-! ### Pairs in SVF's order -/

/-- `i` is queued before `j`: smaller volume, ties by serial number. -/
def prec (v : ℕ → ℕ) (i j : ℕ) : Prop := v i < v j ∨ (v i = v j ∧ i < j)

instance (v : ℕ → ℕ) (i j : ℕ) : Decidable (prec v i j) := by
  unfold prec; infer_instance

/-- A sum over `List.range` is a sum over `Finset.range`. -/
theorem list_sum_range (f : ℕ → ℕ) (n : ℕ) :
    ((List.range n).map f).sum = ∑ i ∈ range n, f i := by
  induction n with
  | zero => rfl
  | succ n ih => simp [List.range_succ, Finset.sum_range_succ, ih]

/-- Summing, over each request, the volumes queued before it counts each
unordered pair once, at its smaller volume. -/
theorem sum_prec (v : ℕ → ℕ) (n : ℕ) :
    (∑ j ∈ Finset.range n, ∑ i ∈ (Finset.range n).filter (fun i => prec v i j), v i)
      = pmin ((List.range n).map v) := by
  simp only [Finset.sum_filter]
  induction n with
  | zero => rfl
  | succ n ih =>
    have hperm : ((List.range (n + 1)).map v).Perm (v n :: (List.range n).map v) := by
      rw [List.range_succ, List.map_append]
      exact List.perm_append_comm
    rw [pmin_perm hperm, pmin, ← ih, List.map_map, list_sum_range]
    simp only [Finset.sum_range_succ, Finset.sum_add_distrib]
    have hnn : ¬ prec v n n := by simp [prec]
    have hpair : ∀ j ∈ range n,
        ((if prec v n j then v n else 0) + if prec v j n then v j else 0)
          = (min (v n) ∘ v) j := by
      intro j hj
      have hj := Finset.mem_range.mp hj
      rw [Function.comp_apply]
      rcases lt_trichotomy (v n) (v j) with h | h | h
      · rw [if_pos (show prec v n j from Or.inl h), if_neg (by unfold prec; omega),
          min_eq_left h.le, add_zero]
      · rw [if_neg (by unfold prec; omega), if_pos (show prec v j n from Or.inr ⟨h.symm, hj⟩),
          h, zero_add, min_self]
      · rw [if_neg (by unfold prec; omega), if_pos (show prec v j n from Or.inl h),
          min_eq_right h.le, zero_add]
    rw [if_neg hnn, ← Finset.sum_congr rfl hpair, Finset.sum_add_distrib]
    ring

/-! ### Proposition 3.1 -/

/-- The memory in use at step `t`: request `i` holds `s i + t - x i` at every
step `t` with `x i < t ≤ x i + o i`. -/
def mem (s o x : ℕ → ℕ) (n t : ℕ) : ℕ :=
  ∑ i ∈ (Finset.range n).filter (fun i => x i < t ∧ t ≤ x i + o i), (s i + t - x i)

/-- A schedule never holds more than `M`. -/
def Feasible (s o x : ℕ → ℕ) (n M : ℕ) : Prop := ∀ t, mem s o x n t ≤ M

/-- The total end-to-end latency: the sum of completion times. -/
def TEL (o x : ℕ → ℕ) (n : ℕ) : ℕ := ∑ i ∈ Finset.range n, (x i + o i)

/-- The memory-time of a request: `s·o + o(o+1)/2`. -/
def vol (s o : ℕ → ℕ) (i : ℕ) : ℕ := s i * o i + (o i * o i + o i) / 2

/-- A request's memory over its run is its volume. -/
theorem memtime (s o x : ℕ) :
    ∑ t ∈ Finset.Ioc x (x + o), (s + t - x) = s * o + (o * o + o) / 2 := by
  induction o with
  | zero => simp
  | succ o ih =>
    rw [← add_assoc, Finset.sum_Ioc_succ_top (by omega), ih]
    have h : (o + 1) * (o + 1) + (o + 1) = (o * o + o) + (o + 1) * 2 := by ring
    rw [h, Nat.add_mul_div_right _ _ two_pos]
    have : s + (x + o + 1) - x = s + o + 1 := by omega
    rw [this]
    ring

/-- The requests complete by `c` used memory only in steps `1, …, c`, so their
volumes are at most `M · c`. -/
theorem done_le (s o x : ℕ → ℕ) (n M : ℕ) (h : Feasible s o x n M) (c : ℕ) :
    ∑ i ∈ (Finset.range n).filter (fun i => x i + o i ≤ c), vol s o i ≤ M * c := by
  have hmem : ∑ t ∈ Finset.Ioc 0 c, mem s o x n t ≤ M * c := by
    have := Finset.sum_le_card_nsmul (Finset.Ioc 0 c) (mem s o x n) M (fun t _ => h t)
    simpa [mul_comm] using this
  refine le_trans ?_ hmem
  simp only [mem, Finset.sum_filter]
  rw [Finset.sum_comm]
  refine Finset.sum_le_sum fun i _ => ?_
  split_ifs with hi
  · rw [← Finset.sum_filter]
    have hset : (Finset.Ioc 0 c).filter (fun t => x i < t ∧ t ≤ x i + o i)
        = Finset.Ioc (x i) (x i + o i) := by
      ext t; simp only [Finset.mem_filter, Finset.mem_Ioc]; omega
    rw [hset, memtime]
    rfl
  · exact Nat.zero_le _

/-- The filtered sum over a list, as a sum of `if`. -/
theorem sum_filter_map (p : ℕ → Prop) [DecidablePred p] (f : ℕ → ℕ) (l : List ℕ) :
    ((l.filter (fun i => decide (p i))).map f).sum = (l.map fun i => if p i then f i else 0).sum := by
  induction l with
  | nil => rfl
  | cons a l ih => by_cases ha : p a <;> simp [ha, ih]

/-- The exchange step: a list of requests sorted by completion, latest first,
whose every completion prefix fits in memory, has
`M · Σ C ≥ total + pairwise minima` of its volumes. -/
theorem lower_bound_list (V C : ℕ → ℕ) (M : ℕ) :
    ∀ L : List ℕ, L.Pairwise (fun i j => C i ≥ C j) →
      (∀ c, ((L.filter (fun i => decide (C i ≤ c))).map V).sum ≤ M * c) →
      (L.map V).sum + pmin (L.map V) ≤ M * (L.map C).sum
  | [], _, _ => by simp [pmin]
  | a :: L, hs, hd => by
    rw [List.pairwise_cons] at hs
    have hall : ((a :: L).map V).sum ≤ M * C a := by
      have := hd (C a)
      rwa [List.filter_eq_self.mpr] at this
      intro i hi
      rcases List.mem_cons.mp hi with rfl | hi
      · simp
      · simpa using hs.1 i hi
    have hrest : ∀ c, ((L.filter (fun i => decide (C i ≤ c))).map V).sum ≤ M * c := by
      intro c
      refine le_trans ?_ (hd c)
      by_cases hc : C a ≤ c <;> simp [hc]
    have ih := lower_bound_list V C M L hs.2 hrest
    have hmin : (List.map (min (V a)) (L.map V)).sum ≤ (L.map V).sum := by
      rw [List.map_map]
      exact List.sum_le_sum fun i _ => min_le_right _ _
    simp only [List.map_cons, List.sum_cons, pmin] at hall ⊢
    nlinarith

/-- Proposition 3.1: every feasible schedule has
`M · TEL ≥ Σ_k (sum of the k smallest volumes)`. -/
theorem opt_lower_bound (s o x : ℕ → ℕ) (n M : ℕ) (h : Feasible s o x n M) :
    prefixTotal ((List.range n).map (vol s o)) ≤ M * TEL o x n := by
  let C : ℕ → ℕ := fun i => x i + o i
  let L := (List.range n).mergeSort (fun i j => decide (C i ≥ C j))
  have hp : L.Perm (List.range n) := List.mergeSort_perm _ _
  have hs : L.Pairwise (fun i j => C i ≥ C j) := by
    have := List.pairwise_mergeSort (le := fun i j : ℕ => decide (C i ≥ C j))
      (fun a b c hab hbc => by simp only [decide_eq_true_eq] at *; omega)
      (fun a b => by simp only [Bool.or_eq_true, decide_eq_true_eq]; omega) (List.range n)
    simpa using this
  have hd : ∀ c, ((L.filter (fun i => decide (C i ≤ c))).map (vol s o)).sum ≤ M * c := by
    intro c
    rw [sum_filter_map (fun i => C i ≤ c), (hp.map _).sum_eq, list_sum_range,
      ← Finset.sum_filter]
    exact done_le s o x n M h c
  have := lower_bound_list (vol s o) C M L hs hd
  rw [prefixTotal_eq, ← (hp.map _).sum_eq, ← pmin_perm (hp.map _)]
  unfold TEL
  rw [← list_sum_range, ← (hp.map _).sum_eq]
  exact this

/-! ### Theorem 3.2 -/

/-- Every request decodes its output after time `0`. -/
theorem tel_ge_out (o x : ℕ → ℕ) (n : ℕ) : (∑ i ∈ Finset.range n, o i) ≤ TEL o x n :=
  Finset.sum_le_sum fun _ _ => Nat.le_add_left _ _

/-- The competitive ratio `(3M − P)/(M − P) = 1 + 2/(1 − α)`, `α = P/M`, from
SVF's bound on its waiting (`hcl`), the lower bound (`hpt`) and the decode
time every schedule pays (`hso`). -/
theorem competitive (M P telSvf sumO sumV pt telOpt : ℕ) (_hP : P < M)
    (hcl : (M - P) * (telSvf - sumO) ≤ 2 * (pt - sumV)) (hlo : sumO ≤ telSvf)
    (hpt : pt ≤ M * telOpt) (hso : sumO ≤ telOpt) :
    (M - P) * telSvf ≤ (3 * M - P) * telOpt := by
  obtain ⟨d, rfl⟩ := Nat.exists_eq_add_of_le hlo
  obtain ⟨a, ha⟩ := Nat.exists_eq_add_of_le (Nat.le_of_lt ‹P < M›)
  have h3 : 3 * M - P = 2 * M + (M - P) := by omega
  have hmp : M - P = a := by omega
  rw [Nat.add_sub_cancel_left] at hcl
  rw [h3, hmp]
  rw [hmp] at hcl
  have h1 : a * d ≤ 2 * M * telOpt := by
    have : pt - sumV ≤ pt := Nat.sub_le _ _
    nlinarith
  have h2 : a * sumO ≤ a * telOpt := Nat.mul_le_mul_left _ hso
  nlinarith

/-- `p · o ≤ 2 · vol − o`, the step from peaks to volumes. -/
theorem vol_bound (s o : ℕ) : (s + o) * o + o ≤ 2 * (s * o + (o * o + o) / 2) := by
  have hev : 2 ∣ o * o + o := by
    have := Nat.even_mul_succ_self o
    rw [mul_add, mul_one] at this
    exact even_iff_two_dvd.mp this
  have h := Nat.mul_div_cancel' hev
  nlinarith

end KongMath
end Papers
end SerqLang
