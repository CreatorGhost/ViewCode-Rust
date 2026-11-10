## Deep review

A lead agent (you) and child agents. This uses several agents and more of the user's plan.

1. **Split.** Read the change and divide it into areas a reviewer can hold at once.
2. **Lenses.** Spawn one child agent per lens with the agent tools you have (your own
   subagent or task tool). Name each
   "Review: <lens>" in one or two words ("Review: tests", "Review: security"), which the user
   sees in their chat. Give each the change's identifiers, the areas, these ground rules and its
   lens:
   - correctness and edge cases
   - security, hunted by attack class: injection (shell, SQL, template, prompt), path traversal
     and SSRF; authorization and data isolation; secrets in code, logs or URLs and crypto misuse;
     unsafe parsing; new dependencies and scripts; resource exhaustion; XSS and IPC surfaces
   - performance and resource use
   - tests and behaviour coverage
   - API and contract changes, and consistency across every caller and surface
   - the repository's rules (the files named in the ground rules), quoting each rule broken
   - history: `git blame` and `git log` on the changed lines and earlier review comments on
     these files, to tell a deliberate behaviour from an accident
   If you cannot spawn agents, run the lenses yourself as separate passes, one after another.
3. **Run the project's checks** when the pull request's head is in this repository (never for a
   fork or an outside contributor's branch, whose code you must not run): add a temporary git
   worktree outside the user's checkout at the fetched head, run the lint, typecheck and tests the
   project documents for the packages the change touches, read the failures as evidence, then
   remove the worktree. Never run them in the user's checkout.
4. **Adversarial verification.** Hand every candidate finding to a fresh agent (or a fresh pass)
   whose only job is to disprove it from the code. Report only findings that survive. Put the ones
   nobody could settle under **Needs checking**, with what would settle each. Drop disproved ones;
   you may list them under **Rejected**, one line each. Each time a lens's findings come back
   from verification, send everything verified so far with `complete: false` (see Findings as
   you go), so the reader is not left waiting for the last lens.
5. **Consolidate.** Merge duplicates across lenses, keep the strongest evidence, and write the
   report.
