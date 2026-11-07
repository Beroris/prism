//! Platform-neutral core of the screenshot tool: the annotation document, rendering, pixel
//! operations, file naming and settings. It makes no OS calls and starts no threads; hosts own
//! all of that.

pub mod color;
pub mod doc;
pub mod geom;
pub mod icon;
pub mod image;
pub mod naming;
pub mod render;
pub mod settings;

pub use color::Rgba;
pub use doc::{Doc, Shape, Style, Tool};
pub use geom::{FRect, IRect, Pt};
pub use image::Image;
