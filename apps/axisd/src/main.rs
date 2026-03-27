mod agent_runtime;
mod gui_launcher;
mod persistence;
mod pty_host;
mod registry;
mod request_handler;
mod transcript_store;

use axis_core::paths::{axis_user_data_dir, daemon_socket_path};
use std::process::ExitCode;

fn main() -> ExitCode {
    match request_handler::run_forever(daemon_socket_path(), axis_user_data_dir()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}
