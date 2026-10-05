/-
# Bari et al. over `g` nodes, with the random planner

The paper's planner sends each request to one of `g` nodes, uniformly and
independently of the others. Here `g` nodes each run `bari_rad.sq`
(`examples/papers/BariStable.lean`), and a slot is one iteration of every node:
the arrivals `A.arr o` of outcome `o` are drawn as there, each is routed to a
node uniformly (`r : Fin n → Fin g`, probability `A.p o / g ^ n`), and node
`i` runs `BariStable.slot` on the requests routed to it, in their order
(`route`). The nodes' slots are synchronised: that is this model's, not the
paper's.

A request reaches node `i` with probability `1 / g` (`mean_route`), so the
work node `i` receives has mean `load / g`. Outside `F` at node `i` its
backlog drifts down by `128 − load / g` (`drift`), and Foster's criterion
bounds the expected time until node `i`'s batch is not full by
`backlog_i / (128 − load / g)` (`hitTime_le`) when `load < 128 g`, the
capacity of `g` nodes. The routing does not read the state, so node `i`
alone is `BariStable`'s chain with the requests routed to it (`marginal`,
`thin`), of load `load / g` (`load_thin`), and `BariRecurrent` makes every
node's chain on job lists positive recurrent (`positive_recurrent`): Theorem
2 at each node. Summed over the nodes, the total backlog drifts down by
`128 g − load` while every batch is full (`drift_sum`): the capacity of `g`
nodes is the sum of the nodes'. The squared backlogs drift down outside a
bounded set (`driftQ`), and when a slot brings no request with positive
probability, every node empties at once in bounded expected time and the
states with every node empty are a positive recurrent atom of the `g`-node
chain (`hit_idle_le`, `return_idle`): Theorem 2 for the chain as a whole.
-/
import papers.BariProgram

namespace SerqLang

namespace Papers
namespace BariNodes

open Foster BariStable

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
  Kernel.ofFintype (prob A g) (prob_nonneg A) (prob_sum A) (step A)

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
  rw [kernel, Kernel.apply_ofFintype, Fintype.sum_sigma]
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
  integrable := Kernel.integrable_ofFintype _ _ _ _ _
  pos := by
    have hg : (0 : ℝ) < g := Nat.cast_pos.mpr (Nat.pos_of_ne_zero (NeZero.ne g))
    unfold ε; rw [sub_pos, div_lt_iff₀ hg]; linarith
  drift x hx := by rw [apply_V A i x hx, ε]; linarith

/-- Foster's drift at node `i`: from every state, the expected number of slots
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

/-- Foster's drift at node `i`: from every state of `F i`, the expected return
time to `F i` is finite. -/
theorem returnTime_le {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g)
    (x : State g) (hx : F i x) :
    returnTime (kernel A) (F i) x ≤ 1 + (kernel A).apply (V i) x / ε A g :=
  Foster.returnTime_le_of_drift (drift A hA i) x hx

/-! ### One node is `BariStable`'s chain, with thinned arrivals

The routing does not read the state, and node `i`'s next machine reads only
its own machine and the requests routed to it. So node `i` alone is a
Markov chain: `BariStable`'s, whose slot brings `route (A.arr o) r i` with
probability `prob A g ⟨o, r⟩` (`thin`, `marginal`), a load of `load / g`
(`load_thin`). Below the capacity of `g` nodes, every node's chain on job
lists is then positive recurrent (`positive_recurrent`), by `BariRecurrent`. -/

theorem card_pos {N g : ℕ} [NeZero g] (A : Arrivals N) : 0 < Fintype.card (Outcome A g) :=
  Fintype.card_pos_iff.mpr ⟨⟨0, fun _ => ⟨0, Nat.pos_of_ne_zero (NeZero.ne g)⟩⟩⟩

