/-
# serQ: an executable semantics

`Serq/Core.lean` gives the syntax of serQ (`Route Env V`) and the relational
semantics of one memory pool (`Step`, with the memory invariant). This
module gives an *executable* semantics of the fragment that production
schedulers are written in: values are natural numbers (tokens), time is
a natural number, and the deployment is a list of pools plus the engine
(stage 0), whose iteration lasts `cost` clock units (`fun _ => 1` is the
step clock, one iteration per unit); every other stage is a delay. Time is
event-driven, as in the Rust interpreter: `step` moves the clock to the
next event (the end of the running iteration or of a delay) and handles
everything that happens then. The workload is data (`Workload`): each session's preset
attributes and, optionally, its turns, which a `turn` statement reads in
order (as serQ reads explicit sessions of its IR, or an ordered trace).

* **Pools** allocate in blocks, keep released prefixes as cache entries
  (per session, tail blocks evicted first, least recently released entry
  first, ties by release order), admit the head of their queue when every
  pool of its hold has room for its `fits` units (allocated units never
  count the cache), with a `cache` clause consume at most `reuse` of the
  own prefix (the rest stays cached, dead, with its age; a hold without the
  clause leaves the entry where it is, serQ IR 10), and on a failed growth preempt the
  most recently admitted holder, whose hold is released (its computed
  prefix cached) and re-queued at the head. A pool marked `viaEngine`
  (`admit via engine`) is admitted by the engine at the start of an
  iteration, with the budget its residents leave (`budgetLeft`), and not
  in an iteration that preempted.
* **The engine** hands its budget to its residents in admission order, one
  token to a decoding job and up to `chunk` to a prefilling one, grows the
  holds of `growing` jobs before the tokens are committed, then admits.

These are the rules of serQ `src/sim.rs`, which reproduces the real vLLM
v1 scheduler step for step (serQ `docs/language.md` §7). The vLLM
scheduler scenarios of serQ `tools/oracle/` are then theorems about serQ
programs (`Serq/Oracle.lean`), checked by evaluation in the kernel.

Key definitions: `Exec.Deployment`, `Exec.Workload`, `Exec.Machine`,
`Exec.step`, `Exec.run`, `Exec.runW`.
Key theorems: `Exec.makeRoomFast_eq` and `@[csimp] Exec.makeRoom_eq_fast`
(the compiled code evicts an entry's blocks at once, and that is the
definition), `Exec.Attrs.get_upd` (stored attributes are `Function.update`),
`Exec.makeRoom_used` (eviction never touches the allocation),
`Exec.makeRoom_room` (the eviction loop makes the room it is asked for, or
empties the cache).
-/
import Serq.Core

namespace SerqLang
namespace Exec

/-- What an expression of the executable fragment reads. -/
structure Env where
  attr : ℕ → ℕ
  serial : ℕ
  now : ℕ
  /-- units consumed at the session's last admission -/
  cached : ℕ
  /-- the session's own cached units, per pool -/
  cachedIn : ℕ → ℕ
  /-- tokens the engine's current iteration leaves after its residents -/
  budgetLeft : ℕ

/-- Programs of the executable fragment. -/
abbrev Prog := Route Env ℕ

structure PoolDef where
  cap : ℕ
  block : ℕ
  viaEngine : Bool

/-- What an iteration's cost reads (serQ's context variables): the tokens it
serves (`tokens`), the prefill tokens among them (`prefilled`), the decoding
residents it advances (`decoders`), the memory those hold on the engine's
pool (`kv_decode`), and twice the attention work of its prefill chunks,
`Σ n (2K + n)` with `K` the position before the chunk (`2 · attention`,
so that it is a natural number). -/
structure IterStats where
  tokens : ℕ
  prefilled : ℕ
  decoders : ℕ
  kvDecode : ℕ
  attention2 : ℕ

/-- A deployment: pools, the step engine (stage 0) and a delay stage (1). -/
structure Deployment where
  pools : List PoolDef
  budget : ℕ
  /-- per-request chunk cap (`long_prefill_token_threshold`), 0 = none -/
  chunk : ℕ
  /-- the engine's memory pool (`memory` of the step stage), if any -/
  memory : Option ℕ
  /-- the length of an iteration in clock units (at least 1 is used);
  `fun _ => 1` is the step clock -/
  cost : IterStats → ℕ

structure Entry where
  owner : ℕ
  size : ℕ
  last : ℕ
  seq : ℕ

structure HoldRec where
  /-- (pool, allocated units, position: consumed prefix + tokens computed) -/
  pools : List (ℕ × ℕ × ℕ)
  grown : Bool
  cache : Option (Env → ℕ)
  /-- the statement, to re-execute after a preemption -/
  stmt : Prog

inductive Frame
  | seq (k : Prog)
  | hold (h : HoldRec) (k : Prog)
  | loop (body : Prog)

inductive Status
  | ready
  | queued
  | engine
  /-- in a delay until `wake`; `seq` is the delay's event number (events at
  the same time are handled in the order they were scheduled, as in the
  interpreter) -/
  | delay (wake seq : ℕ)
  | ended
  deriving DecidableEq

/-- A session's attributes: the preset values `base`, overridden slot by slot
by `vals` (slots below `vals.length`). `get` is the attribute function the
programs read and `upd` is `Function.update` on it (`get_upd`); stored this
way, a read costs at most one pass over the program's few slots however many
times the session has set them. -/
structure Attrs where
  base : ℕ → ℕ
  vals : List ℕ

namespace Attrs

def get (a : Attrs) (k : ℕ) : ℕ := a.vals.getD k (a.base k)

/-- The values `base n, base (n+1), …, base (n+c-1)`. -/
def fill (base : ℕ → ℕ) (n : ℕ) : ℕ → List ℕ
  | 0 => []
  | c + 1 => base n :: fill base (n + 1) c

def upd (a : Attrs) (k v : ℕ) : Attrs :=
  if k < a.vals.length then { a with vals := a.vals.set k v }
  else { a with vals := a.vals ++ fill a.base a.vals.length (k - a.vals.length) ++ [v] }

theorem fill_length (base : ℕ → ℕ) (n c : ℕ) : (fill base n c).length = c := by
  induction c generalizing n with
  | zero => rfl
  | succ c ih => simp [fill, ih]

