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

`DaiStable.CI` says what each job is; here we follow the list itself. No
pool admits, so every session keeps admission number 0 (`A0`), and a `run`
on the engine appends its job (`Exec.exec` inserts behind the jobs of no
later admission). -/

open DaiSarathi (Ghost Cat isJob isReady P1 P2 P3 P4)

local notation "Dd" => DaiStable.D

/-- Every session has admission number 0. -/
def A0 (m : Machine) : Prop := ∀ j, (getS m j).admSeq = 0

theorem A0_setS {m : Machine} (hA : A0 m) {i : ℕ} (s : Sess) (hs : s.admSeq = 0) (hi : i < m.sess.size) :
    A0 (setS m i s) := fun j => by
  rw [Exec.getS_setS m s hi]
  split_ifs
  · exact hs
  · exact hA j

theorem span_all {α : Type} (p : α → Bool) : ∀ l : List α, (∀ x ∈ l, p x = true) → l.span p = (l, [])
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.span_eq_takeWhile_dropWhile, List.takeWhile_cons_of_pos (h x List.mem_cons_self),
      List.dropWhile_cons_of_pos (h x List.mem_cons_self)]
    have := span_all p xs fun y hy => h y (List.mem_cons_of_mem _ hy)
    rw [List.span_eq_takeWhile_dropWhile] at this
    simp only [Prod.mk.injEq] at this
    rw [this.1, this.2]

