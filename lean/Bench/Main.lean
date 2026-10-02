/-
Run the executable semantics (`Serq.Exec`) of the vLLM replay program
(`Serq.Oracle.vllmTurn`) on a workload read from a JSON file, and print the
observations as CSV. A performance probe for "Lean core, Rust shell": the
program is compiled in; the deployment and the sessions come from the file

  {"pools": [[cap, block, viaEngine], ...], "budget": B, "chunk": c,
   "horizon": T, "turnSlot": t, "moreSlot": m,
   "init": [[[slot, value], ...], ...], "sessions": [[[[slot, value], ...], ...], ...]}

(`sessions[i]` is session `i`'s turns, each a list of attribute assignments),
which `scripts/lean_bench.py` writes from an IR file.
-/
import Serq

open Lean SerqLang Exec

def natOf (j : Json) : Except String ℕ := do
  let n ← j.getNat?
  pure n

def pairOf (j : Json) : Except String (ℕ × ℕ) := do
  let a ← j.getArr?
  if h : a.size = 2 then pure (← natOf a[0], ← natOf a[1]) else throw "pair"

def listOf {α} (f : Json → Except String α) (j : Json) : Except String (List α) := do
  let a ← j.getArr?
  (a.toList.mapM f)

def poolOf (j : Json) : Except String PoolDef := do
  let a ← j.getArr?
  if h : a.size = 3 then
    pure ⟨← natOf a[0], ← natOf a[1], (← natOf a[2]) ≠ 0⟩
  else throw "pool"

def allEnded (m : Machine) : Bool := m.sess.all (·.status = .ended)

partial def loop (D : Deployment) (horizon : ℕ) (m : Machine) : Machine :=
  if allEnded m || m.now ≥ horizon then m else loop D horizon (tick D m)

def main (args : List String) : IO UInt32 := do
  let path := args.headD "workload.json"
  let txt ← IO.FS.readFile path
  let j ← IO.ofExcept (Json.parse txt)
  let pools ← IO.ofExcept (j.getObjValD "pools" |> listOf poolOf)
  let budget ← IO.ofExcept (natOf (j.getObjValD "budget"))
  let chunk ← IO.ofExcept (natOf (j.getObjValD "chunk"))
  let horizon ← IO.ofExcept (natOf (j.getObjValD "horizon"))
  let turnSlot ← IO.ofExcept (natOf (j.getObjValD "turnSlot"))
  let moreSlot ← IO.ofExcept (natOf (j.getObjValD "moreSlot"))
  let init ← IO.ofExcept (j.getObjValD "init" |> listOf (listOf pairOf))
  let sessions ← IO.ofExcept (j.getObjValD "sessions" |> listOf (listOf (listOf pairOf)))
  let D : Deployment := ⟨pools, budget, chunk⟩
  let w : Workload := ⟨init, sessions, some turnSlot, moreSlot⟩
  let t0 ← IO.monoMsNow
  let m := loop D horizon (start D w.init.length w.attr Oracle.vllmTurn w)
  let n := m.obs.length
  let t1 ← IO.monoMsNow
  IO.eprintln s!"ticks {m.now}  sessions {m.sess.size}  ended {(m.sess.filter (·.status = .ended)).size}  observations {n}  ms {t1 - t0}"
  IO.println "name,serial,time,value"
  for (nm, s, t, v) in m.obs.reverse do
    IO.println s!"{nm},{s},{t},{v}"
  pure 0
