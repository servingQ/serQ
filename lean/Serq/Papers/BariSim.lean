/-
# The machine chain projects onto RAD's chain on job lists

At a slot boundary the machine of `bari_rad.sq` determines its next slot's
job list from its own job list alone: `σ (slot rs m) = absSlot rs (σ m)` for
every machine the chain reaches. So the machine chain is lumpable onto
`BariChain`'s chain, and what that chain does (`BariRecurrent`) the program
does.
-/
import Serq.Papers.BariChain

namespace SerqLang
namespace Papers
namespace BariSim

open Exec BariChain

/-- A machine's job list: each job's mode, left work and its session's
output length (slot 11), in admission order. -/
def σ (m : Machine) : List AJob := m.jobs.map fun j => (j.mode, j.left, (getS m j.owner).attr.get 11)

theorem σ_empty : σ BariStable.empty = [] := rfl

/-! ### The job list, exactly

`Serq/Slot.lean` follows the list through an instant; what is RAD's own is
its batch. A job's output is its request's `L i`, slot 11 of its session. -/

open Slot

local notation "Db" => BariStable.D
local notation "Mb" => BariStable.M

/-- An iteration starts: the job list is unchanged, and a running batch is
the greedy fill of the jobs RAD serves. -/
theorem start_facts {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI Mb L g m) (hA : A0 m) :
    (startIteration Db m).jobs = m.jobs ∧ A0 (startIteration Db m) ∧
      (startIteration Db m).sess = m.sess ∧
      ((startIteration Db m).iterEnd.isSome = true →
        (startIteration Db m).iter = fillIter Db (m.jobs.filter (serves Db m)) 128 ∧
          (startIteration Db m).iter ≠ []) := by
  have hq : engineQueuesEmpty Db m := fun p hp => by simp [pdef, BariStable.D, Claims.BariRad.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter_only Db m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range BariStable.D.pools.length).any fun p =>
      (pdef Db p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [BariStable.D, Claims.BariRad.deployment]
  have hb : BariStable.D.budget = 128 := rfl
  unfold startIteration
  simp only [iterDeployment_of_none _ (rfl : Claims.BariRad.deployment.chunkAt = none)]
  rw [hvia, Bool.or_false]
  by_cases hjs : m.jobs = []
  · have he0 : m.jobs.isEmpty = true := by simp [hjs]
    simp only [he0, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    exact ⟨by first | rfl | trivial, fun j => hA j, by first | rfl | trivial, fun h => by simp at h⟩
  · have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    split
    · rename_i hc
      refine ⟨rfl, fun j => hA j, rfl, fun _ => ⟨rfl, ?_⟩⟩
      simpa using hc
    · exact ⟨rfl, fun j => hA j, rfl, fun h => by simp at h⟩

/-! ### The batch on the list -/

/-- A job list, each job with its request's output. -/
def σL (L : ℕ → ℕ × ℕ) (js : List Job) : List AJob := js.map fun j => (j.mode, j.left, (L j.owner).2)

theorem σ_eq {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI Mb L g m) : σ m = σL L m.jobs :=
  List.map_congr_left fun j hj =>
    congrArg (fun x => (j.mode, j.left, x)) (congrArg Prod.snd (hI.sess _ (hI.jobs j hj).1).2.2.1)

theorem shareOf_fill_out (js : List Job) (B o : ℕ) (h : o ∉ js.map (·.owner)) :
    shareOf (fillIter Db js B) o = 0 := by
  have h1 := Exec.shareOf_fillIter Db js B o
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

/-- Each job's share of the greedy fill of the jobs `q` serves is its share
in `shares`, when `want` is what `q` lets each job take. -/
theorem fill_sub (q : Job → Bool) (dm : Bool) (out : Job → ℕ) : ∀ (js : List Job),
    (js.map (·.owner)).Nodup →
    (∀ j ∈ js, (if q j then wantOf Db j else 0) = want dm (j.mode, j.left, out j)) → ∀ B,
    js.map (fun j => (j.mode, j.left - shareOf (fillIter Db (js.filter q) B) j.owner, out j)) =
      List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) (js.map fun j => (j.mode, j.left, out j))
        (shares dm (js.map fun j => (j.mode, j.left, out j)) B)
  | [], _, _, _ => rfl
  | j :: js, hn, hw, B => by
    simp only [List.map_cons, List.nodup_cons] at hn
    have hnot : j.owner ∉ (js.filter q).map (·.owner) := fun h => by
      obtain ⟨x, hx, he⟩ := List.mem_map.mp h
      exact hn.1 (he ▸ List.mem_map.mpr ⟨x, (List.mem_filter.mp hx).1, rfl⟩)
    have hout : ∀ B', shareOf (fillIter Db (js.filter q) B') j.owner = 0 :=
      fun B' => shareOf_fill_out _ B' _ hnot
    have hne : ∀ x ∈ js, j.owner ≠ x.owner := fun x hx he => hn.1 (he ▸ List.mem_map.mpr ⟨x, hx, rfl⟩)
    have ih := fill_sub q dm out js hn.2 fun x hx => hw x (List.mem_cons_of_mem _ hx)
    have hwj := hw j List.mem_cons_self
    simp only [List.map_cons, shares, List.zipWith_cons_cons]
    rw [← hwj]
    by_cases hq : q j = true
    · rw [if_pos hq] at hwj ⊢
      rw [List.filter_cons_of_pos hq]
      have hfill : fillIter Db (j :: js.filter q) B =
          if min (wantOf Db j) B = 0 then fillIter Db (js.filter q) B
          else if B - min (wantOf Db j) B = 0 then [(j.owner, min (wantOf Db j) B)]
          else (j.owner, min (wantOf Db j) B) :: fillIter Db (js.filter q) (B - min (wantOf Db j) B) := by
        rw [fillIter]
      rw [hfill]
      by_cases h0 : min (wantOf Db j) B = 0
      · rw [if_pos h0, hout, h0, Nat.sub_zero, Nat.sub_zero, ← ih B]
      · rw [if_neg h0]
        by_cases h1 : B - min (wantOf Db j) B = 0
        · rw [if_pos h1, h1, ← ih 0, fillIter_zero]
          congr 1
          · simp [Exec.shareOf_cons, Exec.shareOf_nil]
          · refine List.map_congr_left fun x hx => ?_
            simp [Exec.shareOf_cons, hne x hx, Exec.shareOf_nil]
        · rw [if_neg h1, ← ih (B - min (wantOf Db j) B)]
          congr 1
          · simp [Exec.shareOf_cons, hout]
          · refine List.map_congr_left fun x hx => ?_
            simp [Exec.shareOf_cons, hne x hx]
    · rw [if_neg hq]
      rw [List.filter_cons_of_neg hq, hout, Nat.zero_min, Nat.sub_zero, Nat.sub_zero, ← ih B]

