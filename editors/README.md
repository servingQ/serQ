# Editor support for `.seq`

## Neovim / Vim

```vim
set runtimepath+=/path/to/seQ/editors
au BufRead,BufNewFile *.seq set filetype=seq
```

or copy `seq.vim` to `~/.config/nvim/syntax/seq.vim` and add the autocommand.

## VS Code and anything that reads TextMate grammars

`seq.tmLanguage.json`, scope `source.seq`. For a local VS Code extension, put
it in `syntaxes/` with

```json
"contributes": {
  "languages": [{ "id": "seq", "extensions": [".seq"] }],
  "grammars": [{ "language": "seq", "scopeName": "source.seq",
                 "path": "./syntaxes/seq.tmLanguage.json" }]
}
```

## GitHub

GitHub highlights by [Linguist](https://github.com/github-linguist/linguist),
which has no seQ, and adding one there needs the language to be in use across
many public repositories. Until then `.gitattributes` maps `.seq` to Rust,
which shares the comment syntax, the number literals and four keywords. The
words that carry a program's meaning stay plain — that is the cost of an
approximation we do not control, and it is still better than no colour at all.

Markdown fences are a different matter: a ```seq fence renders plain on
GitHub and highlighted on the site, because a fence's language is Linguist's
to resolve and the site's renderer is ours.

## The documentation site

`docs/hooks/seq_lexer.py` is a Pygments lexer that mkdocs loads through
`hooks:`, so a ```seq fence is highlighted on the published pages.

## Keeping these in step

Three lists of the same words, and the parser has the real one. `make check`
fails if a keyword the parser matches is missing from any of them
(`tests/editors.rs`), so adding or renaming one moves all three or the build
says which file was left behind.
