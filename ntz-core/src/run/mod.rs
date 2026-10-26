//! Run configurations: what the Run tool window can launch, and the processes it launches.
//!
//! A configuration is a command line run by the system shell from a working directory.
//! Custom configurations are saved in the project (`.ntz/run.json`); others are detected
//! from the project's build files and regenerated each time.

pub mod ansi;
mod config;
mod detect;
pub(crate) mod job;
pub(crate) mod process;

pub use config::{RunConfig, config_file, load, save};
pub use detect::detect;
pub use process::{OutputLine, RunningProcess, Stream};
