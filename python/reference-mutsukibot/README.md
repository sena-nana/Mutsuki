# MutsukiBot Python Reference Framework

This folder contains the earlier Python implementation of MutsukiBot:

- `mutsukibot/`
- `mutsukibot_ext/`
- Python tests
- Python documentation
- Python examples
- Python packaging metadata

It is kept as a reference and migration layer after the root project moved to a
Rust-first runtime framework. Work on the current runtime should happen in the
root Cargo workspace. This folder is not the root runtime implementation, but it
is not marked as disposable or deprecated by its directory name.

Run Python checks from this directory, not from the repository root.
