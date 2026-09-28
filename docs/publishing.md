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

### Private GitHub Pages — visible only to people with repository access

Needs a plan that supports private Pages (GitHub Pro, Team or Enterprise
Cloud). On the plan this repository is on today the API answers
`422 Current plan does not support private GitHub Pages`, and a Pages site
created anyway is **public**.

Once the plan allows it:

1. Settings → Pages → Source: **GitHub Actions**, Visibility: **Private**.
2. Restore the deploy job — add back to `.github/workflows/docs.yml`:

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

3. Confirm the visibility afterwards — creating a Pages site defaults to
   public:

    ```bash
    gh api repos/vrvrv/seQ/pages --jq '.public'   # must be false
    ```

### Public GitHub Pages

The same, without step 3 — and with the understanding that
`vrvrv.github.io/seQ` is world-readable and indexed. Before that, decide what
should come out of the pages: [How seQ is checked](validation.md) and the
[case study](case-study-vllm.md) both quote measured numbers and the
pre-registered predictions.

### Somewhere else

`mkdocs build` writes a self-contained static site to `site/`. It needs no
server-side anything, so any internal static host will serve it.
