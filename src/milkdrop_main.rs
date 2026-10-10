//! The MilkDrop add-on, `lwspot-milkdrop.exe`: the projectM window LWSpot
//! starts when MilkDrop is turned on. It is built with the `milkdrop`
//! feature and kept out of `lwspot.exe`, which downloads it on first use.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use lwspot::milkdrop::{addon, args::Args, child};

fn main() {
    // The app runs this before it trusts the add-on: the answer has to be
    // its own version, or the add-on is replaced.
    if std::env::args_os().skip(1).any(|arg| arg == "--version") {
        println!("{}", addon::version_line(addon::VERSION));
        return;
    }
    match Args::parse() {
        Some(Ok(args)) => std::process::exit(child::run(args)),
        Some(Err(error)) => {
            eprintln!("lwspot-milkdrop: {error}");
            std::process::exit(2);
        }
        None => {
            eprintln!(
                "lwspot-milkdrop is LWSpot's MilkDrop window. Turn MilkDrop on in LWSpot to \
                 open it."
            );
            std::process::exit(2);
        }
    }
}
