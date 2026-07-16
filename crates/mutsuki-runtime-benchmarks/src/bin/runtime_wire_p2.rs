#[allow(dead_code)]
#[path = "../allocator.rs"]
mod allocator;
#[allow(dead_code)]
#[path = "../report.rs"]
mod report;
#[path = "../wire_p2/mod.rs"]
mod wire_p2;

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
