use std::path::PathBuf;
use std::process::ExitCode;

use chudtendo::{RunMode, run};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let mut run_mode = RunMode::Interactive;
    let mut rom_path: Option<PathBuf> = None;
    let mut dmg_mode = false;

    for argument in std::env::args().skip(1) {
        if argument == "--smoke-test" {
            run_mode = RunMode::SmokeTest;
            continue;
        }

        if argument == "--dmg" {
            dmg_mode = true;
            continue;
        }

        if rom_path.is_none() {
            rom_path = Some(PathBuf::from(argument));
            continue;
        }

        eprintln!("unsupported argument: {argument}");
        eprintln!("usage: cargo run -- [--smoke-test] [--dmg] [rom-path]");
        return ExitCode::FAILURE;
    }

    if let Err(error) = run(run_mode, rom_path.as_deref(), dmg_mode) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
