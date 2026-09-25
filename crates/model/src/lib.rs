//! Typed project model.
//!
//! The model is the single source of truth shared by the Studio (Simple and
//! Advanced mode are two views of the same [`Project`]), the Doctor, the
//! code generator and the build system. It has no UI dependency and no
//! scripting language: everything an installer does is a typed value that
//! code generation turns into static Rust.

pub mod action;
pub mod condition;
pub mod features;
pub mod graph;
pub mod ids;
pub mod io;
pub mod platform;
pub mod policy;
pub mod privileges;
pub mod project;
pub mod text;
pub mod validate;
pub mod value;

pub use action::{Action, ActionKind, LifecycleEvent};
pub use condition::Condition;
pub use graph::{InstallGraph, Node, Step};
pub use ids::{ProductId, Version, VersionReq};
pub use platform::{Arch, Os, Target};
pub use project::Project;
pub use text::LocalizedText;
