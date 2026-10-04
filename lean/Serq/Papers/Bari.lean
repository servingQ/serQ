/-
# Bari, Hegde, de Veciana: optimal scheduling for LLM inference

The claims of `examples/papers/bari_rad.sq` (arXiv 2508.01002: one inference
node running RAD with `b_col = b_row = b_red = 128`, the batch time (7)
without its attention terms, time in µs), proved about the executable
semantics of that program.

* `token_rate`, Theorem 1: before every iteration the node has served at
  most one token per `t_Lin / b_col + 1 / c_nLin` µs, under any scheduler.
  It is `Exec.served_rate` with the tiled cost: a batch of `b ≤ b_col`
  tokens lasts `t_Lin ⌈b / b_col⌉ + b / c_nLin ≥ b (t_Lin / b_col + 1 / c_nLin)`.
-/
import Serq.Claims
import Serq.Work

namespace SerqLang
namespace Papers

open Exec

/-- The tiled cost's rate: a batch of `b ≤ bcol` tokens lasts at least
`b (tlin / bcol + tnl)`. -/
theorem tiled_rate (tlin tnl bcol b : ℕ) (hb : b ≤ bcol) (hpos : 0 < bcol) :
    b * (tlin + tnl * bcol) ≤ bcol * max 1 (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) := by
  rcases Nat.eq_zero_or_pos b with rfl | hb0
  · simp
  have hq : 1 ≤ (b + (bcol - 1)) / bcol := (Nat.le_div_iff_mul_le hpos).mpr (by omega)
  calc b * (tlin + tnl * bcol) = b * tlin + bcol * (tnl * b) := by ring
    _ ≤ bcol * tlin + bcol * (tnl * b) := Nat.add_le_add_right (Nat.mul_le_mul_right _ hb) _
    _ ≤ bcol * (tlin * ((b + (bcol - 1)) / bcol)) + bcol * (tnl * b) := by
        apply Nat.add_le_add_right
        apply Nat.mul_le_mul_left
        exact Nat.le_mul_of_pos_right _ hq
    _ = bcol * (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) := by ring
    _ ≤ bcol * max 1 (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) :=
        Nat.mul_le_mul_left _ (le_max_right _ _)

namespace BariRad

theorem token_rate : Claims.BariRad.token_rate :=
  served_rate _ (fun st h => tiled_rate 4000 5 128 st.tokens h (by norm_num)) _ _

end BariRad

namespace BariRad

local notation "Dr" => Claims.BariRad.deployment
local notation "Pr" => Claims.BariRad.prog

/-! ### The tiling claim

RAD's program runs the gate, sets `t0`, runs a prefill of `vp` and a decode
of `vd`, observes and ends: no hold, no branch, no loop, and `vp` is never
set (`RadOK`). The invariant `R` carries what the claim needs from one
iteration to the next: every job at the engine has work left, a prefill's
is a multiple of 128 (it starts at `vp`, Assumption 3, and loses 128 each
time it is served), the jobs' owners are distinct and at the engine, and an
entry of the running batch is a full tile or a decode's token. At the start
of an iteration the batch is the greedy fill of the residents RAD serves
(`assign_eq_fillIter_only`): in Decode Mode one token to each of up to 128
decodes, in Prefill Mode one chunk of 128 of the oldest prefill. -/

/-- The statements of the RAD program. -/
def RadOK : Prog → Prop
  | .done => True
  | .stop => True
  | .set slot _ k => slot ≠ 10 ∧ RadOK k
  | .observe _ _ k => RadOK k
  | .run s md w g k => g = none ∧
      ((s ≠ 0 ∧ md = .plain) ∨ (s = 0 ∧ md = .prefill ∧ ∀ x, w x = x.attr 10) ∨
        (s = 0 ∧ md = .decode)) ∧ RadOK k
  | _ => False

