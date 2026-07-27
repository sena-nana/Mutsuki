# Mutsuki release train

## Version baseline

While framework interfaces are evolving, one minor version identifies one compatible repository
revision. Individual packages may publish patch versions only when they remain compatible with that
minor baseline. Breaking cross-package changes are made in one repository change.

## Release gate

1. Run the monorepo boundary checker and locked Rust workspace validation.
2. Run Python, frontend, integration and owner performance suites.
3. Validate the canonical Bot template export in an isolated directory.
4. Validate external business consumers against the candidate revision.
5. Update the compatibility matrix and migration notes.
6. Tag the exact validated commit and publish release notes.
7. Re-run clean-clone metadata, build, tests and smoke performance from the tag.

Products must pin a tag or commit. Depending on `main`, mixing package revisions, or pinning former
framework repositories is unsupported.

## Package selection

Cargo resolves packages from the repository root. Consumers may select any package directly; there
is no default root facade. A package must not require unrelated Hosts, Kits or plugins merely
because they share this repository.
