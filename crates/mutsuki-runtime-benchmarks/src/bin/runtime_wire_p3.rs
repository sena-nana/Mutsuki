#[allow(dead_code)]
#[path = "../allocator.rs"]
mod allocator;
#[allow(dead_code)]
#[path = "../report.rs"]
mod report;
#[path = "../wire_p3/mod.rs"]
mod wire_p3;

use std::process::ExitCode;

use allocator::TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

fn main() -> ExitCode {
    match wire_p3::run(&ALLOCATOR) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtime wire P3 benchmark failed: {error}");
            ExitCode::FAILURE
        }
    }
}