theorem radOK_sub {q : Prog} (h : Sub Pr q) : RadOK q := by
  induction h with
  | refl =>
    simp [Claims.BariRad.prog, RadOK]
  | turn _ ih => exact ih.elim
  | set _ ih => exact ih.2
  | observe _ ih => exact ih
  | run _ ih => exact ih.2.2
  | holdBody _ ih => exact ih.elim
  | holdK _ ih => exact ih.elim
  | yes _ ih => exact ih.elim
  | no _ ih => exact ih.elim
  | branchK _ ih => exact ih.elim
  | loopBody _ ih => exact ih.elim

/-- Session `i` of an array, as `getS` reads it. -/
def sg (ss : Array Sess) (i : ℕ) : Sess := ss.getD i ⟨i, ⟨fun _ => 0, []⟩, 0, .stop, [], .ended, 0, 0⟩

theorem getS_eq_sg (m : Machine) (i : ℕ) : getS m i = sg m.sess i := rfl

/-- What one iteration hands to the next. It reads the sessions, the jobs and
the batch only, so a machine that differs elsewhere satisfies it by
definition. -/
def RS (ss : Array Sess) (js : List Job) (it : List (ℕ × ℕ)) : Prop :=
  (∀ j ∈ js, 0 < j.left ∧ (j.mode = .prefill → 128 ∣ j.left) ∧
    (sg ss j.owner).status = .engine ∧ j.mode ≠ .plain ∧ j.growing = none) ∧
  (js.map (·.owner)).Nodup ∧
  (∀ s ∈ ss.toList, 128 ∣ s.attr.get 10) ∧
  (∀ s ∈ ss.toList, s.stack = []) ∧
  (∀ e ∈ it, e.2 = 128 ∨ ∀ j ∈ js, j.owner = e.1 → j.mode = .decode) ∧
  (∀ e ∈ it, (sg ss e.1).status = .engine)

abbrev R (m : Machine) : Prop := RS m.sess m.jobs m.iter

theorem R.jobs {m : Machine} (h : R m) : ∀ j ∈ m.jobs, 0 < j.left ∧ (j.mode = .prefill → 128 ∣ j.left) ∧
    (getS m j.owner).status = .engine ∧ j.mode ≠ .plain ∧ j.growing = none := h.1
theorem R.nodup {m : Machine} (h : R m) : (m.jobs.map (·.owner)).Nodup := h.2.1
theorem R.attr {m : Machine} (h : R m) : ∀ s ∈ m.sess.toList, 128 ∣ s.attr.get 10 := h.2.2.1
theorem R.stack {m : Machine} (h : R m) : ∀ s ∈ m.sess.toList, s.stack = [] := h.2.2.2.1
theorem R.iterT {m : Machine} (h : R m) :
    ∀ e ∈ m.iter, e.2 = 128 ∨ ∀ j ∈ m.jobs, j.owner = e.1 → j.mode = .decode := h.2.2.2.2.1
theorem R.iterS {m : Machine} (h : R m) : ∀ e ∈ m.iter, (getS m e.1).status = .engine := h.2.2.2.2.2

theorem R.mk' {m : Machine}
    (h1 : ∀ j ∈ m.jobs, 0 < j.left ∧ (j.mode = .prefill → 128 ∣ j.left) ∧
      (getS m j.owner).status = .engine ∧ j.mode ≠ .plain ∧ j.growing = none)
    (h2 : (m.jobs.map (·.owner)).Nodup) (h3 : ∀ s ∈ m.sess.toList, 128 ∣ s.attr.get 10)
    (h4 : ∀ s ∈ m.sess.toList, s.stack = [])
    (h5 : ∀ e ∈ m.iter, e.2 = 128 ∨ ∀ j ∈ m.jobs, j.owner = e.1 → j.mode = .decode)
    (h6 : ∀ e ∈ m.iter, (getS m e.1).status = .engine) : R m :=
  ⟨h1, h2, h3, h4, h5, h6⟩

