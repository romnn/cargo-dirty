# cargo-dirty

A `cargo` subcommand that runs a user-provided Cargo command with extra verbosity, then summarizes *only the crates that actually did work* (i.e. were not cached/fresh), in the order Cargo executed them.

## Install

From this repo:

```bash
cargo install --path .
```

## Usage

Run your normal cargo command via `cargo dirty`:

```bash
cargo dirty build
```

Show fresh crates too:

```bash
cargo dirty --show-fresh build
```

Reduce interleaving for a more trustworthy "first culprit" ordering:

```bash
cargo dirty --linear build
```

Enable deep fingerprint tracing (best-effort; output is not a stable API):

```bash
cargo dirty --deep build
```

## Notes

- This is v1 (MVP) and is intentionally conservative.
- The stable, structured signal source is Cargo JSON messages (`--message-format=json`).
- The human-facing invalidation reasons are currently best-effort from `-vv` stderr lines (e.g. `Dirty ...: ...`).
- In `--deep` mode, cargo fingerprint tracing is captured for future use; v1 does not yet fully attribute detailed reasons from these traces.
