" seQ syntax, for Vim and Neovim.
"   :set runtimepath+=/path/to/seQ/editors
" or copy this to ~/.config/nvim/syntax/seq.vim and add
"   au BufRead,BufNewFile *.seq set filetype=seq
if exists("b:current_syntax") | finish | endif

syn keyword seqDeclaration let pool stage workload session server run
syn keyword seqStatement    turn request set observe hold enter admit if fit where
syn keyword seqStatement    grow drop branch with
syn keyword seqStatement    loop choose end reserve reuse cache keep at admission
syn keyword seqStatement    growing on in by else prefill transfer decode tool
syn keyword seqOption       cap block evict lru preempt lifo none queue fifo admit
syn keyword seqOption       via spill when ps delay step budget cost chunk exclusive
syn keyword seqOption       first memory arrive poisson closed batch trace ordered
syn keyword seqOption       init horizon warmup seed
syn keyword seqBuiltin      busy work used free cachedin holders queued price
syn keyword seqBuiltin      budget_left est_lambda
syn keyword seqBuiltin      est_rho est_wait now size age last ntok ndec npre nres
syn keyword seqBuiltin      kvb kvp attn cached serial turn_no new out think more forced

" Four roles, four groups: the skeleton (Structure), what a session does
" (Function), the knobs (Type), and what it reads (Identifier). Arithmetic -
" min, floor, pow - stays plain: it is how a program computes, not what it
" means.
"
" a distribution is written `~name(...)`; the tilde is where randomness enters
syn match   seqSample   "\~\w\+"
syn match   seqNumber   "\<\d\+\.\=\d*\([eE][-+]\=\d\+\)\=\>"
syn match   seqComment  "//.*$"
syn region  seqComment  start="/\*" end="\*/"
syn region  seqString   start='"' end='"'

hi def link seqDeclaration Structure
hi def link seqStatement   Function
hi def link seqOption      Type
hi def link seqBuiltin     Identifier
hi def link seqSample      Special
hi def link seqNumber      Number
hi def link seqComment     Comment
hi def link seqString      String

let b:current_syntax = "seq"
