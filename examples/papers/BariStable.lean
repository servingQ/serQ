/-
# Bari et al., Theorem 2, for random arrivals

The RAD program of `examples/papers/bari_rad.sq`, with requests arriving at
random: in each slot (one iteration of the engine) the arrivals are the list
`A.arr o` of (prompt, output) lengths with probability `A.p o`, independently
of the past (`Serq/Chain.lean`). Prompts are whole tiles (Assumption 3) and
outputs at most 512, as in the program. The states are the machines this
reaches from the empty one.

Theorem 2 of the paper: RAD is stable below the bound of Theorem 1. Here the
load is the mean tokens a slot brings, `Σ_o p_o Σ_{(v_p, v_d) ∈ arr o} (v_p + v_d)`,
and the capacity is `b_col = 128` tokens per slot. RAD's batch is full unless
every resident decodes and fewer than 128 do (`optimal_tiling`, the set `F`),
so outside `F` the work at the engine drifts down by `ε = 128 − load`, and
Foster's criterion (`Serq/Foster.lean`) bounds the expected time to reach
`F` by `backlog / ε`. The invariant of a slot's end is `Serq/Slot.lean`'s,
with each request's lengths read from slots 10 and 11 and prompts in tiles
of 128; what is RAD's own is its batch (`ci_start`).
-/
import Serq.Slot
import papers.Bari

namespace SerqLang

namespace Papers
namespace BariStable

open Exec Foster Slot

/-- The deployment of `bari_rad.sq`. -/
abbrev D : Deployment := Claims.BariRad.deployment

/-- A request RAD's hypotheses allow: a prompt of 1 to 8 whole tiles
(Assumption 3) and an output of 1 to 512 tokens. -/
def Fits (r : ℕ × ℕ) : Prop := 128 ∣ r.1 ∧ 128 ≤ r.1 ∧ r.1 ≤ 1024 ∧ 1 ≤ r.2 ∧ r.2 ≤ 512

/-- `bari_rad.sq` once a request has arrived: its prompt is slot 10, its
output slot 11, and prompts are whole tiles. -/
abbrev M : Slot.Model where
  D := D
  admit := BariRad.admitAll_rad
  len a := (a 10, a 11)
  len_set a v := by simp [Function.update]
  Fits := Fits
  u := 128
  fits _ h := ⟨by have := h.2.1; omega, h.1, h.2.2.2.1⟩
  B := 128

/-- What a request runs once it has arrived: `bari_rad.sq`'s session without
its arrival delay. -/
def arrived : Prog :=
  match Claims.BariRad.prog with
  | .run _ _ _ _ k => k
  | p => p

theorem arrived_eq : arrived = Q1 M := rfl

/-- A request's attributes: its prompt in slot 10 and its output in slot 11. -/
def attrs (r : ℕ × ℕ) : ℕ → ℕ := fun s => if s = 10 then r.1 else if s = 11 then r.2 else 0

theorem len_attrs (r : ℕ × ℕ) : M.len (attrs r) = r := by simp [M, attrs]

/-- No request, the engine idle. -/
def empty : Machine := Slot.empty M

/-- One slot whose arrivals have the (prompt, output) lengths `rs`. -/
def slot (rs : List (ℕ × ℕ)) (m : Machine) : Machine := Exec.slotL D (Q1 M) (rs.map attrs) m

/-- The machines reached from `empty` by slots of fitting arrivals. -/
inductive Reach : Machine → Prop
  | empty : Reach empty
  | slot {m : Machine} (rs : List (ℕ × ℕ)) : (∀ r ∈ rs, Fits r) →
      Reach m → Reach (slot rs m)

