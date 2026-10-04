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

As for Sarathi (`Serq/Papers/DaiSim.lean`): `BariStable.CI` says what each
job is, and here we follow the list itself. No pool admits, so every
session keeps admission number 0 (`A0`) and a `run` on the engine appends
its job. A job's output is its request's `L i`, slot 11 of its session. -/

open DaiSarathi (Ghost Cat isJob isReady shareOf)
open BariStable (Q1 Q2 Q3 Q4)

local notation "Db" => BariStable.D

/-- Every session has admission number 0. -/
def A0 (m : Machine) : Prop := ∀ j, (getS m j).admSeq = 0

theorem A0_setS {m : Machine} (hA : A0 m) {i : ℕ} (s : Sess) (hs : s.admSeq = 0) (hi : i < m.sess.size) :
    A0 (setS m i s) := fun j => by
  rw [DaiSarathi.getS_setS m s hi]
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

/-- An arrived request's prefill of its prompt joins the end of the job list. -/
theorem exec_r1_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    (exec Db 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .prefill, (L i).1, none⟩] ∧
      A0 (exec Db 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, h10, -, hfit⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    DaiSarathi.exec_set Db _ m0 i 9 (fun x => x.now) Q2 (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := Q2 }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hA1 : A0 m1 := A0_setS (fun j => hA j) s1 (hA i) (by simpa using hin)
  have hg1 : getS m1 i = s1 := KongSvf.getS_setS_self m0 s1 (by simpa using hin)
  have hev : evalE m1 i (fun x => x.attr 10) = (L i).1 := by
    show (getS m1 i).attr.get 10 = _
    rw [hg1]; exact (DaiSarathi.attr_upd10 _ _ _ (by decide)).trans h10
  rw [KongSvf.exec_runEngine Db 9998 m1 i .prefill (fun x => x.attr 10) Q3 (by rw [hg1]; exact hst)
    (by rw [hg1]; rfl) (by rw [hev]; have := hfit.2.1; omega)]
  rw [span_all _ _ fun j _ => by simp [hA1 j.owner, hA1 i]]
  refine ⟨?_, fun j => ?_⟩
  · show m1.jobs ++ [⟨i, .prefill, evalE m1 i (fun x => x.attr 10), none⟩] = _
    rw [hev]; try rfl
  · show (getS (setS m1 i { getS m1 i with prog := Q3, status := .engine }) j).admSeq = 0
    exact A0_setS hA1 { getS m1 i with prog := Q3, status := .engine } (hA1 i)
      (by simpa [hm1, hm0] using hin) j

/-- A prefilled request's decode of its output joins the end of the job list. -/
theorem exec_r3_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    (exec Db 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .decode, (L i).2, none⟩] ∧
      A0 (exec Db 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, -, h11, hfit⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hA0 : A0 m0 := fun j => hA j
  have hev : evalE m0 i (fun x => x.attr 11) = (L i).2 := h11
  rw [show (10000 : ℕ) = 9999 + 1 from rfl,
    KongSvf.exec_runEngine Db 9999 m0 i .decode (fun x => x.attr 11) Q4 (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl) (by rw [hev]; have := hfit.2.2.2.1; omega)]
  rw [span_all _ _ fun j _ => by simp [hA0 j.owner, hA0 i]]
  refine ⟨?_, fun j => ?_⟩
  · show m0.jobs ++ [⟨i, .decode, evalE m0 i (fun x => x.attr 11), none⟩] = _
    rw [hev]; try rfl
  · show (getS (setS m0 i { getS m0 i with prog := Q4, status := .engine }) j).admSeq = 0
    exact A0_setS hA0 { getS m0 i with prog := Q4, status := .engine } (hA0 i) (by simpa [hm0] using hin) j

/-- A decoded request leaves the job list as it is. -/
theorem exec_r4_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    (exec Db 10000 { m with ready := rest } i).jobs = m.jobs ∧
      A0 (exec Db 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    KongSvf.exec_observe Db _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    KongSvf.getS_setS_self m0 _ (by simpa using hin)
  have hA1 : A0 m1 := fun j => by
    show (getS (setS m0 i { getS m0 i with prog := .stop }) j).admSeq = 0
    exact A0_setS (m := m0) (fun j => hA j) { getS m0 i with prog := .stop } (hA i) (by simpa using hin) j
  rw [KongSvf.exec_stop Db _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  exact ⟨rfl, A0_setS hA1 _ (hA1 i) (by simpa [hm1, hm0] using hin)⟩

/-- The job a ready request adds to the list when it runs. -/
def contrib (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) : List Job :=
  match g.c i with
  | .r1 => [⟨i, .prefill, (L i).1, none⟩]
  | .r3 => [⟨i, .decode, (L i).2, none⟩]
  | _ => []

/-- **Draining** appends each ready request's job, in the ready list's order. -/
theorem drain_jobs {L : ℕ → ℕ × ℕ} : ∀ (f : ℕ) (g : Ghost) (m : Machine), BariStable.CI L g m → A0 m →
    m.ready.length ≤ f →
    (drain Db f m).jobs = m.jobs ++ m.ready.flatMap (contrib L g) ∧ A0 (drain Db f m)
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
      have hsh := (hI.sess i hin).1
      have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
      obtain ⟨g1, hI1, hr1, hJ, hA1, hc1⟩ : ∃ g1, BariStable.CI L g1 (exec Db 10000 { m with ready := rest } i) ∧
          (exec Db 10000 { m with ready := rest } i).ready = rest ∧
          (exec Db 10000 { m with ready := rest } i).jobs = m.jobs ++ contrib L g i ∧
          A0 (exec Db 10000 { m with ready := rest } i) ∧ (∀ j, j ≠ i → g1.c j = g.c j) := by
        cases hc : g.c i with
        | r1 =>
          obtain ⟨h1, h2, -, -⟩ := BariStable.ci_r1 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r1_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | r3 =>
          obtain ⟨h1, h2, -, -⟩ := BariStable.ci_r3 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r3_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | r4 =>
          obtain ⟨h1, h2, -, -⟩ := BariStable.ci_r4 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r4_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2,
            fun j hj => by simp [DaiSarathi.Ghost.set_c, hj]⟩
        | s0 => rw [hc] at hsh; exact hsh.elim
        | w => rw [hc] at hsh; exact hsh.elim
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨d1, d2⟩ := drain_jobs f g1 _ hI1 hA1 (by rw [hr1]; exact hlen)
      refine ⟨?_, d2⟩
      rw [d1, hJ, hr1, hr, List.flatMap_cons, List.append_assoc]
      congr 2
      exact List.flatMap_congr fun j hj => by
        have hji : j ≠ i := fun h => hir (h ▸ hj)
        simp only [contrib, hc1 j hji]

/-- **Settling an instant** of at most 10 000 ready requests. -/
theorem settle_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) (hA : A0 m)
    (hf : m.ready.length ≤ 10000) :
    (settle Db m).jobs = m.jobs ++ m.ready.flatMap (contrib L g) ∧ A0 (settle Db m) := by
  obtain ⟨g', h1, h2, -⟩ := BariStable.drain_ci 10000 g m hI hf
  have hs : settle Db m = drain Db 10000 m := by
    unfold settle
    rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
    simp only [BariRad.admitAll_rad, h2, List.isEmpty_nil, ↓reduceIte]
  rw [hs]
  exact drain_jobs 10000 g m hI hA hf

/-- The machine after a slot's arrivals. -/
def injL (rs : List (ℕ × ℕ)) (m : Machine) : Machine :=
  (rs.map BariStable.attrs).foldl (fun m a => inject BariStable.arrived a m) m

/-- A slot's arrivals: the job list is unchanged, the old sessions too, and
the new ones are ready, in order, with their lengths. -/
theorem injects_facts : ∀ (rs : List (ℕ × ℕ)) (m : Machine), A0 m →
    (injL rs m).jobs = m.jobs ∧ (injL rs m).ready = m.ready ++ List.range' m.sess.size rs.length ∧
    (injL rs m).sess.size = m.sess.size + rs.length ∧ A0 (injL rs m) ∧
    (∀ j < m.sess.size, getS (injL rs m) j = getS m j) ∧
    (∀ t < rs.length, getS (injL rs m) (m.sess.size + t) =
      ⟨m.sess.size + t, ⟨BariStable.attrs (rs.getD t (0, 0)), []⟩, 0, BariStable.arrived, [], .ready, 0, 0⟩)
  | [], m, hA => ⟨rfl, by simp [injL], by simp [injL], hA, fun _ _ => rfl, fun t ht => by simp at ht⟩
  | r :: rs, m, hA => by
    set m' := inject BariStable.arrived (BariStable.attrs r) m with hm'
    have hsz : m'.sess.size = m.sess.size + 1 := by simp [hm', inject]
    have hA' : A0 m' := fun j => by
      rw [hm', BariStable.getS_inject]; split_ifs
      · rfl
      · exact hA j
    have hg' : ∀ j, getS m' j = if j = m.sess.size then
        ⟨m.sess.size, ⟨BariStable.attrs r, []⟩, 0, BariStable.arrived, [], .ready, 0, 0⟩ else getS m j :=
      BariStable.getS_inject r m
    have hI : injL (r :: rs) m = injL rs m' := rfl
    obtain ⟨h1, h2, h3, h4, h5, h6⟩ := injects_facts rs m' hA'
    rw [hI]
    refine ⟨h1, ?_, by rw [h3, hsz]; simp; ring, h4, fun j hj => ?_, fun t ht => ?_⟩
    · rw [h2, hsz]
      show m.ready ++ [m.sess.size] ++ _ = _
      simp [List.range'_succ]
    · rw [h5 j (by rw [hsz]; omega), hg', if_neg (by omega)]
    · cases t with
      | zero =>
        rw [h5 _ (by rw [hsz]; omega), hg', if_pos (by omega)]
        simp
      | succ t =>
        have := h6 t (by simp at ht; omega)
        rw [hsz] at this
        rw [show m.sess.size + (t + 1) = m.sess.size + 1 + t by ring, this]
        simp

/-- A ready session at the start of its program has arrived. -/
theorem cat_r1 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) {j : ℕ}
    (hj : j < m.sess.size) (hp : (getS m j).prog = Q1) (hs : (getS m j).status = .ready) : g.c j = .r1 := by
  have hsh := (hI.sess j hj).1
  cases hc : g.c j <;> rw [hc] at hsh <;> simp_all [BariStable.Shape, Q1, Q2, Q3, Q4]

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

theorem range_map {β : Type} (f : ℕ → β) (g : ℕ × ℕ → β) :
    ∀ (rs : List (ℕ × ℕ)) (n : ℕ), (∀ t < rs.length, f (n + t) = g (rs.getD t (0, 0))) →
      (List.range' n rs.length).map f = rs.map g
  | [], _, _ => rfl
  | r :: rs, n, h => by
    rw [List.length_cons, List.range'_succ, List.map_cons, List.map_cons]
    have h0 := h 0 (by simp)
    simp only [Nat.add_zero, List.getD_cons_zero] at h0
    rw [h0, range_map f g rs (n + 1) fun t ht => by
      have := h (t + 1) (by simp; omega)
      rw [show n + (t + 1) = n + 1 + t by ring] at this
      simpa using this]

/-- An iteration starts: the job list is unchanged, and a running batch is
the greedy fill of the jobs RAD serves. -/
theorem start_facts {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) (hA : A0 m) :
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
  set M1 : Machine := { m with now := a, iterEnd := none, jobs := jobs'.filter (·.left ≠ 0), iter := [] }
    with hM1
  have hH : handle m a qa = KongSvf.readyAll done M1 := by
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
  obtain ⟨f1, f2, -, -, -, -, -, -, -, -, f11⟩ := KongSvf.readyAll_fields done M1 hdn hdb
  rw [hH]
  refine ⟨f2, f1, fun j => ?_⟩
  rw [f11]
  split_ifs
  · exact hA j
  · exact hA j

/-! ### The batch on the list -/

/-- A job list, each job with its request's output. -/
def σL (L : ℕ → ℕ × ℕ) (js : List Job) : List AJob := js.map fun j => (j.mode, j.left, (L j.owner).2)

theorem σ_eq {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : BariStable.CI L g m) : σ m = σL L m.jobs :=
  List.map_congr_left fun j hj => by
    rw [(hI.sess _ (hI.jobs j hj).1).2.2.2.1]

theorem shareOf_fill_out (js : List Job) (B o : ℕ) (h : o ∉ js.map (·.owner)) :
    shareOf (fillIter Db js B) o = 0 := by
  have h1 := DaiSarathi.shareOf_fillIter Db js B o
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
          · simp [DaiSarathi.shareOf_cons, DaiSarathi.shareOf_nil]
          · refine List.map_congr_left fun x hx => ?_
            simp [DaiSarathi.shareOf_cons, hne x hx, DaiSarathi.shareOf_nil]
        · rw [if_neg h1, ← ih (B - min (wantOf Db j) B)]
          congr 1
          · simp [DaiSarathi.shareOf_cons, hout]
          · refine List.map_congr_left fun x hx => ?_
            simp [DaiSarathi.shareOf_cons, hne x hx]
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
  ⟨_, _, BariStable.empty_sb, fun j => by simp [getS, BariStable.empty, Exec.initial],
    fun h => by simp [BariStable.empty, Exec.initial] at h⟩

theorem slot_inv {m : Machine} (h : Inv m) (rs : List (ℕ × ℕ)) (hl : rs.length ≤ 10000)
    (hf : ∀ r ∈ rs, BariStable.Fits r) :
    Inv (BariStable.slot rs m) ∧ σ (BariStable.slot rs m) = absSlot rs (σ m) := by
  obtain ⟨L, g, hB, hA, hit⟩ := h
  obtain ⟨L', g', hB', -⟩ := BariStable.slot_sb hB rs hl hf
  obtain ⟨L1, g1, h1, -, -, h4, -⟩ := BariStable.ci_injects rs hB.toCI hf
  set m1 := injL rs m with hm1
  change BariStable.CI L1 g1 m1 at h1
  change DaiSarathi.Keeps m1 m at h4
  obtain ⟨i1, i2, i3, i4, i5, i6⟩ := injects_facts rs m hA
  rw [← hm1] at i1 i2 i3 i4 i5 i6
  have hr1 : m1.ready = List.range' m.sess.size rs.length := by rw [i2, hB.rdy, List.nil_append]
  have hlen : m1.ready.length ≤ 10000 := by rw [hr1, List.length_range']; exact hl
  -- the new requests' lengths
  have hLn : ∀ t < rs.length, L1 (m.sess.size + t) = rs.getD t (0, 0) := by
    intro t ht
    obtain ⟨-, -, a10, a11, -⟩ := h1.sess (m.sess.size + t) (by rw [i3]; omega)
    rw [i6 t ht] at a10 a11
    simp only [Attrs.get, List.getD_nil, BariStable.attrs] at a10 a11
    simp at a10 a11
    exact Prod.ext a10.symm a11.symm
  set newP := (List.range' m.sess.size rs.length).map fun j => (⟨j, .prefill, (L1 j).1, none⟩ : Job)
    with hnewP
  have hnew : m1.ready.flatMap (contrib L1 g1) = newP := by
    rw [hr1]
    apply flatMap_single
    intro j hj
    rw [List.mem_range'_1] at hj
    obtain ⟨t, rfl⟩ : ∃ t, j = m.sess.size + t := ⟨j - m.sess.size, by omega⟩
    have hc := cat_r1 h1 (j := m.sess.size + t) (by rw [i3]; omega) (by rw [i6 t (by omega)]; rfl)
      (by rw [i6 t (by omega)])
    simp [contrib, hc]
  have hσnew : σL L1 newP = rs.map fun r => ((.prefill, r.1, r.2) : AJob) := by
    rw [hnewP, σL, List.map_map]
    apply range_map
    intro t ht
    simp [hLn t ht]
  -- the old jobs, read with the new lengths
  have hσm : σ m = σL L1 m.jobs := by
    rw [σ_eq hB.toCI]
    refine List.map_congr_left fun j hj => ?_
    have hj1 := (hB.jobs j hj).1
    have e1 := (hB.sess _ hj1).2.2.2.1
    have e2 := (h1.sess j.owner (by rw [i3]; omega)).2.2.2.1
    rw [i5 _ hj1] at e2
    simp only [Prod.mk.injEq, true_and]
    rw [← e1, ← e2]
  obtain ⟨g2, c1, c2, c3, -, -⟩ := BariStable.settle_ci h1 hlen
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
    have hA2 : afterEvent Db m1 = m2 := BariStable.after_busy m1 hie2
    have hstep : step Db m2 = afterEvent Db (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := BariStable.ci_end c1 c2 hie2
    obtain ⟨hj, hrd, hAH⟩ := handle_facts hie2 c1.jobsNodup sA (fun j hj => (c1.jobs j hj).1)
    set H := handle m2 a qa with hH
    obtain ⟨g4, d1, d2, d3, -, -⟩ := BariStable.settle_ci e1 (by omega)
    obtain ⟨sj2, sA2⟩ := settle_jobs e1 hAH (by omega)
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
      have : shareOf it j = 0 := DaiStable.shareOf_eq_zero fun e he => by
        have := hB.iterOwn e (by rw [← hiter2]; exact he); omega
      simp [hsub, this]
    have hP0 : ∀ x ∈ newP, x.left ≠ 0 := fun x hx => by
      obtain ⟨j, hj, rfl⟩ := List.mem_map.mp hx
      rw [List.mem_range'_1] at hj
      have := (h1.sess j (by rw [i3]; omega)).2.2.2.2.2.1
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
      · simp [hsub, contrib, DaiSarathi.Ghost.tick, hc, hm, isJob, hle, ← hit2]
      · simp [hsub, contrib, DaiSarathi.Ghost.tick, hc, hm, isJob, hle, ← hit2]
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
