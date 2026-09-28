# Publishing this site

`mkdocs build --strict` runs on every pull request that touches the docs, and
uploads the built site as a workflow artifact named `site`. Anyone with
repository access can download it and open `index.html`.

It is an artifact rather than a deployment on purpose. **This repository is
private, and these pages quote unpublished work** — the pre-registered
predictions of `data/exp/seq/prereg/`, the A100 measurements, and the paper's
replica. Publishing them is a decision about the research, not about the
tooling, so it is not the default.

## Reading it locally

```bash
pip install -r docs/requirements.txt
mkdocs serve          # http://127.0.0.1:8000
```

## If you decide to publish

### Private GitHub Pages — not available for this repository

Access control for a Pages site is a **GitHub Enterprise Cloud** feature, and
only for project sites published from a private or internal repository **owned
by an organization**:

> To publish a GitHub Pages site privately, your organization must use GitHub
> Enterprise Cloud.

`vrvrv/seQ` is a personal repository, so this is not a plan upgrade away — the
option does not apply. The API says so directly:

```console
$ gh api -X PUT repos/vrvrv/seQ/pages -F public=false
422 Current plan does not support private GitHub Pages
```

Note that the two things are separate, and the first one already works here:

| | Requires |
|---|---|
| publishing a Pages site **from** a private repository | GitHub Pro or above (Free needs a public repository) |
| making the published **site** private | Enterprise Cloud, organization-owned repository |

Making this work would mean transferring the repository to an organization on
Enterprise Cloud. That is a decision about where the project lives, not about
documentation.

### Public GitHub Pages

This works on the current plan today. Restore the deploy job — add back to
`.github/workflows/docs.yml`:

```yaml
permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true
```

and, after the build step:

```yaml
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: site

  deploy:
    needs: build
    runs-on: ubuntu-22.04
    environment:
      name: github-pages
      url: ${{ steps.deploy.outputs.page_url }}
    steps:
      - id: deploy
        uses: actions/deploy-pages@v4
```

Then Settings → Pages → Source: **GitHub Actions**. The site is served at
`vrvrv.github.io/seQ` — with the understanding that
it is world-readable and indexed. Before that, decide what should come out of
the pages: [How seQ is checked](validation.md) and the [case
study](case-study-vllm.md) both quote measured numbers and the pre-registered
predictions.

### Somewhere else

`mkdocs build` writes a self-contained static site to `site/`. It needs no
server-side anything, so any internal static host will serve it.