theorem mem_setS_sess {m : Machine} {i : ℕ} {s x : Sess} (hx : x ∈ (setS m i s).sess.toList) :
    x ∈ m.sess.toList ∨ x = s := by
  simp only [setS] at hx
  rw [Array.toList_setIfInBounds] at hx
  by_cases hi : i < m.sess.size
  · exact List.mem_or_eq_of_mem_set hx
  · rw [List.set_eq_of_length_le (by simpa using hi)] at hx
    exact Or.inl hx

/-- Session `i`, not at the engine, changes into a session not at the engine
that keeps `vp` a multiple of 128 and an empty stack. -/
theorem R.upd {m : Machine} (h : R m) (i : ℕ) (s : Sess) (hi : (getS m i).status ≠ .engine)
    (hs : s.status ≠ .engine) (ha : 128 ∣ s.attr.get 10) (hk : s.stack = []) : R (setS m i s) := by
  have hne : ∀ o, (getS m o).status = .engine → o ≠ i := fun o ho he => hi (he ▸ ho)
  refine R.mk' (fun j hj => ?_) h.nodup (fun x hx => ?_) (fun x hx => ?_) h.iterT (fun e he => ?_)
  · obtain ⟨a, b, c, d, e⟩ := h.jobs j hj
    exact ⟨a, b, by rw [getS_setS_ne m s (hne _ c).symm]; exact c, d, e⟩
  · rcases mem_setS_sess hx with hx | rfl
    · exact h.attr x hx
    · exact ha
  · rcases mem_setS_sess hx with hx | rfl
    · exact h.stack x hx
    · exact hk
  · have := h.iterS e he
    rw [getS_setS_ne m s (hne _ this).symm]; exact this

theorem lt_size_of_engine {m : Machine} {i : ℕ} (h : (getS m i).status = .engine) : i < m.sess.size := by
  by_contra hi
  simp [getS, Array.getD_eq_getD_getElem?, hi] at h

theorem R.attr_get {m : Machine} (h : R m) (i : ℕ) : 128 ∣ (getS m i).attr.get 10 := by
  by_cases hi : i < m.sess.size
  · exact h.attr _ (getS_mem m hi)
  · simp [getS, Array.getD_eq_getD_getElem?, hi, Attrs.get]

theorem R.stack_get {m : Machine} (h : R m) (i : ℕ) : (getS m i).stack = [] := by
  by_cases hi : i < m.sess.size
  · exact h.stack _ (getS_mem m hi)
  · simp [getS, Array.getD_eq_getD_getElem?, hi]

theorem attr_upd10 (a : Attrs) (k v : ℕ) (hk : k ≠ 10) : (a.upd k v).get 10 = a.get 10 := by
  rw [Attrs.get_upd]; simp [Function.update, Ne.symm hk]

