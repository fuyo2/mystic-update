// SPDX-License-Identifier: MPL-2.0

use std::io::{self, BufRead, BufReader, Write};
use std::process::Command;

pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut lines = BufReader::new(stdin.lock()).lines();
    let mut stdout = io::stdout();

    while let Some(line) = lines.next() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "EXIT" {
            break;
        }

        let mut parts = trimmed.split_whitespace();
        let command = parts.next();
        if command != Some("RUN") {
            continue;
        }

        let program = match parts.next() {
            Some(program) => program,
            None => {
                send_response(&mut stdout, b"missing program", 1)?;
                continue;
            }
        };

        let args: Vec<&str> = parts.collect();
        let output = Command::new(program).args(&args).output();

        match output {
            Ok(result) => {
                let mut combined = result.stdout;
                combined.extend_from_slice(&result.stderr);
                let status_code = result.status.code().unwrap_or(1);
                send_response(&mut stdout, &combined, status_code)?;
            }
            Err(err) => {
                let message = format!("failed to run {program}: {err}");
                send_response(&mut stdout, message.as_bytes(), 1)?;
            }
        }
    }

    Ok(())
}

fn send_response(stdout: &mut io::Stdout, bytes: &[u8], status_code: i32) -> io::Result<()> {
    writeln!(stdout, "OUT {}", bytes.len())?;
    stdout.write_all(bytes)?;
    stdout.write_all(b"\n")?;
    writeln!(stdout, "STATUS {status_code}")?;
    stdout.flush()
}
