#![warn(dead_code)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::missing_panics_doc)]
#![warn(clippy::must_use_candidate)]
#![warn(clippy::module_name_repetitions)]
#![warn(clippy::too_many_arguments)]

mod gem;
mod installer;
#[allow(clippy::module_inception)]
mod ruby;
mod version;

pub(crate) use ruby::Ruby;
pub(crate) use version::RubyRequest;
