// SPDX-License-Identifier: MPL-2.0

use std::sync::LazyLock;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ManagerId {
    Flatpak,
    Snap,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Emerge,
    NixEnv,
    Brew,
    Apt,
}

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub program: &'static str,
    pub args: &'static [&'static str],
    pub requires_privilege: bool,
}

#[derive(Clone, Debug)]
pub struct ManagerSpec {
    pub label: &'static str,
    pub command_name: &'static str,
    pub requires_privilege: bool,
    pub commands: &'static [CommandSpec],
    pub check_commands: &'static [CommandSpec],
}

#[derive(Clone, Debug)]
pub enum TaskStatus {
    Idle,
    Running,
    Success,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct UpdateOutcome {
    pub output: String,
    pub status: TaskStatus,
}

const FLATPAK_COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        program: "flatpak",
        args: &["update", "-y"],
        requires_privilege: false,
    },
    CommandSpec {
        program: "flatpak",
        args: &["update", "--system", "-y"],
        requires_privilege: true,
    },
];
const FLATPAK_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "flatpak",
    args: &["remote-ls", "--updates"],
    requires_privilege: false,
}];

const SNAP_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "snap",
    args: &["refresh"],
    requires_privilege: true,
}];
const SNAP_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "snap",
    args: &["refresh", "--list"],
    requires_privilege: false,
}];

const DNF_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "dnf",
    args: &["update", "-y"],
    requires_privilege: true,
}];
const DNF_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "dnf",
    args: &["check-update"],
    requires_privilege: false,
}];

const YUM_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "yum",
    args: &["update", "-y"],
    requires_privilege: true,
}];
const YUM_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "yum",
    args: &["check-update"],
    requires_privilege: false,
}];

const PACMAN_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "pacman",
    args: &["-Syu", "--noconfirm"],
    requires_privilege: true,
}];
const PACMAN_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "pacman",
    args: &["-Qu"],
    requires_privilege: false,
}];

const ZYPPER_COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        program: "zypper",
        args: &["refresh"],
        requires_privilege: true,
    },
    CommandSpec {
        program: "zypper",
        args: &["update", "-y"],
        requires_privilege: true,
    },
];
const ZYPPER_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "zypper",
    args: &["list-updates"],
    requires_privilege: false,
}];

const EMERGE_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "emerge",
    args: &["--update", "--deep", "@world"],
    requires_privilege: true,
}];
const EMERGE_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "emerge",
    args: &["--pretend", "--update", "--deep", "@world"],
    requires_privilege: false,
}];

const NIX_ENV_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "nix-env",
    args: &["-u", "*"],
    requires_privilege: false,
}];
const NIX_ENV_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "nix-env",
    args: &["-u", "*", "--dry-run"],
    requires_privilege: false,
}];

const BREW_COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        program: "brew",
        args: &["update"],
        requires_privilege: false,
    },
    CommandSpec {
        program: "brew",
        args: &["upgrade"],
        requires_privilege: false,
    },
];
const BREW_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "brew",
    args: &["outdated"],
    requires_privilege: false,
}];

const APT_COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        program: "apt-get",
        args: &["update"],
        requires_privilege: true,
    },
    CommandSpec {
        program: "apt-get",
        args: &["upgrade", "-y"],
        requires_privilege: true,
    },
];
const APT_CHECK_COMMANDS: &[CommandSpec] = &[CommandSpec {
    program: "apt-get",
    args: &["-s", "upgrade"],
    requires_privilege: false,
}];

