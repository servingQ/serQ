# Frontend diagnostics

Name resolution must identify the reference that failed, rather than searching
for the first token with the same spelling. A name may occur in a comment, in a
different namespace, or in a server that is expanded into several requests.

The text AST carries optional spans on references and declarations, and a
`Located` wrapper on identifier expressions. A span records the lexer's
one-based line and character column, plus the identifier's length. Substitution
and server expansion clone this metadata along with the expression. The linker
attaches the innermost failing span to its error, so a missing name in a header
binding points to the binding's definition, not to the substituted use.
Declaration metadata supplies notes and correction candidates. Suggestions
require a unique nearby name in the namespace being resolved.

Ownership checks for `lease`, `release`, and `load` compare references and index
expressions without source locations. They still compare the written syntax:
`q[i]` and `q[j]` are different targets, even if both attributes currently have
the same value. This keeps diagnostic metadata out of ownership decisions.

Locations stop at the linker. The serialized IR, its version, the interpreter,
and oracle artifacts do not change. IR validation keeps its existing structural
context. Manually constructed ASTs can leave reference/declaration spans absent.
`--set` expressions have their own coordinates, so their errors name the override
and do not underline unrelated text in the program file.

The public frontend AST and `LinkError` carry new metadata; callers matching AST
variants must handle `Expr::Located` transparently. The top-level compilation
APIs still return formatted `String` errors. Parser equivalence tests remove
location metadata before comparing desugared syntax, while diagnostic tests
assert locations separately.

A full source map for every IR validation error, machine-readable diagnostics,
and multi-error recovery are separate work. This change deliberately does not
assign source locations to IR nodes or change executable program meaning.
