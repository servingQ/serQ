# Contributing examples and oracle scenarios

Follow [the documentation principles](documentation.md) for site changes.
Paths below are relative to the repository root.

## Add a program

A program is a deployment and a workload, two files
([paired files](../docs/design/paired-workload.md)):

1. The deployment (`fn main()`, its devices, engines, pools and `server`,
   no `workload`) goes in the `examples/` directory of its workload
   (`single-turn/`, `multi-turn/`, `subagent/`, `pd-disaggregation/`,
   `replay/`, …); names are unique across them.
2. The workload (no `fn main()`: its `use`s, `let`s, `def`s, its own stages
   and pools, and one `workload { … }`) goes in `examples/workloads/`,
   unless an existing one already sends what the server's signature reads:
   reuse it then.
3. Add the pair to `examples/pairs.json`, both paths relative to the
   repository's root: `"examples/<dir>/<name>.sq": "examples/workloads/<w>.sq"`.

`make check` then refuses a program with no line there (`scripts/pairs.py
--check`), links it with its workload and draws it in both formats; every
tool that runs an example (`scripts/unused.py`, the sweeps under `tools/`)
reads its workload from `examples/pairs.json`. Add a row to `docs/language.md` (Programs) saying what it
models and what it is checked against, and a test that checks that claim.
A program that cites vLLM cites it as `file.py:lines`, and
`scripts/check_citations.py` holds the citation to the pinned source:
`make check` resolves every cited range in the pinned `ref/vllm` and hashes
its text (`tools/citations.json`), so a range that moved, a file that is
gone or a citation nobody recorded fails the build. That check cannot see
upstream move; `.github/workflows/citation-drift.yml` asks that daily, by
looking for each range's pinned text in the same files at
`vllm-project/vllm@main` (`scripts/check_citations.py --tip`). It gates no
pull request: it says the cited code is gone, not that the scheduler now
computes something else.


## Add an oracle scenario

1. Write `tools/oracle/<name>.json`: the engine (`budget`, `max_seqs`,
   `block_size`, `num_blocks`, optionally `chunk` and `adaptive`) and the
   `requests` (`prompt`, `out`, optionally `arrive`).
2. Run `VLLM_TARGET_DEVICE=cpu VLLM_PLUGINS= python tools/vllm_oracle.py
   tools/oracle/<name>.json` and save its output as
   `tools/oracle/<name>.out.json`. This is the answer the theorem will
   state. The script drives the real scheduler, imported from the full
   vLLM checkout at the pinned revision in this repository's `ref/vllm`
   (`scripts/fetch_vllm_ref.sh`; the sparse checkout that `--sparse` makes
   for the citation check is not enough). vLLM need not be installed: the
   Python environment needs torch (the CPU build is enough) and
   `ref/vllm/requirements/common.txt`. `VLLM_TARGET_DEVICE=cpu` is for a
   machine without a GPU, and `VLLM_PLUGINS=` disables an installed
   platform plugin.
3. Add the name to the list in `tests/vllm_oracle.rs::scenarios`, run
   `make oracle-ir`, then `make check` and `make lean`. The interpreter and
   the Lean semantics now have to agree. The generator finds the scenarios
   by listing the directory, so it needs no list of its own.


## Language changes

Follow `AGENTS.md` and `docs/ir.md` for compatibility and generation rules.
`make metrics` updates `tools/metrics.json` when the language's surface
changes; `make check` rejects stale metrics. Regenerate affected oracle IR
and figures in the same change as their sources.

## Build and publish documentation

```bash
pip install -r docs/requirements.txt
mkdocs serve
mkdocs build --strict
```

The site is public. `.github/workflows/docs.yml` builds documentation pull
requests and uploads a `site` artifact for preview. On `main`, the workflow
deploys through `actions/deploy-pages`. Dependencies are pinned in
`docs/requirements.txt`.

## Design records

Keep proposals, rejected alternatives, implementation history and their
self-critiques in [docs/design/](../docs/design/index.md). These records and
[docs/review.md](../docs/review.md) remain in the repository but are excluded
from the documentation site's navigation, search and build. Current usage
belongs in the API reference or a use case; current semantics belong in
`docs/language.md` and `docs/ir.md`.
