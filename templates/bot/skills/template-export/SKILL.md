---
name: template-export
description: Export the canonical Bot template source from the Mutsuki monorepo into an independently buildable GitHub Template pinned to one Mutsuki release tag or commit. Use for template dependencies, generation, release pins, lockfiles, or clean-clone validation.
---

# Template Export

The source under `templates/bot` uses root Workspace dependencies. Exported templates must not.

## Workflow

1. Validate the monorepo workspace and selected product assembly.
2. Export only user-facing template files, examples, simple config and required documentation.
3. Replace workspace dependencies with `https://github.com/sena-nana/Mutsuki.git` at one tag or
   fixed commit; all Mutsuki packages use the same revision.
4. Generate the exported `Cargo.lock`; do not copy the monorepo root lock.
5. Reject repository-external paths, local patches, credentials and advanced local Host config.
6. Validate metadata, format, check and tests in a clean directory with no parent workspace.
7. Sync the generated result to the unarchived `sena-nana/MutsukiBotTemplate` repository only
   after byte-for-byte comparison, using its write-scoped deploy key.
8. Keep the target marked as a GitHub Template and reject manual or independently evolved source.

Report the source revision, output revision, exact package pins and clean-clone commands.
