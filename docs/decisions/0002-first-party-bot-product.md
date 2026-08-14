# ADR 0002: Run the first-party Bot product from the Mutsuki monorepo

- Status: Accepted
- Date: 2026-08-13

## Context

The generated `MutsukiBotTemplate` repository duplicated a release and lockfile boundary after its
implementation, Issues and package dependencies had already moved into Mutsuki. The canonical
`mutsuki-bot` binary also ran as a root Workspace member, so exporting another Workspace no longer
provided an ownership boundary.

## Decision

- Maintain the first-party Bot product under `products/bot` and run it with the root Workspace and
  `Cargo.lock`.
- Keep Runtime Core domain-neutral and preserve ServiceHost, Bot, Agent and Std package ownership.
- Do not add a `BotHost`, root facade crate or product-specific behavior to Core.
- Stop exporting or publishing an independent Bot template repository.
- Provide `mutsuki-create-bot` under `products/bot` for generating thin external product shells.
  Generated projects call the public `mutsuki-bot` product API and pin one immutable Mutsuki
  revision; they do not copy product, Host, Agent, Bot or plugin implementations.
- Redirect and archive `sena-nana/MutsukiBotTemplate` after the in-tree migration is published.
- Keep other business products in their own repositories pinned to one Mutsuki release revision.

## Consequences

Bot runtime changes, owner packages, configuration, tests and release compatibility can be changed
atomically. Users build the first-party Bot directly from Mutsuki instead of resolving a second
Workspace and generated lockfile. The repository has both framework and product responsibilities,
while package dependency rules continue to prevent the product from entering Core.

External business Bot shells remain independent consumers rather than a second first-party product
source. Their generated lockfile belongs to that consumer, while every Mutsuki dependency resolves
from the single revision selected by the scaffold.
