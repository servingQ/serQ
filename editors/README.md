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

## The documentation site

`docs/hooks/seq_lexer.py` is a Pygments lexer that mkdocs loads through
`hooks:`, so a ```seq fence is highlighted on the published pages.

## Keeping these in step

Three lists of the same words, and the parser has the real one. `make check`
fails if a keyword the parser matches is missing from any of them
(`tests/editors.rs`), so adding or renaming one moves all three or the build
says which file was left behind.
