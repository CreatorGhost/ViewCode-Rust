# Computer driver

The helper process behind computer use: it reads and controls macOS windows through
[agent-desktop](https://github.com/vercel-labs/agent-desktop), pinned to one reviewed
revision. Copied unchanged from ViewCode's `native/computer-driver`.

The app spawns it, sends one JSON request per line on stdin and reads one reply per
line on stdout. Before any input the driver asks the app to authorize the action, so
the app's gate (look-only mode, protected apps, approval before input, stale-window
refusal) has the last word. The gate is not ported yet; until it is, nothing in the app
starts this helper.

macOS only. Its rules sit behind a `Desktop` trait and are tested on any platform:

```bash
cargo test --locked --manifest-path native/computer-driver/Cargo.toml
```

It has not yet been run on a Mac from this repository.