/-- An arrived request's prefill joins the end of the job list. -/
theorem exec_r1_jobs {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) (hA : A0 m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    (exec Dd 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .prefill, 290, none⟩] ∧
      A0 (exec Dd 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_set Dd _ m0 i 9 (fun x => x.now) P2 (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := P2 }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hA1 : A0 m1 := A0_setS (fun j => hA j) s1 (hA i) (by simpa using hin)
  have hg1 : getS m1 i = s1 := Exec.getS_setS_self m0 s1 (by simpa using hin)
  rw [Exec.exec_runEngine Dd 9998 m1 i .prefill (fun _ => 290) P3 (by rw [hg1]; exact hst)
    (by rw [hg1]; rfl) (by simp [evalE])]
  rw [span_all _ _ fun j _ => by simp [hA1 j.owner, hA1 i]]
  refine ⟨rfl, fun j => ?_⟩
  show (getS (setS m1 i { getS m1 i with prog := P3, status := .engine }) j).admSeq = 0
  exact A0_setS hA1 { getS m1 i with prog := P3, status := .engine } (hA1 i) (by simpa [hm1, hm0] using hin) j

/-- A prefilled request's decode joins the end of the job list. -/
theorem exec_r3_jobs {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) (hA : A0 m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    (exec Dd 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .decode, 990, none⟩] ∧
      A0 (exec Dd 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hA0 : A0 m0 := fun j => hA j
  rw [show (10000 : ℕ) = 9999 + 1 from rfl,
    Exec.exec_runEngine Dd 9999 m0 i .decode (fun _ => 990) P4 (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl) (by simp [evalE])]
  rw [span_all _ _ fun j _ => by simp [hA0 j.owner, hA0 i]]
  refine ⟨rfl, fun j => ?_⟩
  show (getS (setS m0 i { getS m0 i with prog := P4, status := .engine }) j).admSeq = 0
  exact A0_setS hA0 { getS m0 i with prog := P4, status := .engine } (hA0 i) (by simpa [hm0] using hin) j

/-- A decoded request leaves the job list as it is. -/
theorem exec_r4_jobs {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) (hA : A0 m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    (exec Dd 10000 { m with ready := rest } i).jobs = m.jobs ∧
      A0 (exec Dd 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_observe Dd _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    Exec.getS_setS_self m0 _ (by simpa using hin)
  have hA1 : A0 m1 := fun j => by
    show (getS (setS m0 i { getS m0 i with prog := .stop }) j).admSeq = 0
    exact A0_setS (m := m0) (fun j => hA j) { getS m0 i with prog := .stop } (hA i) (by simpa using hin) j
  rw [Exec.exec_stop Dd _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  exact ⟨rfl, A0_setS hA1 _ (hA1 i) (by simpa [hm1, hm0] using hin)⟩

/-- The job a ready request adds to the list when it runs. -/
def contrib (g : Ghost) (i : ℕ) : List Job :=
  match g.c i with
  | .r1 => [⟨i, .prefill, 290, none⟩]
  | .r3 => [⟨i, .decode, 990, none⟩]
  | _ => []

/-- **Draining** appends each ready request's job, in the ready list's order. -/
theorem drain_jobs : ∀ (f : ℕ) (g : Ghost) (m : Machine), DaiStable.CI g m → A0 m → m.ready.length ≤ f →
    (drain Dd f m).jobs = m.jobs ++ m.ready.flatMap (contrib g) ∧ A0 (drain Dd f m)
  | 0, g, m, hI, hA, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨by simp [drain, hr], hA⟩
  | f + 1, g, m, hI, hA, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨by simp [hr], hA⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      obtain ⟨hin, hcr⟩ := hI.lt_of_ready hr
      have hfr := (hI.sess i hin).2.2
      have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
      obtain ⟨g1, hI1, hr1, hJ, hA1, hc1⟩ : ∃ g1, DaiStable.CI g1 (exec Dd 10000 { m with ready := rest } i) ∧
          (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
          (exec Dd 10000 { m with ready := rest } i).jobs = m.jobs ++ contrib g i ∧
          A0 (exec Dd 10000 { m with ready := rest } i) ∧ (∀ j, j ≠ i → g1.c j = g.c j) := by
        cases hc : g.c i with
        | r1 =>
          obtain ⟨h1, h2, -, -⟩ := DaiStable.ci_r1 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r1_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | r3 =>
          obtain ⟨h1, h2, -, -⟩ := DaiStable.ci_r3 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r3_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | r4 =>
          obtain ⟨h1, h2, -, -⟩ := DaiStable.ci_r4 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r4_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | s0 => rw [hc] at hfr; simp [DaiSarathi.fresh] at hfr
        | w => rw [hc] at hfr; simp [DaiSarathi.fresh] at hfr
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨d1, d2⟩ := drain_jobs f g1 _ hI1 hA1 (by rw [hr1]; exact hlen)
      refine ⟨?_, d2⟩
      rw [d1, hJ, hr1, hr, List.flatMap_cons, List.append_assoc]
      congr 2
      exact List.flatMap_congr fun j hj => by
        have hji : j ≠ i := fun h => hir (h ▸ hj)
        simp only [contrib, hc1 j hji]

/-- **Settling an instant** of at most 10 000 ready requests. -/
theorem settle_jobs {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) (hA : A0 m)
    (hf : m.ready.length ≤ 10000) :
    (settle Dd m).jobs = m.jobs ++ m.ready.flatMap (contrib g) ∧ A0 (settle Dd m) := by
  obtain ⟨g', h1, h2, -⟩ := DaiStable.drain_ci 10000 g m hI hf
  have hs : settle Dd m = drain Dd 10000 m := by
    unfold settle
    rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
    simp only [DaiSarathi.admitAll_dai, h2, List.isEmpty_nil, ↓reduceIte]
  rw [hs]
  exact drain_jobs 10000 g m hI hA hf

/-- `k` arrivals: the job list is unchanged, and the new sessions are ready,
in order, at the start of their program. -/
theorem injects_facts (m : Machine) (hA : A0 m) : ∀ k,
    ((inject DaiStable.arrived (fun _ => 0))^[k] m).jobs = m.jobs ∧
    ((inject DaiStable.arrived (fun _ => 0))^[k] m).ready = m.ready ++ List.range' m.sess.size k ∧
    ((inject DaiStable.arrived (fun _ => 0))^[k] m).sess.size = m.sess.size + k ∧
    A0 ((inject DaiStable.arrived (fun _ => 0))^[k] m) ∧
    (∀ j, m.sess.size ≤ j → j < m.sess.size + k →
      (getS ((inject DaiStable.arrived (fun _ => 0))^[k] m) j).prog = P1 ∧
      (getS ((inject DaiStable.arrived (fun _ => 0))^[k] m) j).status = .ready)
  | 0 => ⟨rfl, by simp, by simp, hA, fun j h1 h2 => by omega⟩
  | k + 1 => by
    obtain ⟨h1, h2, h3, h4, h5⟩ := injects_facts m hA k
    rw [Function.iterate_succ_apply']
    set M := (inject DaiStable.arrived (fun _ => 0))^[k] m with hM
    refine ⟨h1, ?_, ?_, ?_, ?_⟩
    · show M.ready ++ [M.sess.size] = _
      rw [h2, h3, List.range'_concat, List.append_assoc]
      simp
    · show (M.sess.push _).size = _
      simp [h3]; ring
    · intro j
      rw [DaiStable.getS_inject]
      split_ifs
      · rfl
      · exact h4 j
    · intro j hj1 hj2
      rw [DaiStable.getS_inject]
      split_ifs with h
      · exact ⟨rfl, rfl⟩
      · exact h5 j hj1 (by rw [h3] at h; omega)

/-- A ready session at the start of its program has arrived. -/
theorem cat_r1 {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) {j : ℕ} (hj : j < m.sess.size)
    (hp : (getS m j).prog = P1) (hs : (getS m j).status = .ready) : g.c j = .r1 := by
  obtain ⟨hsh, -, hfr⟩ := hI.sess j hj
  cases hc : g.c j <;> rw [hc] at hsh hfr <;>
    simp_all [DaiSarathi.Shape, DaiSarathi.fresh, P1, P2, P3, P4]

theorem flatMap_single {α β : Type} (f : α → List β) (c : α → β) :
    ∀ l : List α, (∀ x ∈ l, f x = [c x]) → l.flatMap f = l.map c
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.flatMap_cons, h x List.mem_cons_self,
      flatMap_single f c xs fun y hy => h y (List.mem_cons_of_mem _ hy)]
    rfl

theorem flatMap_ite {α β : Type} (f : α → List β) (p : α → Bool) (c : α → β) :
    ∀ l : List α, (∀ x ∈ l, f x = if p x then [c x] else []) → l.flatMap f = (l.filter p).map c
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.flatMap_cons, h x List.mem_cons_self,
      flatMap_ite f p c xs fun y hy => h y (List.mem_cons_of_mem _ hy)]
    by_cases hx : p x <;> simp [hx]

/-- An iteration starts: the job list is unchanged, and a running batch is
the greedy fill of the jobs. -/
theorem start_facts {g : Ghost} {m : Machine} (hI : DaiStable.CI g m) (hA : A0 m) :
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

/-- An iteration ends: each job loses its share; the finished ones leave the
list and become ready, in the list's order. -/
theorem handle_facts {m : Machine} {a qa : ℕ} (hie : m.iterEnd = some (a, qa))
    (hn : (m.jobs.map (·.owner)).Nodup) (hA : A0 m) (hb : ∀ j ∈ m.jobs, j.owner < m.sess.size) :
    (handle m a qa).jobs =
        (m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner }).filter (·.left ≠ 0) ∧
      (handle m a qa).ready = m.ready ++
        ((m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner }).filter (·.left = 0)).map
          (·.owner) ∧
      A0 (handle m a qa) := by
  set jobs' := m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner } with hjobs'
  set done := (jobs'.filter (·.left = 0)).map (·.owner) with hdone
  set M1 : Machine := { m with now := a, iterEnd := none, jobs := jobs'.filter (·.left ≠ 0), iter := [] } with hM1
  have hH : handle m a qa = Exec.readyAll done M1 := by
    unfold handle
    simp only [hie, if_true]
    rfl
  have hdn : done.Nodup := by
    refine List.Nodup.sublist ((List.filter_sublist).map _) ?_
    simpa [hjobs', List.map_map, Function.comp_def] using hn
  have hdb : ∀ i ∈ done, i < M1.sess.size := fun i hi => by
    obtain ⟨x, hx, rfl⟩ := List.mem_map.mp hi
    obtain ⟨x0, hx0, rfl⟩ := List.mem_map.mp (List.mem_filter.mp hx).1
    exact hb x0 hx0
  obtain ⟨f1, f2, -, -, -, -, -, -, -, -, f11⟩ := Exec.readyAll_fields done M1 hdn hdb
  rw [hH]
  refine ⟨f2, f1, fun j => ?_⟩
  rw [f11]
  split_ifs
  · exact hA j
  · exact hA j

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
  ∃ g, DaiStable.SB g m ∧ A0 m ∧
    (m.iterEnd.isSome = true → m.iter = fillIter Dd m.jobs 128 ∧ m.iter ≠ [])

theorem empty_inv : Inv DaiStable.empty :=
  ⟨_, DaiStable.empty_sb, fun j => by simp [getS, DaiStable.empty, Exec.initial],
    fun h => by simp [DaiStable.empty, Exec.initial] at h⟩

theorem slot_inv {m : Machine} (h : Inv m) (k : ℕ) (hk : k ≤ 10000) :
    Inv (DaiStable.slot k m) ∧ σ (DaiStable.slot k m) = absSlot k (σ m) := by
  obtain ⟨g, hB, hA, hit⟩ := h
  obtain ⟨g', hB', -⟩ := DaiStable.slot_sb hB k hk
  obtain ⟨g1, h1, -, -, h4, -⟩ := DaiStable.ci_injects hB.toCI k
  set m1 := (inject DaiStable.arrived (fun _ => 0))^[k] m with hm1
  obtain ⟨i1, i2, i3, i4, i5⟩ := injects_facts m hA k
  have hr1 : m1.ready = List.range' m.sess.size k := by rw [i2, hB.rdy, List.nil_append]
  have hlen : m1.ready.length ≤ 10000 := by rw [hr1, List.length_range']; exact hk
  have hnew : m1.ready.flatMap (contrib g1) =
      (List.range' m.sess.size k).map fun j => (⟨j, .prefill, 290, none⟩ : Job) := by
    rw [hr1]
    apply flatMap_single
    intro j hj
    rw [List.mem_range'_1] at hj
    have hc := cat_r1 h1 (by rw [i3]; omega) (i5 j hj.1 hj.2).1 (i5 j hj.1 hj.2).2
    simp [contrib, hc]
  have hσnew : ((List.range' m.sess.size k).map fun j => (⟨j, .prefill, 290, none⟩ : Job)).map
      (fun j => (j.mode, j.left)) = List.replicate k (.prefill, 290) := by
    simp [List.map_map, Function.comp_def, List.map_const']
  obtain ⟨g2, c1, c2, c3, -, -⟩ := DaiStable.settle_ci h1 hlen
  obtain ⟨sj, sA⟩ := settle_jobs h1 i4 hlen
  set m2 := settle Dd m1 with hm2
  have hsl : DaiStable.slot k m =
      if m.iterEnd.isSome then step Dd (afterEvent Dd m1) else afterEvent Dd m1 := rfl
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
    refine ⟨⟨g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
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
    have hA2 : afterEvent Dd m1 = m2 := DaiStable.after_busy m1 hie2
    have hstep : step Dd m2 = afterEvent Dd (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := DaiStable.ci_end c1 c2 hie2
    obtain ⟨hj, hrd, hAH⟩ := handle_facts hie2 c1.jobsNodup sA (fun j hj => (c1.jobs j hj).1)
    set H := handle m2 a qa with hH
    obtain ⟨g4, d1, d2, d3, -, -⟩ := DaiStable.settle_ci e1 (by omega)
    obtain ⟨sj2, sA2⟩ := settle_jobs e1 hAH (by omega)
    set m3 := settle Dd H with hm3
    have hie3 : m3.iterEnd = none := d3.iterEnd.trans e4
    have hpend3 : pendingBy m3 m3.now = false := by simp [pendingBy, nextEvent, hie3, d1.delays]
    have hA3 : afterEvent Dd H = startIteration Dd m3 := by unfold afterEvent; simp [← hm3, hie3, hpend3]
    obtain ⟨t1, t2, t3⟩ := start_facts d1 sA2
    have hS : DaiStable.slot k m = startIteration Dd m3 := by
      rw [hsl, hie]; simp only [Option.isSome_some, ↓reduceIte]; rw [hA2, hstep, hA3]
    rw [hS]
    refine ⟨⟨g', hS ▸ hB', t2, fun h => ?_⟩, ?_⟩
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
      have : shareOf it j = 0 := DaiStable.shareOf_eq_zero fun e he => by
        have := hB.iterOwn e (by rw [← hiter2]; exact he); omega
      simp [hsub, this]
    have hP0 : ∀ x ∈ newP, x.left ≠ 0 := fun x hx => by
      obtain ⟨j, -, rfl⟩ := List.mem_map.mp hx; simp
    have hHj : H.jobs = (m.jobs.map sub).filter (·.left ≠ 0) ++ newP := by
      rw [hj, hm2j, List.map_append, hsubP, List.filter_append,
        (List.filter_eq_self (l := newP)).mpr fun x hx => by simpa using hP0 x hx]
    have hdone : (H.ready).flatMap (contrib (g2.tick m2.iter)) =
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
      · simp [hsub, contrib, DaiSarathi.Ghost.tick, hc, hm, isJob, hle, ← hit2]
      · simp [hsub, contrib, DaiSarathi.Ghost.tick, hc, hm, isJob, hle, ← hit2]
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
theorem reach_inv {K : ℕ} (hK : K ≤ 10000) {m : Machine} (h : DaiStable.Reach K m) : Inv m := by
  induction h with
  | empty => exact empty_inv
  | slot k hk _ ih => exact (slot_inv ih k (hk.trans hK)).1

/-- The projection commutes with a slot. -/
theorem simulation {K : ℕ} (hK : K ≤ 10000) (x : DaiStable.State K) (k : ℕ) (hk : k ≤ K) :
    σ (DaiStable.slot k x.1) = absSlot k (σ x.1) :=
  (slot_inv (reach_inv hK x.2) k (hk.trans hK)).2

end DaiSim
end Papers
end SerqLang
