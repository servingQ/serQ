/-
# The machine chain projects onto RAD's chain on job lists

At a slot boundary the machine of `bari_rad.sq` determines its next slot's
job list from its own job list alone: `σ (slot rs m) = absSlot rs (σ m)` for
every machine the chain reaches. So the machine chain is lumpable onto
`BariChain`'s chain, and what that chain does (`BariRecurrent`) the program
does.
-/
import papers.BariChain

namespace SerqLang
namespace Papers
namespace BariSim

open Exec BariChain

/-- A machine's job list: each job's mode, left work and its session's
output length (slot 11), in admission order. -/
def σ (m : Machine) : List AJob := m.jobs.map fun j => (j.mode, j.left, (getS m j.owner).attr.get 11)

theorem σ_empty : σ BariStable.empty = [] := rfl

/-! ### The job list, exactly

`Serq/Slot.lean` follows the list through a slot (`slot_jobs`); what is
RAD's own is its batch, read here on the chain's `shares`. A job's output is
its request's `L i`, slot 11 of its session. -/

open Slot

local notation "Db" => BariStable.D
local notation "Mb" => BariStable.M

/-- A job list, each job with its request's output. -/
def σL (L : ℕ → ℕ × ℕ) (js : List Job) : List AJob := js.map fun j => (j.mode, j.left, (L j.owner).2)

theorem σ_eq {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI Mb L g m) : σ m = σL L m.jobs :=
  List.map_congr_left fun j hj =>
    congrArg (fun x => (j.mode, j.left, x)) (congrArg Prod.snd (hI.sess _ (hI.jobs j hj).1).2.2.1)

/-- Each job's share of the greedy fill of the jobs `q` serves is its share
in `shares`, when `want` is what `q` lets each job take. -/
theorem fill_sub (q : Job → Bool) (dm : Bool) (out : Job → ℕ) : ∀ (js : List Job),
    (js.map (·.owner)).Nodup →
    (∀ j ∈ js, (if q j then wantOf Db j else 0) = want dm (j.mode, j.left, out j)) → ∀ B,
    js.map (fun j => (j.mode, j.left - shareOf (fillIter Db (js.filter q) B) j.owner, out j)) =
      List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) (js.map fun j => (j.mode, j.left, out j))
        (shares dm (js.map fun j => (j.mode, j.left, out j)) B) :=
  shareOf_fillIter_split Db q (fun j => (j.mode, j.left, out j)) (want dm) (shares dm) (fun _ _ _ => rfl)
    (fun j s => (j.1, j.2.1 - s, j.2.2))

instance (m : Machine) : Decidable (BariRad.decodeMode m) := by
  unfold BariRad.decodeMode; infer_instance

/-- RAD's `serve only` lets each job take what `want` says in its mode. -/
theorem want_rad (m : Machine) (j : Job) (hj : j.mode ≠ .plain) (o : ℕ) :
    (if serves Db m j then wantOf Db j else 0) = want (decide (BariRad.decodeMode m)) (j.mode, j.left, o) := by
  have hs := BariRad.serves_rad m j
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
      -- a prefill takes at most the chunk the program's deployment sets
      rw [this]; simp [want, hd, wantOf, BariStable.D, Claims.BariRad.deployment]
  | decode =>
    by_cases hd : BariRad.decodeMode m
    · have : serves Db m ⟨ow, .decode, l, gr⟩ = true := hs.mpr ⟨fun _ => rfl, fun h => absurd hd h⟩
      rw [this]; simp [want, hd, wantOf]
    · have : serves Db m ⟨ow, .decode, l, gr⟩ = false := by
        cases h : serves Db m ⟨ow, .decode, l, gr⟩
        · rfl
        · exact absurd rfl ((hs.mp h).2 hd)
      rw [this]; simp [want, hd]

theorem decodeMode_eq (L : ℕ → ℕ × ℕ) (m : Machine) :
    decodeMode (σL L m.jobs) = decide (BariRad.decodeMode m) := by
  simp [decodeMode, BariRad.decodeMode, σL, List.filter_map, Function.comp_def]

/-! ### A slot -/

/-- What the chain carries from slot to slot. -/
def Inv (m : Machine) : Prop := ∃ L g, BariStable.SB L g m ∧ A0 m

theorem empty_inv : Inv BariStable.empty :=
  ⟨_, _, BariStable.empty_sb, fun j => by simp [getS, BariStable.empty, Slot.empty, Exec.initial]⟩