theorem fill_getElem? (base : ℕ → ℕ) (n c i : ℕ) (h : i < c) :
    (fill base n c)[i]? = some (base (n + i)) := by
  induction c generalizing n i with
  | zero => omega
  | succ c ih =>
    cases i with
    | zero => simp [fill]
    | succ i =>
      simp only [fill, List.getElem?_cons_succ]
      rw [ih (n + 1) i (by omega)]; congr 2; omega

/-- `upd` is `Function.update` on the attribute function. -/
theorem get_upd (a : Attrs) (k v j : ℕ) :
    (a.upd k v).get j = Function.update a.get k v j := by
  unfold upd get
  simp only [List.getD_eq_getElem?_getD]
  by_cases hk : k < a.vals.length
  · rw [if_pos hk]
    simp only [List.getElem?_set]
    by_cases hjk : j = k
    · subst hjk; simp [hk]
    · simp [Ne.symm hjk, Function.update, hjk]
  · rw [if_neg hk]
    have hl := fill_length a.base a.vals.length (k - a.vals.length)
    by_cases hjk : j = k
    · subst hjk
      rw [List.getElem?_append_right (by simp [hl]; omega)]
      have h0 : j - (a.vals.length + (j - a.vals.length)) = 0 := by omega
      simp [hl, Function.update, h0]
    · simp only [Function.update, hjk, dite_false]
      by_cases hj : j < a.vals.length
      · rw [List.append_assoc, List.getElem?_append_left hj]
      · rw [List.append_assoc, List.getElem?_append_right (by omega)]
        by_cases hjk' : j < k
        · rw [List.getElem?_append_left (by rw [hl]; omega),
            fill_getElem? _ _ _ _ (by omega)]
          have : a.vals[j]? = none := List.getElem?_eq_none (by omega)
          simp [this]; congr 1; omega
        · rw [List.getElem?_append_right (by rw [hl]; omega)]
          have h2 : j - a.vals.length - (fill a.base a.vals.length (k - a.vals.length)).length ≠ 0 := by
            rw [hl]; omega
          have : a.vals[j]? = none := List.getElem?_eq_none (by omega)
          simp [this]
          rw [List.getElem?_eq_none (by simp; omega)]
          simp

end Attrs

structure Sess where
  serial : ℕ
  attr : Attrs
  cached : ℕ
  prog : Prog
  stack : List Frame
  status : Status
  admSeq : ℕ
  /-- the next of the session's turns (`Workload.turns`) -/
  turnIx : ℕ := 0

/-- The workload instance: per session, preset attributes (slot, value) and
its turns, each a list of (slot, value). A `turn` statement counts turns in
`turnSlot`, sets the next turn's values, and sets `moreSlot` to 1 while
another turn remains and to 0 after the last (serQ `src/sim.rs`, `do_turn`).
A session without turns leaves `moreSlot` alone. -/
structure Workload where
  init : List (List (ℕ × ℕ))
  turns : List (List (List (ℕ × ℕ))) := []
  turnSlot : Option ℕ := none
  moreSlot : ℕ := 0

structure Job where
  owner : ℕ
  mode : Mode
  left : ℕ
  growing : Option ℕ

structure PoolSt where
  used : ℕ
  entries : List Entry
  holders : List ℕ
  queue : List ℕ

