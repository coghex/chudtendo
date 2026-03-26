//! Agent control client: sends a command to the agent server and prints the response.
//!
//! Usage: agent-ctl <command> [args...]
//! Examples:
//!   agent-ctl screenshot
//!   agent-ctl press A
//!   agent-ctl hold right 2
//!   agent-ctl state
//!   agent-ctl status
//!   agent-ctl frames 60
//!   agent-ctl save 1
//!   agent-ctl load 1

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

const ADDR: &str = "127.0.0.1:31337";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: agent-ctl <command> [args...]");
        eprintln!("commands: screenshot, state, status, press, hold, release, frames, save, load, help");
        std::process::exit(1);
    }

    let cmd = args.join(" ");

    let mut stream = TcpStream::connect(ADDR).unwrap_or_else(|e| {
        eprintln!("failed to connect to {ADDR}: {e}");
        eprintln!("is agent-server running?");
        std::process::exit(1);
    });

    stream.write_all(cmd.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();

    // Shut down the write half so the server knows we're done sending.
    stream.shutdown(std::net::Shutdown::Write).unwrap();

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        match line {
            Ok(line) => println!("{line}"),
            Err(_) => break,
        }
    }
}
