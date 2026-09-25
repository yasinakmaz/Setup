//! Product naming.
//!
//! The commercial name of the product is not decided yet. Every user-visible
//! occurrence of the product name, and every on-disk location derived from it,
//! must come from this crate so that a rename is a one-file change.
//!
//! Crate names, module paths and file formats are intentionally brand-neutral.

#![no_std]

/// Display name of the developer application.
pub const STUDIO_NAME: &str = "Installer Studio";

/// Display name of the code that ships inside every generated installer.
pub const RUNTIME_NAME: &str = "Installer Runtime";

/// Lower-case identifier used for per-user directories of the Studio
/// (configuration, download cache, build cache).
pub const STUDIO_DIR_NAME: &str = "installer-studio";

/// Lower-case identifier used by generated installers for their per-user
/// bookkeeping (installed product registry on Linux, partial downloads).
pub const RUNTIME_DIR_NAME: &str = "installer-runtime";

/// File extension of project files (without the dot).
pub const PROJECT_EXTENSION: &str = "instproj";

/// Name of the CLI binary.
pub const CLI_NAME: &str = "installer-studio-cli";
