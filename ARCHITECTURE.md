# Architecture

## Two programs, one model

Everything is built around a single typed project description (an
`.instproj` TOML file, `inst_model::project::Project`). Two very different
programs consume it:

- **Installer Studio** (`inst-studio`, `inst-cli`) is a developer tool. It
  edits the project, validates it, imports existing applications into it,
  diagnoses it, and compiles it. It never ships to an end user.
- **The generated installer** is what an end user runs. It is a small,
  static Rust binary produced by compiling generated code
  (`inst-codegen`) against the runtime engine (`inst-runtime`), with only
  the runtime features the project actually needs turned on.

The two never share a process, and the runtime crate has no dependency,
even an optional one, on the Studio, the analyzer, the doctor, or cargo
itself. This is the boundary that keeps generated installers small: a
project with no services, no registry writes and no scripts links none of
that code in.

## Pipeline

```
.instproj (TOML)
   |  inst_model::io::from_toml
   v
Project  ---------------------------+
   |  inst_model::validate::validate |  (also: inst_doctor::examine)
   v                                 |
Diagnostics (blocking / advisory) <--+
   |
   v  inst_codegen::generate
Generated crate (main.rs, static descriptors, the installation graph
as a sequence of typed Rust calls — no interpreter, no bytecode)
   |
   v  inst_builder::build
  1. pack the application payload (inst-payload: solid/grouped-solid
     zstd or xz blocks, content hashed, streamed rather than fully
     buffered)
  2. cargo build --release the generated crate, once per requested
     target, with only the cargo features the project uses
  3. append the payload to the compiled stub (single self-contained
     file; no installer + separate data file to keep together)
  4. sign (Windows, when configured) and report
   v
Product-Setup-x64.exe / Product-Setup-x86_64.AppImage
```

`inst_codegen` rejects, at generation time, anything it cannot turn into
real code (see [ROADMAP.md](ROADMAP.md) for what that currently includes)
rather than emitting something that silently does less than the project
says. A build fails loudly instead of shipping a lie.

## The installation graph

`inst_model::graph` turns declarative project settings (files to copy,
services to install, registry keys, shortcuts, custom actions with
`before-install`/`after-install`/`before-uninstall`/`after-uninstall`
ordering and per-action conditions) into an explicit, ordered list of
steps with each step's rollback counterpart. `inst-codegen` emits this
graph as a plain Rust function that calls typed `inst_runtime` methods in
order; `inst-runtime`'s `journal` module makes each step's effect
undoable, so a mid-install failure rolls back everything already done
instead of leaving a half-installed product.

## Concurrency in the GPUI frontends

GPUI entities (`Entity<T>` / `Context<T>`) are not `Send` and must stay on
the main thread. Both GUI frontends (`inst-runtime-ui`, the installer's own
progress screen, and `inst-studio`'s build/doctor/analyze actions) use the
same pattern to run blocking work without freezing the UI:

1. A `TaskProgress`/`Progress2`-style snapshot struct lives behind
   `Arc<Mutex<T>>`.
2. The blocking work (install/uninstall, or codegen+cargo build) runs on
   `cx.background_executor()`, writing snapshots into the mutex as it goes.
3. The view polls the snapshot on a `cx.background_executor().timer(...)`
   loop and calls `cx.notify()` to repaint — never blocking the render
   thread, never touching GPUI state from the background thread.

Native file/folder pickers (`cx.prompt_for_paths`) are async and return a
`oneshot::Receiver`; the window handle is captured before the `.await` and
`AsyncApp::update_window` is used afterward to regain `&mut Window`, since
the window may have been (re)laid out while the picker was open.

Both binaries fail gracefully instead of panicking when the host has no
GPU-capable rendering surface (`cx.open_window` returning `Err`, e.g. a
bare X server with no software rasterizer): they print a clear error and
exit with a non-zero status.

## Internationalization

`inst_i18n::define_messages!` generates a message enum plus a
`text(Language) -> &'static str` method from a table with one column per
supported language, each column gated behind its own `lang-XX` Cargo
feature. Missing a translation for a compiled-in language is a compile
error, not a runtime fallback; a *disabled* language's column simply
isn't compiled and looking it up falls back to English. There are two
independent message sets:

- `inst_i18n::installer::Msg` — strings a generated installer can show.
  Only the languages a given project actually enables need to be compiled
  into that project's generated crate.
- `inst_i18n::studio::Msg` — the Studio's own chrome. The Studio always
  compiles all seven supported languages (`en`, `tr`, `ar`, `es`, `fr`,
  `de`, `ru`), independent of what any open project targets.

Right-to-left languages (Arabic) get real RTL layout from GPUI's text
system, not a mirrored stylesheet hack.

## Crate dependency shape

```
inst-brand, inst-wire, inst-fsx, inst-log     (leaves; no internal deps)
        ^
inst-i18n, inst-payload, inst-fetch            (depend only on the leaves)
        ^
inst-model                                     (the typed project + graph)
        ^
inst-catalog  ---------------+
        ^                    |
inst-runtime  <---------------+   (prerequisite facts come from the catalog)
        ^
inst-runtime-ui                                (optional GUI for the runtime)
        ^
inst-analyzer, inst-doctor, inst-codegen       (all read inst-model)
        ^
inst-builder                                    (codegen + cargo + payload + sign)
        ^
inst-cli, inst-studio                           (front ends over the builder)
```

No arrow points backward: `inst-runtime` cannot see `inst-builder`,
`inst-studio` cannot be linked into a generated installer.