structure Machine where
  wl : Workload := ⟨[], [], none, 0⟩
  now : ℕ
  sess : Array Sess
  pools : List PoolSt
  jobs : List Job
  iter : List (ℕ × ℕ)
  /-- (name, serial, time, value), the most recent first (`observed` reads
  them in the order they were made) -/
  obs : List (ℕ × ℕ × ℕ × ℕ)
  preempts : ℕ
  nextAdm : ℕ
  nextRel : ℕ
  nextDead : ℕ
  /-- the number of the next scheduled event (a delay's end or an
  iteration's end): events at the same time are handled in this order -/
  nextDelay : ℕ
  ready : List ℕ
  /-- the delays scheduled, as (end, event number, session), by end time and
  then event number. A preempted session's entry stays until its time and is
  then ignored: the session's status no longer names that delay. -/
  delays : List (ℕ × ℕ × ℕ) := []
  /-- the end of the engine's running iteration and its event number;
  `none` while the engine is idle -/
  iterEnd : Option (ℕ × ℕ) := none

variable (D : Deployment)

/-! ### Helpers -/

/-- Insert a delay into the list ordered by end time, after the delays that
end no later (they started earlier). -/
def insertDelay (d : ℕ × ℕ × ℕ) : List (ℕ × ℕ × ℕ) → List (ℕ × ℕ × ℕ)
  | [] => [d]
  | x :: xs => if d.1 < x.1 then d :: x :: xs else x :: insertDelay d xs

def roundUp (b u : ℕ) : ℕ := if b = 0 then u else (u + b - 1) / b * b
def roundDown (b u : ℕ) : ℕ := if b = 0 then u else u / b * b

def pdef (p : ℕ) : PoolDef := D.pools.getD p ⟨0, 1, false⟩
def pst (m : Machine) (p : ℕ) : PoolSt := m.pools.getD p ⟨0, [], [], []⟩
def setPool (m : Machine) (p : ℕ) (s : PoolSt) : Machine := { m with pools := m.pools.set p s }
def getS (m : Machine) (i : ℕ) : Sess := m.sess.getD i ⟨i, ⟨fun _ => 0, []⟩, 0, .stop, [], .ended, 0, 0⟩
def setS (m : Machine) (i : ℕ) (s : Sess) : Machine := { m with sess := m.sess.setIfInBounds i s }

def cachedTotal (s : PoolSt) : ℕ := (s.entries.map Entry.size).sum
def ownEntry (s : PoolSt) (r : ℕ) : ℕ := ((s.entries.find? (·.owner = r)).map Entry.size).getD 0
def removeEntry (s : PoolSt) (r : ℕ) : PoolSt := { s with entries := s.entries.filter (·.owner ≠ r) }

def env (m : Machine) (i : ℕ) (left : ℕ := 0) : Env :=
  let s := getS m i
  { attr := s.attr.get, serial := s.serial, now := m.now, cached := s.cached,
    cachedIn := fun p => ownEntry (pst m p) s.serial, budgetLeft := left }

def evalE (m : Machine) (i : ℕ) (e : Env → ℕ) (left : ℕ := 0) : ℕ := e (env m i left)

/-- The least recently released entry (ties: release order). -/
def lru : List Entry → Option Entry
  | [] => none
  | e :: es => match lru es with
    | none => some e
    | some f => if e.last < f.last ∨ (e.last = f.last ∧ e.seq ≤ f.seq) then some e else some f

/-- Evict one block from the tail of the least recently released entry. -/
def evictOne (b : ℕ) (s : PoolSt) : PoolSt :=
  match lru s.entries with
  | none => s
  | some e =>
    let rm := min (max b 1) e.size
    let es := s.entries.filter (fun f => ¬ (f.owner = e.owner ∧ f.seq = e.seq))
    { s with entries := if e.size - rm = 0 then es else { e with size := e.size - rm } :: es }

/-- Evict until `need` more units fit next to the allocation and the cache. -/
def makeRoom (b cap need : ℕ) : ℕ → PoolSt → PoolSt
  | 0, s => s
  | f + 1, s =>
    if s.used + cachedTotal s + need ≤ cap ∨ s.entries = [] then s
    else makeRoom b cap need f (evictOne b s)

theorem evictOne_used (b : ℕ) (s : PoolSt) : (evictOne b s).used = s.used := by
  unfold evictOne; split <;> rfl

/-- Eviction never touches the allocation. -/
theorem makeRoom_used (b cap need f : ℕ) (s : PoolSt) : (makeRoom b cap need f s).used = s.used := by
  induction f generalizing s with
  | zero => rfl
  | succ f ih =>
    unfold makeRoom
    split_ifs
    · rfl
    · rw [ih, evictOne_used]

/-! ### The eviction loop makes room -/

theorem lru_mem : ∀ {l : List Entry} {e : Entry}, lru l = some e → e ∈ l
  | [], _, h => by simp [lru] at h
  | x :: xs, e, h => by
    unfold lru at h
    cases hx : lru xs with
    | none => rw [hx] at h; simp at h; simp [h]
    | some f =>
      rw [hx] at h
      simp only at h
      split_ifs at h with hc
      · simp at h; simp [h]
      · simp at h; exact List.mem_cons_of_mem _ (h ▸ lru_mem hx)

theorem sum_filter_le (q : Entry → Bool) :
    ∀ l : List Entry, ((l.filter q).map Entry.size).sum ≤ (l.map Entry.size).sum
  | [] => by simp
  | x :: xs => by
    have := sum_filter_le q xs
    cases hq : q x <;> simp [List.filter_cons, hq] <;> omega

/-- Removing (at least) one occurrence of `e` removes at least its size. -/
theorem sum_filter_add_le (q : Entry → Bool) :
    ∀ {l : List Entry} {e : Entry}, e ∈ l → q e = false →
      ((l.filter q).map Entry.size).sum + e.size ≤ (l.map Entry.size).sum
  | [], _, h, _ => by simp at h
  | x :: xs, e, h, hq => by
    rcases List.mem_cons.mp h with rfl | h
    · have := sum_filter_le q xs
      simp [List.filter_cons, hq]; omega
    · have := sum_filter_add_le q h hq
      cases hx : q x <;> simp [List.filter_cons, hx] <;> omega

theorem lru_some : ∀ {l : List Entry}, l ≠ [] → ∃ e, lru l = some e
  | [], h => absurd rfl h
  | x :: xs, _ => by
    unfold lru
    cases lru xs with
    | none => exact ⟨x, rfl⟩
    | some f => simp only; split_ifs <;> exact ⟨_, rfl⟩

/-- With positive entries, one eviction strictly shrinks the cache. -/
theorem evictOne_lt (b : ℕ) (s : PoolSt) (hne : s.entries ≠ [])
    (hpos : ∀ e ∈ s.entries, 0 < e.size) : cachedTotal (evictOne b s) < cachedTotal s := by
  obtain ⟨e, hl⟩ := lru_some hne
  have he := lru_mem hl
  have hsz := hpos e he
  have key := sum_filter_add_le (fun f => decide (¬ (f.owner = e.owner ∧ f.seq = e.seq))) he
    (by simp)
  have hrm : 0 < min (max b 1) e.size := lt_min (by omega) hsz
  have hrm' : min (max b 1) e.size ≤ e.size := min_le_right _ _
  unfold evictOne cachedTotal
  rw [hl]
  simp only
  split_ifs with h0
  · omega
  · simp only [List.map_cons, List.sum_cons]
    omega

theorem evictOne_pos (b : ℕ) (s : PoolSt) (hpos : ∀ e ∈ s.entries, 0 < e.size) :
    ∀ e ∈ (evictOne b s).entries, 0 < e.size := by
  unfold evictOne
  cases hl : lru s.entries with
  | none => simpa using hpos
  | some e =>
    simp only
    split_ifs with h0
    · intro f hf; exact hpos f (List.mem_of_mem_filter hf)
    · intro f hf
      rcases List.mem_cons.mp hf with rfl | hf
      · simp only; omega
      · exact hpos f (List.mem_of_mem_filter hf)

/-- The eviction loop of an admission or a growth, given more fuel than
cached units, ends with room for `need` next to the allocation and the
cache, or with the cache empty (then the guard `used + need ≤ cap` of the
admission gives the room). A block of size 0 is treated as 1. -/
theorem makeRoom_room (b cap need : ℕ) :
    ∀ (f : ℕ) (s : PoolSt), cachedTotal s < f → (∀ e ∈ s.entries, 0 < e.size) →
      let t := makeRoom b cap need f s
      t.used + cachedTotal t + need ≤ cap ∨ t.entries = []
  | 0, s, h, _ => by simp at h
  | f + 1, s, h, hpos => by
    unfold makeRoom
    split_ifs with hc
    · rcases hc with hc | hc
      · exact Or.inl hc
      · exact Or.inr hc
    · push_neg at hc
      have hlt := evictOne_lt b s hc.2 hpos
      have := makeRoom_room b cap need f (evictOne b s) (by omega) (evictOne_pos b s hpos)
      simpa [evictOne_used] using this

/-! ### Evicting an entry's blocks at once

`makeRoom` is the definition: one block per step. `makeRoomFast` evicts, in
one step, as many blocks of the least recently released entry as the
definition would evict one by one, so it finds that entry and sums the cache
once per entry instead of once per block. `makeRoom_eq_fast` proves the two
equal, and `@[csimp]` makes the compiler run the fast one; every theorem is
still about `makeRoom`. The attribute applies to code compiled after it, so
it must precede the first caller of `makeRoom` (`admit`, `grow`). -/

/-- `e` comes no later than `g` in eviction order (release time, then order). -/
def Entry.before (e g : Entry) : Prop := e.last < g.last ∨ (e.last = g.last ∧ e.seq ≤ g.seq)

theorem Entry.before_trans {e f g : Entry} (h1 : e.before f) (h2 : f.before g) : e.before g := by
  unfold Entry.before at *; omega

theorem Entry.before_total (e f : Entry) : e.before f ∨ f.before e := by
  unfold Entry.before; omega

/-- The least recently released entry comes no later than any other. -/
theorem lru_before : ∀ {l : List Entry} {e : Entry}, lru l = some e → ∀ g ∈ l, e.before g
  | [], _, h, _, _ => by simp [lru] at h
  | x :: xs, e, h, g, hg => by
    unfold lru at h
    cases hx : lru xs with
    | none =>
      rw [hx] at h; simp at h; subst h
      rcases List.mem_cons.mp hg with rfl | hg
      · unfold Entry.before; omega
      · obtain ⟨_, hy⟩ := lru_some (l := xs) (List.ne_nil_of_mem hg)
        rw [hx] at hy; simp at hy
    | some f =>
      rw [hx] at h
      simp only at h
      have hf := lru_before hx
      split_ifs at h with hc
      · simp at h; subst h
        rcases List.mem_cons.mp hg with rfl | hg
        · unfold Entry.before; omega
        · exact Entry.before_trans hc (hf g hg)
      · simp at h; subst h
        have hfx : f.before x := by
          rcases Entry.before_total x f with h' | h'
          · exact absurd h' hc
          · exact h'
        rcases List.mem_cons.mp hg with rfl | hg
        · exact hfx
        · exact hf g hg

/-- An entry that comes no later than every other is the one `lru` picks
when it is first. -/
theorem lru_cons_of_before {e : Entry} {es : List Entry} (h : ∀ g ∈ es, e.before g) :
    lru (e :: es) = some e := by
  unfold lru
  cases hx : lru es with
  | none => rfl
  | some f =>
    have := h f (lru_mem hx)
    unfold Entry.before at this
    simp only [this, if_true]

/-- The entries other than `e` (by owner and release order). -/
def othersIn (e : Entry) (s : PoolSt) : List Entry :=
  s.entries.filter (fun f => ¬ (f.owner = e.owner ∧ f.seq = e.seq))

/-- Evict `k` blocks from the tail of entry `e`. -/
def evictK (b k : ℕ) (e : Entry) (s : PoolSt) : PoolSt :=
  { s with entries :=
      if e.size - min (k * max b 1) e.size = 0 then othersIn e s
      else { e with size := e.size - min (k * max b 1) e.size } :: othersIn e s }

theorem evictOne_eq (b : ℕ) {s : PoolSt} {e : Entry} (h : lru s.entries = some e) :
    evictOne b s = evictK b 1 e s := by
  simp [evictOne, evictK, h, othersIn]

theorem othersIn_filter_none (e : Entry) (s : PoolSt) :
    ∀ g ∈ othersIn e s, ¬ (g.owner = e.owner ∧ g.seq = e.seq) := by
  intro g hg
  simp only [othersIn, List.mem_filter, decide_eq_true_eq] at hg
  exact hg.2

/-- One more block from an entry that is still there. -/
theorem evictOne_evictK (b : ℕ) {s : PoolSt} {e : Entry} (h : lru s.entries = some e)
    (j : ℕ) (hj : j * max b 1 < e.size) :
    evictOne b (evictK b j e s) = evictK b (j + 1) e s := by
  have hrm : min (j * max b 1) e.size = j * max b 1 := min_eq_left hj.le
  have hne : ¬ (e.size - j * max b 1 = 0) := by omega
  have hbef : ∀ g ∈ othersIn e s, ({ e with size := e.size - j * max b 1 } : Entry).before g :=
    fun g hg => lru_before h g (List.mem_of_mem_filter hg)
  have hent : (evictK b j e s).entries = { e with size := e.size - j * max b 1 } :: othersIn e s := by
    simp only [evictK, hrm, hne, ↓reduceIte]
  have hl : lru ((evictK b j e s).entries) = some { e with size := e.size - j * max b 1 } := by
    rw [hent]; exact lru_cons_of_before hbef
  have hfilt : ((evictK b j e s).entries.filter
      (fun f => ¬ (f.owner = e.owner ∧ f.seq = e.seq))) = othersIn e s := by
    rw [hent, List.filter_cons_of_neg (by simp)]
    exact List.filter_eq_self.mpr fun g hg => decide_eq_true (othersIn_filter_none e s g hg)
  have key : e.size - j * max b 1 - min (max b 1) (e.size - j * max b 1)
      = e.size - min ((j + 1) * max b 1) e.size := by
    rw [Nat.succ_mul]; generalize j * max b 1 = P at *; omega
  rw [evictOne, hl]
  simp only [hfilt, key]
  rfl

/-- Unrolling the definition: while the states in between do not fit,
`makeRoom` evicts `j` blocks of the least recently released entry. -/
theorem makeRoom_unroll (b cap need : ℕ) {s : PoolSt} {e : Entry} (h : lru s.entries = some e) :
    ∀ (j f : ℕ), 1 ≤ j → j ≤ f → (j - 1) * max b 1 < e.size ∨ j = 1 →
      ¬ (s.used + cachedTotal s + need ≤ cap ∨ s.entries = []) →
      (∀ i, 1 ≤ i → i < j →
        ¬ (s.used + cachedTotal (evictK b i e s) + need ≤ cap ∨ (evictK b i e s).entries = [])) →
      makeRoom b cap need f s = makeRoom b cap need (f - j) (evictK b j e s)
  | 0, _, h1, _, _, _, _ => by omega
  | 1, f + 1, _, _, _, hs, _ => by
    rw [makeRoom, if_neg hs, evictOne_eq b h]; rfl
  | 1, 0, _, h2, _, _, _ => by omega
  | j + 2, f, _, hjf, hsz, hs, hmid => by
    have hsz' : (j + 1) * max b 1 < e.size := by
      rcases hsz with h' | h'
      · simpa using h'
      · omega
    have hle : j * max b 1 ≤ (j + 1) * max b 1 := Nat.mul_le_mul_right _ (by omega)
    have ih := makeRoom_unroll b cap need h (j + 1) f (by omega) (by omega)
      (Or.inl (by simp only [Nat.add_sub_cancel]; omega)) hs (fun i h1 h2 => hmid i h1 (by omega))
    rw [ih]
    have hfj : f - (j + 1) = (f - (j + 2)) + 1 := by omega
    have hnot := hmid (j + 1) (by omega) (by omega)
    have hnot' : ¬ ((evictK b (j + 1) e s).used + cachedTotal (evictK b (j + 1) e s) + need ≤ cap ∨
        (evictK b (j + 1) e s).entries = []) := hnot
    rw [hfj, makeRoom, if_neg hnot', evictOne_evictK b h (j + 1) hsz']

