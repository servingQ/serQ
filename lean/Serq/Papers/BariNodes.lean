/-
# Bari et al. over `g` nodes, with the random planner

The paper's planner sends each request to one of `g` nodes, uniformly and
independently of the others. Here `g` nodes each run `bari_rad.sq`
(`Serq/Papers/BariStable.lean`), and a slot is one iteration of every node:
the arrivals `A.arr o` of outcome `o` are drawn as there, each is routed to a
node uniformly (`r : Fin n → Fin g`, probability `A.p o / g ^ n`), and node
`i` runs `BariStable.slot` on the requests routed to it, in their order
(`route`). The nodes' slots are synchronised: that is this model's, not the
paper's.

A request reaches node `i` with probability `1 / g` (`mean_route`), so the
work node `i` receives has mean `load / g`. Outside `F` at node `i` its
backlog drifts down by `128 − load / g` (`drift`), and Foster's criterion
bounds the expected time until node `i`'s batch is not full by
`backlog_i / (128 − load / g)` (`hitTime_le`): every node is stable when
`load < 128 g`, the capacity of `g` nodes. Summed over the nodes, the total
backlog drifts down by `128 g − load` while every batch is full
(`drift_sum`): Theorem 1 for `g` nodes is the sum of the per-node bound.
-/
import Serq.Papers.BariStable

namespace SerqLang

namespace Papers
namespace BariNodes

open Foster BariStable

/-! ### A kernel over a finite type of outcomes -/

/-- The kernel of a random choice `k` of a finite type, with probabilities
`p k`. -/
noncomputable def ofFintype {α ι : Type*} [Fintype ι] (p : ι → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k, p k = 1) (f : α → ι → α) : Kernel α ι where
  p _ := p
  next := f
  nonneg _ := hp0
  sum_one _ := hp1 ▸ hasSum_fintype p

/-- The expectation under `ofFintype` is the weighted sum over the choices. -/
theorem apply_ofFintype {α ι : Type*} [Fintype ι] (p : ι → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k, p k = 1) (f : α → ι → α) (V : α → ℝ) (x : α) :
    (ofFintype p hp0 hp1 f).apply V x = ∑ k, p k * V (f x k) :=
  tsum_fintype _

/-- Finitely many outcomes: every expectation converges. -/
theorem integrable_ofFintype {α ι : Type*} [Fintype ι] (p : ι → ℝ) (hp0 : ∀ k, 0 ≤ p k)
    (hp1 : ∑ k, p k = 1) (f : α → ι → α) (V : α → ℝ) : (ofFintype p hp0 hp1 f).Integrable V :=
  fun _ => (hasSum_fintype _).summable

/-! ### The random planner -/

/-- The requests of `l` that the routing `r` sends to node `i`, in order. -/
def route {g : ℕ} (l : List (ℕ × ℕ)) (r : Fin l.length → Fin g) (i : Fin g) : List (ℕ × ℕ) :=
  ((List.finRange l.length).filter fun k => r k = i).map l.get

theorem route_length {g : ℕ} (l : List (ℕ × ℕ)) (r : Fin l.length → Fin g) (i : Fin g) :
    (route l r i).length ≤ l.length := by
  unfold route
  rw [List.length_map]
  exact (List.length_filter_le _ _).trans (by simp)

theorem mem_route {g : ℕ} {l : List (ℕ × ℕ)} {r : Fin l.length → Fin g} {i : Fin g} {q : ℕ × ℕ}
    (h : q ∈ route l r i) : q ∈ l := by
  obtain ⟨k, -, rfl⟩ := List.mem_map.mp h
  exact List.get_mem l k

/-- The tokens a list of requests brings. -/
def work (l : List (ℕ × ℕ)) : ℕ := (l.map fun q => q.1 + q.2).sum

theorem sum_filter_map {β : Type*} (L : List β) (P : β → Bool) (h : β → ℕ) :
    ((L.filter P).map h).sum = (L.map fun k => if P k then h k else 0).sum := by
  induction L with
  | nil => rfl
  | cons a L ih => by_cases hp : P a <;> simp [hp, ih]

