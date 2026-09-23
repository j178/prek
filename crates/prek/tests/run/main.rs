#[path = "../common/mod.rs"]
mod common;

#[cfg(unix)]
mod completion;
mod config;
mod execution;
mod files;
mod git;
mod output;
mod repositories;
mod scheduling;
mod selection;