theorem cachedTotal_evictK (b k : ℕ) (e : Entry) (s : PoolSt) :
    cachedTotal (evictK b k e s)
      = ((othersIn e s).map Entry.size).sum + (e.size - min (k * max b 1) e.size) := by
  unfold cachedTotal evictK
  dsimp only
  split_ifs with h0
  · rw [h0]; simp
  · simp only [List.map_cons, List.sum_cons]; omega

/-- `makeRoom`, evicting an entry's blocks in one step. -/
def makeRoomFast (b cap need : ℕ) (f : ℕ) (s : PoolSt) : PoolSt :=
  match f with
  | 0 => s
  | f + 1 =>
    if s.used + cachedTotal s + need ≤ cap ∨ s.entries = [] then s
    else match lru s.entries with
      | none => s
      | some e =>
        -- the cache left after removing `e`, and the blocks to evict from it
        let rest := ((othersIn e s).map Entry.size).sum
        let deficit := s.used + rest + e.size + need - cap
        let k := min (max 1 (min ((deficit + max b 1 - 1) / max b 1)
          ((e.size + max b 1 - 1) / max b 1))) (f + 1)
        makeRoomFast b cap need (f + 1 - k) (evictK b k e s)
termination_by f
decreasing_by
  exact Nat.sub_lt (Nat.succ_pos f)
    (lt_of_lt_of_le Nat.zero_lt_one (le_min (le_max_left _ _) (Nat.succ_pos f)))

