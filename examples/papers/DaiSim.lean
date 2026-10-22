/-
# The machine chain projects onto Sarathi's chain on job lists

At a slot boundary the machine of `dai_sarathi.sq` determines its next
slot's job list from its own job list alone: `σ (slot k m) = absSlot k (σ m)`
for every machine the chain reaches. So the machine chain is lumpable onto
`DaiChain`'s chain, and what that chain does (`DaiRecurrent`) the program
does.
-/
import papers.DaiChain

namespace SerqLang
namespace Papers
namespace DaiSim

open Exec DaiChain

/-- A machine's job list: each job's mode and left work, in admission order. -/
def σ (m : Machine) : List AJob := m.jobs.map fun j => (j.mode, j.left)

/-! ### The job list, exactly

`Serq/Slot.lean` follows the list through a slot (`slot_jobs`); what is
Sarathi's own is its batch, the greedy fill, read here on the chain's
`shares`. -/

open Slot

local notation "Dd" => DaiStable.D

/-- A machine's job list, as a function of the jobs. -/
def σL (js : List Job) : List AJob := js.map fun j => (j.mode, j.left)

theorem want_eq (j : Job) : wantOf Dd j = want (j.mode, j.left) := by
  unfold wantOf
  rcases j with ⟨o, md, l, gr⟩
  cases md <;> simp [want, Claims.DaiSarathi.deployment]

/-- Each job's share of the greedy fill is its share in `shares`. -/
theorem fill_sub (js : List Job) (hn : (js.map (·.owner)).Nodup) (B : ℕ) :
    js.map (fun j => (j.mode, j.left - shareOf (fillIter Dd js B) j.owner)) =
      List.zipWith (fun j s => (j.1, j.2 - s)) (σL js) (shares (σL js) B) := by
  simpa [σL] using shareOf_fillIter_split Dd (fun _ => true) (fun j => (j.mode, j.left)) want shares
    (fun _ _ _ => rfl) (fun j s => (j.1, j.2 - s)) js hn (fun j _ => want_eq j) B

/-! ### A slot -/

/-- What the chain carries from slot to slot. -/
def Inv (m : Machine) : Prop := ∃ L g, DaiStable.SB L g m ∧ A0 m

theorem empty_inv : Inv DaiStable.empty :=
  ⟨_, _, DaiStable.empty_sb, fun j => by simp [getS, DaiStable.empty, Slot.empty, Exec.initial]⟩

theorem slot_inv {m : Machine} (h : Inv m) (k : ℕ) :
    Inv (DaiStable.slot k m) ∧ σ (DaiStable.slot k m) = absSlot k (σ m) := by
  obtain ⟨L, g, hB, hA⟩ := h
  obtain ⟨L', g', hB', hA', hLo, hLn, hj⟩ :=
    slot_jobs DaiStable.ci_start hB hA (List.replicate k fun _ => 0) fun _ _ => rfl
  rw [← DaiStable.slot_eq] at hB' hA' hj
  simp only [List.length_replicate] at hLn hj
  refine ⟨⟨L', g', hB', hA'⟩, ?_⟩
  -- the arrivals' prefills, each of Sarathi's prompt
  have hnew : ((List.range' m.sess.size k).map fun i => (⟨i, .prefill, (L' i).1, none⟩ : Job)).map
      (fun j => (j.mode, j.left)) = List.replicate k (.prefill, (DaiStable.M.len fun _ => 0).1) := by
    rw [List.map_map, List.eq_replicate_iff]
    refine ⟨by simp, fun b hb => ?_⟩
    obtain ⟨i, hi, rfl⟩ := List.mem_map.mp hb
    obtain ⟨t, ht, rfl⟩ : ∃ t < k, i = m.sess.size + t := by
      rw [List.mem_range'_1] at hi; exact ⟨i - m.sess.size, by omega, by omega⟩
    simp [hLn t ht]
  show (DaiStable.slot k m).jobs.map _ = _
  rw [hj]
  unfold absSlot
  split_ifs with h0
  · -- an idle engine: the arrivals alone
    have hjobs0 : m.jobs = [] := by simpa [σ] using h0
    simp only [hjobs0, after, List.map_nil, List.filter_nil, List.nil_append, List.append_nil]
    exact hnew
  · -- a busy engine: its batch is the greedy fill, `shares` on the list
    have hjne : m.jobs ≠ [] := by simpa [σ] using h0
    have hbusy : m.iterEnd.isSome = true := by
      cases hie : m.iterEnd with
      | none => exact absurd (hB.idle hie) hjne
      | some _ => rfl
    obtain ⟨-, -, hit⟩ := hB.busy hbusy
    have hafter : List.zipWith (fun j s => (j.1, j.2 - s)) (σ m) (shares (σ m) DaiStable.M.B) =
        (after m.iter m.jobs).map fun j => (j.mode, j.left) := by
      rw [show σ m = σL m.jobs from rfl, ← fill_sub m.jobs hB.jobsNodup DaiStable.M.B, ← hit]
      simp [after, Function.comp_def]
    simp only []
    rw [hafter]
    simp only [List.map_append, List.map_map, List.filter_map, Function.comp_def, hnew]
    congr 1
    refine List.map_congr_left fun x hx => ?_
    obtain ⟨j, hj, rfl⟩ := List.mem_map.mp (List.mem_filter.mp hx).1
    have hjs := (hB.jobs j hj).1
    simp [hLo _ hjs, DaiStable.len_of hB.toCI hjs]

/-- Every state of the chain carries the invariant. -/
theorem reach_inv {K : ℕ} {m : Machine} (h : DaiStable.Reach K m) : Inv m := by
  induction h with
  | empty => exact empty_inv
  | slot k hk _ ih => exact (slot_inv ih k).1

/-- The projection commutes with a slot. -/
theorem simulation {K : ℕ} (x : DaiStable.State K) (k : ℕ) (hk : k ≤ K) :
    σ (DaiStable.slot k x.1) = absSlot k (σ x.1) :=
  (slot_inv (reach_inv x.2) k).2

end DaiSim
end Papers
end SerqLang
