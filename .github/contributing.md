# Contributing examples and oracle scenarios

Follow [the documentation principles](documentation.md) for site changes.
Paths below are relative to the repository root.

## Add a program

Put it in the `examples/` directory of its workload (`single-turn/`, `multi-turn/`, `subagent/`, `pd-disaggregation/`, `replay/`); names are unique across them. `make check` then links it and draws it
in both formats. Add a row to `docs/language.md` (Programs) saying what it
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
   `block_size`, `num_blocks`, optionally `chunk`) and the `requests`
   (`prompt`, `out`, optionally `arrive`).
2. Run `VLLM_PLUGINS= python tools/vllm_oracle.py tools/oracle/<name>.json`
   and save its output as `tools/oracle/<name>.out.json`. This is the answer
   the theorem will state. The script drives the real scheduler: it needs a
   Python environment with vLLM installed and a full vLLM checkout at the
   pinned revision beside the serQ checkout (`../ref/vllm`, which it imports
   `tests.v1.core.utils` from). The sparse `ref/vllm` that
   `scripts/fetch_vllm_ref.sh --sparse` makes for the citation check is not
   enough.
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
