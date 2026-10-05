/-
# Lemmas about single steps of the executable semantics

What the proofs of the paper programs (`Serq/Papers/`) need about one
command or one bookkeeping operation of `Serq/Exec.lean`, independently of
the program: how `exec` runs `set`, `observe`, `run` and `stop`, what
`setS`, `setPool` and `readyAll` leave alone, what a step of one ready
request keeps (`Keeps`), and the tokens a batch gives a request (`shareOf`).
-/
import Serq.Claim
import Serq.Fill

namespace SerqLang
namespace Exec

theorem getS_setS_self (m : Machine) {i : ℕ} (s : Sess) (hi : i < m.sess.size) :
    getS (setS m i s) i = s := by
  simp [getS, setS, Array.getD_eq_getD_getElem?, hi]

theorem exec_observe (D : Deployment) (f : ℕ) (m : Machine) (i n : ℕ) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .observe n e k) :
    exec D (f + 1) m i = exec D f { setS m i { getS m i with prog := k } with
      obs := (n, (getS m i).serial, m.now, evalE m i e) :: m.obs } i := by
  rw [exec]; simp [hs, hp]

theorem pst_setPool_self (m : Machine) (s : PoolSt) (h : m.pools.length = 1) : pst (setPool m 0 s) 0 = s := by
  unfold pst setPool
  rcases m with ⟨_, _, _, pools, _⟩
  simp only at h ⊢
  match pools, h with
  | [_], _ => rfl

theorem pools_setPool (m : Machine) (s : PoolSt) : (setPool m 0 s).pools.length = m.pools.length := by
  simp [setPool]

theorem makeRoom_empty (b cap need f : ℕ) (s : PoolSt) (h : s.entries = []) : makeRoom b cap need f s = s := by
  cases f with
  | zero => rfl
  | succ f => simp [makeRoom, h]



@[simp] theorem setS_pools (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).pools = m.pools := rfl

@[simp] theorem setS_jobs (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).jobs = m.jobs := rfl

@[simp] theorem setS_ready (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).ready = m.ready := rfl

@[simp] theorem setS_obs (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).obs = m.obs := rfl

@[simp] theorem setS_now (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).now = m.now := rfl

@[simp] theorem setS_nextAdm (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).nextAdm = m.nextAdm := rfl

@[simp] theorem setPool_sess (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).sess = m.sess := rfl

@[simp] theorem setPool_nextAdm (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).nextAdm = m.nextAdm := rfl

@[simp] theorem setPool_ready (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).ready = m.ready := rfl

@[simp] theorem setPool_obs (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).obs = m.obs := rfl

@[simp] theorem setPool_now (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).now = m.now := rfl
@[simp] theorem setPool_jobs (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).jobs = m.jobs := rfl

theorem getS_setPool (m : Machine) (p i : ℕ) (s : PoolSt) : getS (setPool m p s) i = getS m i := rfl

theorem argminKey_mem (k : ℕ → ℕ) : ∀ (l : List ℕ) (h : ℕ), argminKey k l = some h → h ∈ l
  | [], _, hh => by simp [argminKey] at hh
  | x :: xs, h, hh => by
    unfold argminKey at hh
    split at hh
    · simp only [Option.some.injEq] at hh; subst hh; exact List.mem_cons_self
    · rename_i y hy
      split at hh <;> simp only [Option.some.injEq] at hh <;> subst hh
      · exact List.mem_cons_of_mem _ (argminKey_mem k xs y hy)
      · exact List.mem_cons_self

theorem argminKey_none (k : ℕ → ℕ) : ∀ (l : List ℕ), argminKey k l = none → l = []
  | [], _ => rfl
  | x :: xs, hh => by
    unfold argminKey at hh
    split at hh <;> [simp at hh; (split at hh <;> simp at hh)]