impl ManagerId {
    pub fn all() -> &'static [ManagerId] {
        &[
            ManagerId::Flatpak,
            ManagerId::Snap,
            ManagerId::Dnf,
            ManagerId::Yum,
            ManagerId::Pacman,
            ManagerId::Zypper,
            ManagerId::Emerge,
            ManagerId::NixEnv,
            ManagerId::Brew,
            ManagerId::Apt,
        ]
    }

    pub fn spec(self) -> ManagerSpec {
        match self {
            ManagerId::Flatpak => ManagerSpec {
                label: "Flatpak",
                command_name: "flatpak",
                requires_privilege: true,
                commands: FLATPAK_COMMANDS,
                check_commands: FLATPAK_CHECK_COMMANDS,
            },
            ManagerId::Snap => ManagerSpec {
                label: "Snap",
                command_name: "snap",
                requires_privilege: true,
                commands: SNAP_COMMANDS,
                check_commands: SNAP_CHECK_COMMANDS,
            },
            ManagerId::Dnf => ManagerSpec {
                label: "DNF",
                command_name: "dnf",
                requires_privilege: true,
                commands: DNF_COMMANDS,
                check_commands: DNF_CHECK_COMMANDS,
            },
            ManagerId::Yum => ManagerSpec {
                label: "YUM",
                command_name: "yum",
                requires_privilege: true,
                commands: YUM_COMMANDS,
                check_commands: YUM_CHECK_COMMANDS,
            },
            ManagerId::Pacman => ManagerSpec {
                label: "Pacman",
                command_name: "pacman",
                requires_privilege: true,
                commands: PACMAN_COMMANDS,
                check_commands: PACMAN_CHECK_COMMANDS,
            },
            ManagerId::Zypper => ManagerSpec {
                label: "Zypper",
                command_name: "zypper",
                requires_privilege: true,
                commands: ZYPPER_COMMANDS,
                check_commands: ZYPPER_CHECK_COMMANDS,
            },
            ManagerId::Emerge => ManagerSpec {
                label: "Emerge",
                command_name: "emerge",
                requires_privilege: true,
                commands: EMERGE_COMMANDS,
                check_commands: EMERGE_CHECK_COMMANDS,
            },
            ManagerId::NixEnv => ManagerSpec {
                label: "Nix",
                command_name: "nix-env",
                requires_privilege: false,
                commands: NIX_ENV_COMMANDS,
                check_commands: NIX_ENV_CHECK_COMMANDS,
            },
            ManagerId::Brew => ManagerSpec {
                label: "Homebrew",
                command_name: "brew",
                requires_privilege: false,
                commands: BREW_COMMANDS,
                check_commands: BREW_CHECK_COMMANDS,
            },
            ManagerId::Apt => ManagerSpec {
                label: "APT",
                command_name: "apt-get",
                requires_privilege: true,
                commands: APT_COMMANDS,
                check_commands: APT_CHECK_COMMANDS,
            },
        }
    }
}

pub async fn detect_managers() -> Vec<(ManagerId, bool)> {
    let mut results = Vec::new();

    for id in ManagerId::all() {
        let spec = id.spec();
        let available = command_exists(spec.command_name).await;
        results.push((*id, available));
    }

    results
}

pub async fn run_manager(id: ManagerId) -> UpdateOutcome {
    let spec = id.spec();
    let mut output = String::new();

    for cmd in spec.commands {
        let line = if cmd.requires_privilege {
            format!("$ pkexec {} {}\n", cmd.program, cmd.args.join(" "))
        } else {
            format!("$ {} {}\n", cmd.program, cmd.args.join(" "))
        };
        output.push_str(&line);

        let run_result = if cmd.requires_privilege {
            run_privileged(cmd.program, cmd.args).await
        } else {
            run_unprivileged(cmd.program, cmd.args).await
        };

        match run_result {
            Ok((bytes, status_code)) => {
                output.push_str(&String::from_utf8_lossy(&bytes));
                if status_code != 0 {
                    let status = format!("Exited with status: {status_code}");
                    return UpdateOutcome {
                        output,
                        status: TaskStatus::Failed(status),
                    };
                }
            }
            Err(err) => {
                return UpdateOutcome {
                    output,
                    status: TaskStatus::Failed(err),
                };
            }
        }
    }

    UpdateOutcome {
        output,
        status: TaskStatus::Success,
    }
}

pub async fn check_manager(id: ManagerId, force_privileged: bool) -> CheckOutcome {
    let spec = id.spec();
    let mut output = String::new();
    let mut last_status = 0;

    for cmd in spec.check_commands {
        let line = format!("$ {} {}\n", cmd.program, cmd.args.join(" "));
        output.push_str(&line);

        let needs_privilege = cmd.requires_privilege || (force_privileged && spec.requires_privilege);
        match run_command(cmd.program, cmd.args, needs_privilege).await {
            Ok((bytes, status_code)) => {
                output.push_str(&String::from_utf8_lossy(&bytes));
                last_status = status_code;
            }
            Err(err) => {
                return CheckOutcome {
                    output,
                    has_updates: false,
                    error: Some(err),
                };
            }
        }
    }

    CheckOutcome {
        has_updates: has_updates(id, &output, last_status),
        output,
        error: None,
    }
}

#[derive(Clone, Debug)]
pub struct CheckOutcome {
    pub output: String,
    pub has_updates: bool,
    pub error: Option<String>,
}

pub async fn reboot_system() -> Result<(), String> {
    let (_, status_code) = run_privileged("systemctl", &["reboot"]).await?;
    if status_code == 0 {
        Ok(())
    } else {
        Err(format!("reboot failed with status {status_code}"))
    }
}

