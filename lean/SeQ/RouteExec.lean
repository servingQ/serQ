/-
# ROUTE: an executable semantics

`Route.lean` gives the syntax of ROUTE (`Route Env V`) and the relational
semantics of one memory pool (`Step`, with the memory invariant). This
module gives an *executable* semantics of the fragment that production
schedulers are written in: values are natural numbers (tokens), time is
the clock of a `step` engine (one iteration per tick), and the deployment
is a list of pools plus the engine (stage 0) and a delay stage (stage 1).

* **Pools** allocate in blocks, keep released prefixes as cache entries
  (per session, tail blocks evicted first, least recently released entry
  first, ties by release order), admit the head of their queue when every
  pool of its hold has room for its `fits` units (allocated units never
  count the cache), consume at most `reuse` of the own prefix (the rest
  stays cached, dead, with its age), and on a failed growth preempt the
  most recently admitted holder, whose hold is released (its computed
  prefix cached) and re-queued at the head. A pool marked `viaEngine`
  (`admit via engine`) is admitted by the engine at the start of an
  iteration, with the budget its residents leave (`budgetLeft`), and not
  in an iteration that preempted.
* **The engine** hands its budget to its residents in admission order, one
  token to a decoding job and up to `chunk` to a prefilling one, grows the
  holds of `growing` jobs before the tokens are committed, then admits.

These are the rules of `src/sim.rs`, which reproduces the real vLLM
v1 scheduler step for step (`docs/language.md` §7). The vLLM
scheduler scenarios of `tools/oracle/` are then theorems about ROUTE
programs (`RouteOracle.lean`), checked by evaluation in the kernel.

Key definitions: `Exec.Deployment`, `Exec.Machine`, `Exec.tick`, `Exec.run`.
Key theorems: `Exec.makeRoom_used` (eviction never touches the allocation),
`Exec.makeRoom_room` (the eviction loop makes the room it is asked for, or
empties the cache).
-/
import SeQ.Route

namespace SeQ
namespace RouteLang
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

/-- A deployment: pools, the step engine (stage 0) and a delay stage (1). -/
structure Deployment where
  pools : List PoolDef
  budget : ℕ
  /-- per-request chunk cap (`long_prefill_token_threshold`), 0 = none -/
  chunk : ℕ

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
  /-- in a delay until `wake`; `seq` orders delays that end at the same tick
  by when they started (the event order of the Rust interpreter) -/
  | delay (wake seq : ℕ)
  | ended
  deriving DecidableEq

structure Sess where
  serial : ℕ
  attr : ℕ → ℕ
  cached : ℕ
  prog : Prog
  stack : List Frame
  status : Status
  admSeq : ℕ

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
  now : ℕ
  sess : List Sess
  pools : List PoolSt
  jobs : List Job
  iter : List (ℕ × ℕ)
  /-- (name, serial, time, value) -/
  obs : List (ℕ × ℕ × ℕ × ℕ)
  preempts : ℕ
  nextAdm : ℕ
  nextRel : ℕ
  nextDead : ℕ
  nextDelay : ℕ
  ready : List ℕ

variable (D : Deployment)

/-! ### Helpers -/

def roundUp (b u : ℕ) : ℕ := if b = 0 then u else (u + b - 1) / b * b
def roundDown (b u : ℕ) : ℕ := if b = 0 then u else u / b * b

def pdef (p : ℕ) : PoolDef := D.pools.getD p ⟨0, 1, false⟩
def pst (m : Machine) (p : ℕ) : PoolSt := m.pools.getD p ⟨0, [], [], []⟩
def setPool (m : Machine) (p : ℕ) (s : PoolSt) : Machine := { m with pools := m.pools.set p s }
def getS (m : Machine) (i : ℕ) : Sess := m.sess.getD i ⟨i, fun _ => 0, 0, .stop, [], .ended, 0⟩
def setS (m : Machine) (i : ℕ) (s : Sess) : Machine := { m with sess := m.sess.set i s }