/-- On a strictly increasing list, the first element of least key precedes
every other element in (key, value) order. -/
theorem argminKey_least (k : ℕ → ℕ) : ∀ (l : List ℕ), l.Pairwise (· < ·) → ∀ h, argminKey k l = some h →
    ∀ j ∈ l, j ≠ h → k h < k j ∨ (k h = k j ∧ h < j)
  | [], _, _, hh => by simp [argminKey] at hh
  | x :: xs, hs, h, hh => by
    rw [List.pairwise_cons] at hs
    unfold argminKey at hh
    split at hh
    · rename_i hn
      simp only [Option.some.injEq] at hh
      subst hh
      have := argminKey_none k xs hn
      subst this
      intro j hj hne; simp at hj; exact absurd hj hne
    · rename_i y hy
      have hym := argminKey_mem k xs y hy
      have ih := argminKey_least k xs hs.2 y hy
      split at hh
      · rename_i hlt
        simp only [Option.some.injEq] at hh
        subst hh
        intro j hj hne
        rcases List.mem_cons.mp hj with rfl | hj
        · exact Or.inl hlt
        · exact ih j hj hne
      · rename_i hge
        simp only [Option.some.injEq] at hh
        subst hh
        intro j hj hne
        rcases List.mem_cons.mp hj with rfl | hj
        · exact absurd rfl hne
        · have hxj := hs.1 j hj
          by_cases hjy : j = y
          · subst hjy
            rcases Nat.lt_or_ge (k x) (k j) with h1 | h1
            · exact Or.inl h1
            · exact Or.inr ⟨by omega, hxj⟩
          · rcases ih j hj hjy with h1 | ⟨h1, _⟩
            · rcases Nat.lt_or_ge (k x) (k j) with h2 | h2
              · exact Or.inl h2
              · exact Or.inr ⟨by omega, hxj⟩
            · rcases Nat.lt_or_ge (k x) (k j) with h2 | h2
              · exact Or.inl h2
              · exact Or.inr ⟨by omega, hxj⟩

