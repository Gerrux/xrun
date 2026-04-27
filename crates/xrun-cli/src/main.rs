#![deny(unsafe_code)]

use clap::Parser;

#[derive(Parser)]
#[command(name = "xrun", version = "0.1.0", about = "ML experiment runner")]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
