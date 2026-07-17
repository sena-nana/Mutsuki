#[allow(dead_code)]
#[path = "../allocator.rs"]
mod allocator;
#[path = "../environment.rs"]
mod environment;
#[allow(dead_code)]
#[path = "../report.rs"]
mod report;
#[path = "../wire/mod.rs"]
mod wire;
#[path = "../wire_report.rs"]
mod wire_report;

use std::process::ExitCode;

use allocator::TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

fn main() -> ExitCode {
    match wire::run(&ALLOCATOR) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtime wire benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}
