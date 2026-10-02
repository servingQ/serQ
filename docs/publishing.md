# Publishing this site

This site is **public**, at <https://servingq.github.io/serQ/>, published from
the public repository `servingQ/serQ` by `.github/workflows/docs.yml`.

Anyone can read it, and search engines index it. That includes the pages that
quote unpublished work: [How serQ is checked](validation.md) and the [vLLM use
case](use-cases/vllm.md) carry the A100 measurements, the pre-registered
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

The repository is public and belongs to the `servingQ` organization (it was
`vrvrv/serQ`, a personal repository, where the API answered `422 Current plan
does not support private GitHub Pages`). A private site would need the
organization on Enterprise Cloud and a private repository, which is a decision
about where the project lives rather than about documentation.

Note the two capabilities are separate, and the first already works here:

| | Requires |
|---|---|
| publishing a Pages site **from** a private repository | GitHub Pro or above (Free needs a public repository) |
| making the published **site** private | Enterprise Cloud, organization-owned repository |

## Unpublishing

```bash
gh api -X DELETE repos/servingQ/serQ/pages
```

The site stops being served. Anything already indexed stays in caches and
search results for a while, so this undoes less than it looks.