theorem ceil_div_pred_lt {x b k : ℕ} (hb : 0 < b) (hk : k ≤ (x + b - 1) / b) (hk2 : 2 ≤ k) :
    (k - 1) * b < x := by
  have h1 : k * b ≤ (x + b - 1) / b * b := Nat.mul_le_mul_right _ hk
  have h2 : (x + b - 1) / b * b ≤ x + b - 1 := Nat.div_mul_le_self _ _
  have h3 : 2 * b ≤ k * b := Nat.mul_le_mul_right _ hk2
  rw [Nat.sub_mul, one_mul]
  generalize k * b = P at *
  generalize (x + b - 1) / b * b = Q at *
  omega

theorem lt_ceil_div {d b i : ℕ} (hb : 0 < b) (hi : i < (d + b - 1) / b) : i * b < d := by
  have h1 : (i + 1) * b ≤ (d + b - 1) / b * b := Nat.mul_le_mul_right _ hi
  have h2 : (d + b - 1) / b * b ≤ d + b - 1 := Nat.div_mul_le_self _ _
  rw [Nat.succ_mul] at h1
  generalize i * b = P at *
  generalize (d + b - 1) / b * b = Q at *
  omega

/-- The fast loop is the definition. -/
theorem makeRoomFast_eq (b cap need : ℕ) :
    ∀ (f : ℕ) (s : PoolSt), makeRoomFast b cap need f s = makeRoom b cap need f s := by
  intro f
  induction f using Nat.strong_induction_on with
  | _ f ih =>
    intro s
    cases f with
    | zero => rw [makeRoomFast, makeRoom]
    | succ f =>
      rw [makeRoomFast]
      by_cases hs : s.used + cachedTotal s + need ≤ cap ∨ s.entries = []
      · rw [if_pos hs, makeRoom, if_pos hs]
      · rw [if_neg hs]
        have hne : s.entries ≠ [] := fun h => hs (Or.inr h)
        obtain ⟨e, he⟩ := lru_some hne
        simp only [he]
        have hb : 0 < max b 1 := lt_of_lt_of_le Nat.zero_lt_one (le_max_right b 1)
        generalize hrest : ((othersIn e s).map Entry.size).sum = rest
        generalize hwant : (s.used + rest + e.size + need - cap + max b 1 - 1) / max b 1 = want
        generalize hblocks : (e.size + max b 1 - 1) / max b 1 = blocks
        generalize hk : min (max 1 (min want blocks)) (f + 1) = k
        have hk1 : 1 ≤ k := hk ▸ le_min (le_max_left _ _) (by omega)
        have hkf : k ≤ f + 1 := hk ▸ min_le_right _ _
        have hkm : k ≤ max 1 (min want blocks) := hk ▸ min_le_left _ _
        rw [ih (f + 1 - k) (by omega)]
        symm
        refine makeRoom_unroll b cap need he k (f + 1) hk1 hkf ?_ hs ?_
        · -- the entry is still there before the last block
          by_cases hk2 : 2 ≤ k
          · left
            have : k ≤ blocks := by
              have : max 1 (min want blocks) = min want blocks := by
                rcases le_total 1 (min want blocks) with h' | h'
                · exact max_eq_right h'
                · omega
              omega
            exact ceil_div_pred_lt hb (hblocks ▸ this) hk2
          · right; omega
        · intro i hi1 hik
          have hmin : i < min want blocks := by
            rcases le_total 1 (min want blocks) with h' | h'
            · rw [max_eq_right h'] at hkm; omega
            · rw [max_eq_left h'] at hkm; omega
          have hiw : i * max b 1 < s.used + rest + e.size + need - cap :=
            lt_ceil_div hb (hwant ▸ lt_of_lt_of_le hmin (min_le_left _ _))
          have hib : i * max b 1 < e.size :=
            lt_ceil_div hb (hblocks ▸ lt_of_lt_of_le hmin (min_le_right _ _))
          rw [not_or]
          refine ⟨?_, ?_⟩
          · rw [cachedTotal_evictK, hrest]
            have : min (i * max b 1) e.size = i * max b 1 := min_eq_left hib.le
            rw [this]; omega
          · have hne' : ¬ (e.size - min (i * max b 1) e.size = 0) := by
              rw [min_eq_left hib.le]; omega
            simp [evictK, hne']

/-- The compiler runs `makeRoomFast` wherever the definition says `makeRoom`. -/
@[csimp] theorem makeRoom_eq_fast : @makeRoom = @makeRoomFast := by
  funext b cap need f s
  exact (makeRoomFast_eq b cap need f s).symm

/-! ### Holds -/

/-- Units and admission units of the hold statement of session `i`. -/
def holdNeeds (m : Machine) (i : ℕ) (left : ℕ) : Prog → List (ℕ × ℕ × ℕ)
  | .hold ps _ _ _ _ => ps.map fun (p, u, f) =>
      let u' := evalE m i u left
      (p, u', max u' ((f.map (evalE m i · left)).getD 0))
  | _ => []

def fitsAll (m : Machine) (ns : List (ℕ × ℕ × ℕ)) : Bool :=
  ns.all fun (p, _, need) => decide ((pst m p).used + roundUp (pdef D p).block need ≤ (pdef D p).cap)

/-- Admit session `i` (its `prog` is a hold statement) with the budget
`left` visible to its unit expressions. -/
def admit (m : Machine) (i : ℕ) (left : ℕ) : Machine :=
  match (getS m i).prog with
  | stmt@(.hold _ reuse body cache k) =>
    let ns := holdNeeds m i left stmt
    let r? := reuse.map (evalE m i · left)
    let serial := (getS m i).serial
    let (m, held, consumed) := ns.foldl (fun (acc : Machine × List (ℕ × ℕ × ℕ) × ℕ) (p, u, _) =>
      let (m, held, cons) := acc
      let pd := pdef D p
      let s := pst m p
      -- a hold takes from the prefix cache only if it will give back to
      -- it: without a `cache` clause the entry stays where it is (IR 10)
      let own := if cache.isSome then ownEntry s serial else 0
      let s := if cache.isSome then removeEntry s serial else s
      let keepR := match r? with
        | some r => roundDown pd.block (min r own)
        | none => own
      let dead := own - keepR
      let lastOf := ((( (pst m p).entries.find? (·.owner = serial)).map fun e => (e.last, e.seq))).getD (m.now, 0)
      let (s, m) := if dead > 0 then
          ({ s with entries := ⟨1000000 + m.nextDead, dead, lastOf.1, lastOf.2⟩ :: s.entries },
           { m with nextDead := m.nextDead + 1 })
        else (s, m)
      let alloc := roundUp pd.block u
      let s := makeRoom pd.block pd.cap alloc (pd.cap + 1) s
      let s := { s with used := s.used + alloc, holders := s.holders ++ [i] }
      (setPool m p s, held ++ [(p, alloc, keepR)], max cons keepR)) (m, [], 0)
    let h : HoldRec := ⟨held, false, cache, stmt⟩
    let s := getS m i
    let st' : List Frame := Frame.hold h k :: s.stack
    let m := setS m i { s with cached := consumed, prog := body, stack := st', status := .ready, admSeq := m.nextAdm }
    { m with nextAdm := m.nextAdm + 1, ready := m.ready ++ [i] }
  | _ => m

/-- Release a hold of session `i`: free the units, cache what was computed. -/
def release (m : Machine) (i : ℕ) (h : HoldRec) : Machine :=
  h.pools.foldl (fun m (p, alloc, pos) =>
    let pd := pdef D p
    let serial := (getS m i).serial
    let s := pst m p
    let s := { s with used := s.used - alloc, holders := s.holders.filter (· ≠ i) }
    match h.cache with
    | none => setPool m p s
    | some c =>
      let keep := roundDown pd.block (min (evalE m i c) (if h.grown then pos else alloc))
      if keep = 0 then setPool m p s
      else
        let s := removeEntry s serial
        let s := { s with entries := ⟨serial, keep, m.now, m.nextRel⟩ :: s.entries }
        { setPool m p s with nextRel := m.nextRel + 1 }) m

/-- Queue session `i` at the first pool of its hold (`front`: after a
preemption, at the head). -/
def enqueue (m : Machine) (i : ℕ) (front : Bool) : Machine :=
  match (getS m i).prog with
  | .hold ((p, _, _) :: _) _ _ _ _ =>
    let s := pst m p
    let m := setPool m p { s with queue := if front then i :: s.queue else s.queue ++ [i] }
    setS m i { getS m i with status := .queued }
  | _ => m

/-! ### Running a session's commands (zero time) -/

def endSession (m : Machine) (i : ℕ) : Machine :=
  let s := getS m i
  let m := s.stack.foldl (fun m f => match f with
    | .hold h _ => release D m i h
    | _ => m) m
  setS m i { getS m i with status := .ended, stack := [] }

/-- Execute commands of session `i` until it blocks (at most `f` of them). -/
def exec : ℕ → Machine → ℕ → Machine
  | 0, m, _ => m
  | f + 1, m, i =>
    let s := getS m i
    if s.status ≠ .ready then m else
    match s.prog with
    | .done =>
      match s.stack with
      | [] => setS m i { s with status := .ended }
      | .seq k :: st => exec f (setS m i { s with prog := k, stack := st }) i
      | .hold h k :: st =>
        let m := release D (setS m i { s with stack := st }) i h
        exec f (setS m i { getS m i with prog := k }) i
      | .loop body :: st => exec f (setS m i { s with prog := body, stack := List.cons (Frame.loop body) st }) i
    | .stop => endSession D m i
    | .turn k =>
      let a := match m.wl.turnSlot with
        | some t => s.attr.upd t (s.attr.get t + 1)
        | none => s.attr
      let ts := m.wl.turns.getD s.serial []
      if ts = [] then exec f (setS m i { s with attr := a, prog := k }) i
      else match ts[s.turnIx]? with
        | some asg =>
          let a := asg.foldl (fun a p => a.upd p.1 p.2) a
          let a := a.upd m.wl.moreSlot (if s.turnIx + 1 < ts.length then 1 else 0)
          exec f (setS m i { s with attr := a, prog := k, turnIx := s.turnIx + 1 }) i
        | none =>
          exec f (setS m i { s with attr := a.upd m.wl.moreSlot 0, prog := k }) i
    | .set slot e k =>
      let v := evalE m i e
      exec f (setS m i { s with attr := s.attr.upd slot v, prog := k }) i
    | .observe n e k =>
      let v := evalE m i e
      exec f { setS m i { s with prog := k } with obs := (n, s.serial, m.now, v) :: m.obs } i
    | .branch p a b k =>
      let c := evalE m i p
      exec f (setS m i { s with prog := if c ≠ 0 then a else b, stack := List.cons (Frame.seq k) s.stack }) i
    | .loop body => exec f (setS m i { s with prog := body, stack := List.cons (Frame.loop body) s.stack }) i
    | .run st mode w g k =>
      let work := evalE m i w
      if work = 0 then exec f (setS m i { s with prog := k }) i
      else if st ≠ 0 then
        { setS m i { s with prog := k, status := .delay (m.now + work) m.nextDelay } with
          nextDelay := m.nextDelay + 1
          delays := insertDelay (m.now + work, m.nextDelay, i) m.delays }
      else
        -- residents in order of their sessions' hold admissions
        let (a, b) := m.jobs.span fun j => (getS m j.owner).admSeq ≤ s.admSeq
        { setS m i { s with prog := k, status := .engine } with jobs := a ++ ⟨i, mode, work, g⟩ :: b }
    | .hold _ _ _ _ _ => enqueue m i false

/-- Run every ready session. -/
def drain : ℕ → Machine → Machine
  | 0, m => m
  | f + 1, m => match m.ready with
    | [] => m
    | i :: rest => drain f (exec D 10000 { m with ready := rest } i)

/-- Admit from the queues not served by the engine, head first. -/
def admitFree : ℕ → Machine → Machine
  | 0, m => m
  | f + 1, m =>
    let pick := (List.range D.pools.length).find? fun p =>
      !(pdef D p).viaEngine && match (pst m p).queue with
        | i :: _ => fitsAll D m (holdNeeds m i 0 (getS m i).prog)
        | [] => false
    match pick with
    | none => m
    | some p => match (pst m p).queue with
      | i :: q =>
        let m := setPool m p { pst m p with queue := q }
        admitFree f (drain D 10000 (admit D m i 0))
      | [] => m

def settle (m : Machine) : Machine := admitFree D 1000 (drain D 10000 m)

/-! ### The engine -/

/-- Allocation and position of session `i`'s innermost hold on pool `p`. -/
def holdOn (s : Sess) (p : ℕ) : Option (ℕ × ℕ) :=
  (s.stack.findSome? fun f => match f with
    | .hold h _ => (h.pools.find? (·.1 = p)).map fun (_, a, q) => (a, q)
    | _ => none)

def mapHold (s : Sess) (p : ℕ) (g : ℕ × ℕ → ℕ × ℕ) : Sess :=
  let rec go : List Frame → Bool → List Frame
    | [], _ => []
    | .hold h k :: fs, false =>
      if h.pools.any (·.1 = p) then
        .hold { h with grown := true, pools := h.pools.map fun (q, a, x) =>
          if q = p then (q, (g (a, x)).1, (g (a, x)).2) else (q, a, x) } k :: go fs true
      else .hold h k :: go fs false
    | f :: fs, done => f :: go fs done
  { s with stack := go s.stack false }

/-- Preempt the most recently admitted holder of pool `p`. -/
def preemptLast (m : Machine) (p : ℕ) : Machine × ℕ :=
  match (pst m p).holders.getLast? with
  | none => (m, 0)
  | some v =>
    let s := getS m v
    let jobs := m.jobs.filter (·.owner ≠ v)
    let iter := m.iter.filter (·.1 ≠ v)
    -- unwind to the hold on `p`, release it (its computed prefix cached)
    let rec unwind (m : Machine) : List Frame → Machine
      | [] => m
      | .hold h _ :: fs =>
        let m := release D m v h
        if h.pools.any (·.1 = p) then
          let m := setS m v { getS m v with prog := h.stmt, stack := fs, status := .ready }
          enqueue m v true
        else unwind m fs
      | _ :: fs => unwind m fs
    let m := unwind { m with jobs := jobs, iter := iter } s.stack
    ({ m with preempts := m.preempts + 1 }, v)

/-- Grow session `i`'s hold on `p` to cover `d` more units; preempt on
failure. Returns whether `i` still holds (and grew). -/
def grow : ℕ → Machine → ℕ → ℕ → ℕ → Machine × Bool
  | 0, m, _, _, _ => (m, false)
  | f + 1, m, i, p, d =>
    match holdOn (getS m i) p with
    | none => (m, false)
    | some (a, _) =>
      let pd := pdef D p
      let need := roundUp pd.block (a + d) - a
      let s := pst m p
      if s.used + need ≤ pd.cap then
        let s := makeRoom pd.block pd.cap need (pd.cap + 1) s
        let m := setPool m p { s with used := s.used + need }
        (setS m i (mapHold (getS m i) p fun (a, x) => (a + need, x)), true)
      else
        let (m, v) := preemptLast D m p
        if v = i then (m, false) else grow f m i p d

/-- Admit from a queue the engine serves, with `left` tokens left. -/
def admitVia (m : Machine) (left : ℕ) : Machine × Bool :=
  match (List.range D.pools.length).find? fun p => (pdef D p).viaEngine && !(pst m p).queue.isEmpty with
  | none => (m, false)
  | some p => match (pst m p).queue with
    | i :: q =>
      if fitsAll D m (holdNeeds m i left (getS m i).prog) then
        let m := setPool m p { pst m p with queue := q }
        (drain D 10000 (admit D m i left), true)
      else (m, false)
    | [] => (m, false)

/-- Build the iteration starting now. -/
def assign : ℕ → Machine → ℕ → ℕ → ℕ → Machine
  | 0, m, _, _, _ => m
  | f + 1, m, idx, left, pre0 =>
    match m.jobs[idx]? with
    | none =>
      if left > 0 ∧ m.preempts = pre0 then
        match admitVia D m left with
        | (m, true) => assign f m idx left pre0
        | (m, false) => m
      else m
    | some j =>
      let want := match j.mode with
        | .decode => min 1 j.left
        | _ => if D.chunk > 0 then min j.left D.chunk else j.left
      let t := min want left
      if t = 0 then assign f m (idx + 1) left pre0 else
      match j.growing with
      | none =>
        let m := { m with iter := m.iter ++ [(j.owner, t)] }
        if left - t = 0 then m else assign f m (idx + 1) (left - t) pre0
      | some p =>
        match holdOn (getS m j.owner) p with
        | none => assign f m (idx + 1) left pre0
        | some (a, x) =>
          let (m, ok) := if x + t > a then grow D (D.pools.length * 100000) m j.owner p (x + t - a)
            else (m, true)
          if ok then
            let m := setS m j.owner (mapHold (getS m j.owner) p fun (a, x) => (a, x + t))
            let m := { m with iter := m.iter ++ [(j.owner, t)] }
            if left - t = 0 then m else assign f m (idx + 1) (left - t) pre0
          else assign f m idx left pre0

/-- What the cost of the iteration `m.iter` reads, after its growths (as
the interpreter reads it). A growing prefill chunk of `t` tokens has moved its
hold's position to `x`, so it started at `x - t`. -/
def iterStats (m : Machine) : IterStats :=
  let jobOf (o : ℕ) := m.jobs.find? (·.owner = o)
  let pre := m.iter.filter fun (o, _) => (jobOf o).map (·.mode) = some .prefill
  let dec := m.iter.filter fun (o, _) => (jobOf o).map (·.mode) = some .decode
  let chunk2 (o t : ℕ) : ℕ :=
    match (jobOf o).bind (·.growing) with
    | some p => match holdOn (getS m o) p with
      | some (_, x) => t * (2 * x - t)
      | none => t * t
    | none => t * t
  let held (o : ℕ) : ℕ := match D.memory with
    | some p => ((holdOn (getS m o) p).map (·.1)).getD 0
    | none => 0
  { tokens := (m.iter.map (·.2)).sum
    prefilled := (pre.map (·.2)).sum
    decoders := dec.length
    kvDecode := (dec.map fun (o, _) => held o).sum
    attention2 := (pre.map fun (o, t) => chunk2 o t).sum }

/-- Start an iteration on the idle engine. It lasts `cost` (at least one
clock unit) if it serves a token or preempted; otherwise the engine stays
idle until the next event. -/
def startIteration (m : Machine) : Machine :=
  let busy := !m.jobs.isEmpty ||
    (List.range D.pools.length).any fun p => (pdef D p).viaEngine && !(pst m p).queue.isEmpty
  if busy then
    let m' := assign D 100000 { m with iter := [] } 0 D.budget m.preempts
    if !m'.iter.isEmpty || m'.preempts ≠ m.preempts then
      { m' with iterEnd := some (m'.now + max 1 (D.cost (iterStats D m')), m'.nextDelay)
                nextDelay := m'.nextDelay + 1 }
    else { m' with iterEnd := none }
  else { m with iter := [], iterEnd := none }

/-- End the current iteration: apply the tokens, finish jobs. -/
def endIteration (m : Machine) : Machine :=
  let jobs := m.jobs.map fun j => { j with left := j.left - ((m.iter.filter (·.1 = j.owner)).map (·.2)).sum }
  let done := jobs.filter (·.left = 0) |>.map (·.owner)
  let m := { m with jobs := jobs.filter (·.left ≠ 0), iter := [] }
  done.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) m

def insertBy (a : ℕ × ℕ) : List (ℕ × ℕ) → List (ℕ × ℕ)
  | [] => [a]
  | b :: bs => if a.1 ≤ b.1 then a :: b :: bs else b :: insertBy a bs

/-- The next event, as (time, event number): the end of the running
iteration or of the first delay, whichever comes first in that order. -/
def nextEvent (m : Machine) : Option (ℕ × ℕ) :=
  match m.iterEnd, m.delays.head? with
  | some (a, qa), some (t, q, _) => some (if t < a ∨ (t = a ∧ q < qa) then (t, q) else (a, qa))
  | some a, none => some a
  | none, some (t, q, _) => some (t, q)
  | none, none => none

/-- Whether an event is pending at time `t` or before. -/
def pendingBy (m : Machine) (t : ℕ) : Bool := (nextEvent m).any (·.1 ≤ t)

/-- After an event: run what it enabled, then, if the engine is idle and no
other event is pending at this time, start an iteration (a scheduler step
sees every event of its instant, as in the interpreter's `settle`). -/
def afterEvent (m : Machine) : Machine :=
  let m := settle D m
  if m.iterEnd.isNone && !pendingBy m m.now then startIteration D m else m

/-- One event, in (time, event number) order: the clock moves to it, and it
is handled: a delay that ends wakes its session (unless the session was
preempted meanwhile and its status names another state), an iteration that
ends applies its tokens; then `afterEvent`. -/
def step (m : Machine) : Machine :=
  match nextEvent m with
  | none => m
  | some (t, q) =>
    let m : Machine := { m with now := t }
    let m : Machine :=
      if m.iterEnd = some (t, q) then endIteration { m with iterEnd := none }
      else match m.delays with
        | (u, q', i) :: rest =>
          let m : Machine := { m with delays := rest }
          if (getS m i).status = .delay u q' then
            { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }
          else m
        | [] => m
    afterEvent D m

/-- Run the events up to time `horizon` (at most `f` of them). -/
def runUntil (horizon : ℕ) : ℕ → Machine → Machine
  | 0, m => m
  | f + 1, m => match nextEvent m with
    | some (t, _) => if t ≤ horizon then runUntil horizon f (step D m) else m
    | none => m

/-- `n` sessions with attributes `init i`, all running `prog`, from time 0. -/
def start (n : ℕ) (init : ℕ → ℕ → ℕ) (prog : Prog) (wl : Workload := ⟨[], [], none, 0⟩) : Machine :=
  let m : Machine := {
    wl := wl
    now := 0
    sess := ((List.range n).map fun i => ⟨i, ⟨init i, []⟩, 0, prog, [], .ready, 0, 0⟩).toArray
    pools := D.pools.map fun _ => ⟨0, [], [], []⟩
    jobs := [], iter := [], obs := [], preempts := 0
    nextAdm := 0, nextRel := 0, nextDead := 0, nextDelay := 0
    ready := List.range n }
  afterEvent D m

/-- Enough events for time `horizon`: at each instant every session ends at
most one delay and the engine at most one iteration, and every event a step
creates is later than it (a delay of positive work, an iteration of at least
one unit). -/
def eventBound (horizon n : ℕ) : ℕ := (horizon + 1) * (n + 1)

/-- Run `n` sessions up to time `horizon`. -/
def run (horizon n : ℕ) (init : ℕ → ℕ → ℕ) (prog : Prog) : Machine :=
  runUntil D horizon (eventBound horizon n) (start D n init prog)

/-- A session's preset value of `slot`. -/
def Workload.attr (w : Workload) (i slot : ℕ) : ℕ :=
  (((w.init.getD i []).find? (·.1 = slot)).map (·.2)).getD 0

/-- Run a workload instance: one session per `init` entry. -/
def runW (horizon : ℕ) (w : Workload) (prog : Prog) : Machine :=
  runUntil D horizon (eventBound horizon w.init.length) (start D w.init.length w.attr prog w)

/-- The values of observation `name`, as (serial, value), in serial order. -/
def observed (m : Machine) (name : ℕ) : List (ℕ × ℕ) :=
  ((m.obs.reverse.filter (·.1 = name)).map fun (_, s, _, v) => (s, v)).foldr insertBy []

end Exec
end SerqLang
