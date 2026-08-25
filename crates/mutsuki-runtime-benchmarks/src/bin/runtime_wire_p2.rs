// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::map_unwrap_or
)]

#[allow(dead_code)]
#[path = "../allocator.rs"]
mod allocator;
#[path = "../environment.rs"]
mod environment;
#[allow(dead_code)]
#[path = "../report.rs"]
mod report;
#[path = "../wire_p2/mod.rs"]
mod wire_p2;
#[path = "../wire_report.rs"]
mod wire_report;

use std::process::ExitCode;

use allocator::TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

fn main() -> ExitCode {
    match wire_p2::run(&ALLOCATOR) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtime wire P2 benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}
