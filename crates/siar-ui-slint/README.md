# siar-ui-slint

Siar's desktop UI: Slint views plus the bridge to `siar-ui-state`.

- `ui/*.slint` — design tokens (`theme.slint`, ui-ux-22), shared components,
  pages and the shell (`main.slint`, ui-ux-02). `app_state.slint` is the
  entire UI↔Rust contract.
- `src/hub.rs` — `UiHub`: thread-safe state handle (replaces Dioxus signals),
  coalesced redraws, non-blocking command queue.
- `src/projection.rs` — slices → Slint models (keyed in-place diff, image cache).
- `src/bridge.rs` — Slint callbacks → validated `AppCommand`s.
- Depends on no networking/crypto/storage crate: the UI cannot call the core
  except through `AppCommand`.

Tests: `cargo test -p siar-ui-slint` (headless; renders every page with the
software renderer — set `SIAR_SHOT_DIR` to keep the PNGs, default
`/tmp/siar-shots`).

Slint is pinned `=1.16.1` (1.17+ needs rustc 1.92; the workspace is on 1.91).