/-- Node `i`'s work is the work of the requests routed to it. -/
theorem work_route {g : ℕ} (l : List (ℕ × ℕ)) (r : Fin l.length → Fin g) (i : Fin g) :
    work (route l r i) = ∑ k, if r k = i then (l.get k).1 + (l.get k).2 else 0 := by
  unfold work route
  rw [List.map_map, sum_filter_map, Fin.sum_univ_def]
  simp

/-- A request is routed to node `i` by `g ^ n / g` of the `g ^ n` routings. -/
theorem card_route {n g : ℕ} [NeZero g] (k : Fin n) (i : Fin g) :
    (∑ r : Fin n → Fin g, if r k = i then (1 : ℝ) else 0) = (g : ℝ) ^ n / g := by
  have hsym : ∀ j : Fin g, (∑ r : Fin n → Fin g, if r k = j then (1 : ℝ) else 0) =
      ∑ r : Fin n → Fin g, if r k = i then (1 : ℝ) else 0 := by
    intro j
    refine Fintype.sum_equiv (Equiv.arrowCongr (Equiv.refl (Fin n)) (Equiv.swap j i)) _ _ fun r => ?_
    simp [Equiv.arrowCongr_apply, Equiv.swap_apply_eq_iff]
  have htot : ∑ j : Fin g, (∑ r : Fin n → Fin g, if r k = j then (1 : ℝ) else 0) = (g : ℝ) ^ n := by
    rw [Finset.sum_comm]
    simp
  rw [Finset.sum_congr rfl fun j _ => hsym j] at htot
  simp only [Finset.sum_const, Finset.card_univ, Fintype.card_fin, nsmul_eq_mul] at htot
  have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
  rw [← htot]
  field_simp

/-- Uniform routing: the work node `i` receives has mean `work / g`. -/
theorem mean_route {g : ℕ} [NeZero g] (l : List (ℕ × ℕ)) (i : Fin g) :
    ∑ r : Fin l.length → Fin g, (work (route l r i) : ℝ) / (g : ℝ) ^ l.length = work l / g := by
  have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
  have hgn : (g : ℝ) ^ l.length ≠ 0 := pow_ne_zero _ hg
  simp only [work_route, Nat.cast_sum, Nat.cast_ite, Nat.cast_zero]
  rw [← Finset.sum_div, Finset.sum_comm]
  have : ∀ k : Fin l.length, (∑ r : Fin l.length → Fin g,
      if r k = i then (((l.get k).1 + (l.get k).2 : ℕ) : ℝ) else 0) =
      (((l.get k).1 + (l.get k).2 : ℕ) : ℝ) * ((g : ℝ) ^ l.length / g) := by
    intro k
    rw [← card_route k i, Finset.mul_sum]
    refine Finset.sum_congr rfl fun r _ => ?_
    split_ifs <;> simp
  rw [Finset.sum_congr rfl fun k _ => this k, ← Finset.sum_mul]
  have hw : (∑ k : Fin l.length, (((l.get k).1 + (l.get k).2 : ℕ) : ℝ)) = (work l : ℝ) := by
    have h : (∑ k : Fin l.length, ((l.get k).1 + (l.get k).2)) = work l := by
      rw [Fin.sum_univ_def]
      unfold work
      conv_rhs => rw [← List.map_get_finRange l]
      rw [List.map_map]; rfl
    exact_mod_cast h
  rw [hw]
  field_simp

/-! ### The chain of `g` nodes -/

/-- The chain's states: every node's machine. -/
def State (g : ℕ) : Type := Fin g → BariStable.State

/-- An outcome of a slot: the arrivals `o`, and the node of each. -/
abbrev Outcome {N : ℕ} (A : Arrivals N) (g : ℕ) : Type :=
  Σ o : Fin (N + 1), (Fin (A.arr o).length → Fin g)

/-- Its probability: `A.p o`, and each routing equally likely. -/
noncomputable def prob {N : ℕ} (A : Arrivals N) (g : ℕ) (ω : Outcome A g) : ℝ :=
  A.p ω.1 / (g : ℝ) ^ (A.arr ω.1).length

