//! Run/Debug configurations, launch classpaths and Maven commands - shared on-disk
//! formats with java-debug-model so nvim and the Rust IDE see the same configs.
pub mod classpath;
pub mod compile;
pub mod config;
pub mod maven;
pub mod test;
pub mod test_profiles;
