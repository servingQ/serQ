# Documentation principles

Write for someone using serQ, not someone reconstructing its development.

- Give each page one purpose and each section one reader question. Start with the answer or the action.
- Describe current behavior. Remove issue numbers, implementation chronology, rejected alternatives and explanations of why work was not done from user guides and references. Keep design decisions and their self-critiques in `docs/design/`; ordinary history belongs in Git and issues.
- Keep limitations that change how a reader uses a feature or interprets a result. Put assumptions next to the claim they qualify. Never shorten a conditional result into an unconditional one.
- Explain a concept once in its reference page. Elsewhere, show its use and link to that definition. Avoid parallel lists of the same results and inventories of internal helper functions.
- Use runnable examples from the repository. Include only the output needed to interpret them; identify excerpts. Check changed commands and keep links and heading anchors valid.
- Present mathematical results in a `theorem` or `proposition` admonition with a descriptive title. State the assumptions, define symbols, then display the conclusion. Distinguish a paper's result, a theorem of the Lean model, and a simulation observation. Follow with a short interpretation and the proof's location; put proof mechanics in the source unless the derivation teaches something needed here.
- Keep navigation focused on learning, using and understanding the current system. Design records are contributor material, not prerequisites for using a feature.
- Prefer concrete sentences, short paragraphs and descriptive headings. Delete rhetorical emphasis and claims of completeness that the evidence does not support.

Review one section at a time: identify its reader question; keep, shorten, move or delete its content; verify the remaining claims and links before proceeding. A smaller word count alone is not evidence of a better section.

For documentation changes, run `mkdocs build --strict` and the repository's `make check` gate. Inspect rendered mathematical blocks, including their assumptions and proof links. Do not change the semantics or regenerate unrelated artifacts as part of an editorial change.