instance (m : Machine) : Decidable (BariRad.decodeMode m) := by
  unfold BariRad.decodeMode; infer_instance

/-- RAD's `serve only` lets each job take what `want` says in its mode. -/
theorem want_rad (m : Machine) (j : Job) (hj : j.mode ≠ .plain) (o : ℕ) :
    (if serves Db m j then wantOf Db j else 0) = want (decide (BariRad.decodeMode m)) (j.mode, j.left, o) := by
  have hs := BariRad.serves_rad m j
  have hch : BariStable.D.chunk = 128 := rfl
  rcases j with ⟨ow, md, l, gr⟩
  cases md with
  | plain => exact absurd rfl hj
  | prefill =>
    by_cases hd : BariRad.decodeMode m
    · have : serves Db m ⟨ow, .prefill, l, gr⟩ = false := by
        cases h : serves Db m ⟨ow, .prefill, l, gr⟩
        · rfl
        · exact absurd ((hs.mp h).1 hd) (by simp)
      rw [this]; simp [want, hd]
    · have : serves Db m ⟨ow, .prefill, l, gr⟩ = true := hs.mpr ⟨fun h => absurd h hd, fun _ => by simp⟩
      rw [this]; simp [want, hd, wantOf, hch]
  | decode =>
    by_cases hd : BariRad.decodeMode m
    · have : serves Db m ⟨ow, .decode, l, gr⟩ = true := hs.mpr ⟨fun _ => rfl, fun h => absurd hd h⟩
      rw [this]; simp [want, hd, wantOf]
    · have : serves Db m ⟨ow, .decode, l, gr⟩ = false := by
        cases h : serves Db m ⟨ow, .decode, l, gr⟩
        · rfl
        · exact absurd rfl ((hs.mp h).2 hd)
      rw [this]; simp [want, hd]

