## Security audit

Scope: the change, and the code paths it reaches. This uses several agents and more of the user's
plan. Never run the target's code outside a sandbox; anything that can only be proven by running it
is marked needs validation.

1. **Reconnaissance.** Map the trust boundaries the change touches: inputs and who controls them,
   authentication and authorization checks, secrets, outbound calls, files, processes and IPC.
2. **Hunt by attack class**, across the change and the paths it reaches:
   - injection (shell, SQL, template, prompt), path traversal, SSRF
   - authorization bypass and data isolation between users, projects or tenants
   - unsafe deserialization and parsing
   - secrets in code, logs, errors or URLs; crypto misuse
   - dependency and supply-chain changes (new packages, scripts, pinned versions)
   - resource exhaustion (unbounded input, loops, retries, memory)
   - client side: XSS, `postMessage` origin checks, unsafe HTML
   - desktop and IPC surfaces exposed to web content or other processes
   Use child agents for the classes (your own subagent or task tool, each named "Audit: <class>" in one
   or two words, such as "Audit: injection") when you have them; otherwise work through them
   yourself.
3. **Adversarial validation**, as in a deep review: a fresh agent or pass tries to disprove each
   candidate. Each time an attack class or batch is validated, send everything confirmed or
   needing validation so far with `complete: false` (see Findings as you go).
4. **Verdicts.** Each finding is **confirmed** (with a complete trace from the attacker-controlled
   source to the sink, file:line at each step), **needs validation** (with what would prove it), or
   **rejected** (one line why).

If the user has Cloudflare's `security-audit` skill installed, say at the end that a full
repository audit is available with it. Do not run it yourself.
