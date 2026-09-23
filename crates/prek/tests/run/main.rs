#[path = "../common/mod.rs"]
mod common;

#[cfg(unix)]
mod completion;
mod config;
mod execution;
mod files;
mod git;
mod include_deleted;
mod modifications;
mod output;
mod repositories;
mod scheduling;
mod selection;
mod skipped_hooks;
