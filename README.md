# cargo-dirty

A `cargo` subcommand that runs a user-provided Cargo command and prints a *minimal* summary of what was recompiled.

By default it does **not** forward Cargo's own output (no `Fresh ...` spam, no build script warnings). Instead, it:

- Streams *only* crates that actually did work (compile/check/build)
- Prints a final summary line (time + counts)

## Install

From this repo:

```bash
cargo install --path .
```

## Usage

Run your normal cargo command via `cargo dirty`:

```bash
cargo dirty check --workspace --all-targets
```

Stream in a more deterministic order (adds `--jobs=1` to Cargo unless you already set jobs):

```bash
cargo dirty --linear check --workspace --all-targets
```

Show fresh crates too:

```bash
cargo dirty --show-fresh check --workspace --all-targets
```

Enable deep fingerprint tracing (best-effort; mainly useful for debugging):

```bash
cargo dirty --deep check --workspace --all-targets
```

## Notes

- On success, output is intentionally terse.
- On failure, only compiler **errors** are printed (from Cargo JSON messages).
- Invalidation reasons are best-effort from Cargo `-vv` status lines (e.g. `Dirty ...: ...`).
