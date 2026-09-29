# Publishing this site

This site is **public**, at <https://vrvrv.github.io/seQ/>, published from a
private repository by `.github/workflows/docs.yml`.

Anyone can read it, and search engines index it. That includes the pages that
quote unpublished work: [How seQ is checked](validation.md) and the [case
study](case-study-vllm.md) carry the A100 measurements, the pre-registered
predictions of `data/exp/seq/prereg/` and the paper's replica. Treat anything
you add here as published.

## How it works

`main` builds with `mkdocs build --strict` and deploys through
`actions/deploy-pages`, rather than `mkdocs gh-deploy`, so the site is always
rebuilt from the commit it describes. A pull request touching `docs/`,
`examples/` or `mkdocs.yml` builds and uploads the result as a `site`
artifact but does not deploy — download it from the run to review a change
before it lands.

`mkdocs-material` is pinned in `docs/requirements.txt` so that rebuilding an
old commit produces the page that commit described.

## Reading it locally

```bash
pip install -r docs/requirements.txt
mkdocs serve          # http://127.0.0.1:8000
```

## Making it private later

Not a plan upgrade. Access control for a Pages site is a **GitHub Enterprise
Cloud** feature, and only for project sites published from a repository owned
by an **organization**:

> To publish a GitHub Pages site privately, your organization must use GitHub
> Enterprise Cloud.

`vrvrv/seQ` is a personal repository, so the option does not apply to it at
any tier — the API answers `422 Current plan does not support private GitHub
Pages`. Making it private would mean transferring the repository to an
organization on Enterprise Cloud, which is a decision about where the project
lives rather than about documentation.

Note the two capabilities are separate, and the first already works here:

| | Requires |
|---|---|
| publishing a Pages site **from** a private repository | GitHub Pro or above (Free needs a public repository) |
| making the published **site** private | Enterprise Cloud, organization-owned repository |

## Unpublishing

```bash
gh api -X DELETE repos/vrvrv/seQ/pages
```

The site stops being served. Anything already indexed stays in caches and
search results for a while, so this undoes less than it looks.
