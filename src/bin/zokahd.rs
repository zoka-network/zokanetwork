use std::process::{Command, ExitCode};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn print_usage() {
    println!("zokahd (automatic fullnode)");
    println!(
        "Usage: zokahd [--address <wallet-address>] [--password <pwd>] [--seed <multiaddr>] [--network-config <file>] [--no-network-config] [--allow-clearnet-seeds] [--seconds <n>] [--show-keys] [--no-mine] [--db <path>] [--genesis <file>] [--inline]"
    );
    println!("Runs fullnode + sync warmup + auto-mining by default until interrupted.");
    println!("On Windows it opens in its own console window by default. Use --inline to keep it in the current terminal.");
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| arg == "--help" || arg == "-h" || arg == "help")
    {
        print_usage();
        return ExitCode::SUCCESS;
    }
    let inline = if let Some(index) = args.iter().position(|arg| arg == "--inline") {
        args.remove(index);
        true
    } else {
        false
    };

    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to locate current executable path: {err}");
            return ExitCode::FAILURE;
        }
    };

    let zoka_exe_name = if cfg!(windows) { "zoka.exe" } else { "zoka" };
    let zoka_exe = current_exe.with_file_name(zoka_exe_name);
    if !zoka_exe.exists() {
        eprintln!(
            "required sibling binary '{}' was not found next to zokahd",
            zoka_exe_name
        );
        return ExitCode::FAILURE;
    }

    #[cfg(windows)]
    if !inline {
        let mut command = Command::new(&zoka_exe);
        command
            .arg("fullnode")
            .args(&args)
            .creation_flags(CREATE_NEW_CONSOLE);
        match command.spawn() {
            Ok(_) => return ExitCode::SUCCESS,
            Err(err) => {
                eprintln!(
                    "failed to launch '{}' in a new console window: {}",
                    zoka_exe.display(),
                    err
                );
                return ExitCode::FAILURE;
            }
        }
    }

    let status = match Command::new(&zoka_exe).arg("fullnode").args(&args).status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("failed to launch '{}': {}", zoka_exe.display(), err);
            return ExitCode::FAILURE;
        }
    };

    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