/-- The chain's states. -/
def State : Type := {m : Machine // Reach m}

/-- The tokens the engine still has to serve: a prefill's left work and the
output it will then decode (slot 11 of its session), a decode's left work. -/
abbrev backlog (m : Machine) : ℕ := Slot.backlog M m

/-- The engine is idle, or its batch is not full. -/
def F (x : State) : Prop := x.1.iterEnd = none ∨ x.1.last.stats.tokens < 128

instance : DecidablePred F := fun x => by unfold F; infer_instance

/-- The arrival distribution: outcome `o ≤ N` has probability `p o` and
brings the requests `arr o`, at most 10 000 (a bound on a slot's work that
`BariRecurrent` uses; nothing in a slot needs it). -/
structure Arrivals (N : ℕ) where
  p : ℕ → ℝ
  nonneg : ∀ o, 0 ≤ p o
  sum_one : ∑ o ∈ Finset.range (N + 1), p o = 1
  arr : ℕ → List (ℕ × ℕ)
  small : ∀ o, (arr o).length ≤ 10000
  fits : ∀ o, ∀ r ∈ arr o, Fits r

/-- The tokens outcome `o` brings. -/
def Arrivals.work {N : ℕ} (A : Arrivals N) (o : ℕ) : ℕ := ((A.arr o).map fun r => r.1 + r.2).sum

/-- The mean tokens a slot brings. -/
def Arrivals.load {N : ℕ} (A : Arrivals N) : ℝ := ∑ o ∈ Finset.range (N + 1), A.p o * A.work o

/-- The chain: a slot with the arrivals of outcome `o`, drawn from `A`. -/
noncomputable def kernel {N : ℕ} (A : Arrivals N) : Kernel State ℕ :=
  Kernel.ofOutcomes N A.p A.nonneg A.sum_one fun x o =>
    ⟨slot (A.arr o) x.1, Reach.slot _ (A.fits o) x.2⟩

/-- The drift: `ε = 128 − load`. -/
def ε {N : ℕ} (A : Arrivals N) : ℝ := 128 - A.load

/-! ### RAD's batch -/

/-- A running batch is full, or every resident decodes in it. -/
def Busy (_ : ℕ → ℕ × ℕ) (_ : Ghost) (m : Machine) : Prop :=
  m.last.stats.tokens = tokSum m.iter ∧
    (tokSum m.iter = 128 ∨ ((∀ j ∈ m.jobs, j.mode = .decode) ∧ tokSum m.iter = m.jobs.length))

/-- The invariant of a slot's end. -/
abbrev SB := Slot.SB M Busy

/-- RAD's invariant `R` (`examples/papers/Bari.lean`) holds on an idle machine. -/
theorem ci_R {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hit : m.iter = []) : BariRad.R m := by
  refine BariRad.R.mk' (fun j hj => ?_) hI.jobsNodup (fun s hs => ?_) (fun s hs => ?_)
    (fun e he => by rw [hit] at he; simp at he) (fun e he => by rw [hit] at he; simp at he)
  · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs j hj
    have hsh := (hI.sess _ h1).1
    rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
    · have hl := hI.leftP _ h1 hc
      rw [hc] at hsh
      refine ⟨by rw [h4]; omega, fun _ => by rw [h4]; exact hl.2.1, hsh.2, by rw [hm]; decide, h3⟩
    · have hl := hI.leftD _ h1 hc
      rw [hc] at hsh
      refine ⟨by rw [h4]; omega, fun h => by rw [hm] at h; exact absurd h (by decide), hsh.2,
        by rw [hm]; decide, h3⟩
  · rw [Exec.sess_toList] at hs
    obtain ⟨i, hi, rfl⟩ := List.mem_map.mp hs
    obtain ⟨-, -, hl, hfit⟩ := hI.sess i (List.mem_range.mp hi)
    rw [show (getS m i).attr.get 10 = (L i).1 from congrArg Prod.fst hl]; exact hfit.1
  · rw [Exec.sess_toList] at hs
    obtain ⟨i, hi, rfl⟩ := List.mem_map.mp hs
    exact (hI.sess i (List.mem_range.mp hi)).2.1

/-- RAD's batch is full, or every resident decodes and gets one token. -/
theorem rad_full (m : Machine) (h : BariRad.R m) :
    tokSum (fillIter D (m.jobs.filter (serves D m)) 128) = 128 ∨
      ((∀ j ∈ m.jobs, j.mode = .decode) ∧
        tokSum (fillIter D (m.jobs.filter (serves D m)) 128) = m.jobs.length) := by
  set S := m.jobs.filter (serves D m) with hS
  have hFm : ∀ j ∈ S, j ∈ m.jobs := fun j hj => (List.mem_filter.mp hj).1
  by_cases hc : BariRad.decodeMode m
  · have hdec : ∀ j ∈ S, j.mode = .decode := fun j hj =>
      ((BariRad.serves_rad m j).mp (List.mem_filter.mp hj).2).1 hc
    have hw1 : ∀ j ∈ S, wantOf D j = 1 := fun j hj => by
      have := (h.jobs j (hFm j hj)).1
      simp only [wantOf, hdec j hj]; omega
    have hFd : S = m.jobs.filter (·.mode = .decode) := by
      rw [hS]
      apply List.filter_congr
      intro j _
      have := BariRad.serves_rad m j
      by_cases hd : j.mode = .decode
      · simp only [hd, decide_true]; exact this.mpr ⟨fun _ => hd, fun h' => absurd hc h'⟩
      · simp only [hd, decide_false]
        cases hs : serves D m j
        · rfl
        · exact absurd ((this.mp hs).1 hc) hd
    have hsum : (S.map (wantOf D)).sum = S.length := by
      rw [List.map_congr_left hw1]; simp
    rw [tokSum_fillIter, hsum]
    by_cases hl : 128 ≤ S.length
    · left; omega
    · right
      have hlen : S.length = m.jobs.length := by
        rcases hc with hc | hc
        · rw [hFd] at hl; omega
        · rw [hFd]; exact hc
      refine ⟨fun j hj => ?_, by omega⟩
      have hall : m.jobs.filter (fun j : Job => decide (j.mode = .decode)) = m.jobs :=
        List.Sublist.eq_of_length (List.filter_sublist) (by rw [← hFd]; exact hlen)
      have : j ∈ m.jobs.filter (fun j : Job => decide (j.mode = .decode)) := by rw [hall]; exact hj
      simpa using (List.mem_filter.mp this).2
  · left
    have hpre : ∀ j ∈ S, j.mode = .prefill := fun j hj => by
      have h1 := ((BariRad.serves_rad m j).mp (List.mem_filter.mp hj).2).2 hc
      have h2 := (h.jobs j (hFm j hj)).2.2.2.1
      cases hm : j.mode <;> simp_all
    have hne : S ≠ [] := by
      intro he
      apply hc
      right
      have : ∀ j ∈ m.jobs, j.mode = .decode := by
        intro j hj
        by_contra hd
        have : j ∈ S := List.mem_filter.mpr ⟨hj, (BariRad.serves_rad m j).mpr ⟨fun h' => absurd h' hc, fun _ => hd⟩⟩
        rw [he] at this; simp at this
      rw [List.filter_eq_self.mpr (by simpa using this)]
    obtain ⟨f, rest, hfr⟩ := List.exists_cons_of_ne_nil hne
    have hwf : wantOf D f = 128 := by
      have hj := h.jobs f (hFm f (by rw [hfr]; exact List.mem_cons_self))
      have hp := hpre f (by rw [hfr]; exact List.mem_cons_self)
      obtain ⟨c, hcf⟩ := hj.2.1 hp
      have : 0 < c := by rcases Nat.eq_zero_or_pos c with rfl | h0 <;> simp_all
      simp only [wantOf, hp, D, Claims.BariRad.deployment]
      simp; rw [hcf]; omega
    have hfill : fillIter D S 128 = [(f.owner, 128)] := by
      rw [hfr]; unfold fillIter; rw [hwf]; simp
    rw [hfill]; simp [tokSum]

/-- A list's entries for `i` are each a whole tile. -/
theorem dvd_shareOf (l : List (ℕ × ℕ)) (i : ℕ) (h : ∀ e ∈ l, e.1 = i → e.2 = 128) : 128 ∣ shareOf l i := by
  induction l with
  | nil => simp [shareOf]
  | cons e l ih =>
    rw [Exec.shareOf_cons]
    have := ih fun x hx => h x (List.mem_cons_of_mem _ hx)
    split_ifs with he
    · rw [h e List.mem_cons_self he]; exact Nat.dvd_add (dvd_refl _) this
    · simpa using this

theorem wantOf_le (j : Job) : wantOf D j ≤ j.left := by
  unfold wantOf
  split
  · exact min_le_right _ _
  · split
    · exact min_le_left _ _
    · exact le_rfl

/-- **An iteration starts** on an idle, settled engine. -/
theorem ci_start : Starts M Busy := by
  intro L g m hI hr hie
  show SB L g (startIteration D m) ∧ (startIteration D m).sess.size = m.sess.size
  have hq : engineQueuesEmpty D m := fun p hp => by simp [pdef, D, Claims.BariRad.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter_only D m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range D.pools.length).any fun p =>
      (pdef D p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [D, Claims.BariRad.deployment]
  have hb : D.budget = 128 := rfl
  have hR := ci_R hI (hI.iterNone hie)
  unfold startIteration
  -- the deployment has no `chunkAt`: every iteration runs it as it is
  simp only [iterDeployment_of_none _ (rfl : Claims.BariRad.deployment.chunkAt = none)]
  rw [hvia, Bool.or_false]
  by_cases hjs : m.jobs = []
  · have he0 : m.jobs.isEmpty = true := by simp [hjs]
    simp only [he0, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    exact ⟨⟨{ hI with
        iterNone := fun _ => rfl
        iterOwn := fun e he => by simp at he
        share := fun i hi => by simp [shareOf]
        shareP := fun i hi _ => by simp [shareOf]
        tok := by simp [tokSum] }, hr, fun _ => hjs, fun h => by simp at h⟩, by first | rfl | trivial⟩
  · have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    set it := fillIter D (m.jobs.filter (serves D m)) 128 with hit
    have hfull := rad_full m hR
    rw [← hit] at hfull
    obtain ⟨hT, -⟩ := BariRad.rad_batch m hR
    rw [← hit] at hT
    have hne : it ≠ [] := by
      intro h0
      rw [h0] at hfull
      rcases hfull with h | ⟨-, h⟩
      · simp [tokSum] at h
      · simp [tokSum] at h; exact hjs (List.eq_nil_of_length_eq_zero h.symm)
    have hne' : it.isEmpty = false := by simpa using hne
    simp only [hne', Bool.not_false, Bool.true_or, ↓reduceIte]
    have htok : tokSum it ≤ 128 := by rw [hit, tokSum_fillIter]; exact min_le_left _ _
    have hmemit : ∀ e ∈ it, ∃ j ∈ m.jobs, e.1 = j.owner := fun e he => by
      obtain ⟨j, hj, h1, -⟩ := mem_fillIter D _ 128 e he
      exact ⟨j, (List.mem_filter.mp hj).1, h1⟩
    have hsh : ∀ i < m.sess.size, shareOf it i ≤ if isJob (g.c i) then g.left i else 0 := by
      intro i hi
      have h1 := Exec.shareOf_fillIter D (m.jobs.filter (serves D m)) 128 i
      rw [← hit] at h1
      have h2 : (((m.jobs.filter (serves D m)).filter fun j => decide (j.owner = i)).map (wantOf D)).sum ≤
          ((m.jobs.filter fun j => decide (j.owner = i)).map (wantOf D)).sum :=
        List.Sublist.sum_le_sum ((List.filter_sublist.filter _).map _) (fun _ _ => Nat.zero_le _)
      by_cases hj : isJob (g.c i) = true
      · rw [if_pos hj]
        obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi hj
        rw [Exec.filter_owner_eq hI.jobsNodup hjm] at h2
        simp only [List.map_cons, List.map_nil, List.sum_cons, List.sum_nil, Nat.add_zero] at h2
        have h3 := wantOf_le j
        rw [(hI.jobs j hjm).2.2.2] at h3
        omega
      · rw [if_neg hj]
        have : m.jobs.filter (fun x => decide (x.owner = i)) = [] := by
          rw [List.filter_eq_nil_iff]
          intro x hx
          simp only [decide_eq_true_eq]
          intro he
          obtain ⟨-, h2, -⟩ := hI.jobs x hx
          rw [he] at h2
          rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩ <;> rw [hc] at hj <;> simp [isJob] at hj
        rw [this, List.map_nil, List.sum_nil] at h2; omega
    have hshP : ∀ i < m.sess.size, g.c i = .p → 128 ∣ shareOf it i := by
      intro i hi hc
      obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi (by rw [hc]; rfl)
      have hmj : j.mode = .prefill := by
        rcases (hI.jobs j hjm).2.1 with ⟨-, hm⟩ | ⟨hc', -⟩
        · exact hm
        · rw [hc] at hc'; exact absurd hc' (by decide)
      apply dvd_shareOf
      intro e he heo
      rcases hT e he with h | h
      · exact h
      · have := h j hjm heo.symm
        rw [hmj] at this; exact absurd this (by decide)
    exact ⟨⟨{ hI with
        iterNone := fun h => by simp at h
        iterOwn := fun e he => by
          obtain ⟨j, hj, he1⟩ := hmemit e he
          rw [he1]; exact (hI.jobs j hj).1
        share := hsh
        shareP := hshP
        tok := htok }, hr, fun h => by simp at h, fun _ => ⟨rfl, hfull⟩⟩, by first | rfl | trivial⟩

/-! ### A slot -/

theorem fits_attrs {rs : List (ℕ × ℕ)} (hf : ∀ r ∈ rs, Fits r) : ∀ a ∈ rs.map attrs, M.Fits (M.len a) := by
  simp only [List.mem_map]
  rintro _ ⟨r, hr, rfl⟩
  simpa [attrs] using hf r hr

theorem work_attrs (rs : List (ℕ × ℕ)) : ((rs.map attrs).map M.work).sum = (rs.map fun r => r.1 + r.2).sum := by
  simp [List.map_map, Function.comp_def, Model.work, attrs, List.sum_map_add]

/-- **A slot** keeps the invariant. -/
theorem slot_sb {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB L g m) (rs : List (ℕ × ℕ))
    (hf : ∀ r ∈ rs, Fits r) :
    ∃ L' g', SB L' g' (slot rs m) ∧ (slot rs m).sess.size = m.sess.size + rs.length ∧
      WnC (m.sess.size + rs.length) L' g' + (if m.iterEnd.isSome then tokSum m.iter else 0) =
        WnC m.sess.size L g + (rs.map fun r => r.1 + r.2).sum := by
  obtain ⟨L', g', h1, h2, h3⟩ := Slot.slot_sb ci_start hB (rs.map attrs) (fits_attrs hf)
  rw [List.length_map, work_attrs] at *
  exact ⟨L', g', h1, h2, h3⟩

/-- The chain starts from no request. -/
theorem empty_sb : SB (fun _ => (128, 1)) ⟨fun _ => .e, fun _ => 0⟩ empty := Slot.empty_sb _

/-- **Every state of the chain is at a slot's end.** -/
theorem reach_sb {m : Machine} (h : Reach m) : ∃ L g, SB L g m := by
  induction h with
  | empty => exact ⟨_, _, empty_sb⟩
  | slot rs hf _ ih =>
    obtain ⟨L, g, hB⟩ := ih
    obtain ⟨L', g', h1, -⟩ := slot_sb hB rs hf
    exact ⟨L', g', h1⟩

/-- A full batch serves 128 tokens and the arrivals bring their prompts and
outputs: the backlog after the slot. -/
theorem backlog_slot (x : State) (hx : ¬ F x) (rs : List (ℕ × ℕ))
    (hf : ∀ r ∈ rs, Fits r) :
    backlog (slot rs x.1) + 128 = backlog x.1 + (rs.map fun r => r.1 + r.2).sum := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  simp only [F, not_or, not_lt] at hx
  have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hx.1
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have h128 : tokSum x.1.iter = 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
  have := Slot.backlog_slot ci_start hB hbusy (rs.map attrs) (fits_attrs hf)
  rw [h128, work_attrs] at this
  exact this

/-- `F` is small: RAD's batch is full unless every resident decodes and
fewer than 128 do (`optimal_tiling`), each with at most 512 tokens left. -/
theorem backlog_lt_of_F (x : State) (hx : F x) : backlog x.1 < 128 * 512 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · simp [Slot.backlog, hB.idle hie]
  · have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    have htok : x.1.last.stats.tokens < 128 := by
      rcases hx with hx | hx
      · rw [hie] at hx; simp at hx
      · exact hx
    obtain ⟨hb1, hb2⟩ := hB.busy hbusy
    rw [hb1] at htok
    rcases hb2 with h | ⟨hdec, hlen⟩
    · omega
    · have hle : ∀ v ∈ x.1.jobs.map (fun j => j.left + if j.mode = .prefill then (getS x.1 j.owner).attr.get 11 else 0),
          v ≤ 512 := by
        intro v hv
        obtain ⟨j, hj, rfl⟩ := List.mem_map.mp hv
        have hm := hdec j hj
        obtain ⟨h1, h2, -, h4⟩ := hB.jobs j hj
        have hc : g.c j.owner = .d := by
          rcases h2 with ⟨-, hm'⟩ | ⟨hc, -⟩
          · rw [hm] at hm'; exact absurd hm' (by decide)
          · exact hc
        have := (hB.leftD _ h1 hc).2
        have := (hB.sess _ h1).2.2.2.2.2.2.2
        simp only [hm, reduceCtorEq, if_false, Nat.add_zero]
        omega
      have := List.sum_le_card_nsmul _ 512 hle
      simp only [List.length_map, smul_eq_mul] at this
      show (x.1.jobs.map fun j => j.left + if j.mode = .prefill then (getS x.1 j.owner).attr.get 11 else 0).sum
        < 128 * 512
      omega

/-- Foster's drift condition, below capacity. -/
theorem drift {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    Drift (kernel A) F (fun x => (backlog x.1 : ℝ)) (ε A) where
  nonneg _ := Nat.cast_nonneg _
  integrable := Kernel.integrable_ofOutcomes _ _ _ _ _ _
  pos := by unfold ε; linarith
  drift x hx := by
    -- the expectation over the outcomes (a `rw` cannot abstract a `State` behind the
    -- coercion to `Machine`)
    have hE : (kernel A).apply (fun x => (backlog x.1 : ℝ)) x =
        ∑ o ∈ Finset.range (N + 1), A.p o * (backlog (slot (A.arr o) x.1) : ℝ) :=
      Kernel.apply_ofOutcomes _ _ _ _ _ _ _
    rw [hE]
    calc _ = ∑ o ∈ Finset.range (N + 1),
            (A.p o * backlog x.1 + A.p o * A.work o - 128 * A.p o) := by
          refine Finset.sum_congr rfl fun o _ => ?_
          have h1 := backlog_slot x hx (A.arr o) (A.fits o)
          have h2 : (backlog (slot (A.arr o) x.1) : ℝ) + 128 = backlog x.1 + A.work o := by
            unfold Arrivals.work; exact_mod_cast h1
          have h3 : (backlog (slot (A.arr o) x.1) : ℝ) = backlog x.1 + A.work o - 128 := by linarith
          rw [h3]; ring
      _ = (backlog x.1 : ℝ) * ∑ o ∈ Finset.range (N + 1), A.p o + A.load -
            128 * ∑ o ∈ Finset.range (N + 1), A.p o := by
          rw [Finset.sum_sub_distrib, Finset.sum_add_distrib, ← Finset.mul_sum, Arrivals.load,
            ← Finset.sum_mul]; ring
      _ ≤ (backlog x.1 : ℝ) - ε A := by
          rw [A.sum_one]; unfold ε; linarith

/-- Theorem 2 (Foster's half): from every state, the expected number of slots until the
batch is not full (or the engine idle) is at most `backlog / ε`. -/
theorem hitTime_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) :
    ε A * hitTime (kernel A) F x ≤ backlog x.1 :=
  Foster.hitTime_le (drift A hA) x

/-- Theorem 2 (Foster's half): from every state of `F`, the expected return time to `F` is
finite. -/
theorem returnTime_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) (hx : F x) :
    returnTime (kernel A) F x ≤ 1 + (kernel A).apply (fun y => (backlog y.1 : ℝ)) x / ε A :=
  Foster.returnTime_le_of_drift (drift A hA) x hx

/-- The expected hitting time is finite: the truncated expectations are
bounded and converge to `hitTime`. (`hitTime` is a supremum in ℝ, which
would read 0 were they unbounded, so `hitTime_le` alone does not say this.) -/
theorem hit_tendsto {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) :
    Filter.Tendsto (fun n => hit (kernel A) F n x) Filter.atTop (nhds (hitTime (kernel A) F x)) :=
  Foster.hit_tendsto (drift A hA) x

/-- The theorem is not vacuous: one request of one tile makes a full batch
(Prefill Mode, a chunk of 128), a state outside `F`. -/
example : ¬ F ⟨slot [(128, 1)] empty,
    Reach.slot _ (by intro r hr; simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩) .empty⟩ := by
  have h : (slot [(128, 1)] empty).iterEnd.isSome = true ∧
      (slot [(128, 1)] empty).last.stats.tokens = 128 := by
    decide +kernel
  intro hF
  change (slot [(128, 1)] empty).iterEnd = none ∨ (slot [(128, 1)] empty).last.stats.tokens < 128 at hF
  rcases hF with hF | hF
  · rw [hF] at h; exact absurd h.1 (by decide)
  · omega

/-- … and the load condition can hold: a request of one tile and one output
token in a slot with probability 1/2 is 64.5 tokens per slot. -/
example : ∃ A : Arrivals 1, A.load < 128 :=
  ⟨⟨fun o => if o = 0 then 1 / 2 else if o = 1 then 1 / 2 else 0,
      fun o => by split_ifs <;> norm_num,
      by simp [Finset.sum_range_succ]; norm_num,
      fun o => if o = 1 then [(128, 1)] else [],
      fun o => by split_ifs <;> simp,
      fun o r hr => by split_ifs at hr <;> simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩⟩,
    by simp [Arrivals.load, Arrivals.work, Finset.sum_range_succ]; norm_num⟩

end BariStable
end Papers
end SerqLang