/-- One slot: node `i` runs a slot with the requests routed to it. -/
def step {N g : ℕ} (A : Arrivals N) (x : State g) (ω : Outcome A g) : State g := fun i =>
  ⟨slot (route (A.arr ω.1) ω.2 i) (x i).1,
    Reach.slot _ (fun _ hq => A.fits _ _ (mem_route hq)) (x i).2⟩

theorem prob_nonneg {N g : ℕ} (A : Arrivals N) (ω : Outcome A g) : 0 ≤ prob A g ω :=
  div_nonneg (A.nonneg _) (pow_nonneg (Nat.cast_nonneg _) _)

theorem prob_sum {N g : ℕ} [NeZero g] (A : Arrivals N) : ∑ ω, prob A g ω = 1 := by
  have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
  rw [Fintype.sum_sigma, ← A.sum_one, ← Fin.sum_univ_eq_sum_range (fun o => A.p o) (N + 1)]
  refine Finset.sum_congr rfl fun o _ => ?_
  simp only [prob, Finset.sum_const, Finset.card_univ, Fintype.card_fun, Fintype.card_fin,
    nsmul_eq_mul, Nat.cast_pow]
  field_simp

/-- The chain: arrivals drawn from `A`, each routed to a node uniformly. -/
noncomputable def kernel {N g : ℕ} [NeZero g] (A : Arrivals N) : Kernel (State g) (Outcome A g) :=
  ofFintype (prob A g) (prob_nonneg A) (prob_sum A) (step A)

/-- Node `i` is idle, or its batch is not full. -/
def F {g : ℕ} (i : Fin g) (x : State g) : Prop := BariStable.F (x i)

instance {g : ℕ} (i : Fin g) : DecidablePred (F i) := fun x => by unfold F; infer_instance

/-- Node `i`'s backlog. -/
def V {g : ℕ} (i : Fin g) (x : State g) : ℝ := backlog (x i).1

/-- Node `i`'s drift: `ε = 128 − load / g`. -/
noncomputable def ε {N : ℕ} (A : Arrivals N) (g : ℕ) : ℝ := 128 - A.load / g

/-- Outside `F` at node `i`, its backlog moves by the work routed to it, of
mean `load / g`, less the 128 tokens of its full batch. -/
theorem apply_V {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) (x : State g) (hx : ¬ F i x) :
    (kernel A).apply (V i) x = V i x + A.load / g - 128 := by
  rw [kernel, apply_ofFintype, Fintype.sum_sigma]
  have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
  have hstep : ∀ (o : Fin (N + 1)) (r : Fin (A.arr o).length → Fin g),
      prob A g ⟨o, r⟩ * V i (step A x ⟨o, r⟩) =
        prob A g ⟨o, r⟩ * (V i x - 128) + A.p o * ((work (route (A.arr o) r i) : ℝ) / (g : ℝ) ^ (A.arr o).length) := by
    intro o r
    have h1 := backlog_slot (x i) hx (route (A.arr o) r i)
      (fun _ hq => A.fits _ _ (mem_route hq))
    have h1' : (backlog (slot (route (A.arr o) r i) (x i).1) : ℝ) + 128 =
        backlog (x i).1 + (work (route (A.arr o) r i) : ℝ) := by
      unfold work; exact_mod_cast h1
    have h2 : V i (step A x ⟨o, r⟩) = V i x + work (route (A.arr o) r i) - 128 := by
      unfold V step; simp only; linarith
    rw [h2, prob]; ring
  simp only [hstep, Finset.sum_add_distrib, ← Finset.sum_mul, ← Finset.mul_sum, mean_route]
  have hs : ∑ o : Fin (N + 1), ∑ r : Fin (A.arr o).length → Fin g, prob A g ⟨o, r⟩ = 1 := by
    rw [← prob_sum (g := g) A]; exact (Fintype.sum_sigma _).symm
  have hl : A.load / g = ∑ o : Fin (N + 1), A.p o * ((work (A.arr o) : ℝ) / g) := by
    rw [Arrivals.load, ← Fin.sum_univ_eq_sum_range (fun o => A.p o * A.work o) (N + 1),
      Finset.sum_div]
    refine Finset.sum_congr rfl fun o _ => ?_
    simp only [Arrivals.work, work]; ring
  rw [hs, hl]; ring

