use std::env;
use std::io::{self, Write};
use std::process;

use shared_memory::ShmemConf;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <shared-memory-name> <len>", args[0]);
        process::exit(2);
    }
    let name = &args[1];
    let len: usize = match args[2].parse() {
        Ok(len) => len,
        Err(error) => {
            eprintln!("invalid len: {error}");
            process::exit(2);
        }
    };
    let mapping = match ShmemConf::new().os_id(name).open() {
        Ok(mapping) => mapping,
        Err(error) => {
            eprintln!("open failed: {error}");
            process::exit(1);
        }
    };
    if len > mapping.len() {
        eprintln!("requested len {len} exceeds mapping len {}", mapping.len());
        process::exit(1);
    }
    // SAFETY: bytes are copied to stdout immediately and no borrowed slice escapes.
    let bytes = unsafe { &mapping.as_slice()[..len] };
    if let Err(error) = io::stdout().write_all(bytes) {
        eprintln!("stdout write failed: {error}");
        process::exit(1);
    }
}
