"""Register a Pygments lexer for serQ, so ```serq fences highlight.

Four roles, four colours. A program's shape is `engine`/`workload`/`server`;
what a request *does* is `hold`, `run`, `observe`; the knobs are `cap`,
`evict`, `schedule`; and what it *reads* is `cachedin`, `running.count`, `now`.
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
STRUCTURE = ("fn", "let", "def", "use", "device", "engine", "pool", "stage", "workload", "session", "server", "queue", "gateway", "link", "route", "pull", "push", "gauge", "claim")

# Statements, in the session and server blocks.
STATEMENTS = (
    "Size", "Cost", "turn", "request", "set", "observe", "hold", "admit", "mark", "self", "grow", "drop", "release", "load", "lease", "from", "to", "branch", "with",
    "while", "loop", "fork", "join", "choose", "end", "run", "reserve", "reuse", "cache",
    "at", "admission", "growing", "on", "in", "by", "else", "sum",
    # the serving vocabulary: sugar over hold and run
    "prefill", "transfer", "decode", "tool",
)

# Pool and stage options, workload forms, and the resource cost constructor.
OPTIONS = (
    "cap", "block", "evict", "lru", "preempt", "lifo", "held", "requeue", "head", "tail", "none", "queue", "fifo",
    "admit", "while", "state", "via", "spill", "when", "ps", "delay", "cost",
    "granule", "serve", "latency", "nic", "exclusive", "first", "only", "arrive", "arrivals", "poisson", "renewal", "closed", "hidden",
    "batch", "trace", "ordered", "init", "horizon", "warmup", "seed",
    "share", "maxmin", "bottleneck",
    # an engine's schedule and execution
    "schedule", "advance", "each", "most", "execute",
    # no program writes `stage : step` any more, but the parser still matches
    # `step` to say an engine is written instead, and tests/docs_lexer.rs
    # asks the lexer for every word the parser matches
    "step",
    # a claim's forms
    "given", "every", "some", "iteration", "of",
)

# An engine's values, read as `list.field` and coloured whole as what a
# program reads: `batch` alone is an option (`batch` arrivals), `running` alone
# is nothing. `tests/docs_lexer.rs` holds this to the parser's `ENGINE_VALUES`.
ENGINE_VALUES = (
    "running.count", "running.decoding", "running.kv_decode", "running.kv_prefill",
    "running.preempted",
    "waiting.count", "waiting.admitted",
    "batch.tokens", "batch.prefilled", "batch.decoding", "batch.kv_decode",
    "batch.kv_prefill", "batch.attention",
)

# Observables and arithmetic: things a program reads rather than declares.
# A context variable's name is the language's everywhere in a program: since
# #231 the linker refuses an attribute or a `let` that takes one, so `tokens`
# is never a program's own and colouring it says something true.
# Arithmetic is not a role: `min`, `floor` and `pow` are how a program
# computes, not what it means, and they read as calls without help. Leaving
# them plain is what lets the observable colour mean exactly one thing.
# `tests/docs_lexer.rs` holds these lists to the linker's (`CONTEXT_VARS`,
# `FUNCTIONS`, `AGGREGATES`, `FOLDED`, `BUILTIN_ATTRS`).
ARITHMETIC = ("min", "max", "abs", "floor", "ceil", "sqrt", "exp", "ln", "pow")

BUILTINS = (
    # functions
    "busy", "work", "used", "free", "cachedin", "holders", "queued",
    "price", "budget_left", "est_lambda", "est_rho", "est_wait", "blocksize",
    # context variables
    "now", "waited", "size", "age", "last", "waiting", "present", "tokens",
    "decoders", "prefilled", "residents", "kv_decode", "kv_prefill",
    "attention", "decoding", "admission", "remaining", "position", "demand",
    "served", "arrived", "admitted", "preempted", "inf",
    # attributes the language sets (docs/api/attributes.md); `new` and `out`
    # stay plain, as a program also names an `observe` so (`total(out)`)
    "cached", "serial", "turn_no", "think", "more", "forced", "computed",
)

# A run's aggregates, which a claim `at end` reads: `count` and `total` are
# ordinary words, so they are coloured only as a call.
AGGREGATES = ("total", "count", "largest", "smallest", "prefix_total")


class SerqLexer(RegexLexer):
    name = "serQ"
    aliases = ["serq", "seq"]
    filenames = ["*.sq"]

    tokens = {
        "root": [
            (r"//.*?$", Comment.Single),
            (r"/\*", Comment.Multiline, "block-comment"),
            (r'"[^"]*"', String),
            # the grammar blocks of docs/api quote a terminal, `'['`, and
            # elide with `…`
            (r"'[^'\n]*'", String.Char),
            (r"…", Punctuation),
            # a distribution is written `~name(...)`, and the tilde is the
            # thing to see: it is where the randomness enters
            (r"(~)([a-z_][\w]*)", bygroups(String.Escape, String.Escape)),
            # The four roles have to land in four *different* colour groups.
            # A theme that renders `Keyword`, `Keyword.Declaration` and
            # `Keyword.Pseudo` alike - which most do, Material included -
            # would collapse them back into one, which is the whole point of
            # separating them.
            (words(ENGINE_VALUES, prefix=r"\b", suffix=r"\b"), Name.Variable),
            # a schedule's two statements are one phrase each: `running`
            # and `waiting` there name the engine's lists, not a value
            (r"\b(?:advance\s+running|admit\s+waiting)\b", Name.Function),
            # `tokens cap B` is an engine's clause, not the context variable
            (r"\btokens(?=\s+cap\b)", Name.Builtin),
            (words(STRUCTURE, suffix=r"\b"), Keyword),          # the skeleton
            (words(STATEMENTS, suffix=r"\b"), Name.Function),   # what a session does
            (words(OPTIONS, suffix=r"\b"), Name.Builtin),       # the knobs
            (words(BUILTINS, suffix=r"\b"), Name.Variable),     # what a program reads
            (words(AGGREGATES, suffix=r"(?=\s*\()"), Name.Variable),
            (words(ARITHMETIC, suffix=r"\b"), Name),            # how it computes
            (r"\d+\.?\d*([eE][-+]?\d+)?", Number),
            (r"[-+*/^<>=!&|?:]+", Operator),
            (r"[{}()\[\],;.]", Punctuation),
            # a placeholder may be Greek, `reuse (ρ)`
            (r"[^\W\d]\w*", Name),
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
