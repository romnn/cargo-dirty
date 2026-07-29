# cargo-dirty

A `cargo` subcommand that runs a user-provided Cargo command and prints a *minimal* summary of what was recompiled.

By default it does **not** forward Cargo's own output (no `Fresh ...` spam, no build script warnings). Instead, it:

- Streams *only* crates that actually did work (compile/check/build)
- Prints a final summary line (time + counts)

## Install

With Cargo:

```bash
cargo install --locked cargo-dirty
```

With Homebrew:

```bash
brew install --cask romnn/tap/cargo-dirty
```

Prebuilt archives are also available from
[GitHub Releases](https://github.com/romnn/cargo-dirty/releases).

From a local checkout:

```bash
cargo install --locked --path .
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

Explain *why* the rebuild happened — the root change that started the cascade, plus every crate
downstream of it and the reason each was invalidated:

```bash
cargo dirty --explain check --workspace --all-targets
```

Enable deep fingerprint tracing (best-effort; mainly useful for debugging). On its own it only
collects Cargo's fingerprint trace; combined with `--explain` it adds per-crate `detail:` lines
explaining which fingerprint comparison failed:

```bash
cargo dirty --deep --explain check --workspace --all-targets
```

Run a different Cargo binary (defaults to `cargo` from `PATH`):

```bash
cargo dirty --cargo-path /path/to/cargo check
```

Print the version:

```bash
cargo dirty --version
```

## Flags

| Flag | Effect |
|------|--------|
| `--show-fresh` | Also list crates Cargo considered fresh |
| `--explain` | Print the culprit crate and the rebuild cascade it caused |
| `--deep` | Collect Cargo's fingerprint trace; enriches `--explain` with `detail:` lines |
| `--linear` | Add `--jobs=1` so work is streamed in a deterministic order |
| `--cargo-path <PATH>` | Cargo binary to run |
| `--version` | Print the cargo-dirty version |

Everything that is not one of these flags is forwarded to Cargo verbatim, including arguments
after a `--` separator, which reach the built binary untouched.

## Notes

- On success, output is intentionally terse.
- On failure, compiler **errors** are printed (from Cargo JSON messages); if Cargo failed
  without producing any, Cargo's own stderr is shown instead.
- Invalidation reasons are best-effort from Cargo `-vv` status lines (e.g. `Dirty ...: ...`).

## Limitations

- Reasons and counts come entirely from Cargo's `-vv` output. They are best-effort: Cargo may
  declare a crate dirty that then does no separately reported work, so `dirty` and `work` counts
  need not agree.
- `--quiet` / `-q` cannot be used. cargo-dirty injects `-vv`, and Cargo refuses to run with both;
  cargo-dirty warns before Cargo rejects the invocation.
- Output of `cargo dirty run` and `cargo dirty test` is forwarded as-is, interleaved line-wise
  with cargo-dirty's own status lines.
- `--explain` names a *likely* root cause inferred from reason text. With two versions of the
  same crate in one build it declines to guess rather than risk misattribution.
- `--deep` `detail:` lines are Cargo's internal fingerprint diagnostics reproduced verbatim.
  Their wording is not a stable interface and changes between Cargo releases; when a release
  changes it again, details go missing rather than becoming wrong.