theorem slot_inv {m : Machine} (h : Inv m) (rs : List (ℕ × ℕ))
    (hf : ∀ r ∈ rs, BariStable.Fits r) :
    Inv (BariStable.slot rs m) ∧ σ (BariStable.slot rs m) = absSlot rs (σ m) := by
  obtain ⟨L, g, hB, hA⟩ := h
  obtain ⟨L', g', hB', hA', hLo, hLn, hj⟩ :=
    slot_jobs BariStable.ci_start hB hA (rs.map BariStable.attrs) (BariStable.fits_attrs hf)
  change BariStable.SB L' g' (BariStable.slot rs m) at hB'
  change A0 (BariStable.slot rs m) at hA'
  change (BariStable.slot rs m).jobs = _ at hj
  simp only [List.length_map] at hLn hj
  refine ⟨⟨L', g', hB', hA'⟩, ?_⟩
  -- the new requests' lengths
  have hLn' : ∀ t < rs.length, L' (m.sess.size + t) = rs.getD t (0, 0) := fun t ht => by
    have ha : (rs.map BariStable.attrs).getD t (fun _ => 0) = BariStable.attrs (rs.getD t (0, 0)) := by
      simp [List.getD_eq_getElem?_getD, ht]
    rw [hLn t ht, ha]
    exact BariStable.len_attrs _
  have hσnew : σL L' ((List.range' m.sess.size rs.length).map fun i => (⟨i, .prefill, (L' i).1, none⟩ : Job)) =
      rs.map fun r => ((.prefill, r.1, r.2) : AJob) := by
    rw [σL, List.map_map]
    apply range_map (0, 0)
    intro t ht
    simp [hLn' t ht]
  -- the old jobs, read with the new lengths
  have hσm : σ m = σL L' m.jobs := by
    rw [σ_eq hB.toCI]
    exact List.map_congr_left fun j hj => by rw [hLo _ (hB.jobs j hj).1]
  rw [σ_eq hB'.toCI, hj, hσm]
  unfold absSlot
  split_ifs with h0
  · -- an idle engine: the arrivals alone
    have hjobs0 : m.jobs = [] := by simpa [σL] using h0
    simp only [hjobs0, after, List.map_nil, List.filter_nil, List.nil_append, List.append_nil]
    exact hσnew
  · -- a busy engine: its batch is RAD's greedy fill, `shares` on the list
    have hjne : m.jobs ≠ [] := by simpa [σL] using h0
    have hbusy : m.iterEnd.isSome = true := by
      cases hie : m.iterEnd with
      | none => exact absurd (hB.idle hie) hjne
      | some _ => rfl
    obtain ⟨-, -, hit⟩ := hB.busy hbusy
    have hafter : List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) (σL L' m.jobs)
        (shares (decodeMode (σL L' m.jobs)) (σL L' m.jobs) BariStable.M.B) = σL L' (after m.iter m.jobs) := by
      have := fill_sub (serves Db m) (decide (BariRad.decodeMode m)) (fun j => (L' j.owner).2) m.jobs
        hB.jobsNodup (fun j hj => want_rad m j (by
          rcases (hB.jobs j hj).2.1 with ⟨-, hm⟩ | ⟨-, hm⟩ <;> rw [hm] <;> decide) _) BariStable.M.B
      rw [← hit] at this
      rw [decodeMode_eq, show σL L' m.jobs = m.jobs.map fun j => (j.mode, j.left, (L' j.owner).2) from rfl,
        ← this]
      simp [σL, after]
    simp only []
    rw [hafter, ← hσnew]
    simp only [σL, List.map_append, List.map_map, List.filter_map, Function.comp_def]

/-- Every state of the chain carries the invariant. -/
theorem reach_inv {m : Machine} (h : BariStable.Reach m) : Inv m := by
  induction h with
  | empty => exact empty_inv
  | slot rs hf _ ih => exact (slot_inv ih rs hf).1

/-- The projection commutes with a slot. -/
theorem simulation (x : BariStable.State) (rs : List (ℕ × ℕ))
    (hf : ∀ r ∈ rs, BariStable.Fits r) :
    σ (BariStable.slot rs x.1) = absSlot rs (σ x.1) :=
  (slot_inv (reach_inv x.2) rs hf).2

end BariSim
end Papers
end SerqLang
