#[allow(dead_code)]
#[path = "../allocator.rs"]
mod allocator;
#[path = "../environment.rs"]
mod environment;
#[allow(dead_code)]
#[path = "../report.rs"]
mod report;
#[path = "../wire_p1/mod.rs"]
mod wire_p1;
#[path = "../wire_report.rs"]
mod wire_report;

use std::process::ExitCode;

use allocator::TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

fn main() -> ExitCode {
    match wire_p1::run(&ALLOCATOR) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtime wire P1 benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}