/-- A filtered list over `range n` whose predicate becomes true at one more
index `i < n`. -/
theorem perm_filter_add {n i : ℕ} (hi : i < n) (p p' : ℕ → Bool) (f : ℕ → ℕ)
    (hpi : p i = false) (hp'i : p' i = true) (hrest : ∀ j, j ≠ i → p' j = p j) :
    (((List.range n).filter p').map f).Perm (f i :: ((List.range n).filter p).map f) := by
  induction n with
  | zero => omega
  | succ n ih =>
    simp only [List.range_succ, List.filter_append, List.map_append, List.filter_cons, List.filter_nil]
    by_cases hin : i = n
    · subst hin
      have heq : (List.range i).filter p' = (List.range i).filter p := by
        apply List.filter_congr
        intro j hj
        exact hrest j (by simp at hj; omega)
      rw [heq, hp'i, hpi]
      simp only [if_true, List.map_cons, List.map_nil, Bool.false_eq_true, if_false, List.append_nil]
      exact List.perm_append_singleton _ _
    · have hlt : i < n := by omega
      rw [hrest n (Ne.symm hin)]
      have := ih hlt
      by_cases hpn : p n = true
      · simp only [hpn, if_true, List.map_cons, List.map_nil]
        exact (this.append_right _).trans (by simp)
      · simp only [hpn, Bool.false_eq_true, if_false, List.map_nil, List.append_nil]
        exact this

theorem values_cons (m : Machine) (k i t v : ℕ) (name : ℕ) :
    values { m with obs := (k, i, t, v) :: m.obs } name =
      if k = name then v :: values m name else values m name := by
  simp only [values, List.filter_cons]
  split_ifs with h1 h2 h2 <;> simp_all

theorem exec_runEngine (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ) (md : Mode) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .run 0 md e none k) (hw : evalE m i e ≠ 0) :
    exec D (f + 1) m i =
      { setS m i { getS m i with prog := k, status := .engine } with
        jobs := (m.jobs.span fun j => (getS m j.owner).admSeq ≤ (getS m i).admSeq).1 ++
          ⟨i, md, evalE m i e, none⟩ :: (m.jobs.span fun j => (getS m j.owner).admSeq ≤ (getS m i).admSeq).2 } := by
  rw [exec]; simp [hs, hp, hw]

theorem span_mem {α : Type} (p : α → Bool) (l : List α) (x : α) :
    x ∈ (l.span p).1 ++ (l.span p).2 ↔ x ∈ l := by
  rw [List.span_eq_takeWhile_dropWhile, List.takeWhile_append_dropWhile]

theorem exec_stop (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .stop) (hst : (getS m i).stack = []) :
    exec D (f + 1) m i = setS m i { getS m i with status := .ended, stack := [] } := by
  rw [exec]; simp [hs, hp, endSession, hst]

theorem sum_filter_ne {l : List ℕ} (hn : l.Nodup) {i : ℕ} (hi : i ∈ l) (f : ℕ → ℕ) :
    ((l.filter (· ≠ i)).map f).sum + f i = (l.map f).sum := by
  induction l with
  | nil => simp at hi
  | cons a l ih =>
    rw [List.nodup_cons] at hn
    by_cases hai : a = i
    · subst hai
      have : l.filter (· ≠ a) = l := List.filter_eq_self.mpr (fun x hx => by
        simp only [ne_eq, decide_eq_true_eq]; rintro rfl; exact hn.1 hx)
      rw [List.filter_cons_of_neg (by simp), this, List.map_cons, List.sum_cons, Nat.add_comm]
    · have hi' : i ∈ l := by
        rcases List.mem_cons.mp hi with h | h
        · exact absurd h.symm hai
        · exact h
      rw [List.filter_cons_of_pos (by simpa using hai), List.map_cons, List.sum_cons, List.map_cons,
        List.sum_cons, Nat.add_assoc, ih hn.2 hi']

theorem length_le_of_nodup_lt {l : List ℕ} (hn : l.Nodup) {n : ℕ} (h : ∀ x ∈ l, x < n) : l.length ≤ n := by
  rw [← List.toFinset_card_of_nodup hn]
  calc l.toFinset.card ≤ (Finset.range n).card :=
        Finset.card_le_card fun x hx => Finset.mem_range.mpr (h x (List.mem_toFinset.mp hx))
    _ = n := Finset.card_range n

/-- Readying the finished requests: each becomes ready and joins the ready
list, nothing else changes. -/
def readyAll (done : List ℕ) (m : Machine) : Machine :=
  done.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) m

theorem readyAll_fields : ∀ (done : List ℕ) (m : Machine), done.Nodup → (∀ i ∈ done, i < m.sess.size) →
    (readyAll done m).ready = m.ready ++ done ∧ (readyAll done m).jobs = m.jobs ∧
    (readyAll done m).pools = m.pools ∧ (readyAll done m).obs = m.obs ∧ (readyAll done m).now = m.now ∧
    (readyAll done m).delays = m.delays ∧ (readyAll done m).wl = m.wl ∧ (readyAll done m).iter = m.iter ∧
    (readyAll done m).iterEnd = m.iterEnd ∧ (readyAll done m).sess.size = m.sess.size ∧
    ∀ j, getS (readyAll done m) j = if j ∈ done then { getS m j with status := .ready } else getS m j
  | [], m, _, _ => by simp [readyAll]
  | i :: rest, m, hn, hb => by
    rw [List.nodup_cons] at hn
    have ih := readyAll_fields rest { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] } hn.2
      (fun j hj => by simpa [setS] using hb j (List.mem_cons_of_mem _ hj))
    simp only [readyAll, List.foldl_cons] at ih ⊢
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := ih
    refine ⟨by rw [h1]; simp, h2, h3, h4, h5, h6, h7, h8, h9, by rw [h10]; simp [setS], fun j => ?_⟩
    rw [h11 j]
    by_cases hjr : j ∈ rest
    · have hji : j ≠ i := fun h => hn.1 (h ▸ hjr)
      simp only [hjr, if_true, List.mem_cons, hji, false_or]
      rw [show getS { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] } j =
        getS m j from getS_setS_ne m _ (Ne.symm hji)]
    · simp only [hjr, if_false, List.mem_cons, or_false]
      by_cases hji : j = i
      · subst hji
        simp only [if_true]
        exact getS_setS_self m _ (hb j List.mem_cons_self)
      · simp only [hji, if_false]
        exact getS_setS_ne m _ (Ne.symm hji)

theorem sum_iter_owner {js : List Job} (hn : (js.map (·.owner)).Nodup) {j : Job} (hj : j ∈ js) :
    (((js.map fun j => (j.owner, 1)).filter (fun e => decide (e.1 = j.owner))).map (·.2)).sum = 1 := by
  induction js with
  | nil => simp at hj
  | cons x xs ih =>
    simp only [List.map_cons, List.nodup_cons] at hn
    rcases List.mem_cons.mp hj with rfl | hj'
    · have : ((xs.map fun j => (j.owner, 1)).filter (fun e => decide (e.1 = j.owner))) = [] := by
        rw [List.filter_eq_nil_iff]
        intro e he
        obtain ⟨y, hy, rfl⟩ := List.mem_map.mp he
        simp only [decide_eq_true_eq]
        intro h; exact hn.1 (h ▸ List.mem_map.mpr ⟨y, hy, rfl⟩)
      simp [List.filter_cons, this]
    · have hne : x.owner ≠ j.owner := fun h => hn.1 (h ▸ List.mem_map.mpr ⟨j, hj', rfl⟩)
      simp [List.filter_cons, hne, ih hn.2 hj']

theorem nextEvent_of_delays {m : Machine} (h : m.delays = []) : nextEvent m = m.iterEnd := by
  unfold nextEvent; rw [h]; cases m.iterEnd <;> rfl

/-- The tokens a list of batch entries gives request `i`. -/
def shareOf (it : List (ℕ × ℕ)) (i : ℕ) : ℕ := ((it.filter fun e => decide (e.1 = i)).map (·.2)).sum

theorem getS_setS (m : Machine) {i : ℕ} (s : Sess) (hi : i < m.sess.size) (j : ℕ) :
    getS (setS m i s) j = if j = i then s else getS m j := by
  split_ifs with h
  · subst h; exact getS_setS_self m s hi
  · exact getS_setS_ne m s (Ne.symm h)

@[simp] theorem setS_size (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).sess.size = m.sess.size := by
  simp [setS]

theorem Attrs.upd_base (a : Attrs) (k v : ℕ) : (a.upd k v).base = a.base := by
  unfold Attrs.upd; split <;> rfl

/-- `upd` leaves the other slots. -/
theorem Attrs.get_upd_ne (a : Attrs) {k j : ℕ} (v : ℕ) (hk : j ≠ k) : (a.upd k v).get j = a.get j := by
  rw [Attrs.get_upd]; simp [Function.update, hk]

theorem exec_set (D : Deployment) (f : ℕ) (m : Machine) (i slot : ℕ) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .set slot e k) :
    exec D (f + 1) m i =
      exec D f (setS m i { getS m i with attr := (getS m i).attr.upd slot (evalE m i e), prog := k }) i := by
  rw [exec]; simp [hs, hp]

theorem exec_runDelay (D : Deployment) (f : ℕ) (m : Machine) (i st : ℕ) (md : Mode) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .run st md e none k) (hw : evalE m i e ≠ 0)
    (hst : st ≠ 0) :
    exec D (f + 1) m i =
      { setS m i { getS m i with prog := k, status := .delay (m.now + evalE m i e) m.nextDelay } with
        nextDelay := m.nextDelay + 1
        delays := insertDelay (m.now + evalE m i e, m.nextDelay, i) m.delays } := by
  rw [exec]; simp [hs, hp, hw, hst]

/-- One term of a sum over `range n` changes. -/
theorem sum_update {n i : ℕ} (hi : i < n) (f f' : ℕ → ℕ) (h : ∀ j, j ≠ i → f' j = f j) :
    ∑ j ∈ Finset.range n, f' j + f i = ∑ j ∈ Finset.range n, f j + f' i := by
  rw [← Finset.add_sum_erase _ _ (Finset.mem_range.mpr hi), ← Finset.add_sum_erase _ f (Finset.mem_range.mpr hi)]
  rw [Finset.sum_congr rfl fun j hj => h j (Finset.ne_of_mem_erase hj)]
  ring

/-- One more index satisfies the filter. -/
theorem card_update {n i : ℕ} (hi : i < n) (p p' : ℕ → Prop) [DecidablePred p] [DecidablePred p']
    (hpi : ¬ p i) (hp'i : p' i) (h : ∀ j, j ≠ i → (p' j ↔ p j)) :
    ((Finset.range n).filter p').card = ((Finset.range n).filter p).card + 1 := by
  have : (Finset.range n).filter p' = insert i ((Finset.range n).filter p) := by
    ext j
    simp only [Finset.mem_filter, Finset.mem_range, Finset.mem_insert]
    by_cases hj : j = i
    · subst hj; simp [hi, hp'i]
    · simp [hj, h j hj]
  rw [this, Finset.card_insert_of_notMem (by simp [hpi])]

theorem exec_runEngine' (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ) (md : Mode) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .run 0 md e none k) (hw : evalE m i e ≠ 0) :
    ∃ a b, a ++ b = m.jobs ∧ exec D (f + 1) m i =
      { setS m i { getS m i with prog := k, status := .engine } with
        jobs := a ++ ⟨i, md, evalE m i e, none⟩ :: b } :=
  ⟨_, _, by rw [List.span_eq_takeWhile_dropWhile, List.takeWhile_append_dropWhile],
    exec_runEngine D f m i md e k hs hp hw⟩

/-- What a step of one ready request leaves alone. -/
structure Keeps (M m : Machine) : Prop where
  now : M.now = m.now
  iter : M.iter = m.iter
  iterEnd : M.iterEnd = m.iterEnd
  served : M.served = m.served
  last : M.last = m.last

theorem Keeps.trans {a b c : Machine} (h1 : Keeps b a) (h2 : Keeps c b) : Keeps c a :=
  ⟨h2.now.trans h1.now, h2.iter.trans h1.iter, h2.iterEnd.trans h1.iterEnd, h2.served.trans h1.served,
    h2.last.trans h1.last⟩

theorem Keeps.refl (m : Machine) : Keeps m m := ⟨rfl, rfl, rfl, rfl, rfl⟩

theorem insertDelay_perm (d : ℕ × ℕ × ℕ) : ∀ l : List (ℕ × ℕ × ℕ), (insertDelay d l).Perm (d :: l)
  | [] => List.Perm.refl _
  | x :: xs => by
    unfold insertDelay
    split
    · exact List.Perm.refl _
    · exact ((insertDelay_perm d xs).cons x).trans (List.Perm.swap d x xs)

theorem shareOf_nil (i : ℕ) : shareOf [] i = 0 := rfl

theorem shareOf_eq_zero {l : List (ℕ × ℕ)} {i : ℕ} (h : ∀ e ∈ l, e.1 ≠ i) : shareOf l i = 0 := by
  unfold shareOf
  rw [List.filter_eq_nil_iff.mpr fun e he => by simpa using h e he]
  rfl

theorem shareOf_cons (e : ℕ × ℕ) (l : List (ℕ × ℕ)) (i : ℕ) :
    shareOf (e :: l) i = (if e.1 = i then e.2 else 0) + shareOf l i := by
  unfold shareOf
  by_cases h : e.1 = i <;> simp [h]

/-- The batch gives a request at most what its jobs want. -/
theorem shareOf_fillIter (D : Deployment) : ∀ (js : List Job) (B o : ℕ),
    shareOf (fillIter D js B) o ≤ ((js.filter fun j => decide (j.owner = o)).map (wantOf D)).sum
  | [], _, _ => by simp [fillIter, shareOf]
  | j :: js, B, o => by
    have ih := shareOf_fillIter D js
    have hw : (((j :: js).filter fun j => decide (j.owner = o)).map (wantOf D)).sum =
        (if j.owner = o then wantOf D j else 0) + ((js.filter fun j => decide (j.owner = o)).map (wantOf D)).sum := by
      by_cases h : j.owner = o <;> simp [h]
    rw [hw]
    have key : ∀ t ≤ wantOf D j, ∀ l, shareOf l o ≤ ((js.filter fun j => decide (j.owner = o)).map (wantOf D)).sum →
        shareOf ((j.owner, t) :: l) o ≤
          (if j.owner = o then wantOf D j else 0) + ((js.filter fun j => decide (j.owner = o)).map (wantOf D)).sum := by
      intro t ht l hl; rw [shareOf_cons]; dsimp only; split_ifs <;> omega
    unfold fillIter
    by_cases h0 : min (wantOf D j) B = 0
    · rw [if_pos h0]; have := ih B o; split_ifs <;> omega
    · rw [if_neg h0]
      by_cases h1 : B - min (wantOf D j) B = 0
      · rw [if_pos h1]; exact key _ (min_le_left _ _) [] (by simp [shareOf])
      · rw [if_neg h1]; exact key _ (min_le_left _ _) _ (ih _ o)

/-- A request's share of a batch of entries with distinct owners. -/
theorem filter_owner_eq {js : List Job} (hn : (js.map (·.owner)).Nodup) {j : Job} (hj : j ∈ js) :
    js.filter (fun x => decide (x.owner = j.owner)) = [j] := by
  induction js with
  | nil => simp at hj
  | cons x xs ih =>
    simp only [List.map_cons, List.nodup_cons] at hn
    rcases List.mem_cons.mp hj with rfl | hj'
    · rw [List.filter_cons_of_pos (by simp), List.filter_eq_nil_iff.mpr]
      intro y hy
      simp only [decide_eq_true_eq]
      intro he; exact hn.1 (he ▸ List.mem_map.mpr ⟨y, hy, rfl⟩)
    · have hne : x.owner ≠ j.owner := fun h => hn.1 (h ▸ List.mem_map.mpr ⟨j, hj', rfl⟩)
      rw [List.filter_cons_of_neg (by simpa using hne)]
      exact ih hn.2 hj'

theorem fillIter_ne_nil (D : Deployment) (j : Job) (js : List Job) (B : ℕ) (hw : 0 < wantOf D j) (hB : 0 < B) :
    fillIter D (j :: js) B ≠ [] := by
  unfold fillIter
  rw [if_neg (by omega)]
  split_ifs <;> simp

/-- With the engine idle and nothing due now, every delay ends later. -/
theorem not_pending {m : Machine} (hie : m.iterEnd = none) (hp : pendingBy m m.now = false)
    (hs : m.delays.Pairwise (fun a b => a.1 ≤ b.1)) : ∀ d ∈ m.delays, m.now < d.1 := by
  unfold pendingBy nextEvent at hp
  rw [hie] at hp
  intro d hd
  rcases hdl : m.delays with _ | ⟨⟨t, q, i⟩, rest⟩
  · rw [hdl] at hd; simp at hd
  · rw [hdl] at hp hd hs
    simp only [List.head?_cons, Option.any_some, decide_eq_false_iff_not, not_le] at hp
    rcases List.mem_cons.mp hd with rfl | hd
    · exact hp
    · exact lt_of_lt_of_le hp ((List.pairwise_cons.mp hs).1 d hd)

theorem sess_toList (m : Machine) : m.sess.toList = (List.range m.sess.size).map (getS m) := by
  apply List.ext_getElem
  · simp
  · intro k h1 h2
    simp [getS, Array.getD_eq_getD_getElem?]
    have : k < m.sess.size := by simpa using h1
    simp [this]

theorem card_range_filter (n : ℕ) (q : ℕ → Prop) [DecidablePred q] :
    ((List.range n).filter fun i => decide (q i)).length = ((Finset.range n).filter q).card := by
  rw [← List.toFinset_card_of_nodup (List.nodup_range.filter _), List.toFinset_filter, List.toFinset_range]
  simp

theorem sum_shareOf (n : ℕ) : ∀ L : List (ℕ × ℕ), (∀ e ∈ L, e.1 < n) →
    ∑ i ∈ Finset.range n, shareOf L i = tokSum L
  | [], _ => by simp [shareOf, tokSum]
  | e :: L, h => by
    simp only [shareOf_cons, Finset.sum_add_distrib]
    rw [sum_shareOf n L fun x hx => h x (List.mem_cons_of_mem _ hx)]
    rw [Finset.sum_ite_eq (Finset.range n) e.1 (fun _ => e.2)]
    rw [if_pos (Finset.mem_range.mpr (h e List.mem_cons_self))]
    simp [tokSum]

theorem readyAll_other : ∀ (done : List ℕ) (m : Machine),
    (readyAll done m).nextDelay = m.nextDelay ∧ (readyAll done m).served = m.served ∧
      (readyAll done m).last = m.last
  | [], m => ⟨rfl, rfl, rfl⟩
  | i :: rest, m => by
    have := readyAll_other rest { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }
    simp only [readyAll, List.foldl_cons] at this ⊢
    exact this

end Exec
end SerqLang
