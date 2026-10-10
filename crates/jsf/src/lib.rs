//! JSF / Facelets support (port of java-debug-model's jsf/*.lua): EL parsing, the bean index,
//! jdtls-driven resolution of EL chains, and .xhtml usage search.
pub mod beans;
pub mod diagnostics;
pub mod el;
pub mod tags;
pub mod generate;
pub mod refactor;
pub mod resolve;
