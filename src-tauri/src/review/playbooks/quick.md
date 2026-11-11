## Quick review

One agent, one pass over the whole change. Cover:

- Correctness: logic errors, edge cases (empty, null, concurrent, repeated, very large), error
  handling, and behaviour the description promises but the code does not deliver.
- Security smells: untrusted input reaching a shell, path, query, URL or HTML; missing
  authorization; secrets in code or logs.
- Tests: what the change touches that no test covers, and tests that assert the implementation
  instead of the behaviour.
- Performance on hot paths: extra work per request, per render or per item, unbounded growth.
- Readability where it hides a bug.

Re-read each finding against the code once and drop the ones that do not hold. Send the ones
that hold as you go (see Findings as you go): once the first ones are checked, then the final set
after the report.
