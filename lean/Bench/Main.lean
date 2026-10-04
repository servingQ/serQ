/-
Run the executable semantics (`Serq.Exec`) of the vLLM replay program
(`Serq.Oracle.vllmTurn`) on a workload read from a JSON file, and print the
observations as CSV. A performance probe for "Lean core, Rust shell": the
program is compiled in; the deployment and the sessions come from the file

  {"pools": [[cap, block, viaEngine], ...], "budget": B, "chunk": c,
   "memory": p, "cost": [c0, c_tok, c_pre, c_dec, c_kv, c_att2],
   "horizon": T, "turnSlot": t, "moreSlot": m, "computedSlot": c,
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
    pure ⟨← natOf a[0], ← natOf a[1], (← natOf a[2]) ≠ 0, none⟩
  else throw "pool"

/-- Run every event up to `horizon`. -/
partial def loop (D : Deployment) (horizon : ℕ) (m : Machine) : Machine :=
  match nextEvent m with
  | some (t, _) => if t ≤ horizon then loop D horizon (step D m) else m
  | none => m

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
  -- the engine's memory pool, and the iteration cost
  -- `c0 + c_tok tokens + c_pre prefilled + c_dec decoders + c_kv kv_decode + c_att2 (2 attention)`
  let memory : Option ℕ := match j.getObjVal? "memory" with
    | .ok v => (natOf v).toOption
    | .error _ => none
  let costArr ← IO.ofExcept (match j.getObjVal? "cost" with
    | .ok v => v.getArr?
    | .error _ => pure #[(1 : Json), 0, 0, 0, 0, 0])
  let cs ← IO.ofExcept (costArr.toList.mapM natOf)
  let c := fun k => cs.getD k 0
  let D : Deployment := ⟨pools, budget, chunk, memory,
    fun st => c 0 + c 1 * st.tokens + c 2 * st.prefilled + c 3 * st.decoders
      + c 4 * st.kvDecode + c 5 * st.attention2, none⟩
  let computedSlot ← IO.ofExcept (natOf (j.getObjValD "computedSlot"))
  let w : Workload := ⟨init, sessions, some turnSlot, moreSlot, some computedSlot⟩
  let t0 ← IO.monoMsNow
  let m := loop D horizon (start D w.init.length w.attr Oracle.vllmTurn w)
  let n := m.obs.length
  let t1 ← IO.monoMsNow
  IO.eprintln s!"last event {m.now}  sessions {m.sess.size}  ended {(m.sess.filter (·.status = .ended)).size}  observations {n}  ms {t1 - t0}"
  IO.println "name,serial,time,value"
  for (nm, s, t, v) in m.obs.reverse do
    IO.println s!"{nm},{s},{t},{v}"
  pure 0