/-- Foster's drift condition at node `i`, below the capacity of `g` nodes. -/
theorem drift {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g) :
    Drift (kernel A) (F i) (V i) (ε A g) where
  nonneg _ := Nat.cast_nonneg _
  integrable := integrable_ofFintype _ _ _ _ _
  pos := by
    have hg : (0 : ℝ) < g := Nat.cast_pos.mpr (Nat.pos_of_ne_zero (NeZero.ne g))
    unfold ε; rw [sub_pos, div_lt_iff₀ hg]; linarith
  drift x hx := by rw [apply_V A i x hx, ε]; linarith

/-- Theorem 2 at node `i`: from every state, the expected number of slots
until node `i`'s batch is not full (or the node idle) is at most
`backlog_i / ε`. -/
theorem hitTime_le {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g)
    (x : State g) : ε A g * hitTime (kernel A) (F i) x ≤ backlog (x i).1 :=
  Foster.hitTime_le (drift A hA i) x

/-- … and it is finite: the truncated expectations converge to it. -/
theorem hit_tendsto {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g)
    (x : State g) :
    Filter.Tendsto (fun n => hit (kernel A) (F i) n x) Filter.atTop (nhds (hitTime (kernel A) (F i) x)) :=
  Foster.hit_tendsto (drift A hA i) x

/-- Theorem 2 at node `i`: from every state of `F i`, the expected return
time to `F i` is finite. -/
theorem returnTime_le {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g)
    (x : State g) (hx : F i x) :
    returnTime (kernel A) (F i) x ≤ 1 + (kernel A).apply (V i) x / ε A g :=
  Foster.returnTime_le_of_drift (drift A hA i) x hx

/-! ### The sum over the nodes -/

/-- Some node is idle or its batch is not full. -/
def Fsome {g : ℕ} (x : State g) : Prop := ∃ i, F i x

instance {g : ℕ} : DecidablePred (Fsome (g := g)) := fun x => by unfold Fsome; infer_instance

/-- The total backlog over the nodes. -/
def Vsum {g : ℕ} (x : State g) : ℝ := ∑ i, V i x

/-- Theorem 1 for `g` nodes is the sum of the per-node bound: while every
batch is full, the total backlog drifts down by `128 g − load`. -/
theorem drift_sum {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) :
    Drift (kernel (g := g) A) Fsome Vsum (128 * g - A.load) where
  nonneg _ := Finset.sum_nonneg fun _ _ => Nat.cast_nonneg _
  integrable := integrable_ofFintype _ _ _ _ _
  pos := by linarith
  drift x hx := by
    have hx' : ∀ i, ¬ F i x := fun i h => hx ⟨i, h⟩
    have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
    have : (kernel A).apply Vsum x = ∑ i, (kernel A).apply (V i) x := by
      simp only [kernel, apply_ofFintype, Vsum, Finset.mul_sum]
      exact Finset.sum_comm
    rw [this, Finset.sum_congr rfl fun i _ => apply_V A i x (hx' i)]
    simp only [Finset.sum_sub_distrib, Finset.sum_add_distrib, Finset.sum_const, Finset.card_univ,
      Fintype.card_fin, nsmul_eq_mul, Vsum]
    field_simp
    linarith

/-! ### Not vacuous -/

/-- A load above one node's capacity and below two nodes': one request of a
tile and one output token in every slot, 129 tokens. -/
example : ∃ A : Arrivals 0, 128 < A.load ∧ A.load < 128 * (2 : ℕ) :=
  ⟨⟨fun o => if o = 0 then 1 else 0,
      fun o => by split_ifs <;> norm_num,
      by simp,
      fun _ => [(128, 1)],
      fun _ => by simp,
      fun _ r hr => by simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩⟩,
    by simp [Arrivals.load, Arrivals.work]; norm_num⟩

end BariNodes
end Papers
end SerqLang