/-- The outcomes numbered `0, …, card − 1`. -/
noncomputable def enum {N g : ℕ} (A : Arrivals N) : Outcome A g ≃ Fin (Fintype.card (Outcome A g)) :=
  Fintype.equivFin _

/-- A sum over the numbered outcomes is the sum over the outcomes. -/
theorem sum_enum {N g : ℕ} [NeZero g] (A : Arrivals N) (G : Outcome A g → ℝ) :
    ∑ o ∈ Finset.range (Fintype.card (Outcome A g) - 1 + 1),
      (if h : o < Fintype.card (Outcome A g) then G ((enum A).symm ⟨o, h⟩) else 0) = ∑ ω, G ω := by
  rw [Nat.sub_add_cancel (card_pos A), Finset.sum_range]
  simp only [Fin.is_lt, dif_pos, Fin.eta]
  exact Equiv.sum_comp (enum A).symm G

/-- Node `i`'s arrivals: outcome `ω` brings what it routes to `i`. -/
noncomputable def thin {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) :
    Arrivals (Fintype.card (Outcome A g) - 1) where
  p o := if h : o < Fintype.card (Outcome A g) then prob A g ((enum A).symm ⟨o, h⟩) else 0
  nonneg o := by split_ifs; exacts [prob_nonneg A _, le_rfl]
  sum_one := (sum_enum A (prob A g)).trans (prob_sum A)
  arr o := if h : o < Fintype.card (Outcome A g) then
    route (A.arr ((enum A).symm ⟨o, h⟩).1) ((enum A).symm ⟨o, h⟩).2 i else []
  small o := by
    split_ifs
    · exact (route_length _ _ _).trans (A.small _)
    · simp
  fits o r hr := by
    split_ifs at hr
    · exact A.fits _ _ (mem_route hr)
    · simp at hr

/-- Node `i`'s load is the `g`-th part of the load. -/
theorem load_thin {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) :
    (thin A i).load = A.load / g := by
  have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
  have h1 : (thin A i).load = ∑ ω, prob A g ω * (work (route (A.arr ω.1) ω.2 i) : ℝ) := by
    rw [Arrivals.load, ← sum_enum A]
    refine Finset.sum_congr rfl fun o _ => ?_
    simp only [thin, Arrivals.work]
    split_ifs <;> simp [work]
  rw [h1, Fintype.sum_sigma, Arrivals.load, ← Fin.sum_univ_eq_sum_range (fun o => A.p o * A.work o) (N + 1),
    Finset.sum_div]
  refine Finset.sum_congr rfl fun o _ => ?_
  have := mean_route (A.arr o) i
  simp only [prob, Arrivals.work]
  rw [show ∑ r : Fin (A.arr o).length → Fin g, A.p o / (g : ℝ) ^ (A.arr o).length *
      (work (route (A.arr o) r i) : ℝ) =
      A.p o * ∑ r : Fin (A.arr o).length → Fin g, (work (route (A.arr o) r i) : ℝ) / (g : ℝ) ^ (A.arr o).length by
    rw [Finset.mul_sum]; exact Finset.sum_congr rfl fun r _ => by ring, this]
  unfold work; ring

/-- **Node `i` alone is a Markov chain**: the expectation of a function of
node `i` after a slot is `BariStable`'s, with node `i`'s arrivals. -/
theorem marginal {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) (f : BariStable.State → ℝ)
    (x : State g) : (kernel A).apply (fun y => f (y i)) x = (BariStable.kernel (thin A i)).apply f (x i) := by
  have hE : (BariStable.kernel (thin A i)).apply f (x i) =
      ∑ o ∈ Finset.range (Fintype.card (Outcome A g) - 1 + 1),
        (thin A i).p o * f ⟨slot ((thin A i).arr o) (x i).1, Reach.slot _ ((thin A i).fits o) (x i).2⟩ :=
    Kernel.apply_ofOutcomes _ _ _ _ _ _ _
  rw [kernel, Kernel.apply_ofFintype, hE, ← sum_enum A]
  refine Finset.sum_congr rfl fun o _ => ?_
  by_cases h : o < Fintype.card (Outcome A g)
  · simp only [thin, dif_pos h]; rfl
  · rw [dif_neg h]
    show 0 = (if h : o < Fintype.card (Outcome A g) then _ else 0) * _
    rw [dif_neg h, zero_mul]

