# Installer Studio

A visual installer builder that produces small, fast, native installers for
Windows (`Product-Setup-x64.exe`) and Linux (`Product-Setup-x86_64.AppImage`)
from one typed project description.

The Studio and the code that runs on end-user machines are fully separate:

```
Typed project (.instproj, TOML)
        |
        v
 Installation Graph (crates/model)
        |
        v
 Static Rust codegen (crates/codegen)  --generates-->  a tiny crate
        |                                                     |
        v                                                     v
   cargo build (crates/builder)  -------------------->  Product-Setup-x64.exe
                                                          Product-Setup-x86_64.AppImage
```

The generated installer links only the runtime code its own project
actually uses (`crates/runtime`, feature-gated per capability), so a
minimal installer compiles to a genuinely small, dependency-free binary.
Nothing about the Studio itself — GPUI, the analyzer, the doctor, cargo
invocation — ships inside a generated installer.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the crates fit together and
[ROADMAP.md](ROADMAP.md) for what is and isn't implemented yet.

## Crates

| Crate | Purpose |
| --- | --- |
| `inst-brand` | Single source of truth for product naming. |
| `inst-model` | Typed project model, installation graph, policy, privilege analysis. |
| `inst-wire` | Compact, versioned binary encoding shared by payloads and manifests. |
| `inst-fsx` | Safe file-system primitives: validated relative paths, atomic writes, secure temp files. |
| `inst-log` | Structured logging with central secret redaction. |
| `inst-i18n` | Languages, text direction, locale detection, built-in installer and Studio strings (7 languages: en, tr, ar, es, fr, de, ru). |
| `inst-payload` | Installer payload format: solid/grouped-solid blocks, streaming extraction, integrity. |
| `inst-fetch` | Resumable, verified downloads and a content-addressed cache. |
| `inst-catalog` | Smart prerequisites catalog (runtimes, redistributables). |
| `inst-runtime` | The transactional engine that generated installers link and run. |
| `inst-analyzer` | Infers a starting project from an existing application folder. |
| `inst-doctor` | Project diagnostics with one-click safe fixes. |
| `inst-codegen` | Turns a validated project into a static Rust crate. |
| `inst-builder` | Packs the payload, invokes `cargo build`, signs, reports. |
| `inst-runtime-ui` | The single-screen GPUI frontend linked into generated installers. |
| `inst-studio` | Installer Studio: the GPUI desktop application. |
| `inst-cli` | Headless CLI (`new`, `analyze`, `doctor`, `build`, `catalog`, `cache`) for scripts and CI. |

## Building

```sh
cargo build --workspace
```

Windows cross-compilation targets `x86_64-pc-windows-gnu` (see
`rust-toolchain.toml`). GPUI needs a GPU-capable rendering surface (Vulkan
on Linux, Direct3D/Metal-equivalent on Windows); the two GUI binaries
(`installer-studio`, and generated installers built with the GUI feature)
print a clean error and exit non-zero instead of panicking when no such
surface is available — for example in a headless CI runner or a bare X
server.

## Trying it end-to-end

```sh
cargo run -p inst-cli -- build examples/hello/hello.instproj --target linux-x64 --no-appimage
```

This builds `examples/hello`, a tiny example application, into a
self-contained Linux installer executable. `examples/hello/hello.instproj`
is a realistic small project: a few UI fields (including a password field
that's redacted from logs), a post-install shell action, and a
Simple-mode Studio project.

## Verifying a change

Every change in this workspace is expected to pass, on both the native
target and `x86_64-pc-windows-gnu`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings
cargo test --workspace --all-features
```

## License

Unlicensed / all rights reserved (see `Cargo.toml`); this is a private
project, not yet ready for distribution.