theorem rad_exec : ∀ (f : ℕ) (m : Machine) (i : ℕ), Inv Pr m → R m → R (exec Dr f m i)
  | 0, _, _, _, h => h
  | f + 1, m, i, hI, h => by
    unfold exec
    simp only
    split
    · exact h
    · rename_i hrd
      have hrd' : (getS m i).status = .ready := by simpa using hrd
      have hne : (getS m i).status ≠ .engine := by rw [hrd']; decide
      have ho := hI.sessOK (lt_of_ready m hrd')
      have hrad := radOK_sub ho.1
      have hstk := h.stack_get i
      split
      · rename_i hp
        split
        · refine h.upd i _ hne ?_ (h.attr_get i) hstk; simp
        · rename_i hst; rw [hstk] at hst; simp at hst
        · rename_i hst; rw [hstk] at hst; simp at hst
        · rename_i hst; rw [hstk] at hst; simp at hst
      · unfold endSession
        simp only [hstk, List.foldl_nil]
        refine h.upd i _ hne ?_ (h.attr_get i) rfl; simp
      · rename_i k hp; rw [hp] at hrad; exact hrad.elim
      · rename_i slot e k hp
        rw [hp] at hrad
        refine rad_exec f _ i (hI.upd' i _ fun ho => ⟨Sub.set (hp ▸ ho.1), ho.2⟩) ?_
        refine h.upd i _ hne ?_ ?_ hstk
        · simpa using hne
        · simp only; rw [attr_upd10 _ _ _ hrad.1]; exact h.attr_get i
      · rename_i n e k hp
        refine rad_exec f _ i (hI.upd' i _ fun ho => ⟨Sub.observe (hp ▸ ho.1), ho.2⟩) ?_
        refine h.upd i _ hne ?_ (h.attr_get i) hstk
        simpa using hne
      · rename_i p a b k hp; rw [hp] at hrad; exact hrad.elim
      · rename_i b hp; rw [hp] at hrad; exact hrad.elim
      · rename_i st md w g k hp
        rw [hp] at hrad
        obtain ⟨hg, hmode, _⟩ := hrad
        split
        · refine h.upd i _ hne ?_ (h.attr_get i) hstk
          simp
        · split
          · refine h.upd i _ hne ?_ (h.attr_get i) hstk
            simp
          · rename_i hw hst0
            have hst : st = 0 := by simpa using hst0
            subst hst
            have hi := lt_of_ready m hrd'
            have hmd : md ≠ .plain := by rcases hmode with h | h | h <;> simp_all
            have hpre : md = .prefill → 128 ∣ evalE m i w := by
              intro hm
              rcases hmode with h' | ⟨_, _, hw10⟩ | ⟨_, h'⟩
              · exact absurd h'.1 (by simp)
              · simp only [evalE, hw10]; exact h.attr_get i
              · rw [hm] at h'; exact absurd h' (by decide)
            have hown : ∀ j ∈ m.jobs, j.owner ≠ i := fun j hj he =>
              hne (he ▸ (h.jobs j hj).2.2.1)
            have hiter : ∀ e ∈ m.iter, e.1 ≠ i := fun e he he' => hne (he' ▸ h.iterS e he)
            generalize hsp : List.span _ m.jobs = ab
            obtain ⟨a, b⟩ := ab
            have hab : a ++ b = m.jobs := by
              rw [List.span_eq_takeWhile_dropWhile] at hsp
              simp only [Prod.mk.injEq] at hsp
              rw [← hsp.1, ← hsp.2, List.takeWhile_append_dropWhile]
            have hmem : ∀ j, j ∈ a ++ ⟨i, md, evalE m i w, g⟩ :: b → j ∈ m.jobs ∨ j = ⟨i, md, evalE m i w, g⟩ := by
              intro j hj
              simp only [List.mem_append, List.mem_cons] at hj
              rcases hj with hj | hj | hj
              · exact Or.inl (hab ▸ List.mem_append_left _ hj)
              · exact Or.inr hj
              · exact Or.inl (hab ▸ List.mem_append_right _ hj)
            have hS : getS (setS m i { getS m i with prog := k, status := .engine }) i =
                { getS m i with prog := k, status := .engine } := by
              simp [getS, setS, Array.getD_eq_getD_getElem?, hi]
            refine R.mk' (fun j hj => ?_) ?_ (fun x hx => ?_) (fun x hx => ?_) (fun e he => ?_) (fun e he => ?_)
            · rcases hmem j hj with hj | rfl
              · obtain ⟨a1, a2, a3, a4, a5⟩ := h.jobs j hj
                exact ⟨a1, a2, (congrArg Sess.status (getS_setS_ne m _ (hown j hj).symm)).trans a3, a4, a5⟩
              · exact ⟨Nat.pos_of_ne_zero hw, hpre, (congrArg Sess.status hS).trans rfl, hmd, hg⟩
            · have hp : List.Perm ((a ++ ⟨i, md, evalE m i w, g⟩ :: b).map (·.owner))
                  (i :: (a ++ b).map (·.owner)) := by
                simp only [List.map_append, List.map_cons]
                exact List.perm_middle
              refine hp.nodup_iff.mpr (List.nodup_cons.mpr ⟨fun hm => ?_, hab ▸ h.nodup⟩)
              obtain ⟨j, hj, hjo⟩ := List.mem_map.mp hm
              exact hown j (hab ▸ hj) hjo
            · rcases mem_setS_sess hx with hx | rfl
              · exact h.attr x hx
              · exact h.attr_get i
            · rcases mem_setS_sess hx with hx | rfl
              · exact h.stack x hx
              · exact hstk
            · rcases h.iterT e he with h1 | h1
              · exact Or.inl h1
              · refine Or.inr fun j hj hjo => ?_
                rcases hmem j hj with hj | rfl
                · exact h1 j hj hjo
                · exact absurd hjo.symm (hiter e he)
            · exact (congrArg Sess.status (getS_setS_ne m _ (hiter e he).symm)).trans (h.iterS e he)
      · rename_i hp; rw [hp] at hrad; exact hrad.elim

theorem admitAll_rad (m : Machine) : admitAll Dr m = m := by
  simp [admitAll, Claims.BariRad.deployment]

theorem rad_drain : ∀ (f : ℕ) (m : Machine), Inv Pr m → R m → R (drain Dr f m)
  | 0, _, _, h => h
  | f + 1, m, hI, h => by
    unfold drain
    split
    · exact h
    · rename_i i q _
      exact rad_drain f _ (inv_exec Dr 10000 { m with ready := q } i hI)
        (rad_exec 10000 { m with ready := q } i hI h)

theorem rad_settle (m : Machine) (hI : Inv Pr m) (h : R m) : R (settle Dr m) := by
  unfold settle
  suffices ∀ f (m : Machine), Inv Pr m → R m → R (settleLoop Dr f m) from this 1000 m hI h
  intro f
  induction f with
  | zero => exact fun _ _ h => h
  | succ f ih =>
    intro m hI h
    unfold settleLoop
    simp only [admitAll_rad]
    have h1 := rad_drain 10000 m hI h
    split
    · exact h1
    · exact ih _ (inv_drain Dr 10000 m hI) h1

theorem R.upd_free {m : Machine} (h : R m) (i : ℕ) (s : Sess) (hj : ∀ j ∈ m.jobs, j.owner ≠ i)
    (he : m.iter = []) (ha : 128 ∣ s.attr.get 10) (hk : s.stack = []) : R (setS m i s) := by
  refine R.mk' (fun j hj' => ?_) h.nodup (fun x hx => ?_) (fun x hx => ?_) (by simp [setS, he])
    (by simp [setS, he])
  · obtain ⟨a, b, c, d, e⟩ := h.jobs j hj'
    exact ⟨a, b, (congrArg Sess.status (getS_setS_ne m s (hj j hj').symm)).trans c, d, e⟩
  · rcases mem_setS_sess hx with hx | rfl
    · exact h.attr x hx
    · exact ha
  · rcases mem_setS_sess hx with hx | rfl
    · exact h.stack x hx
    · exact hk

theorem rad_fold : ∀ (done : List ℕ) (M : Machine), R M → M.iter = [] →
    (∀ i ∈ done, ∀ j ∈ M.jobs, j.owner ≠ i) →
    R (done.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) M)
  | [], _, h, _, _ => h
  | i :: rest, M, h, hit, hf => by
    simp only [List.foldl_cons]
    exact rad_fold rest _ (h.upd_free i _ (hf i List.mem_cons_self) hit (h.attr_get i) (h.stack_get i)) hit
      (fun i' hi' => hf i' (List.mem_cons_of_mem _ hi'))

theorem rad_handle (m : Machine) (t q : ℕ) (h : R m) : R (handle m t q) := by
  unfold handle
  simp only
  split
  · -- the iteration ends: the finished jobs leave, their sessions are ready
    unfold endIteration
    simp only
    refine rad_fold _ _ ?_ rfl ?_
    · refine R.mk' (fun j hj => ?_) ?_ h.attr h.stack (fun e he => by simp at he)
        (fun e he => by simp at he)
      · obtain ⟨hj, hl⟩ := List.mem_filter.mp hj
        obtain ⟨j0, hj0, rfl⟩ := List.mem_map.mp hj
        obtain ⟨a1, a2, a3, a4, a5⟩ := h.jobs j0 hj0
        refine ⟨Nat.pos_of_ne_zero (by simpa using hl), fun hp => ?_, a3, a4, a5⟩
        apply Nat.dvd_sub (a2 hp)
        apply List.dvd_sum
        intro x hx
        obtain ⟨e, he, rfl⟩ := List.mem_map.mp hx
        obtain ⟨he, heo⟩ := List.mem_filter.mp he
        rcases h.iterT e he with h1 | h1
        · rw [h1]
        · have := h1 j0 hj0 (by simp only [decide_eq_true_eq] at heo; exact heo.symm)
          rw [hp] at this; exact absurd this (by decide)
      · refine List.Nodup.sublist ((List.filter_sublist).map _) ?_
        simpa [List.map_map, Function.comp_def] using h.nodup
    · intro i hi j hj hji
      obtain ⟨j1, hj1, rfl⟩ := List.mem_map.mp hi
      obtain ⟨hj1, hl1⟩ := List.mem_filter.mp hj1
      obtain ⟨hj, hl⟩ := List.mem_filter.mp hj
      obtain ⟨a, ha, rfl⟩ := List.mem_map.mp hj1
      obtain ⟨b, hb, rfl⟩ := List.mem_map.mp hj
      have : b = a := List.inj_on_of_nodup_map h.nodup hb ha hji
      subst this
      simp_all
  · split
    · rename_i u q' i rest _
      split
      · rename_i hst
        refine h.upd i _ (by rw [getS_eq_sg] at hst ⊢; simp_all) (by simp) (h.attr_get i) (h.stack_get i)
      · exact h
    · exact h

theorem rad_initial (w : Workload) (hw : Claims.BariRad.family_optimal_tiling w) :
    R (Exec.initial Dr w.init.length w.attr Pr w) := by
  refine R.mk' (fun j hj => by simp [Exec.initial] at hj) (by simp [Exec.initial])
    (fun s hs => ?_) (fun s hs => ?_) (fun e he => by simp [Exec.initial] at he)
    (fun e he => by simp [Exec.initial] at he)
  · simp only [Exec.initial, List.toList_toArray, List.mem_map, List.mem_range] at hs
    obtain ⟨i, hi, rfl⟩ := hs
    have := hw.2.2.2.2.2 i hi
    show 128 ∣ (⟨w.attr i, []⟩ : Attrs).get 10
    simp only [Attrs.get, List.getD_nil]
    exact ⟨_, this⟩
  · simp only [Exec.initial, List.toList_toArray, List.mem_map, List.mem_range] at hs
    obtain ⟨i, hi, rfl⟩ := hs
    rfl

theorem find_owner {js : List Job} (hn : (js.map (·.owner)).Nodup) {j : Job} (hj : j ∈ js) :
    js.find? (fun x => decide (x.owner = j.owner)) = some j := by
  induction js with
  | nil => simp at hj
  | cons x xs ih =>
    simp only [List.map_cons, List.nodup_cons] at hn
    rcases List.mem_cons.mp hj with rfl | hj
    · simp
    · have hne : x.owner ≠ j.owner := fun he => hn.1 (he ▸ List.mem_map.mpr ⟨j, hj, rfl⟩)
      simp [List.find?_cons, hne, ih hn.2 hj]

/-- RAD's mode, as its `serve only` reads it. -/
def decodeMode (m : Machine) : Prop :=
  128 ≤ (m.jobs.filter (·.mode = .decode)).length ∨ (m.jobs.filter (·.mode = .decode)).length = m.jobs.length

theorem serves_rad (m : Machine) (j : Job) :
    serves Dr m j = true ↔ (decodeMode m → j.mode = .decode) ∧ (¬ decodeMode m → j.mode ≠ .decode) := by
  unfold serves decodeMode
  simp only [Claims.BariRad.deployment]
  generalize (m.jobs.filter (·.mode = .decode)).length = d
  generalize m.jobs.length = r
  by_cases hd : j.mode = .decode <;> by_cases hc : (128 ≤ d ∨ d = r) <;> simp [hd, hc] <;> omega

/-- The batch RAD forms from `m`: in Decode Mode a token to each of up to 128
decodes, in Prefill Mode one chunk of 128 of the oldest prefill. Either way
an entry is a full tile or a decode's, and the batch fills its tile unless
every resident decodes. -/
theorem rad_batch (m : Machine) (h : R m) :
    (∀ e ∈ fillIter Dr (m.jobs.filter (serves Dr m)) 128,
      e.2 = 128 ∨ ∀ j ∈ m.jobs, j.owner = e.1 → j.mode = .decode) ∧
    ((iterStats Dr { m with iter := fillIter Dr (m.jobs.filter (serves Dr m)) 128 }).tokens = 128 ∨
      (iterStats Dr { m with iter := fillIter Dr (m.jobs.filter (serves Dr m)) 128 }).decoders =
        m.jobs.length) := by
  set F := m.jobs.filter (serves Dr m) with hF
  have hFm : ∀ j ∈ F, j ∈ m.jobs := fun j hj => (List.mem_filter.mp hj).1
  by_cases hc : decodeMode m
  · have hdec : ∀ j ∈ F, j.mode = .decode := fun j hj =>
      ((serves_rad m j).mp (List.mem_filter.mp hj).2).1 hc
    have hw1 : ∀ j ∈ F, wantOf Dr j = 1 := fun j hj => by
      have := (h.jobs j (hFm j hj)).1
      simp only [wantOf, hdec j hj]; omega
    have hFd : F = m.jobs.filter (·.mode = .decode) := by
      rw [hF]
      apply List.filter_congr
      intro j _
      have := serves_rad m j
      by_cases hd : j.mode = .decode
      · simp only [hd, decide_true]; exact this.mpr ⟨fun _ => hd, fun h' => absurd hc h'⟩
      · simp only [hd, decide_false]
        cases hs : serves Dr m j
        · rfl
        · exact absurd ((this.mp hs).1 hc) hd
    have hsum : (F.map (wantOf Dr)).sum = F.length := by
      rw [List.map_congr_left hw1]; simp
    refine ⟨fun e he => ?_, ?_⟩
    · obtain ⟨j, hj, he1, -, -⟩ := mem_fillIter Dr F 128 e he
      refine Or.inr fun j' hj' hjo => ?_
      rw [List.inj_on_of_nodup_map h.nodup hj' (hFm j hj) (hjo.trans he1)]
      exact hdec j hj
    · by_cases hl : 128 ≤ F.length
      · left
        rw [iterStats_tokens, tokSum_fillIter, hsum]; omega
      · right
        have hfill := fillIter_ones Dr F 128 hw1 (by omega)
        have hlen : F.length = m.jobs.length := by
          rcases hc with hc | hc
          · rw [hFd] at hl; omega
          · rw [hFd]; exact hc
        rw [← hlen]
        simp only [iterStats, hfill]
        rw [List.filter_eq_self.mpr]
        · simp
        · intro e he
          obtain ⟨j, hj, rfl⟩ := List.mem_map.mp he
          simp [find_owner h.nodup (hFm j hj), hdec j hj]
  · -- Prefill Mode: the oldest prefill takes the whole budget
    have hpre : ∀ j ∈ F, j.mode = .prefill := fun j hj => by
      have h1 := ((serves_rad m j).mp (List.mem_filter.mp hj).2).2 hc
      have h2 := (h.jobs j (hFm j hj)).2.2.2.1
      cases hm : j.mode <;> simp_all
    have hne : F ≠ [] := by
      intro he
      apply hc
      right
      have : ∀ j ∈ m.jobs, j.mode = .decode := by
        intro j hj
        by_contra hd
        have : j ∈ F := List.mem_filter.mpr ⟨hj, (serves_rad m j).mpr ⟨fun h' => absurd h' hc, fun _ => hd⟩⟩
        rw [he] at this; simp at this
      rw [List.filter_eq_self.mpr (by simpa using this)]
    obtain ⟨f, rest, hfr⟩ := List.exists_cons_of_ne_nil hne
    have hwf : wantOf Dr f = 128 ∨ 128 < wantOf Dr f := by
      have hj := h.jobs f (hFm f (by rw [hfr]; exact List.mem_cons_self))
      have hp := hpre f (by rw [hfr]; exact List.mem_cons_self)
      obtain ⟨c, hcf⟩ := hj.2.1 hp
      have : 0 < c := by rcases Nat.eq_zero_or_pos c with rfl | h0 <;> simp_all
      simp only [wantOf, hp, Claims.BariRad.deployment]
      left; simp; rw [hcf]; omega
    have hw : min (wantOf Dr f) 128 = 128 := by omega
    have hfill : fillIter Dr F 128 = [(f.owner, 128)] := by
      rw [hfr]; unfold fillIter; rw [hw]; simp
    refine ⟨fun e he => ?_, Or.inl ?_⟩
    · rw [hfill] at he; simp at he; rw [he]; exact Or.inl rfl
    · rw [iterStats_tokens, hfill]; simp [tokSum]

theorem rad_start (m : Machine) (h : R m) :
    R (startIteration Dr m) ∧ ((startIteration Dr m).iterEnd.isSome →
      (startIteration Dr m).last.stats.tokens = 128 ∨
        (startIteration Dr m).last.stats.decoders = (startIteration Dr m).last.residents) := by
  have hq : engineQueuesEmpty Dr m := fun p hp => by simp [pdef, Claims.BariRad.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (h.jobs j hj).2.2.2.2
  have ha := assign_eq_fillIter_only Dr m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  obtain ⟨hT, hclaim⟩ := rad_batch m h
  have hS : ∀ e ∈ fillIter Dr (m.jobs.filter (serves Dr m)) 128, (getS m e.1).status = .engine := by
    intro e he
    obtain ⟨j, hj, he1, -, -⟩ := mem_fillIter Dr _ 128 e he
    rw [he1]; exact (h.jobs j (List.mem_filter.mp hj).1).2.2.1
  have hR : R { m with iter := fillIter Dr (m.jobs.filter (serves Dr m)) 128 } :=
    R.mk' h.jobs h.nodup h.attr h.stack hT hS
  unfold startIteration
  simp only
  split
  · have hb : Claims.BariRad.deployment.budget = 128 := rfl
    rw [hb, ha]
    split
    · exact ⟨hR, fun _ => hclaim⟩
    · exact ⟨hR, fun he => by simp at he⟩
  · exact ⟨R.mk' h.jobs h.nodup h.attr h.stack (fun e he => by simp at he) (fun e he => by simp at he),
      fun he => by simp at he⟩

/-- **Optimal GeMM tiling (§4.2).** With prompts a multiple of the tile
(Assumption 3), every batch RAD runs fills its tile, unless every request at
the node decodes and fewer than 128 do. -/
theorem optimal_tiling : Claims.BariRad.optimal_tiling := by
  intro w hw m hr hs
  have := every_iteration_of Dr (fun m => Inv Pr m ∧ R m)
    (fun r => r.stats.tokens = 128 ∨ r.stats.decoders = r.residents) w Pr
    ⟨inv_initial Dr w Pr, rad_initial w hw⟩
    (fun m t q h => ⟨inv_handle m t q h.1, rad_handle m t q h.2⟩)
    (fun m h => ⟨inv_settle Dr m h.1, rad_settle m h.1 h.2⟩)
    (fun m h => ⟨inv_startIteration Dr m h.1, (rad_start m h.2).1⟩)
    (fun m h => (rad_start m h.2).2) m hr
  exact this.2 hs

end BariRad

end Papers
end SerqLang