/-- Theorem 2 at node `i`: below the capacity of `g` nodes, every state of
node `i`'s chain on job lists is positive recurrent. -/
theorem positive_recurrent {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) (i : Fin g)
    (y : BariChain.AState (thin A i)) : PositiveRecurrent (BariChain.akernel (thin A i)) y := by
  have hg : (0 : ℝ) < g := Nat.cast_pos.mpr (Nat.pos_of_ne_zero (NeZero.ne g))
  refine BariRecurrent.positive_recurrent _ ?_ y
  rw [load_thin, div_lt_iff₀ hg]; linarith

/-! ### The sum over the nodes -/

/-- Some node is idle or its batch is not full. -/
def Fsome {g : ℕ} (x : State g) : Prop := ∃ i, F i x

instance {g : ℕ} : DecidablePred (Fsome (g := g)) := fun x => by unfold Fsome; infer_instance

/-- The total backlog over the nodes. -/
def Vsum {g : ℕ} (x : State g) : ℝ := ∑ i, V i x

/-- The capacity of `g` nodes is the sum of the nodes': while every
batch is full, the total backlog drifts down by `128 g − load`. -/
theorem drift_sum {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) :
    Drift (kernel (g := g) A) Fsome Vsum (128 * g - A.load) where
  nonneg _ := Finset.sum_nonneg fun _ _ => Nat.cast_nonneg _
  integrable := Kernel.integrable_ofFintype _ _ _ _ _
  pos := by linarith
  drift x hx := by
    have hx' : ∀ i, ¬ F i x := fun i h => hx ⟨i, h⟩
    have hg : (g : ℝ) ≠ 0 := Nat.cast_ne_zero.mpr (NeZero.ne g)
    have : (kernel A).apply Vsum x = ∑ i, (kernel A).apply (V i) x := by
      simp only [kernel, Kernel.apply_ofFintype, Vsum, Finset.mul_sum]
      exact Finset.sum_comm
    rw [this, Finset.sum_congr rfl fun i _ => apply_V A i x (hx' i)]
    simp only [Finset.sum_sub_distrib, Finset.sum_add_distrib, Finset.sum_const, Finset.card_univ,
      Fintype.card_fin, nsmul_eq_mul, Vsum]
    field_simp
    linarith

/-! ### Every node empty at once

The total backlog drifts down only while every batch is full (`drift_sum`),
and the set where some node's is not is unbounded. The squares do better:
outside `F i` node `i`'s square drifts by `−2 ε V_i` and a constant, and
inside `F i` it stays below a constant, so `Q = Σ_i V_i²` drifts down by
one outside the bounded set where every backlog is at most `R` (`driftQ`).
From there, `R` slots without arrivals empty every node at once (`drain`),
when a slot brings no request with positive probability. That is a premise
for `g` nodes: below `128 g` the load may bring a request every slot (the
example at the end), and then after the first slot some node holds the
newest request. Poisson arrivals bring none with positive probability, and
one node below `128` does too (`BariRecurrent.exists_empty`). -/

/-- Every node holds no job. -/
def AllIdle {g : ℕ} (x : State g) : Prop := ∀ i, BariProgram.Idle (x i)

instance {g : ℕ} : DecidablePred (AllIdle (g := g)) := fun x => by
  unfold AllIdle BariProgram.Idle; infer_instance

theorem apply_kernel {N g : ℕ} [NeZero g] (A : Arrivals N) (f : State g → ℝ) (x : State g) :
    (kernel A).apply f x = ∑ ω, prob A g ω * f (step A x ω) :=
  Kernel.apply_ofFintype _ _ _ _ _ _

theorem load_nonneg {N : ℕ} (A : Arrivals N) : 0 ≤ A.load :=
  Finset.sum_nonneg fun o _ => mul_nonneg (A.nonneg o) (Nat.cast_nonneg _)

/-- A node receives at most `10 000` requests of `1536` tokens in a slot. -/
theorem work_route_le {N g : ℕ} (A : Arrivals N) (ω : Outcome A g) (i : Fin g) :
    (work (route (A.arr ω.1) ω.2 i) : ℝ) ≤ 15360000 := by
  have h1 := BariRecurrent.work_le (route (A.arr ω.1) ω.2 i) fun _ hq => A.fits _ _ (mem_route hq)
  have h2 := route_length (A.arr ω.1) ω.2 i
  have h3 := A.small ω.1
  have : work (route (A.arr ω.1) ω.2 i) ≤ 15360000 := by unfold work; omega
  exact_mod_cast this

/-- No node's backlog grows by more than a slot's work. -/
theorem V_step_le {N g : ℕ} (A : Arrivals N) (x : State g) (ω : Outcome A g) (i : Fin g) :
    V i (step A x ω) ≤ V i x + 15360000 := by
  have hf : ∀ q ∈ route (A.arr ω.1) ω.2 i, Fits q := fun _ hq => A.fits _ _ (mem_route hq)
  have h1 := BariRecurrent.absSlot_backlog (route (A.arr ω.1) ω.2 i) (BariSim.σ (x i).1)
  rw [← BariSim.simulation (x i) _ hf, ← BariProgram.backlog_eq, ← BariProgram.backlog_eq] at h1
  have h2 : (work (route (A.arr ω.1) ω.2 i) : ℝ) ≤ 15360000 := work_route_le A ω i
  have h3 : (backlog (slot (route (A.arr ω.1) ω.2 i) (x i).1) : ℝ) ≤
      backlog (x i).1 + work (route (A.arr ω.1) ω.2 i) := by
    have : backlog (slot (route (A.arr ω.1) ω.2 i) (x i).1) ≤
        backlog (x i).1 + work (route (A.arr ω.1) ω.2 i) := by unfold work; omega
    exact_mod_cast this
  show (backlog (slot (route (A.arr ω.1) ω.2 i) (x i).1) : ℝ) ≤ _
  unfold V; linarith

/-- Outside `F i`, node `i` gains the work routed to it and serves 128. -/
theorem V_step_off {N g : ℕ} (A : Arrivals N) (x : State g) (ω : Outcome A g) {i : Fin g}
    (hx : ¬ F i x) : V i (step A x ω) = V i x + work (route (A.arr ω.1) ω.2 i) - 128 := by
  have h1 := backlog_slot (x i) hx (route (A.arr ω.1) ω.2 i) (fun _ hq => A.fits _ _ (mem_route hq))
  have h1' : (backlog (slot (route (A.arr ω.1) ω.2 i) (x i).1) : ℝ) + 128 =
      backlog (x i).1 + (work (route (A.arr ω.1) ω.2 i) : ℝ) := by
    unfold work; exact_mod_cast h1
  unfold V step; simp only; linarith

/-- The constant of the squares' drift: `(128 · 512 + 15 360 000)²`. -/
def C : ℝ := (65536 + 15360000) ^ 2

/-- Outside `F i`, node `i`'s square drifts by `−2 ε V_i + C`. -/
theorem apply_sq_off {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) (x : State g) (hx : ¬ F i x) :
    (kernel A).apply (fun y => V i y ^ 2) x ≤ V i x ^ 2 - 2 * ε A g * V i x + C := by
  set v := V i x
  have hpt : ∀ ω, V i (step A x ω) ^ 2 ≤
      (15360000 ^ 2 - (v - 128) ^ 2) + 2 * (v - 128) * V i (step A x ω) := by
    intro ω
    have h1 := V_step_off A x ω hx
    have h2 := work_route_le A ω i
    have h3 : (0 : ℝ) ≤ work (route (A.arr ω.1) ω.2 i) := Nat.cast_nonneg _
    rw [h1]
    nlinarith
  have hE := apply_V A i x hx
  rw [apply_kernel] at hE ⊢
  have h1 : ∑ ω, prob A g ω * V i (step A x ω) ^ 2 ≤
      ∑ ω, prob A g ω * ((15360000 ^ 2 - (v - 128) ^ 2) + 2 * (v - 128) * V i (step A x ω)) :=
    Finset.sum_le_sum fun ω _ => mul_le_mul_of_nonneg_left (hpt ω) (prob_nonneg A ω)
  have h2 : ∑ ω, prob A g ω * ((15360000 ^ 2 - (v - 128) ^ 2) + 2 * (v - 128) * V i (step A x ω)) =
      (15360000 ^ 2 - (v - 128) ^ 2) + 2 * (v - 128) * ∑ ω, prob A g ω * V i (step A x ω) := by
    have e : ∀ ω, prob A g ω * ((15360000 ^ 2 - (v - 128) ^ 2) + 2 * (v - 128) * V i (step A x ω)) =
        (15360000 ^ 2 - (v - 128) ^ 2) * prob A g ω + 2 * (v - 128) * (prob A g ω * V i (step A x ω)) :=
      fun ω => by ring
    rw [Finset.sum_congr rfl fun ω _ => e ω, Finset.sum_add_distrib, ← Finset.mul_sum, ← Finset.mul_sum,
      prob_sum, mul_one]
  have hg : (0 : ℝ) < g := Nat.cast_pos.mpr (Nat.pos_of_ne_zero (NeZero.ne g))
  have hL : 0 ≤ A.load / g := div_nonneg (load_nonneg A) hg.le
  rw [h2, hE] at h1
  unfold ε C
  nlinarith

/-- Inside `F i`, node `i`'s square stays below `C`. -/
theorem apply_sq_in {N g : ℕ} [NeZero g] (A : Arrivals N) (i : Fin g) (x : State g) (hx : F i x) :
    (kernel A).apply (fun y => V i y ^ 2) x ≤ V i x ^ 2 + C := by
  have hb : V i x < 65536 := by unfold V; exact_mod_cast backlog_lt_of_F (x i) hx
  rw [apply_kernel]
  calc ∑ ω, prob A g ω * V i (step A x ω) ^ 2 ≤ ∑ ω, prob A g ω * C :=
        Finset.sum_le_sum fun ω _ => mul_le_mul_of_nonneg_left (by
          have h1 := V_step_le A x ω i
          have h0 : (0 : ℝ) ≤ V i (step A x ω) := Nat.cast_nonneg _
          unfold C; nlinarith) (prob_nonneg A ω)
    _ = C := by rw [← Finset.sum_mul, prob_sum, one_mul]
    _ ≤ V i x ^ 2 + C := by nlinarith [sq_nonneg (V i x)]

/-- The sum of the squared backlogs. -/
def Q {g : ℕ} (x : State g) : ℝ := ∑ i, V i x ^ 2

/-- The bound on the backlogs past which `Q` drifts down by one. -/
noncomputable def R {N : ℕ} (A : Arrivals N) (g : ℕ) : ℕ := 65536 + ⌈(g * C + 1) / (2 * ε A g)⌉₊

/-- Every node's backlog is at most `R`. -/
def Small {N g : ℕ} (A : Arrivals N) (x : State g) : Prop := ∀ i, backlog (x i).1 ≤ R A g

noncomputable instance {N g : ℕ} (A : Arrivals N) : DecidablePred (Small (g := g) A) :=
  fun x => by unfold Small; infer_instance

/-- Foster's drift for the squares, to the set where every backlog is at most `R`. -/
theorem driftQ {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g) :
    Drift (kernel (g := g) A) (Small A) Q 1 where
  nonneg _ := Finset.sum_nonneg fun _ _ => sq_nonneg _
  integrable := Kernel.integrable_ofFintype _ _ _ _ _
  pos := one_pos
  drift x hx := by
    simp only [Small, not_forall, not_le] at hx
    obtain ⟨j, hj⟩ := hx
    have hε := (drift A hA j).pos
    have hjF : ¬ F j x := fun h => by
      have := backlog_lt_of_F (x j) h
      unfold R at hj; omega
    have hnode : ∀ i, (kernel A).apply (fun y => V i y ^ 2) x ≤
        V i x ^ 2 + C - (if i = j then 2 * ε A g * V j x else 0) := by
      intro i
      by_cases hij : i = j
      · subst hij
        simp only [if_true]
        have := apply_sq_off A i x hjF
        linarith
      · simp only [hij, if_false, sub_zero]
        by_cases hF : F i x
        · exact apply_sq_in A i x hF
        · have := apply_sq_off A i x hF
          have : 0 ≤ ε A g * V i x := mul_nonneg hε.le (Nat.cast_nonneg _)
          linarith
    have hQ : (kernel A).apply Q x = ∑ i, (kernel A).apply (fun y => V i y ^ 2) x := by
      simp only [apply_kernel, Q, Finset.mul_sum]
      exact Finset.sum_comm
    have hsum : ∑ i, (V i x ^ 2 + C - (if i = j then 2 * ε A g * V j x else 0)) =
        Q x + g * C - 2 * ε A g * V j x := by
      simp [Q, Finset.sum_sub_distrib, Finset.sum_add_distrib]
    have hR : (g * C + 1) / (2 * ε A g) ≤ R A g :=
      (Nat.le_ceil _).trans (by unfold R; push_cast; linarith)
    rw [div_le_iff₀ (by positivity)] at hR
    have hVj : (R A g : ℝ) < V j x := by unfold V; exact_mod_cast hj
    have := mul_lt_mul_of_pos_left hVj (by positivity : (0 : ℝ) < 2 * ε A g)
    rw [hQ]
    calc ∑ i, (kernel A).apply (fun y => V i y ^ 2) x
        ≤ ∑ i, (V i x ^ 2 + C - (if i = j then 2 * ε A g * V j x else 0)) :=
          Finset.sum_le_sum fun i _ => hnode i
      _ ≤ Q x - 1 := by rw [hsum]; nlinarith

/-- The outcome with no request. -/
def noArrival {N g : ℕ} [NeZero g] (A : Arrivals N) (o0 : Fin (N + 1)) : Outcome A g :=
  ⟨o0, fun _ => ⟨0, Nat.pos_of_ne_zero (NeZero.ne g)⟩⟩

theorem route_nil {N g : ℕ} (A : Arrivals N) {o0 : Fin (N + 1)} (harr : A.arr o0 = [])
    (r : Fin (A.arr o0).length → Fin g) (i : Fin g) : route (A.arr o0) r i = [] :=
  List.eq_nil_of_length_eq_zero (Nat.le_zero.mp ((route_length _ _ _).trans (by simp [harr])))

theorem idle_backlog {m : BariStable.State} (h : BariProgram.Idle m) : backlog m.1 = 0 := by
  rw [BariProgram.backlog_eq, show BariSim.σ m.1 = [] from h]
  simp [BariChain.backlog]

/-- Without arrivals every node drains: from backlogs `≤ j`, every node is
empty within `j` slots with probability at least `p₀ʲ`. -/
theorem drain {N g : ℕ} [NeZero g] (A : Arrivals N) {o0 : Fin (N + 1)} (harr : A.arr o0 = []) :
    ∀ (j : ℕ) (x : State g), (∀ i, backlog (x i).1 ≤ j) → A.p o0 ^ j ≤ reach (kernel A) AllIdle j x
  | 0, x, hb => by
    have hx : AllIdle x := fun i => by
      by_contra h
      have := BariRecurrent.backlog_pos _ (BariProgram.good_σ (x i).2) h
      have := hb i
      rw [BariProgram.backlog_eq] at this; omega
    rw [reach_of_mem _ AllIdle hx]; simp
  | j + 1, x, hb => by
    have hp1 : A.p o0 ≤ 1 := by
      rw [← A.sum_one]
      exact Finset.single_le_sum (f := A.p) (fun o _ => A.nonneg o) (Finset.mem_range.mpr o0.2)
    by_cases hx : AllIdle x
    · rw [reach_of_mem _ AllIdle hx]; exact pow_le_one₀ (A.nonneg _) hp1
    · have hb' : ∀ i, backlog (step A x (noArrival A o0) i).1 ≤ j := by
        intro i
        have h1 := BariRecurrent.absSlot_backlog [] (BariSim.σ (x i).1)
        rw [← BariSim.simulation (x i) [] (by simp), ← BariProgram.backlog_eq,
          ← BariProgram.backlog_eq] at h1
        simp only [List.map_nil, List.sum_nil, add_zero] at h1
        show backlog (slot (route (A.arr o0) _ i) (x i).1) ≤ j
        rw [route_nil A harr (noArrival A o0).2 i]
        by_cases hi : BariProgram.Idle (x i)
        · have := idle_backlog hi; omega
        · have := (BariRecurrent.sS_cases _ (BariProgram.good_σ (x i).2) hi).1
          have := hb i; omega
      have ih := drain A harr j _ hb'
      have h3 := (kernel A).le_apply (reach_integrable _ AllIdle j) (reach_nonneg _ AllIdle j) x
        (noArrival A o0)
      have h4 : (kernel A).p x (noArrival A o0) = A.p o0 := by
        simp [kernel, Kernel.ofFintype, prob, noArrival, harr]
      show _ ≤ (if AllIdle x then 1 else (kernel A).apply (reach (kernel A) AllIdle j) x)
      rw [if_neg hx, pow_succ]
      calc A.p o0 ^ j * A.p o0
          ≤ reach (kernel A) AllIdle j ((kernel A).next x (noArrival A o0)) * (kernel A).p x (noArrival A o0) := by
            rw [h4]; exact mul_le_mul_of_nonneg_right ih (A.nonneg _)
        _ = (kernel A).p x (noArrival A o0) * reach (kernel A) AllIdle j ((kernel A).next x (noArrival A o0)) :=
            mul_comm _ _
        _ ≤ _ := h3

/-- After `n` slots no backlog has grown by more than `n` slots' work. -/
theorem applyN_Q_le {N g : ℕ} [NeZero g] (A : Arrivals N) : ∀ (n : ℕ) (x : State g),
    (kernel A).applyN n Q x ≤ ∑ i, (V i x + 15360000 * (n : ℕ)) ^ 2
  | 0, x => by simp [Kernel.applyN, Q]
  | n + 1, x => by
    show (kernel A).apply ((kernel A).applyN n Q) x ≤ _
    rw [apply_kernel]
    calc ∑ ω, prob A g ω * (kernel A).applyN n Q (step A x ω)
        ≤ ∑ ω, prob A g ω * ∑ i, (V i x + 15360000 * ((n + 1 : ℕ) : ℝ)) ^ 2 :=
          Finset.sum_le_sum fun ω _ => mul_le_mul_of_nonneg_left
            ((applyN_Q_le A n _).trans (Finset.sum_le_sum fun i _ => pow_le_pow_left₀
              (by have : (0 : ℝ) ≤ V i (step A x ω) := Nat.cast_nonneg _; positivity)
              (by have := V_step_le A x ω i; push_cast; linarith) 2)) (prob_nonneg A ω)
      _ = _ := by rw [← Finset.sum_mul, prob_sum, one_mul]

/-- The truncated hitting times of every node empty at once, bounded. -/
theorem hit_le {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g)
    {o0 : Fin (N + 1)} (hp : 0 < A.p o0) (harr : A.arr o0 = []) :
    ∃ c : ℝ, ∀ n (x : State g), hit (kernel A) AllIdle n x ≤ Q x + c := by
  have h := hit_le_of_reach (kernel (g := g) A) AllIdle (fun _ => Kernel.integrable_ofFintype _ _ _ _ _)
    (driftQ A hA) (fun x hx i => by have := idle_backlog (hx i); omega) (L := R A g)
    (B := g * ((R A g : ℝ) + 15360000 * (R A g : ℕ)) ^ 2) (pow_pos hp _)
    (fun x hx => drain A harr _ x hx)
    (fun x hx => by
      refine (applyN_Q_le A (R A g) x).trans ?_
      calc ∑ i, (V i x + 15360000 * ((R A g : ℕ) : ℝ)) ^ 2
          ≤ ∑ _i : Fin g, ((R A g : ℝ) + 15360000 * (R A g : ℕ)) ^ 2 :=
            Finset.sum_le_sum fun i _ => pow_le_pow_left₀ (by unfold V; positivity)
              (by have : V i x ≤ R A g := by unfold V; exact_mod_cast hx i
                  linarith) 2
        _ = _ := by simp)
  exact ⟨_, fun n x => by have := h n x; rwa [div_one] at this⟩

/-- Below the capacity of `g` nodes, and when a slot brings no request with
positive probability, every node empties at once in bounded expected time
from every state. -/
theorem hit_idle_le {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g)
    {o0 : Fin (N + 1)} (hp : 0 < A.p o0) (harr : A.arr o0 = []) :
    ∃ W : State g → ℝ, ∀ n x, hit (kernel A) AllIdle n x ≤ W x := by
  obtain ⟨c, hc⟩ := hit_le A hA hp harr
  exact ⟨_, hc⟩

/-- Theorem 2 for the `g`-node chain as a whole: under the same premises,
from every state with every node empty the expected time until every node
is empty again is bounded by one constant, so the states with every node
empty are a positive recurrent atom. -/
theorem return_idle {N g : ℕ} [NeZero g] (A : Arrivals N) (hA : A.load < 128 * g)
    {o0 : Fin (N + 1)} (hp : 0 < A.p o0) (harr : A.arr o0 = []) :
    ∃ C : ℝ, ∀ n (x : State g), AllIdle x → 1 + (kernel A).apply (hit (kernel A) AllIdle n) x ≤ C := by
  obtain ⟨c, hc⟩ := hit_le A hA hp harr
  refine ⟨_, fun n x hx => return_le_of_hit_le (kernel A) AllIdle (Kernel.integrable_ofFintype _ _ _ _ _)
    (V := Q) (a := 1) (c := c) zero_le_one (fun n y => by linarith [hc n y]) (d := g * 15360000 ^ 2) ?_ n⟩
  have h1 := applyN_Q_le A 1 x
  have h0 : ∀ i, V i x = 0 := fun i => by unfold V; exact_mod_cast idle_backlog (hx i)
  simp only [Kernel.applyN, h0] at h1
  simpa using h1

/-! ### Not vacuous -/

/-- The premise is not implied by the capacity: one request in every slot
is below two nodes' capacity, and no slot is empty. -/
example : ∃ A : Arrivals 0, A.load < 128 * (2 : ℕ) ∧ ∀ o : Fin 1, A.arr o ≠ [] :=
  ⟨⟨fun o => if o = 0 then 1 else 0,
      fun o => by split_ifs <;> norm_num,
      by simp,
      fun _ => [(128, 1)],
      fun _ => by simp,
      fun _ r hr => by simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩⟩,
    by simp [Arrivals.load, Arrivals.work]; norm_num, fun _ => by simp⟩

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
