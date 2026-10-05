"""Register a Pygments lexer for serQ, so ```serq fences highlight.

Four roles, four colours. A program's shape is `pool`/`stage`/`session`; what
a session *does* is `hold`, `prefill`, `observe`; the knobs are `cap`,
`evict`, `budget`; and what it *reads* is `cachedin`, `budget_left`, `now`.
A reader should be able to tell those apart before reading a word, so each
lands in a different colour group, and `~` gets its own because that is where
the randomness enters. Arithmetic stays plain: it is how a program computes,
not what it means.

The pages fenced 25 blocks as ```rust, which is close enough to look right
and wrong in the places that matter: `hold`, `observe` and `~exp`
are not Rust, and the words that carry a serQ program's meaning were the ones
left uncoloured.

mkdocs loads this through `hooks:`. Pygments finds a lexer through its
`_mapping.LEXERS` table, so the module is put on `sys.modules` under the name
that table points at.
"""

import sys
import types

from pygments.lexer import RegexLexer, bygroups, words
from pygments.lexers import _mapping
from pygments.token import Comment, Keyword, Name, Number, Operator, Punctuation, String, Text

# The blocks a program is made of.
STRUCTURE = ("let", "def", "use", "pool", "stage", "workload", "session", "server", "run", "queue", "gateway", "link", "route", "pull", "push", "gauge", "claim")

# Statements, in the session and server blocks.
STATEMENTS = (
    "turn", "request", "set", "observe", "hold", "admit", "mark", "self", "grow", "drop", "release", "load", "lease", "from", "to", "branch", "with",
    "loop", "fork", "join", "choose", "end", "run", "reserve", "reuse", "cache",
    "at", "admission", "growing", "on", "in", "by", "else", "sum",
    # the serving vocabulary: sugar over hold and run
    "prefill", "transfer", "decode", "tool",
)

# Pool and stage options, and workload forms.
OPTIONS = (
    "cap", "block", "evict", "lru", "preempt", "lifo", "held", "requeue", "head", "tail", "none", "queue", "fifo",
    "admit", "while", "state", "via", "spill", "when", "ps", "delay", "step", "budget", "cost",
    "chunk", "granule", "serve", "latency", "nic", "exclusive", "first", "only", "memory", "arrive", "arrivals", "poisson", "renewal", "closed", "hidden",
    "batch", "trace", "ordered", "init", "horizon", "warmup", "seed",
    "share", "maxmin", "bottleneck",
    # a claim's forms
    "given", "every", "some", "iteration", "of",
)

# Observables and arithmetic: things a program reads rather than declares.
# Short, ordinary words are left out on purpose: `size`, `age`, `last`,
# `tokens`, `present`, `waiting`, `residents`, `decoders`, `prefilled`,
# `attention` are context variables in the one place the semantics supplies
# them and ordinary attribute names everywhere else (`out` and `new` are
# attributes), and a lexer cannot tell. Colouring a program's own `tokens` as
# a builtin is worse than leaving it plain.
# Arithmetic is not a role: `min`, `floor` and `pow` are how a program
# computes, not what it means, and they read as calls without help. Leaving
# them plain is what lets the observable colour mean exactly one thing.
ARITHMETIC = ("min", "max", "abs", "floor", "ceil", "sqrt", "exp", "ln", "pow")

BUILTINS = (
    "busy", "work", "used", "free", "cachedin", "holders", "queued",
    "price", "budget_left", "est_lambda", "est_rho", "est_wait",
    "now", "kv_decode", "kv_prefill",
    "decoding", "admission", "remaining", "waited",
    "cached", "serial", "turn_no", "think", "more", "forced", "computed",
)


class SerqLexer(RegexLexer):
    name = "serQ"
    aliases = ["serq", "seq"]
    filenames = ["*.sq"]

    tokens = {
        "root": [
            (r"//.*?$", Comment.Single),
            (r"/\*", Comment.Multiline, "block-comment"),
            (r'"[^"]*"', String),
            # a distribution is written `~name(...)`, and the tilde is the
            # thing to see: it is where the randomness enters
            (r"(~)([a-z_][\w]*)", bygroups(String.Escape, String.Escape)),
            # The four roles have to land in four *different* colour groups.
            # A theme that renders `Keyword`, `Keyword.Declaration` and
            # `Keyword.Pseudo` alike - which most do, Material included -
            # would collapse them back into one, which is the whole point of
            # separating them.
            (words(STRUCTURE, suffix=r"\b"), Keyword),          # the skeleton
            (words(STATEMENTS, suffix=r"\b"), Name.Function),   # what a session does
            (words(OPTIONS, suffix=r"\b"), Name.Builtin),       # the knobs
            (words(BUILTINS, suffix=r"\b"), Name.Variable),     # what a program reads
            (words(ARITHMETIC, suffix=r"\b"), Name),            # how it computes
            (r"\d+\.?\d*([eE][-+]?\d+)?", Number),
            (r"[-+*/^<>=!&|?:]+", Operator),
            (r"[{}()\[\],;]", Punctuation),
            (r"[a-zA-Z_]\w*", Name),
            (r"\s+", Text),
        ],
        "block-comment": [
            (r"[^*/]+", Comment.Multiline),
            (r"/\*", Comment.Multiline, "#push"),
            (r"\*/", Comment.Multiline, "#pop"),
            (r"[*/]", Comment.Multiline),
        ],
    }


def on_config(config, **_):
    """Make `serq` a language Pygments knows, before any page is rendered."""
    module = types.ModuleType("pygments.lexers.sq")
    module.SerqLexer = SerqLexer
    # `get_lexer_by_name` imports the module and reads `__all__` off it
    module.__all__ = ["SerqLexer"]
    sys.modules["pygments.lexers.sq"] = module
    _mapping.LEXERS["SerqLexer"] = (
        "pygments.lexers.sq",
        "serQ",
        ("serq", "seq"),
        ("*.sq",),
        (),
    )
    return config
