# Roadmap

An honest accounting of what works today versus what is modeled but not
yet wired up. "Modeled" means the project schema and validation accept it;
it does not mean a generated installer will do it.

## Solid today

- Typed project model, installation graph, policy and privilege analysis,
  round-tripping through TOML (`inst-model`).
- Static Rust codegen for: file/directory copy, shortcuts, start-menu
  entries, services (Windows SCM + systemd), registry values (Windows),
  environment variables, shell script actions (with a
  `before/after-install/uninstall` event graph and per-action conditions),
  running a bundled program, and prerequisite detection/installation via
  the smart prerequisites catalog.
- Transactional runtime engine: journaled install/uninstall with rollback
  on failure, atomic file operations, integrity-verified payload
  extraction, silent and console modes, structured logging with secret
  redaction, self-uninstall.
- Solid/grouped-solid compressed payload format (zstd/xz) with resumable,
  content-addressed downloads for prerequisites.
- Project analyzer (infers a starting project from an existing app
  folder) and doctor (diagnostics with one-click safe fixes).
- Single-screen GPUI installer UI (`inst-runtime-ui`) and Installer Studio
  (`inst-studio`): DevExpress-dark, resizable/collapsible explorer,
  workspace, inspector and bottom Output/Problems/Build/Doctor panel, a
  command palette, Simple/Advanced mode, and a property editor covering
  Product, Install, UI, Policy, Targets, Compression, Signing and Update.
- Full i18n (en, tr, ar, es, fr, de, ru) for both installer strings and
  Studio chrome, with real RTL for Arabic.
- End-to-end tested path: analyze or hand-write a project → validate →
  generate → build → install → uninstall, verified for Linux x64 in
  `crates/builder/tests/e2e.rs`.

## Modeled but not implemented

These are accepted by the schema and rejected with a clear, actionable
error at codegen or sign time — never silently dropped or half-done:

- **Database actions** (`ActionKind::Database`) and **HTTP actions**
  (`ActionKind::Http`) — `inst-codegen` refuses to generate code for
  either and reports why (`crates/codegen/src/emit.rs`).
- **File and protocol associations, firewall rules**
  (`inst_model::project::Integration`) — accepted by the model and shown
  in privilege analysis, but `inst-codegen` currently always emits empty
  association lists, and the runtime logs a warning and skips them if it
  ever saw a non-empty list (`crates/runtime/src/ops.rs`).
- **OS credential store signing keys** (`CredentialSource::Keychain`) —
  `inst-builder` returns "not implemented yet; use an environment
  variable" rather than silently signing unsigned or crashing
  (`crates/builder/src/sign.rs`). Interactive signing-password prompts are
  likewise CLI/headless-only-error; they're expected to come from the
  Studio UI, which doesn't yet ask for one.
- **ARM64 targets** (`Target::WINDOWS_ARM64`, `Target::LINUX_ARM64`) — the
  model, target-triple mapping and builder all know about them, but
  `rust-toolchain.toml` only installs the x64 targets, and there is no
  end-to-end test for either. Building for them requires installing the
  right Rust target yourself; treat it as untested, not unsupported.

## Studio gaps

The Studio edits and builds real projects, but its coverage of the model
is intentionally partial so far:

- The property inspector has no editor yet for the installation graph
  itself (custom actions, their ordering/conditions), the Integration
  section (shortcuts, file/protocol associations, firewall rules), custom
  UI fields, or the prerequisites catalog selection. These are all
  editable today only by hand-editing the `.instproj` TOML.
  `inst-doctor`/`inst-analyzer` and codegen still see and process
  everything in the file regardless of whether the Studio UI can display
  it, so nothing is lost by round-tripping through the Studio — it just
  can't be authored there yet.
- The toolbar's Build action always builds `linux-x64` regardless of
  which targets the open project has enabled (`StudioView::run_build`).
  Building other targets currently means using `inst-cli build` with
  `--target`.
- The command palette opens only from its toolbar button; it has no
  keyboard shortcut yet, and its filter box doesn't yet filter the action
  list.
- No window-layout persistence: the explorer/inspector/bottom-panel sizes
  and collapsed state reset each time Studio starts.

## Not started

- CI (this is being added now — see `.github/workflows/`).
- Any packaging/distribution of Installer Studio itself (installers for
  the Studio, auto-update for the Studio).
- macOS is out of scope entirely (no `Target` variant, no code path); the
  project only ever targets Windows and Linux end-user machines.
