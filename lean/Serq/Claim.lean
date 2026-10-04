/-
# Claims: what a serQ program says about its own paths

A serQ program may state propositions about every path it can take
(`claim NAME [given (g)] : every iteration of S (p)`, `some iteration of S
(p)`, `at end (p)`; serQ `docs/language.md`, Claims). The interpreter checks
a claim on the path it simulates; this module gives the claim its meaning
as a statement about the executable semantics, for every workload in the
program's family: the machines a run reaches event by event (`Reach`), the
iteration record a claim over iterations reads (`Machine.last`), and the
observations a claim at the end aggregates. `scripts/gen_lean_claims.py`
writes each claim of the programs under `examples/papers/` as such a
statement (`Serq/Claims.lean`), and `Serq/Papers/` proves them.

The module also proves what every program satisfies, whatever its claims:

* an instant's commands leave the clock, the engine's iteration and its
  record alone, and only add delays that end no earlier than now
  (`Same`, `same_settle`);
* the engine's token rate is at most what one iteration of a full budget
  achieves: if every batch of at most `budget` tokens satisfies
  `tokens * T ≤ R * cost`, then before every iteration the engine has
  served at most `R / T` tokens per clock unit (`served_rate`). This is the
  pathwise content of the throughput bounds of Dai et al. (Theorem 2(a))
  and Bari et al. (Theorem 1): it holds for every program, so for every
  scheduler the language can write.

Key definitions: `Exec.Reach`, `Exec.EveryIteration`, `Exec.SomeIteration`,
`Exec.AtEnd`, `Exec.total`, `Exec.prefixTotal`.
Key theorems: `Exec.same_settle`, `Exec.served_rate`.
-/
import Serq.Exec

namespace SerqLang
namespace Exec

variable (D : Deployment)

/-! ### The paths of a program -/

/-- The machines a run of `P` on workload `w` reaches, one event at a time. -/
inductive Reach (w : Workload) (P : Prog) : Machine → Prop
  | start : Reach w P (Exec.start D w.init.length w.attr P w)
  | step {m : Machine} : Reach w P m → Reach w P (Exec.step D m)

/-- `claim … : every iteration of S (q)`: on every path of every workload of
the family `W`, the record of every iteration satisfies `q`. An iteration is
observed at every machine it runs in (`iterEnd` is set), and its record is
`last` there. -/
def EveryIteration (W : Workload → Prop) (P : Prog) (q : IterRec → Prop) : Prop :=
  ∀ w, W w → ∀ m, Reach D w P m → m.iterEnd.isSome → q m.last

/-- `claim … : some iteration of S (q)`: some workload of the family has a
path with an iteration whose record satisfies `q`. -/
def SomeIteration (W : Workload → Prop) (P : Prog) (q : IterRec → Prop) : Prop :=
  ∃ w, W w ∧ ∃ m, Reach D w P m ∧ m.iterEnd.isSome ∧ q m.last

/-- Every session has ended. -/
def Ended (m : Machine) : Prop := ∀ s ∈ m.sess.toList, s.status = .ended

/-- `claim … : at end (q)`: on every path of every workload of the family,
the machine at which every session has ended satisfies `q`. -/
def AtEnd (W : Workload → Prop) (P : Prog) (q : Machine → Prop) : Prop :=
  ∀ w, W w → ∀ m, Reach D w P m → Ended m → q m

/-- The values observed under `name`. -/
def values (m : Machine) (name : ℕ) : List ℕ := (m.obs.filter (·.1 = name)).map (·.2.2.2)

/-- `total(o)`. -/
def total (m : Machine) (name : ℕ) : ℕ := (values m name).sum

/-- `count(o)`. -/
def count (m : Machine) (name : ℕ) : ℕ := (values m name).length

/-- `Σ_k Σ_{i ≤ k} v_(i)` of a list sorted ascending. -/
def prefixSums : List ℕ → ℕ
  | [] => 0
  | v :: vs => (v :: vs).sum + prefixSums vs

/-- `prefix_total(o)`: the values sorted ascending `v₁ ≤ … ≤ vₙ`, the sum
over `k` of `v₁ + … + v_k`, which is `Σ_k (n - k + 1) v_k`. -/
def prefixTotal (l : List ℕ) : ℕ := prefixSums (l.mergeSort (fun a b => decide (a ≥ b))).reverse

/-! ### What an instant's commands leave alone -/

