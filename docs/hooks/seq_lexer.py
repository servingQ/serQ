"""Register a Pygments lexer for seQ, so ```seq fences highlight.

The pages fenced 25 blocks as ```rust, which is close enough to look right
and wrong in the places that matter: `hold`, `enter`, `observe` and `~exp`
are not Rust, and the words that carry a seQ program's meaning were the ones
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
STRUCTURE = ("let", "pool", "stage", "workload", "session", "run")

# Statements, in the session block.
STATEMENTS = (
    "turn", "set", "observe", "hold", "enter", "grow", "drop", "branch", "with",
    "loop", "choose", "end", "run", "reserve", "reuse", "cache", "keep",
    "at", "admission", "growing", "on", "in", "by", "else",
    # the serving vocabulary: sugar over hold and run
    "prefill", "transfer", "decode", "tool",
)

# Pool and stage options, and workload forms.
OPTIONS = (
    "cap", "block", "evict", "lru", "preempt", "lifo", "none", "queue", "fifo",
    "admit", "via", "spill", "when", "ps", "delay", "step", "budget", "cost",
    "chunk", "exclusive", "first", "memory", "arrive", "poisson", "closed",
    "batch", "trace", "ordered", "init", "horizon", "warmup", "seed",
)

# Observables and arithmetic: things a program reads rather than declares.
BUILTINS = (
    "min", "max", "abs", "floor", "ceil", "sqrt", "exp", "ln", "pow",
    "queue", "busy", "work", "used", "free", "cachedin", "holders", "queued",
    "price", "budget_left", "est_lambda", "est_rho", "est_wait",
    "now", "size", "age", "last", "n", "ntok", "ndec", "npre", "nres",
    "kvb", "kvp", "attn", "cached", "serial", "turn_no", "new", "out",
    "think", "more", "forced",
)


class SeqLexer(RegexLexer):
    name = "seQ"
    aliases = ["seq"]
    filenames = ["*.seq"]

    tokens = {
        "root": [
            (r"//.*?$", Comment.Single),
            (r"/\*", Comment.Multiline, "block-comment"),
            (r'"[^"]*"', String),
            # a distribution is written `~name(...)`, and the tilde is the
            # thing to see: it is where the randomness enters
            (r"(~)([a-z_][\w]*)", bygroups(Operator, Name.Builtin)),
            (words(STRUCTURE, suffix=r"\b"), Keyword.Declaration),
            (words(STATEMENTS, suffix=r"\b"), Keyword),
            (words(OPTIONS, suffix=r"\b"), Keyword.Pseudo),
            (words(BUILTINS, suffix=r"\b"), Name.Builtin),
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
    """Make `seq` a language Pygments knows, before any page is rendered."""
    module = types.ModuleType("pygments.lexers.seq")
    module.SeqLexer = SeqLexer
    # `get_lexer_by_name` imports the module and reads `__all__` off it
    module.__all__ = ["SeqLexer"]
    sys.modules["pygments.lexers.seq"] = module
    _mapping.LEXERS["SeqLexer"] = (
        "pygments.lexers.seq",
        "seQ",
        ("seq",),
        ("*.seq",),
        (),
    )
    return config