theorem serves_congr {m m' : Machine} (h1 : m'.jobs = m.jobs) (h2 : m'.sess = m.sess) :
    serves Db m' = serves Db m := by
  funext j
  unfold serves getS
  rw [h1, h2]

theorem decodeMode_eq (L : ℕ → ℕ × ℕ) (m : Machine) :
    decodeMode (σL L m.jobs) = decide (BariRad.decodeMode m) := by
  simp [decodeMode, BariRad.decodeMode, σL, List.filter_map, Function.comp_def]

/-! ### A slot -/

/-- What the chain carries from slot to slot. -/
def Inv (m : Machine) : Prop :=
  ∃ L g, BariStable.SB L g m ∧ A0 m ∧
    (m.iterEnd.isSome = true → m.iter = fillIter Db (m.jobs.filter (serves Db m)) 128 ∧ m.iter ≠ [])

theorem empty_inv : Inv BariStable.empty :=
  ⟨_, _, BariStable.empty_sb, fun j => by simp [getS, BariStable.empty, Slot.empty, Exec.initial],
    fun h => by simp [BariStable.empty, Slot.empty, Exec.initial] at h⟩

theorem slot_inv {m : Machine} (h : Inv m) (rs : List (ℕ × ℕ)) (hl : rs.length ≤ 10000)
    (hf : ∀ r ∈ rs, BariStable.Fits r) :
    Inv (BariStable.slot rs m) ∧ σ (BariStable.slot rs m) = absSlot rs (σ m) := by
  obtain ⟨L, g, hB, hA, hit⟩ := h
  obtain ⟨L', g', hB', -⟩ := BariStable.slot_sb hB rs hl hf
  obtain ⟨L1, g1, h1, -, -, h4, -⟩ := ci_injects (rs.map BariStable.attrs) hB.toCI (BariStable.fits_attrs hf)
  set m1 := injL Mb (rs.map BariStable.attrs) m with hm1
  change CI Mb L1 g1 m1 at h1
  change Exec.Keeps m1 m at h4
  obtain ⟨i1, i2, i3, i4, i5, i6⟩ := injects_facts (rs.map BariStable.attrs) m hA
  rw [← hm1] at i1 i2 i3 i4 i5 i6
  simp only [List.length_map] at i2 i3 i6
  have hr1 : m1.ready = List.range' m.sess.size rs.length := by rw [i2, hB.rdy, List.nil_append]
  have hlen : m1.ready.length ≤ 10000 := by rw [hr1, List.length_range']; exact hl
  -- the new requests' lengths
  have hLn : ∀ t < rs.length, L1 (m.sess.size + t) = rs.getD t (0, 0) := by
    intro t ht
    obtain ⟨-, -, hl, -⟩ := h1.sess (m.sess.size + t) (by rw [i3]; omega)
    rw [i6 t ht] at hl
    have ha : (⟨(rs.map BariStable.attrs).getD t (fun _ => 0), []⟩ : Attrs).get =
        BariStable.attrs (rs.getD t (0, 0)) := by
      funext k; simp [Attrs.get, List.getD_eq_getElem?_getD, ht]
    rw [ha, BariStable.len_attrs] at hl
    exact hl.symm
  set newP := (List.range' m.sess.size rs.length).map fun j => (⟨j, .prefill, (L1 j).1, none⟩ : Job)
    with hnewP
  have hnew : m1.ready.flatMap (contrib L1 g1) = newP := by
    rw [hr1]
    apply flatMap_single
    intro j hj
    rw [List.mem_range'_1] at hj
    obtain ⟨t, rfl⟩ : ∃ t, j = m.sess.size + t := ⟨j - m.sess.size, by omega⟩
    have hc := cat_r1 h1 (j := m.sess.size + t) (by rw [i3]; omega) (by rw [i6 t (by omega)]; try rfl)
      (by rw [i6 t (by omega)])
    simp [contrib, hc]
  have hσnew : σL L1 newP = rs.map fun r => ((.prefill, r.1, r.2) : AJob) := by
    rw [hnewP, σL, List.map_map]
    apply range_map (0, 0)
    intro t ht
    simp [hLn t ht]
  -- the old jobs, read with the new lengths
  have hσm : σ m = σL L1 m.jobs := by
    rw [σ_eq hB.toCI]
    refine List.map_congr_left fun j hj => ?_
    have hj1 := (hB.jobs j hj).1
    have e1 := congrArg Prod.snd (hB.sess _ hj1).2.2.1
    have e2 := congrArg Prod.snd (h1.sess j.owner (by rw [i3]; omega)).2.2.1
    rw [i5 _ hj1] at e2
    simp only [Prod.mk.injEq, true_and]
    exact e1.symm.trans e2
  obtain ⟨g2, c1, c2, c3, -, -⟩ := settle_ci h1 hlen
  obtain ⟨sj, sA⟩ := settle_jobs h1 i4 hlen
  set m2 := settle Db m1 with hm2
  have hsl : BariStable.slot rs m =
      if m.iterEnd.isSome then step Db (afterEvent Db m1) else afterEvent Db m1 := rfl
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · -- an idle engine starts on the new prefills
    have hjobs0 := hB.idle hie
    have hie2 : m2.iterEnd = none := c3.iterEnd.trans (h4.iterEnd.trans hie)
    have hpend : pendingBy m2 m2.now = false := by simp [pendingBy, nextEvent, hie2, c1.delays]
    have hA' : afterEvent Db m1 = startIteration Db m2 := by unfold afterEvent; simp [← hm2, hie2, hpend]
    obtain ⟨t1, t2, t0, t3⟩ := start_facts c1 sA
    obtain ⟨u1, -⟩ := BariStable.ci_start c1 c2 hie2
    have hS : BariStable.slot rs m = startIteration Db m2 := by
      rw [hsl, hie]; simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte]; exact hA'
    rw [hS]
    refine ⟨⟨L', g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
    · obtain ⟨e1, e2⟩ := t3 h; exact ⟨by rw [e1, serves_congr t1 t0, t1], e2⟩
    · rw [σ_eq u1.toCI, t1, sj, i1, hjobs0, hnew, List.nil_append, hσnew, hσm, hjobs0]
      simp [absSlot, σL]
  · -- a busy engine: the arrivals join, then the batch ends and the next starts
    have hbusy : m.iterEnd.isSome = true := by rw [hie]; rfl
    obtain ⟨hitE, hitne⟩ := hit hbusy
    have hjne : m.jobs ≠ [] := fun h0 => hitne (by rw [hitE, h0]; rfl)
    have hie2 : m2.iterEnd = some (a, qa) := c3.iterEnd.trans (h4.iterEnd.trans hie)
    have hiter2 : m2.iter = m.iter := c3.iter.trans h4.iter
    have hA2 : afterEvent Db m1 = m2 := after_busy Db m1 hie2
    have hstep : step Db m2 = afterEvent Db (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := ci_end c1 c2 hie2
    obtain ⟨hj, hrd, hAH⟩ := handle_facts hie2 c1.jobsNodup sA (fun j hj => (c1.jobs j hj).1)
    set H := handle m2 a qa with hH
    obtain ⟨g4, d1, d2, d3, -, -⟩ := settle_ci e1 (e3.trans BariStable.M.small)
    obtain ⟨sj2, sA2⟩ := settle_jobs e1 hAH (e3.trans BariStable.M.small)
    set m3 := settle Db H with hm3
    have hie3 : m3.iterEnd = none := d3.iterEnd.trans e4
    have hpend3 : pendingBy m3 m3.now = false := by simp [pendingBy, nextEvent, hie3, d1.delays]
    have hA3 : afterEvent Db H = startIteration Db m3 := by unfold afterEvent; simp [← hm3, hie3, hpend3]
    obtain ⟨t1, t2, t0, t3⟩ := start_facts d1 sA2
    obtain ⟨u1, -⟩ := BariStable.ci_start d1 d2 hie3
    have hS : BariStable.slot rs m = startIteration Db m3 := by
      rw [hsl, hie]; simp only [Option.isSome_some, ↓reduceIte]; rw [hA2, hstep, hA3]
    rw [hS]
    refine ⟨⟨L', g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
    · obtain ⟨f1, f2⟩ := t3 h; exact ⟨by rw [f1, serves_congr t1 t0, t1], f2⟩
    -- the job list
    set it := m2.iter with hit2
    set sub : Job → Job := fun j => { j with left := j.left - shareOf it j.owner } with hsub
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
      obtain ⟨j, hj, rfl⟩ := List.mem_map.mp hx
      rw [List.mem_range'_1] at hj
      have := (h1.sess j (by rw [i3]; omega)).2.2.2.2.1
      simp; omega
    have hHj : H.jobs = (m.jobs.map sub).filter (·.left ≠ 0) ++ newP := by
      rw [hj, hm2j, List.map_append, hsubP, List.filter_append,
        (List.filter_eq_self (l := newP)).mpr fun x hx => by simpa using hP0 x hx]
    have hdone : (H.ready).flatMap (contrib L1 (g2.tick m2.iter)) =
        (((m.jobs.map sub).filter (·.left = 0)).filter (·.mode = .prefill)).map
          fun x => (⟨x.owner, .decode, (L1 x.owner).2, none⟩ : Job) := by
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
      · simp [hsub, contrib, Ghost.tick, hc, hm, isJob, hle, ← hit2]
      · simp [hsub, contrib, Ghost.tick, hc, hm, isJob, hle, ← hit2]
    have hjne' : σ m ≠ [] := by simpa [σ] using hjne
    have hafter : List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) (σ m)
        (shares (decodeMode (σ m)) (σ m) 128) = σL L1 (m.jobs.map sub) := by
      have := fill_sub (serves Db m) (decide (BariRad.decodeMode m)) (fun j => (L1 j.owner).2) m.jobs
        hB.jobsNodup (fun j hj => want_rad m j (by
          rcases (hB.jobs j hj).2.1 with ⟨-, hm⟩ | ⟨-, hm⟩ <;> rw [hm] <;> decide) _) 128
      rw [← hitE] at this
      rw [hσm, decodeMode_eq, show σL L1 m.jobs = m.jobs.map fun j => (j.mode, j.left, (L1 j.owner).2) from rfl,
        ← this, σL, List.map_map]
      refine List.map_congr_left fun x _ => ?_
      simp only [hsub, Function.comp_apply, hit2]
      rw [← hit2, hiter2]
    have hR : absSlot rs (σ m) =
        (σL L1 (m.jobs.map sub)).filter (·.2.1 ≠ 0) ++ rs.map (fun r => ((.prefill, r.1, r.2) : AJob)) ++
          (((σL L1 (m.jobs.map sub)).filter fun j => j.2.1 = 0 ∧ j.1 = .prefill).map
            fun j => ((.decode, j.2.2, j.2.2) : AJob)) := by
      unfold absSlot
      rw [if_neg hjne', hafter]
    rw [hR, σ_eq u1.toCI, t1, sj2, hHj, hdone]
    unfold σL
    simp only [List.map_append, List.map_map, List.filter_map, List.filter_filter, Function.comp_def]
    rw [← hσnew]
    unfold σL
    simp only [List.append_assoc]
    congr 2
    · congr 1
      apply List.filter_congr
      intro x _
      simp [Bool.and_comm]

/-- Every state of the chain carries the invariant. -/
theorem reach_inv {m : Machine} (h : BariStable.Reach m) : Inv m := by
  induction h with
  | empty => exact empty_inv
  | slot rs hl hf _ ih => exact (slot_inv ih rs hl hf).1

/-- The projection commutes with a slot. -/
theorem simulation (x : BariStable.State) (rs : List (ℕ × ℕ)) (hl : rs.length ≤ 10000)
    (hf : ∀ r ∈ rs, BariStable.Fits r) :
    σ (BariStable.slot rs x.1) = absSlot rs (σ x.1) :=
  (slot_inv (reach_inv x.2) rs hl hf).2

end BariSim
end Papers
end SerqLang
