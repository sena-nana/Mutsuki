#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use std::env;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use std::io::{self, Read, Write};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use std::process;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use shared_memory::ShmemConf;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 3 && args[1] == "--own" {
        own_mapping_until_stdin_closes(args[2].as_bytes());
        return;
    }
    if args.len() != 3 {
        eprintln!(
            "usage: {} <shared-memory-name> <len> | --own <payload>",
            args[0]
        );
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

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("shared-memory mappings are unsupported on this platform");
    std::process::exit(1);
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn own_mapping_until_stdin_closes(bytes: &[u8]) {
    let name = format!("/mtk_child_{:x}", process::id());
    let mut mapping = match ShmemConf::new()
        .os_id(&name)
        .size(bytes.len().max(1))
        .create()
    {
        Ok(mapping) => mapping,
        Err(error) => {
            eprintln!("create failed: {error}");
            process::exit(1);
        }
    };
    if !bytes.is_empty() {
        // SAFETY: the child exclusively owns the new mapping and initializes it before publishing
        // the mapping name to the parent process.
        unsafe {
            mapping.as_slice_mut()[..bytes.len()].copy_from_slice(bytes);
        }
    }
    println!("{name} {}", bytes.len());
    if let Err(error) = io::stdout().flush() {
        eprintln!("stdout flush failed: {error}");
        process::exit(1);
    }
    let mut sink = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut sink) {
        eprintln!("stdin wait failed: {error}");
        process::exit(1);
    }
}