/-- `m'` comes from `m` by commands of one instant: the clock, the served
tokens, the engine's iteration and its record are unchanged, and the delays
are those of `m` and new ones that end no earlier than now, sorted if those
of `m` were. -/
structure Same (m m' : Machine) : Prop where
  now : m'.now = m.now
  served : m'.served = m.served
  iterEnd : m'.iterEnd = m.iterEnd
  last : m'.last = m.last
  delays : ∀ d ∈ m'.delays, d ∈ m.delays ∨ m.now ≤ d.1
  sorted : m.delays.Pairwise (fun a b => a.1 ≤ b.1) → m'.delays.Pairwise (fun a b => a.1 ≤ b.1)
  iter : m'.iter = m.iter

theorem Same.refl (m : Machine) : Same m m :=
  ⟨rfl, rfl, rfl, rfl, fun _ h => Or.inl h, id, rfl⟩

theorem Same.trans {a b c : Machine} (h1 : Same a b) (h2 : Same b c) : Same a c where
  now := h2.now.trans h1.now
  served := h2.served.trans h1.served
  iterEnd := h2.iterEnd.trans h1.iterEnd
  last := h2.last.trans h1.last
  delays d hd := by
    rcases h2.delays d hd with h | h
    · exact h1.delays d h
    · exact Or.inr (h1.now ▸ h)
  sorted h := h2.sorted (h1.sorted h)
  iter := h2.iter.trans h1.iter

/-- A machine that agrees with `m` on the six fields. -/
theorem Same.of_eq {m m' : Machine} (h1 : m'.now = m.now) (h2 : m'.served = m.served)
    (h3 : m'.iterEnd = m.iterEnd) (h4 : m'.last = m.last) (h5 : m'.delays = m.delays)
    (h6 : m'.iter = m.iter) : Same m m' :=
  ⟨h1, h2, h3, h4, fun d hd => Or.inl (h5 ▸ hd), fun h => h5 ▸ h, h6⟩

/-- The fields that the commands of an instant other than a delay's start
leave alone. -/
def key (m : Machine) :
    ℕ × ℕ × Option (ℕ × ℕ) × IterRec × List (ℕ × ℕ × ℕ) × List (ℕ × ℕ) :=
  (m.now, m.served, m.iterEnd, m.last, m.delays, m.iter)

theorem Same.of_key {m m' : Machine} (h : key m' = key m) : Same m m' := by
  simp only [key, Prod.mk.injEq] at h
  exact Same.of_eq h.1 h.2.1 h.2.2.1 h.2.2.2.1 h.2.2.2.2.1 h.2.2.2.2.2

theorem Same.key_trans {m m' m'' : Machine} (h2 : Same m' m'') (h : key m' = key m) : Same m m'' :=
  (Same.of_key h).trans h2

theorem key_setPool (m : Machine) (p : ℕ) (s : PoolSt) : key (setPool m p s) = key m := rfl

theorem key_setS (m : Machine) (i : ℕ) (s : Sess) : key (setS m i s) = key m := rfl

theorem key_foldl {α β : Type} (f : Machine × β → α → Machine × β)
    (hf : ∀ acc a, key (f acc a).1 = key acc.1) :
    ∀ (l : List α) (acc : Machine × β), key (l.foldl f acc).1 = key acc.1
  | [], _ => rfl
  | a :: l, acc => (key_foldl f hf l (f acc a)).trans (hf acc a)

theorem key_foldl' {α : Type} (f : Machine → α → Machine) (hf : ∀ m a, key (f m a) = key m) :
    ∀ (l : List α) (m : Machine), key (l.foldl f m) = key m
  | [], _ => rfl
  | a :: l, m => (key_foldl' f hf l (f m a)).trans (hf m a)

theorem key_release (m : Machine) (i : ℕ) (h : HoldRec) : key (release D m i h) = key m := by
  unfold release
  apply key_foldl'
  intro m ⟨p, alloc, pos⟩
  simp only
  repeat' split
  all_goals rfl

theorem key_admitPool (i serial : ℕ) (cache : Option (Env → ℕ)) (r? : Option ℕ)
    (acc : Machine × List (ℕ × ℕ × ℕ) × ℕ) (x : ℕ × ℕ × ℕ) :
    key (admitPool D i serial cache r? acc x).1 = key acc.1 := by
  unfold admitPool
  simp only
  rw [key_setPool]
  split_ifs <;> rfl

theorem key_admit (m : Machine) (i left : ℕ) : key (admit D m i left) = key m := by
  unfold admit
  split
  · simp only
    rw [show ∀ m' : Machine, key { m' with nextAdm := m'.nextAdm + 1, ready := m'.ready ++ [i] }
      = key m' from fun _ => rfl, key_setS]
    exact key_foldl _ (key_admitPool D i _ _ _) _ _
  · rfl

theorem key_enqueue (m : Machine) (i : ℕ) (front : Bool) : key (enqueue m i front) = key m := by
  unfold enqueue
  split <;> rfl

theorem key_admitHeads (p : ℕ) : ∀ (f : ℕ) (m : Machine), key (admitHeads D p f m) = key m
  | 0, _ => rfl
  | f + 1, m => by
    unfold admitHeads
    split
    · split
      · rw [key_admitHeads p f, key_admit, key_setPool]
      · rfl
    · rfl

theorem key_admitAll (m : Machine) : key (admitAll D m) = key m := by
  unfold admitAll
  apply key_foldl'
  intro m p
  split
  · rfl
  · exact key_admitHeads D p 1000 m

theorem key_endSession (m : Machine) (i : ℕ) : key (endSession D m i) = key m := by
  unfold endSession
  simp only
  rw [key_setS]
  apply key_foldl'
  intro m f
  split
  · exact key_release D m i _
  · rfl

theorem mem_insertDelay (d x : ℕ × ℕ × ℕ) :
    ∀ l : List (ℕ × ℕ × ℕ), x ∈ insertDelay d l ↔ x = d ∨ x ∈ l
  | [] => by simp [insertDelay]
  | y :: l => by
    unfold insertDelay
    split
    · simp
    · simp only [List.mem_cons, mem_insertDelay d x l]
      tauto

theorem sorted_insertDelay (d : ℕ × ℕ × ℕ) :
    ∀ l : List (ℕ × ℕ × ℕ), l.Pairwise (fun a b => a.1 ≤ b.1) →
      (insertDelay d l).Pairwise (fun a b => a.1 ≤ b.1)
  | [], _ => by simp [insertDelay]
  | y :: l, h => by
    unfold insertDelay
    rw [List.pairwise_cons] at h
    split
    · rename_i hlt
      refine List.Pairwise.cons ?_ (List.Pairwise.cons h.1 h.2)
      intro a ha
      rcases List.mem_cons.mp ha with rfl | ha
      · omega
      · have := h.1 a ha; omega
    · rename_i hlt
      refine List.Pairwise.cons ?_ (sorted_insertDelay d l h.2)
      intro a ha
      rcases (mem_insertDelay d a l).mp ha with rfl | ha
      · omega
      · exact h.1 a ha

/-- A delay that ends no earlier than now. -/
theorem same_insertDelay (m m' : Machine) (d : ℕ × ℕ × ℕ) (hd : m.now ≤ d.1)
    (h1 : m'.now = m.now) (h2 : m'.served = m.served) (h3 : m'.iterEnd = m.iterEnd)
    (h4 : m'.last = m.last) (h5 : m'.delays = insertDelay d m.delays) (h6 : m'.iter = m.iter) :
    Same m m' where
  now := h1
  served := h2
  iterEnd := h3
  last := h4
  delays x hx := by
    rw [h5, mem_insertDelay] at hx
    rcases hx with rfl | hx
    · exact Or.inr hd
    · exact Or.inl hx
  sorted h := h5 ▸ sorted_insertDelay d _ h
  iter := h6

theorem same_exec : ∀ (f : ℕ) (m : Machine) (i : ℕ), Same m (exec D f m i)
  | 0, m, _ => Same.refl m
  | f + 1, m, i => by
    unfold exec
    simp only
    split
    · exact Same.refl m
    · split
      · split
        · exact Same.of_key rfl
        · exact Same.key_trans (same_exec f _ i) rfl
        · refine Same.key_trans (same_exec f _ i) ?_
          simp only [key_setS, key_admitAll, key_release]
        · exact Same.key_trans (same_exec f _ i) rfl
      · exact Same.of_key (key_endSession D m i)
      · split
        · exact Same.key_trans (same_exec f _ i) rfl
        · split
          · exact Same.key_trans (same_exec f _ i) rfl
          · exact Same.key_trans (same_exec f _ i) rfl
      · exact Same.key_trans (same_exec f _ i) rfl
      · exact Same.key_trans (same_exec f _ i) rfl
      · exact Same.key_trans (same_exec f _ i) rfl
      · exact Same.key_trans (same_exec f _ i) rfl
      · split
        · exact Same.of_key rfl
        · split
          · exact same_insertDelay m _ _ (Nat.le_add_right _ _) rfl rfl rfl rfl rfl rfl
          · exact Same.of_key rfl
      · exact Same.of_key (key_enqueue m i false)

theorem same_drain : ∀ (f : ℕ) (m : Machine), Same m (drain D f m)
  | 0, m => Same.refl m
  | f + 1, m => by
    unfold drain
    split
    · exact Same.refl m
    · rename_i i q _
      exact (Same.key_trans (m := m) (same_exec D 10000 { m with ready := q } i) rfl).trans
        (same_drain f _)

theorem same_settleLoop : ∀ (f : ℕ) (m : Machine), Same m (settleLoop D f m)
  | 0, m => Same.refl m
  | f + 1, m => by
    unfold settleLoop
    simp only
    have h : Same m (admitAll D (drain D 10000 m)) :=
      (same_drain D 10000 m).trans (Same.of_key (key_admitAll D _))
    split
    · exact h
    · exact h.trans (same_settleLoop f _)

theorem same_settle (m : Machine) : Same m (settle D m) := same_settleLoop D 1000 m

/-! ### Building an iteration -/

/-- The tokens of a batch. -/
def tokSum (l : List (ℕ × ℕ)) : ℕ := (l.map (·.2)).sum

@[simp] theorem setS_iter (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).iter = m.iter := rfl

theorem tokSum_append (a b : List (ℕ × ℕ)) : tokSum (a ++ b) = tokSum a + tokSum b := by
  simp [tokSum]

theorem tokSum_filter (q : ℕ × ℕ → Bool) (l : List (ℕ × ℕ)) : tokSum (l.filter q) ≤ tokSum l := by
  induction l with
  | nil => simp [tokSum]
  | cons a l ih =>
    simp only [tokSum, List.filter_cons] at ih ⊢
    split <;> simp <;> omega

/-- `Same` without the batch: what building an iteration leaves alone. -/
structure Same0 (m m' : Machine) : Prop where
  now : m'.now = m.now
  served : m'.served = m.served
  iterEnd : m'.iterEnd = m.iterEnd
  last : m'.last = m.last
  delays : ∀ d ∈ m'.delays, d ∈ m.delays ∨ m.now ≤ d.1
  sorted : m.delays.Pairwise (fun a b => a.1 ≤ b.1) → m'.delays.Pairwise (fun a b => a.1 ≤ b.1)

theorem Same.same0 {m m' : Machine} (h : Same m m') : Same0 m m' :=
  ⟨h.now, h.served, h.iterEnd, h.last, h.delays, h.sorted⟩

theorem Same0.refl (m : Machine) : Same0 m m := (Same.refl m).same0

theorem Same0.trans {a b c : Machine} (h1 : Same0 a b) (h2 : Same0 b c) : Same0 a c where
  now := h2.now.trans h1.now
  served := h2.served.trans h1.served
  iterEnd := h2.iterEnd.trans h1.iterEnd
  last := h2.last.trans h1.last
  delays d hd := by
    rcases h2.delays d hd with h | h
    · exact h1.delays d h
    · exact Or.inr (h1.now ▸ h)
  sorted h := h2.sorted (h1.sorted h)

/-- The five fields of `key` other than the batch. -/
def key0 (m : Machine) : ℕ × ℕ × Option (ℕ × ℕ) × IterRec × List (ℕ × ℕ × ℕ) :=
  (m.now, m.served, m.iterEnd, m.last, m.delays)

theorem Same0.of_key0 {m m' : Machine} (h : key0 m' = key0 m) : Same0 m m' := by
  simp only [key0, Prod.mk.injEq] at h
  exact ⟨h.1, h.2.1, h.2.2.1, h.2.2.2.1, fun d hd => Or.inl (h.2.2.2.2 ▸ hd), fun hs => h.2.2.2.2 ▸ hs⟩

theorem Same0.key0_trans {m m' m'' : Machine} (h2 : Same0 m' m'') (h : key0 m' = key0 m) :
    Same0 m m'' :=
  (Same0.of_key0 h).trans h2

theorem key0_of_key {m m' : Machine} (h : key m' = key m) : key0 m' = key0 m := by
  simp only [key, key0, Prod.mk.injEq] at h ⊢
  exact ⟨h.1, h.2.1, h.2.2.1, h.2.2.2.1, h.2.2.2.2.1⟩

theorem key_unwind (v p : ℕ) :
    ∀ (m : Machine) (fs : List Frame), key (preemptVictim.unwind D v p m fs) = key m
  | m, [] => rfl
  | m, .hold h _ :: fs => by
    unfold preemptVictim.unwind
    simp only
    split
    · rw [key_enqueue, key_setS, key_release]
    · rw [key_unwind v p _ fs, key_release]
  | m, .seq _ :: fs => by unfold preemptVictim.unwind; exact key_unwind v p m fs
  | m, .loop _ :: fs => by unfold preemptVictim.unwind; exact key_unwind v p m fs

theorem preemptVictim_key0 (m : Machine) (p : ℕ) :
    key0 (preemptVictim D m p).1 = key0 m ∧ tokSum (preemptVictim D m p).1.iter ≤ tokSum m.iter := by
  unfold preemptVictim
  split
  · exact ⟨rfl, le_rfl⟩
  · rename_i v _
    have k0 : ∀ M fs, key0 (preemptVictim.unwind D p v M fs) = key0 M :=
      fun M fs => key0_of_key (key_unwind D p v M fs)
    have ki : ∀ M fs, (preemptVictim.unwind D p v M fs).iter = M.iter := fun M fs => by
      have := key_unwind D p v M fs
      simp only [key, Prod.mk.injEq] at this
      exact this.2.2.2.2.2
    simp only
    refine ⟨?_, ?_⟩
    · show key0 (preemptVictim.unwind D p v _ _) = key0 m
      exact k0 _ _
    · show tokSum (preemptVictim.unwind D p v _ _).iter ≤ _
      rw [ki]
      exact tokSum_filter _ _

theorem grow_key0 :
    ∀ (f : ℕ) (m : Machine) (i p d : ℕ),
      key0 (grow D f m i p d).1 = key0 m ∧ tokSum (grow D f m i p d).1.iter ≤ tokSum m.iter
  | 0, _, _, _, _ => ⟨rfl, le_rfl⟩
  | f + 1, m, i, p, d => by
    unfold grow
    split
    · exact ⟨rfl, le_rfl⟩
    · simp only
      split
      · exact ⟨rfl, le_rfl⟩
      · obtain ⟨h1, h2⟩ := preemptVictim_key0 D m p
        split
        · rename_i m' heq
          rw [heq] at h1 h2
          exact ⟨h1, h2⟩
        · rename_i m' v heq
          rw [heq] at h1 h2
          split
          · exact ⟨h1, h2⟩
          · obtain ⟨h3, h4⟩ := grow_key0 f m' i p d
            exact ⟨h3.trans h1, h4.trans h2⟩

theorem admitVia_same (m : Machine) (left : ℕ) : Same m (admitVia D m left).1 := by
  unfold admitVia
  split
  · exact Same.refl m
  · split
    · split
      · exact Same.key_trans (same_drain D 10000 _) (by rw [key_admit, key_setPool])
      · exact Same.refl m
    · exact Same.refl m

/-- Building an iteration leaves the clock, the engine's iteration and the
delays alone, and hands out at most `left` tokens. -/
theorem assign_same (pre0 : ℕ) : ∀ (f : ℕ) (m : Machine) (idx left : ℕ),
    Same0 m (assign D f m idx left pre0) ∧
      tokSum (assign D f m idx left pre0).iter ≤ tokSum m.iter + left
  | 0, m, _, _ => ⟨Same0.refl m, Nat.le_add_right _ _⟩
  | f + 1, m, idx, left => by
    unfold assign
    split
    · split
      · split
        · rename_i m' heq
          have hs := admitVia_same D m left
          rw [heq] at hs
          obtain ⟨h1, h2⟩ := assign_same pre0 f m' idx left
          exact ⟨hs.same0.trans h1, hs.iter ▸ h2⟩
        · rename_i m' heq
          have hs := admitVia_same D m left
          rw [heq] at hs
          exact ⟨hs.same0, by rw [hs.iter]; omega⟩
      · exact ⟨Same0.refl m, Nat.le_add_right _ _⟩
    · rename_i j _
      split
      · exact assign_same pre0 f m (idx + 1) left
      · simp only
        split
        · exact assign_same pre0 f m (idx + 1) left
        · rename_i ht
          split
          · split
            · refine ⟨Same0.of_key0 rfl, ?_⟩
              simp only [tokSum_append]
              simp [tokSum] <;> omega
            · obtain ⟨h1, h2⟩ := assign_same pre0 f
                { m with iter := m.iter ++ [(j.owner, min (wantOf D j) left)] } (idx + 1)
                (left - min (wantOf D j) left)
              refine ⟨Same0.key0_trans h1 rfl, ?_⟩
              simp only [tokSum_append] at h2
              simp [tokSum] at h2 ⊢; omega
          · split
            · exact assign_same pre0 f m (idx + 1) left
            · rename_i p _ _ a x _
              -- the growth, then the tokens
              have hg : key0 (if x + min (wantOf D j) left > a then
                  grow D (D.pools.length * 100000) m j.owner p (x + min (wantOf D j) left - a)
                  else (m, true)).1 = key0 m ∧
                  tokSum (if x + min (wantOf D j) left > a then
                  grow D (D.pools.length * 100000) m j.owner p (x + min (wantOf D j) left - a)
                  else (m, true)).1.iter ≤ tokSum m.iter := by
                split
                · exact grow_key0 D _ _ _ _ _
                · exact ⟨rfl, le_rfl⟩
              rcases hr : (if x + min (wantOf D j) left > a then
                  grow D (D.pools.length * 100000) m j.owner p (x + min (wantOf D j) left - a)
                  else (m, true)) with ⟨m', ok⟩
              rw [hr] at hg
              try simp only [hr]
              obtain ⟨hk, ht⟩ := hg
              have hs : Same0 m m' := Same0.of_key0 hk
              split
              · split
                · refine ⟨hs.trans (Same0.of_key0 rfl), ?_⟩
                  simp only [tokSum_append]
                  simp only [tokSum] at ht ⊢
                  simp [setS_iter]; omega
                · obtain ⟨h1, h2⟩ := assign_same pre0 f
                    { setS m' j.owner (mapHold (getS m' j.owner) _ fun (a, x) => (a, x + min (wantOf D j) left)) with
                      iter := m'.iter ++ [(j.owner, min (wantOf D j) left)] } (idx + 1)
                    (left - min (wantOf D j) left)
                  refine ⟨hs.trans (Same0.key0_trans h1 rfl), ?_⟩
                  simp only [tokSum_append] at h2
                  simp only [tokSum] at ht h2 ⊢
                  simp [setS_iter] at h2 ⊢; omega
              · obtain ⟨h1, h2⟩ := assign_same pre0 f m' idx left
                exact ⟨hs.trans h1, by simp only at ht; omega⟩

theorem key0_foldl' {α : Type} (f : Machine → α → Machine) (hf : ∀ m a, key0 (f m a) = key0 m) :
    ∀ (l : List α) (m : Machine), key0 (l.foldl f m) = key0 m
  | [], _ => rfl
  | a :: l, m => (key0_foldl' f hf l (f m a)).trans (hf m a)

theorem key0_endIteration (m : Machine) : key0 (endIteration m) = key0 m := by
  unfold endIteration
  simp only
  refine (key0_foldl' _ ?_ _ _).trans rfl
  intro _ _
  rfl

/-! ### The engine's token rate -/

/-- What the rate bound carries from event to event: the delays are sorted
and none has ended, the running iteration ends no earlier than now, and the
tokens served are at most `R / T` per clock unit up to the end of the running
iteration (or now, while the engine is idle); and so for the last iteration's
start. -/
structure RateInv (T R : ℕ) (m : Machine) : Prop where
  sorted : m.delays.Pairwise (fun a b => a.1 ≤ b.1)
  future : ∀ d ∈ m.delays, m.now ≤ d.1
  running : ∀ e q, m.iterEnd = some (e, q) → m.now ≤ e ∧ m.served * T ≤ R * e
  idle : m.iterEnd = none → m.served * T ≤ R * m.now
  last : m.last.served * T ≤ R * m.last.start

/-- The rate hypothesis on a deployment: every batch within the budget lasts
at least `T / R` clock units per token. -/
def RateBound (T R : ℕ) : Prop :=
  ∀ st : IterStats, st.tokens ≤ D.budget → st.tokens * T ≤ R * max 1 (D.cost st)

theorem RateInv.of_same {T R : ℕ} {m m' : Machine} (h : RateInv T R m) (hs : Same m m') :
    RateInv T R m' where
  sorted := hs.sorted h.sorted
  future d hd := by
    rw [hs.now]
    rcases hs.delays d hd with h1 | h1
    · exact h.future d h1
    · exact h1
  running e q he := by
    rw [hs.now, hs.served]
    exact h.running e q (hs.iterEnd ▸ he)
  idle he := by
    rw [hs.now, hs.served]
    exact h.idle (hs.iterEnd ▸ he)
  last := hs.last ▸ h.last

theorem iterStats_tokens (m : Machine) : (iterStats D m).tokens = tokSum m.iter := rfl

theorem startIteration_inv {T R : ℕ} (hb : RateBound D T R) (m : Machine) (h : RateInv T R m)
    (hi : m.iterEnd = none) : RateInv T R (startIteration D m) := by
  have hm := h.idle hi
  unfold startIteration
  simp only
  split
  · obtain ⟨hs, htok⟩ := assign_same D m.preempts (m.jobs.length + 100000) { m with iter := [] } 0 D.budget
    set m' := assign D (m.jobs.length + 100000) { m with iter := [] } 0 D.budget m.preempts with hm'
    have hn : m'.now = m.now := hs.now
    have hsv : m'.served = m.served := hs.served
    have hie : m'.iterEnd = none := hs.iterEnd.trans hi
    have hl : m'.last = m.last := hs.last
    have hdl : ∀ d ∈ m'.delays, m.now ≤ d.1 := fun d hd => by
      rcases hs.delays d hd with h1 | h1
      · exact h.future d h1
      · exact h1
    have hst : m'.delays.Pairwise (fun a b => a.1 ≤ b.1) := hs.sorted h.sorted
    simp only [tokSum, List.map_nil, List.sum_nil, zero_add] at htok
    split
    · have hc := hb (iterStats D m') (by rw [iterStats_tokens]; exact htok)
      rw [iterStats_tokens] at hc
      refine ⟨hst, fun d hd => hn ▸ hdl d hd, ?_, ?_, ?_⟩
      · intro e q he
        simp only [Option.some.injEq, Prod.mk.injEq] at he
        rw [← he.1, hn, hsv]
        constructor
        · omega
        · rw [Nat.add_mul, Nat.mul_add]
          exact Nat.add_le_add hm hc
      · intro he; simp at he
      · show m'.served * T ≤ R * m'.now
        rw [hn, hsv]; exact hm
    · refine ⟨hst, fun d hd => hn ▸ hdl d hd, ?_, ?_, ?_⟩
      · intro e q he; simp at he
      · intro _; rw [hn, hsv]; exact hm
      · rw [hl]; exact h.last
  · exact ⟨h.sorted, h.future, fun e q he => by simp [hi] at he, fun _ => hm, h.last⟩

theorem afterEvent_inv {T R : ℕ} (hb : RateBound D T R) (m : Machine) (h : RateInv T R m) :
    RateInv T R (afterEvent D m) := by
  unfold afterEvent
  simp only
  have h1 := h.of_same (same_settle D m)
  split
  · rename_i hc
    simp only [Bool.and_eq_true, Option.isNone_iff_eq_none] at hc
    exact startIteration_inv D hb _ h1 hc.1
  · exact h1

/-- The next event is the earliest: no delay and no iteration ends before it. -/
theorem nextEvent_le {m : Machine} (hs : m.delays.Pairwise (fun a b => a.1 ≤ b.1)) {t q : ℕ}
    (he : nextEvent m = some (t, q)) :
    (∀ d ∈ m.delays, t ≤ d.1) ∧ (∀ e q', m.iterEnd = some (e, q') → t ≤ e) ∧
      ((∃ q', m.iterEnd = some (t, q')) ∨ ∃ d ∈ m.delays, d.1 = t) := by
  have head : ∀ (u : ℕ × ℕ × ℕ) rest, m.delays = u :: rest → ∀ d ∈ m.delays, u.1 ≤ d.1 := by
    intro u rest hd d hdm
    rw [hd] at hs hdm
    rcases List.mem_cons.mp hdm with rfl | hdm
    · exact le_rfl
    · exact (List.pairwise_cons.mp hs).1 d hdm
  unfold nextEvent at he
  rcases hi : m.iterEnd with _ | ⟨a, qa⟩ <;> rcases hdl : m.delays with _ | ⟨⟨u, qu, i⟩, rest⟩ <;>
    simp only [hi, hdl, List.head?_nil, List.head?_cons, Option.some.injEq, Prod.mk.injEq,
      reduceCtorEq] at he
  · have hh := head _ _ hdl
    refine ⟨fun d hd => he.1 ▸ hh d (hdl ▸ hd), fun e q' h' => by simp at h', Or.inr ⟨(u, qu, i), List.mem_cons_self, he.1⟩⟩
  · refine ⟨fun d hd => by simp [hdl] at hd, fun e q' h' => ?_, Or.inl ⟨qa, by rw [he.1, he.2]⟩⟩
    simp only [Option.some.injEq, Prod.mk.injEq] at h'
    omega
  · have hh := head _ _ hdl
    split at he
    · simp only [Prod.mk.injEq] at he
      refine ⟨fun d hd => he.1 ▸ hh d (hdl ▸ hd), fun e q' h' => ?_, Or.inr ⟨(u, qu, i), List.mem_cons_self, he.1⟩⟩
      simp only [Option.some.injEq, Prod.mk.injEq] at h'
      rename_i hlt
      omega
    · simp only [Prod.mk.injEq] at he
      rename_i hlt
      refine ⟨fun d hd => ?_, fun e q' h' => ?_, Or.inl ⟨qa, by rw [he.1, he.2]⟩⟩
      · have := hh d (hdl ▸ hd)
        simp only at this
        omega
      · simp only [Option.some.injEq, Prod.mk.injEq] at h'
        omega

theorem step_inv {T R : ℕ} (hb : RateBound D T R) (m : Machine) (h : RateInv T R m) :
    RateInv T R (step D m) := by
  unfold step
  split
  · exact h
  · rename_i t q he
    obtain ⟨hd, hie, hwho⟩ := nextEvent_le h.sorted he
    have hnt : m.now ≤ t := by
      rcases hwho with ⟨q', h'⟩ | ⟨d, hdm, hdt⟩
      · exact (h.running t q' h').1
      · exact hdt ▸ h.future d hdm
    apply afterEvent_inv D hb
    unfold handle; simp only
    split
    · rename_i hit
      -- the iteration ends: its tokens were counted at its start
      have hr := (h.running t q hit).2
      have hk := key0_endIteration { m with now := t, iterEnd := none }
      simp only [key0, Prod.mk.injEq] at hk
      refine ⟨?_, ?_, ?_, ?_, ?_⟩
      · rw [hk.2.2.2.2]; exact h.sorted
      · intro d hdm; rw [hk.1]; rw [hk.2.2.2.2] at hdm; exact hd d hdm
      · intro e q' he'; rw [hk.2.2.1] at he'; simp at he'
      · intro _; rw [hk.1, hk.2.1]; exact hr
      · rw [hk.2.2.2.1]; exact h.last
    · split
      · rename_i u q' i rest hdl
        have hrest : ∀ d ∈ rest, t ≤ d.1 := fun d hdm => hd d (by simp [hdl, hdm])
        have hsr : rest.Pairwise (fun a b => a.1 ≤ b.1) := by
          have := h.sorted; rw [hdl] at this; exact (List.pairwise_cons.mp this).2
        have base : RateInv T R { m with now := t, delays := rest } := by
          refine ⟨hsr, hrest, ?_, ?_, h.last⟩
          · intro e q'' he'
            exact ⟨hie e q'' he', (h.running e q'' he').2⟩
          · intro he'
            exact (h.idle he').trans (Nat.mul_le_mul_left _ hnt)
        split
        · exact base.of_same (Same.of_key rfl)
        · exact base
      · rename_i hdl
        refine ⟨by rw [hdl]; simp, by intro d hdm; simp [hdl] at hdm, ?_, ?_, h.last⟩
        · intro e q'' he'
          exact ⟨hie e q'' he', (h.running e q'' he').2⟩
        · intro he'
          exact (h.idle he').trans (Nat.mul_le_mul_left _ hnt)

theorem reach_inv {T R : ℕ} (hb : RateBound D T R) {w : Workload} {P : Prog} {m : Machine}
    (hr : Reach D w P m) : RateInv T R m := by
  induction hr with
  | start =>
    apply afterEvent_inv D hb
    exact ⟨List.Pairwise.nil, fun d hd => by simp [Exec.initial] at hd, fun e q he => by simp [Exec.initial] at he,
      fun _ => by simp [Exec.initial], by simp [Exec.initial]⟩
  | step _ ih => exact step_inv D hb _ ih

/-- **The engine's token rate.** If every batch within the budget lasts at
least `T / R` clock units per token, then on every path of every program the
tokens served before an iteration are at most `R / T` per clock unit of its
start. -/
theorem served_rate {T R : ℕ} (hb : RateBound D T R) (W : Workload → Prop) (P : Prog) :
    EveryIteration D W P fun r => r.served * T ≤ R * r.start :=
  fun _ _ _ hr _ => (reach_inv D hb hr).last

end Exec
end SerqLang