async fn command_exists(command_name: &str) -> bool {
    let check = Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {}", command_name))
        .output()
        .await;

    match check {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

async fn run_unprivileged(
    program: &str,
    args: &[&str],
) -> Result<(Vec<u8>, i32), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|err| err.to_string())?;
    let mut combined = output.stdout;
    combined.extend_from_slice(&output.stderr);
    let status_code = output.status.code().unwrap_or(1);
    Ok((combined, status_code))
}

async fn run_command(
    program: &str,
    args: &[&str],
    requires_privilege: bool,
) -> Result<(Vec<u8>, i32), String> {
    if requires_privilege {
        run_privileged(program, args).await
    } else {
        run_unprivileged(program, args).await
    }
}

async fn run_privileged(
    program: &str,
    args: &[&str],
) -> Result<(Vec<u8>, i32), String> {
    let mut helper_guard = HELPER.lock().await;
    if helper_guard.is_none() {
        *helper_guard = Some(PrivilegedHelper::spawn().await?);
    }

    let helper = helper_guard.as_mut().expect("helper just created");
    match helper.run(program, args).await {
        Ok(result) => Ok(result),
        Err(err) => {
            *helper_guard = None;
            Err(err)
        }
    }
}

fn has_updates(id: ManagerId, output: &str, status_code: i32) -> bool {
    match id {
        ManagerId::Apt => apt_has_updates(output),
        ManagerId::Flatpak => has_non_empty_payload(output),
        ManagerId::Snap => !output.contains("All snaps up to date."),
        ManagerId::Dnf | ManagerId::Yum => status_code == 100,
        ManagerId::Pacman => has_non_empty_payload(output),
        ManagerId::Zypper => output.lines().any(|line| line.contains("|")),
        ManagerId::Emerge => output.lines().any(|line| line.contains("ebuild")),
        ManagerId::NixEnv => output.lines().any(|line| line.contains("would be upgraded")),
        ManagerId::Brew => has_non_empty_payload(output),
    }
}

pub fn apt_has_updates(output: &str) -> bool {
    for line in output.lines() {
        if let Some(prefix) = line.split(',').next() {
            let mut parts = prefix.split_whitespace();
            if let (Some(number), Some(word)) = (parts.next(), parts.next()) {
                if word == "upgraded" {
                    if let Ok(count) = number.parse::<u32>() {
                        return count > 0;
                    }
                }
            }
        }
    }
    false
}

fn has_non_empty_payload(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !line.trim_start().starts_with('$'))
        .any(|line| !line.contains("No updates") && !line.contains("Nothing to do."))
}

struct PrivilegedHelper {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

static HELPER: LazyLock<Mutex<Option<PrivilegedHelper>>> =
    LazyLock::new(|| Mutex::new(None));

impl PrivilegedHelper {
    async fn spawn() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        let mut child = Command::new("pkexec")
            .arg(exe)
            .arg("--helper")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| err.to_string())?;

        let stdin = child.stdin.take().ok_or("missing helper stdin")?;
        let stdout = child.stdout.take().ok_or("missing helper stdout")?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    async fn run(&mut self, program: &str, args: &[&str]) -> Result<(Vec<u8>, i32), String> {
        let mut command_line = String::from("RUN ");
        command_line.push_str(program);
        for arg in args {
            command_line.push(' ');
            command_line.push_str(arg);
        }
        command_line.push('\n');

        self.stdin
            .write_all(command_line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        self.stdin.flush().await.map_err(|err| err.to_string())?;

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .await
            .map_err(|err| err.to_string())?;

        if !line.starts_with("OUT ") {
            return Err("invalid helper response".to_string());
        }

        let len: usize = line["OUT ".len()..]
            .trim()
            .parse()
            .map_err(|_| "invalid output length".to_string())?;

        let mut bytes = vec![0u8; len];
        self.stdout
            .read_exact(&mut bytes)
            .await
            .map_err(|err| err.to_string())?;

        let mut newline = [0u8; 1];
        self.stdout
            .read_exact(&mut newline)
            .await
            .map_err(|err| err.to_string())?;

        line.clear();
        self.stdout
            .read_line(&mut line)
            .await
            .map_err(|err| err.to_string())?;

        if !line.starts_with("STATUS ") {
            return Err("invalid helper status".to_string());
        }

        let status_code: i32 = line["STATUS ".len()..]
            .trim()
            .parse()
            .map_err(|_| "invalid status code".to_string())?;

        Ok((bytes, status_code))
    }
}