def cachedTotal (s : PoolSt) : ℕ := (s.entries.map Entry.size).sum
def ownEntry (s : PoolSt) (r : ℕ) : ℕ := ((s.entries.find? (·.owner = r)).map Entry.size).getD 0
def removeEntry (s : PoolSt) (r : ℕ) : PoolSt := { s with entries := s.entries.filter (·.owner ≠ r) }

def env (m : Machine) (i : ℕ) (left : ℕ := 0) : Env :=
  let s := getS m i
  { attr := s.attr, serial := s.serial, now := m.now, cached := s.cached,
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
      let own := ownEntry s serial
      let s := removeEntry s serial
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
    | .turn k => exec f (setS m i { s with prog := k }) i
    | .set slot e k =>
      let v := evalE m i e
      exec f (setS m i { s with attr := Function.update s.attr slot v, prog := k }) i
    | .observe n e k =>
      let v := evalE m i e
      exec f { setS m i { s with prog := k } with obs := m.obs ++ [(n, s.serial, m.now, v)] } i
    | .branch p a b k =>
      let c := evalE m i p
      exec f (setS m i { s with prog := if c ≠ 0 then a else b, stack := List.cons (Frame.seq k) s.stack }) i
    | .loop body => exec f (setS m i { s with prog := body, stack := List.cons (Frame.loop body) s.stack }) i
    | .run st mode w g k =>
      let work := evalE m i w
      if work = 0 then exec f (setS m i { s with prog := k }) i
      else if st = 1 then
        { setS m i { s with prog := k, status := .delay (m.now + work) m.nextDelay } with
          nextDelay := m.nextDelay + 1 }
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

def startIteration (m : Machine) : Machine :=
  let busy := !m.jobs.isEmpty ||
    (List.range D.pools.length).any fun p => (pdef D p).viaEngine && !(pst m p).queue.isEmpty
  if busy then assign D 100000 { m with iter := [] } 0 D.budget m.preempts else { m with iter := [] }

/-- End the current iteration: apply the tokens, finish jobs. -/
def endIteration (m : Machine) : Machine :=
  let jobs := m.jobs.map fun j => { j with left := j.left - ((m.iter.filter (·.1 = j.owner)).map (·.2)).sum }
  let done := jobs.filter (·.left = 0) |>.map (·.owner)
  let m := { m with jobs := jobs.filter (·.left ≠ 0), iter := [] }
  done.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) m

def insertBy (a : ℕ × ℕ) : List (ℕ × ℕ) → List (ℕ × ℕ)
  | [] => [a]
  | b :: bs => if a.1 ≤ b.1 then a :: b :: bs else b :: insertBy a bs

/-- One tick of the clock: delays that end, the iteration that ends, the
commands they enable, and the next iteration. -/
def tick (m : Machine) : Machine :=
  let m := { m with now := m.now + 1 }
  let wake := ((List.range m.sess.length).filterMap fun i => match (getS m i).status with
    | .delay u q => if u ≤ m.now then some (q, i) else none
    | _ => none).foldr insertBy [] |>.map (·.2)
  let m := wake.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) m
  let m := endIteration m
  startIteration D (settle D m)

/-- `n` sessions with attributes `init i`, all running `prog`, from time 0. -/
def start (n : ℕ) (init : ℕ → ℕ → ℕ) (prog : Prog) : Machine :=
  let m : Machine := {
    now := 0
    sess := (List.range n).map fun i => ⟨i, init i, 0, prog, [], .ready, 0⟩
    pools := D.pools.map fun _ => ⟨0, [], [], []⟩
    jobs := [], iter := [], obs := [], preempts := 0
    nextAdm := 0, nextRel := 0, nextDead := 0, nextDelay := 0
    ready := List.range n }
  startIteration D (settle D m)

def run (ticks n : ℕ) (init : ℕ → ℕ → ℕ) (prog : Prog) : Machine :=
  (List.range ticks).foldl (fun m _ => tick D m) (start D n init prog)

/-- The values of observation `name`, as (serial, value), in serial order. -/
def observed (m : Machine) (name : ℕ) : List (ℕ × ℕ) :=
  ((m.obs.filter (·.1 = name)).map fun (_, s, _, v) => (s, v)).foldr insertBy []

end Exec
end RouteLang
end SeQ
