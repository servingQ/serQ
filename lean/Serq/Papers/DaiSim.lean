/-
# The machine chain projects onto Sarathi's chain on job lists

At a slot boundary the machine of `dai_sarathi.sq` determines its next
slot's job list from its own job list alone: `σ (slot k m) = absSlot k (σ m)`
for every machine the chain reaches. So the machine chain is lumpable onto
`DaiChain`'s chain, and what that chain does (`DaiRecurrent`) the program
does.
-/
import Serq.Papers.DaiChain

namespace SerqLang
namespace Papers
namespace DaiSim

open Exec DaiChain

/-- A machine's job list: each job's mode and left work, in admission order. -/
def σ (m : Machine) : List AJob := m.jobs.map fun j => (j.mode, j.left)

theorem σ_empty : σ DaiStable.empty = [] := rfl

/-! ### The job list, exactly

`Serq/Slot.lean` follows the list through an instant; what is Sarathi's own
is its batch, the greedy fill. -/

open Slot

local notation "Dd" => DaiStable.D
local notation "Md" => DaiStable.M

/-- An iteration starts: the job list is unchanged, and a running batch is
the greedy fill of the jobs. -/
theorem start_facts {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI Md L g m) (hA : A0 m) :
    (startIteration Dd m).jobs = m.jobs ∧ A0 (startIteration Dd m) ∧
      ((startIteration Dd m).iterEnd.isSome = true →
        (startIteration Dd m).iter = fillIter Dd m.jobs 128 ∧ (startIteration Dd m).iter ≠ []) := by
  have hq : engineQueuesEmpty Dd m := fun p hp => by simp [pdef, Claims.DaiSarathi.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter Dd rfl m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range DaiStable.D.pools.length).any fun p =>
      (pdef Dd p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [Claims.DaiSarathi.deployment]
  have hb : DaiStable.D.budget = 128 := rfl
  unfold startIteration
  simp only [iterDeployment_of_none _ (rfl : Claims.DaiSarathi.deployment.chunkAt = none)]
  rw [hvia, Bool.or_false]
  by_cases hjs : m.jobs = []
  · have he0 : m.jobs.isEmpty = true := by simp [hjs]
    simp only [he0, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    exact ⟨by first | rfl | trivial, fun j => hA j, fun h => by simp at h⟩
  · have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    split
    · rename_i hc
      refine ⟨rfl, fun j => hA j, fun _ => ⟨rfl, ?_⟩⟩
      simpa using hc
    · exact ⟨rfl, fun j => hA j, fun h => by simp at h⟩

/-! ### The batch on the list -/

/-- A machine's job list, as a function of the jobs. -/
def σL (js : List Job) : List AJob := js.map fun j => (j.mode, j.left)

theorem want_eq (j : Job) : wantOf Dd j = want (j.mode, j.left) := by
  unfold wantOf
  rcases j with ⟨o, md, l, gr⟩
  cases md <;> simp [want, Claims.DaiSarathi.deployment]

theorem shareOf_fill_out (js : List Job) (B o : ℕ) (h : o ∉ js.map (·.owner)) :
    shareOf (fillIter Dd js B) o = 0 := by
  have h1 := Exec.shareOf_fillIter Dd js B o
  have h2 : js.filter (fun j => decide (j.owner = o)) = [] := by
    rw [List.filter_eq_nil_iff]
    intro x hx
    simp only [decide_eq_true_eq]
    intro he
    exact h (List.mem_map.mpr ⟨x, hx, he⟩)
  rw [h2] at h1
  simpa using h1

theorem fillIter_zero (D : Deployment) : ∀ js : List Job, fillIter D js 0 = []
  | [] => rfl
  | j :: js => by unfold fillIter; simp [fillIter_zero D js]

/-- Each job's share of the greedy fill is its share in `shares`. -/
theorem fill_sub : ∀ (js : List Job), (js.map (·.owner)).Nodup → ∀ B,
    js.map (fun j => (j.mode, j.left - shareOf (fillIter Dd js B) j.owner)) =
      List.zipWith (fun j s => (j.1, j.2 - s)) (σL js) (shares (σL js) B)
  | [], _, _ => rfl
  | j :: js, hn, B => by
    simp only [List.map_cons, List.nodup_cons] at hn
    have hout : ∀ B', shareOf (fillIter Dd js B') j.owner = 0 := fun B' => shareOf_fill_out js B' _ hn.1
    have hne : ∀ x ∈ js, j.owner ≠ x.owner := fun x hx he => hn.1 (he ▸ List.mem_map.mpr ⟨x, hx, rfl⟩)
    have ih := fill_sub js hn.2
    have hw := want_eq j
    simp only [σL, List.map_cons, shares, List.zipWith_cons_cons]
    simp only [σL] at ih
    rw [← hw]
    have hfill : fillIter Dd (j :: js) B =
        if min (wantOf Dd j) B = 0 then fillIter Dd js B
        else if B - min (wantOf Dd j) B = 0 then [(j.owner, min (wantOf Dd j) B)]
        else (j.owner, min (wantOf Dd j) B) :: fillIter Dd js (B - min (wantOf Dd j) B) := by
      rw [fillIter]
    rw [hfill]
    by_cases h0 : min (wantOf Dd j) B = 0
    · rw [if_pos h0, hout, h0, Nat.sub_zero, Nat.sub_zero, ← ih B]
    · rw [if_neg h0]
      by_cases h1 : B - min (wantOf Dd j) B = 0
      · rw [if_pos h1, h1, ← ih 0, fillIter_zero]
        congr 1
        · simp [Exec.shareOf_cons, Exec.shareOf_nil]
        · refine List.map_congr_left fun x hx => ?_
          simp [Exec.shareOf_cons, hne x hx, Exec.shareOf_nil]
      · rw [if_neg h1, ← ih (B - min (wantOf Dd j) B)]
        congr 1
        · simp [Exec.shareOf_cons, hout]
        · refine List.map_congr_left fun x hx => ?_
          simp [Exec.shareOf_cons, hne x hx]

/-! ### A slot -/

/-- What the chain carries from slot to slot. -/
def Inv (m : Machine) : Prop :=
  ∃ L g, DaiStable.SB L g m ∧ A0 m ∧
    (m.iterEnd.isSome = true → m.iter = fillIter Dd m.jobs 128 ∧ m.iter ≠ [])

theorem empty_inv : Inv DaiStable.empty :=
  ⟨_, _, DaiStable.empty_sb, fun j => by simp [getS, DaiStable.empty, Slot.empty, Exec.initial],
    fun h => by simp [DaiStable.empty, Slot.empty, Exec.initial] at h⟩

theorem slot_inv {m : Machine} (h : Inv m) (k : ℕ) :
    Inv (DaiStable.slot k m) ∧ σ (DaiStable.slot k m) = absSlot k (σ m) := by
  obtain ⟨L, g, hB, hA, hit⟩ := h
  obtain ⟨L', g', hB', -⟩ := DaiStable.slot_sb hB k
  obtain ⟨L1, g1, h1, -, -, h4, -⟩ := ci_injects (List.replicate k (fun _ => 0)) hB.toCI (fun _ _ => rfl)
  set m1 := injL Md (List.replicate k (fun _ => 0)) m with hm1
  change CI Md L1 g1 m1 at h1
  change Exec.Keeps m1 m at h4
  obtain ⟨i1, i2, i3, i4, -, i6⟩ := injects_facts (List.replicate k (fun _ => 0)) m hA
  rw [← hm1] at i1 i2 i3 i4 i6
  simp only [List.length_replicate] at i2 i3 i6
  have hr1 : m1.ready = List.range' m.sess.size k := by rw [i2, hB.rdy, List.nil_append]
  have hnew : m1.ready.flatMap (contrib L1 g1) =
      (List.range' m.sess.size k).map fun j => (⟨j, .prefill, 290, none⟩ : Job) := by
    rw [hr1]
    apply flatMap_single
    intro j hj
    rw [List.mem_range'_1] at hj
    obtain ⟨t, rfl⟩ : ∃ t, j = m.sess.size + t := ⟨j - m.sess.size, by omega⟩
    have hc := cat_r1 h1 (j := m.sess.size + t) (by rw [i3]; omega) (by rw [i6 t (by omega)]; try rfl)
      (by rw [i6 t (by omega)])
    simp [contrib, hc, DaiStable.len_of h1 (show m.sess.size + t < m1.sess.size by rw [i3]; omega)]
  have hσnew : ((List.range' m.sess.size k).map fun j => (⟨j, .prefill, 290, none⟩ : Job)).map
      (fun j => (j.mode, j.left)) = List.replicate k (.prefill, 290) := by
    simp [List.map_map, Function.comp_def, List.map_const']
  obtain ⟨g2, c1, c2, c3, -, -⟩ := settle_ci h1
  obtain ⟨sj, sA⟩ := settle_jobs h1 i4
  set m2 := settle Dd m1 with hm2
  have hsl : DaiStable.slot k m =
      if m.iterEnd.isSome then step Dd (afterEvent Dd m1) else afterEvent Dd m1 := by
    rw [DaiStable.slot_eq]; rfl
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · -- an idle engine starts on the new prefills
    have hjobs0 := hB.idle hie
    have hie2 : m2.iterEnd = none := c3.iterEnd.trans (h4.iterEnd.trans hie)
    have hpend : pendingBy m2 m2.now = false := by simp [pendingBy, nextEvent, hie2, c1.delays]
    have hA' : afterEvent Dd m1 = startIteration Dd m2 := by unfold afterEvent; simp [← hm2, hie2, hpend]
    obtain ⟨t1, t2, t3⟩ := start_facts c1 sA
    have hS : DaiStable.slot k m = startIteration Dd m2 := by
      rw [hsl, hie]; simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte]; exact hA'
    rw [hS]
    refine ⟨⟨L', g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
    · obtain ⟨e1, e2⟩ := t3 h; exact ⟨by rw [e1, t1], e2⟩
    · unfold σ
      rw [t1, sj, i1, hjobs0, hnew, List.nil_append, hσnew]
      simp [absSlot]
  · -- a busy engine: the arrivals join, then the batch ends and the next starts
    have hbusy : m.iterEnd.isSome = true := by rw [hie]; rfl
    obtain ⟨hitE, hitne⟩ := hit hbusy
    have hjne : m.jobs ≠ [] := fun h0 => hitne (by rw [hitE, h0]; rfl)
    have hie2 : m2.iterEnd = some (a, qa) := c3.iterEnd.trans (h4.iterEnd.trans hie)
    have hiter2 : m2.iter = m.iter := c3.iter.trans h4.iter
    have hA2 : afterEvent Dd m1 = m2 := after_busy Dd m1 hie2
    have hstep : step Dd m2 = afterEvent Dd (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := ci_end c1 c2 hie2
    obtain ⟨hj, hrd, hAH⟩ := handle_facts hie2 c1.jobsNodup sA (fun j hj => (c1.jobs j hj).1)
    set H := handle m2 a qa with hH
    obtain ⟨g4, d1, d2, d3, -, -⟩ := settle_ci e1
    obtain ⟨sj2, sA2⟩ := settle_jobs e1 hAH
    set m3 := settle Dd H with hm3
    have hie3 : m3.iterEnd = none := d3.iterEnd.trans e4
    have hpend3 : pendingBy m3 m3.now = false := by simp [pendingBy, nextEvent, hie3, d1.delays]
    have hA3 : afterEvent Dd H = startIteration Dd m3 := by unfold afterEvent; simp [← hm3, hie3, hpend3]
    obtain ⟨t1, t2, t3⟩ := start_facts d1 sA2
    have hS : DaiStable.slot k m = startIteration Dd m3 := by
      rw [hsl, hie]; simp only [Option.isSome_some, ↓reduceIte]; rw [hA2, hstep, hA3]
    rw [hS]
    refine ⟨⟨L', g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
    · obtain ⟨f1, f2⟩ := t3 h; exact ⟨by rw [f1, t1], f2⟩
    -- the job list
    set it := m2.iter with hit2
    set sub : Job → Job := fun j => { j with left := j.left - shareOf it j.owner } with hsub
    set newP := (List.range' m.sess.size k).map fun j => (⟨j, .prefill, 290, none⟩ : Job) with hnewP
    have hm2j : m2.jobs = m.jobs ++ newP := by rw [sj, i1, hnew]
    have hsubP : newP.map sub = newP := by
      rw [List.map_congr_left (g := id), List.map_id]
      intro x hx
      obtain ⟨j, hj, rfl⟩ := List.mem_map.mp hx
      rw [List.mem_range'_1] at hj
      have : shareOf it j = 0 := Exec.shareOf_eq_zero fun e he => by
        have := hB.iterOwn e (by rw [← hiter2]; exact he); omega
      simp [hsub, this]
    have hP0 : ∀ x ∈ newP, x.left ≠ 0 := fun x hx => by
      obtain ⟨j, -, rfl⟩ := List.mem_map.mp hx; simp
    have hHj : H.jobs = (m.jobs.map sub).filter (·.left ≠ 0) ++ newP := by
      rw [hj, hm2j, List.map_append, hsubP, List.filter_append,
        (List.filter_eq_self (l := newP)).mpr fun x hx => by simpa using hP0 x hx]
    have hdone : (H.ready).flatMap (contrib L1 (g2.tick m2.iter)) =
        (((m.jobs.map sub).filter (·.left = 0)).filter (·.mode = .prefill)).map
          fun x => (⟨x.owner, .decode, 990, none⟩ : Job) := by
      rw [hrd, c2, List.nil_append, hm2j, List.map_append, hsubP, List.filter_append,
        (List.filter_eq_nil_iff (l := newP)).mpr fun x hx => by simpa using hP0 x hx, List.append_nil,
        List.flatMap_map]
      apply flatMap_ite
      intro x hx
      obtain ⟨hx1, hx0⟩ := List.mem_filter.mp hx
      obtain ⟨x0, hx0m, rfl⟩ := List.mem_map.mp hx1
      have hx0m2 : x0 ∈ m2.jobs := by rw [hm2j]; exact List.mem_append_left _ hx0m
      obtain ⟨-, hcat, -, hl⟩ := c1.jobs x0 hx0m2
      simp only [hsub, decide_eq_true_eq] at hx0
      have hle : g2.left x0.owner ≤ shareOf it x0.owner := by rw [← hl]; omega
      rcases hcat with ⟨hc, hm⟩ | ⟨hc, hm⟩
      · simp [hsub, contrib, Ghost.tick, hc, hm, isJob, hle, ← hit2, DaiStable.len_of c1 (c1.jobs x0 hx0m2).1]
      · simp [hsub, contrib, Ghost.tick, hc, hm, isJob, hle, ← hit2]
    have hjne' : σ m ≠ [] := by simpa [σ] using hjne
    have hafter : List.zipWith (fun j s => (j.1, j.2 - s)) (σ m) (shares (σ m) 128) =
        (m.jobs.map sub).map fun j => (j.mode, j.left) := by
      rw [List.map_map]
      have := fill_sub m.jobs hB.jobsNodup 128
      rw [← hitE] at this
      rw [show σ m = σL m.jobs from rfl, ← this]
      refine List.map_congr_left fun x _ => ?_
      simp only [hsub, Function.comp_apply, hit2]
      rw [← hit2, hiter2]
    have hR : absSlot k (σ m) =
        (((m.jobs.map sub).map fun j => (j.mode, j.left)).filter (·.2 ≠ 0)) ++ List.replicate k (.prefill, 290) ++
          ((((m.jobs.map sub).map fun j => (j.mode, j.left)).filter
            fun j => j.2 = 0 ∧ j.1 = .prefill).map fun _ => (.decode, 990)) := by
      unfold absSlot
      rw [if_neg hjne', hafter]
    rw [hR]
    show (startIteration Dd m3).jobs.map _ = _
    rw [t1, sj2, hHj, hdone]
    simp only [List.map_append, List.map_map, List.filter_map, List.filter_filter, Function.comp_def]
    rw [hnewP, hσnew]
    simp only [List.append_assoc]
    congr 2
    · congr 1
      apply List.filter_congr
      intro x _
      simp [Bool.and_comm]

/-- Every state of the chain carries the invariant. -/
theorem reach_inv {K : ℕ} {m : Machine} (h : DaiStable.Reach K m) : Inv m := by
  induction h with
  | empty => exact empty_inv
  | slot k hk _ ih => exact (slot_inv ih k).1

/-- The projection commutes with a slot. -/
theorem simulation {K : ℕ} (x : DaiStable.State K) (k : ℕ) (hk : k ≤ K) :
    σ (DaiStable.slot k x.1) = absSlot k (σ x.1) :=
  (slot_inv (reach_inv x.2) k).2

end DaiSim
end Papers
end SerqLang
