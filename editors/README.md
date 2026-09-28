# Editor support for `.seq`

## Four roles, four colours

A program's **shape** is `pool`, `stage`, `session`; what a session **does** is
`enter`, `prefill`, `observe`, `branch`; the **knobs** are `cap`, `evict`,
`budget`, `cost`; and what it **reads** is `cachedin`, `budget_left`, `now`,
`kvb`. Each lands in a different colour group, so the shape of a program is
legible before a word of it is read, and `~` gets one of its own — that is
where the randomness enters.

Arithmetic (`min`, `floor`, `pow`) stays plain. It is how a program computes,
not what it means, and leaving it uncoloured is what lets the observable
colour mean exactly one thing.

The first attempt put all four in `Keyword`, `Keyword.Declaration` and
`Keyword.Pseudo`, which most themes — Material included — render in a single
colour. Four roles, one colour, which is no better than none.

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

## The documentation site

`docs/hooks/seq_lexer.py` is a Pygments lexer that mkdocs loads through
`hooks:`, so a ```seq fence is highlighted on the published pages.

## Keeping these in step

Three lists of the same words, and the parser has the real one. `make check`
fails if a keyword the parser matches is missing from any of them
(`tests/editors.rs`), so adding or renaming one moves all three or the build
says which file was left behind.
