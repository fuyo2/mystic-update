// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};

#[cfg(feature = "flatpak")]
use libflatpak::{gio::Cancellable, glib, prelude::*, Installation, Ref, Transaction};
#[cfg(feature = "flatpak")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "flatpak")]
use std::ptr;
#[cfg(feature = "flatpak")]
use std::rc::Rc;

#[cfg(feature = "packagekit")]
use packagekit_zbus::{
    PackageKit::PackageKitProxyBlocking,
    Transaction::TransactionProxyBlocking,
    zbus::blocking::Connection,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallScope {
    User,
    System,
}

#[derive(Clone, Debug)]
pub struct UpdateEntry {
    pub manager: ManagerId,
    pub id: String,
    pub name: String,
    pub current_version: Option<String>,
    pub new_version: Option<String>,
    pub scope: Option<InstallScope>,
}

pub type ProgressTracker = Arc<StdMutex<HashMap<ManagerId, f32>>>;

pub fn new_progress_tracker() -> ProgressTracker {
    Arc::new(StdMutex::new(HashMap::new()))
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
const FLATPAK_CHECK_COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        program: "flatpak",
        args: &[
            "list",
            "--columns=application,ref,version,installation",
        ],
        requires_privilege: false,
    },
    CommandSpec {
        program: "flatpak",
        args: &["remote-ls", "--updates", "--columns=application,ref,version,commit"],
        requires_privilege: false,
    },
];

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
    program: "apt",
    args: &["list", "--upgradable"],
    requires_privilege: false,
}];
const APT_LOCK_RETRY_LIMIT: usize = 12;
const APT_LOCK_RETRY_INITIAL_DELAY_SECS: u64 = 5;
const APT_LOCK_RETRY_MAX_DELAY_SECS: u64 = 30;
const APT_LOCK_ERROR_MESSAGE: &str = "APT is locked by another process. Please try again later.";

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

        let run_result =
            run_command_with_optional_apt_retry(cmd.program, cmd.args, cmd.requires_privilege)
                .await;

        match run_result {
            Ok(run) => {
                output.push_str(&String::from_utf8_lossy(&run.bytes));
                if run.apt_lock {
                    output.push_str(APT_LOCK_ERROR_MESSAGE);
                    output.push('\n');
                    return UpdateOutcome {
                        output,
                        status: TaskStatus::Failed(APT_LOCK_ERROR_MESSAGE.to_string()),
                    };
                }
                if run.status_code != 0 {
                    let status = format!("Exited with status: {}", run.status_code);
                    output.push_str(&status);
                    output.push('\n');
                    return UpdateOutcome {
                        output,
                        status: TaskStatus::Failed(status),
                    };
                }
            }
            Err(err) => {
                output.push_str(&err);
                if !err.ends_with('\n') {
                    output.push('\n');
                }
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

async fn run_dnf_yum_with_progress(
    id: ManagerId,
    entries: Option<Vec<UpdateEntry>>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    let spec = id.spec();
    let mut output = String::new();

    let Some(cmd) = spec.commands.first() else {
        return UpdateOutcome {
            output: "No update command configured.\n".to_string(),
            status: TaskStatus::Failed("Missing update command".to_string()),
        };
    };

    let mut args: Vec<String> = cmd.args.iter().map(|arg| (*arg).to_string()).collect();
    if let Some(entries) = entries {
        let mut package_names: Vec<String> = entries
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        package_names.sort();
        package_names.dedup();
        args.extend(package_names);
    }
    let mut program = cmd.program;
    if id == ManagerId::Yum && command_exists("stdbuf").await {
        let mut wrapped_args = Vec::with_capacity(args.len() + 3);
        wrapped_args.push("-o0".to_string());
        wrapped_args.push("-e0".to_string());
        wrapped_args.push(program.to_string());
        wrapped_args.extend(args);
        program = "stdbuf";
        args = wrapped_args;
    }
    let arg_refs: Vec<&str> = args.iter().map(|arg| arg.as_str()).collect();

    let line = if cmd.requires_privilege {
        format!("$ pkexec {} {}\n", program, arg_refs.join(" "))
    } else {
        format!("$ {} {}\n", program, arg_refs.join(" "))
    };
    output.push_str(&line);

    report_progress(&progress, id, 0.0);
    let mut parser = DnfYumProgress::new();
    let run_result = run_command_streaming(
        program,
        &arg_refs,
        cmd.requires_privilege,
        |line| {
            if let Some(value) = parser.update_from_line(line) {
                report_progress(&progress, id, value);
            }
        },
    )
    .await;
    clear_progress(&progress, id);

    match run_result {
        Ok(run) => {
            output.push_str(&String::from_utf8_lossy(&run.bytes));
            if run.apt_lock {
                output.push_str(APT_LOCK_ERROR_MESSAGE);
                output.push('\n');
                return UpdateOutcome {
                    output,
                    status: TaskStatus::Failed(APT_LOCK_ERROR_MESSAGE.to_string()),
                };
            }
            if run.status_code != 0 {
                let status = format!("Exited with status: {}", run.status_code);
                output.push_str(&status);
                output.push('\n');
                return UpdateOutcome {
                    output,
                    status: TaskStatus::Failed(status),
                };
            }
        }
        Err(err) => {
            output.push_str(&err);
            if !err.ends_with('\n') {
                output.push('\n');
            }
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    }

    UpdateOutcome {
        output,
        status: TaskStatus::Success,
    }
}

async fn run_command_streaming(
    program: &str,
    args: &[&str],
    requires_privilege: bool,
    mut on_line: impl FnMut(&str),
) -> Result<CommandRun, String> {
    let mut command = if requires_privilege {
        let mut command = Command::new("pkexec");
        command.arg(program);
        command.args(args);
        command
    } else {
        let mut command = Command::new(program);
        command.args(args);
        command
    };

    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let stdout_tx = tx.clone();
    let stdout_task = tokio::spawn(async move { forward_stream(stdout, stdout_tx).await });

    let stderr_tx = tx.clone();
    let stderr_task = tokio::spawn(async move { forward_stream(stderr, stderr_tx).await });

    drop(tx);

    let mut output = Vec::new();
    let mut pending = String::new();
    while let Some(chunk) = rx.recv().await {
        output.extend_from_slice(&chunk);
        let text = String::from_utf8_lossy(&chunk);
        pending.push_str(&text);
        let mut start = 0usize;
        for (idx, ch) in pending.char_indices() {
            if ch == '\n' || ch == '\r' {
                let segment = pending[start..idx].trim();
                if !segment.is_empty() {
                    on_line(segment);
                }
                start = idx + ch.len_utf8();
            }
        }
        if start > 0 {
            pending = pending[start..].to_string();
        }
    }
    let trailing = pending.trim();
    if !trailing.is_empty() {
        on_line(trailing);
    }

    let status = child.wait().await.map_err(|err| err.to_string())?;
    let status_code = status.code().unwrap_or(1);

    let stdout_result = stdout_task.await.map_err(|err| err.to_string())?;
    if let Err(err) = stdout_result {
        return Err(err);
    }
    let stderr_result = stderr_task.await.map_err(|err| err.to_string())?;
    if let Err(err) = stderr_result {
        return Err(err);
    }

    Ok(CommandRun {
        bytes: output,
        status_code,
        apt_lock: false,
    })
}

async fn forward_stream<R>(
    reader: R,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        if bytes_read == 0 {
            break;
        }
        if tx.send(buffer[..bytes_read].to_vec()).is_err() {
            break;
        }
    }
    Ok(())
}

fn report_progress(
    progress: &Option<ProgressTracker>,
    id: ManagerId,
    value: f32,
) {
    let Some(progress) = progress else {
        return;
    };
    if let Ok(mut map) = progress.lock() {
        map.insert(id, value.clamp(0.0, 100.0));
    }
}

fn clear_progress(progress: &Option<ProgressTracker>, id: ManagerId) {
    let Some(progress) = progress else {
        return;
    };
    if let Ok(mut map) = progress.lock() {
        map.remove(&id);
    }
}

pub async fn run_manager_with_progress(
    id: ManagerId,
    entries: Option<Vec<UpdateEntry>>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    match id {
        ManagerId::Apt => {
            #[cfg(feature = "packagekit")]
            {
                return run_apt_packagekit(entries, progress).await;
            }
            #[cfg(not(feature = "packagekit"))]
            {
                return match entries {
                    Some(_) => run_manager(id).await,
                    None => run_manager(id).await,
                };
            }
        }
        ManagerId::Flatpak => {
            #[cfg(feature = "flatpak")]
            {
                if let Some(entries) = entries {
                    return run_flatpak_libflatpak(entries, progress).await;
                }
            }
            return match entries {
                Some(entries) => run_flatpak_selected(&entries).await,
                None => run_manager(id).await,
            };
        }
        ManagerId::Dnf | ManagerId::Yum => {
            return run_dnf_yum_with_progress(id, entries, progress).await;
        }
        _ => run_manager(id).await,
    }
}

pub async fn check_manager(id: ManagerId, force_privileged: bool) -> CheckOutcome {
    let spec = id.spec();
    let mut output = String::new();
    let mut last_status = 0;
    let force_privileged = if id == ManagerId::Apt && cfg!(feature = "packagekit") {
        false
    } else {
        force_privileged
    };

    for cmd in spec.check_commands {
        let line = format!("$ {} {}\n", cmd.program, cmd.args.join(" "));
        output.push_str(&line);

        let needs_privilege = cmd.requires_privilege
            || (force_privileged
                && spec.requires_privilege
                && id != ManagerId::Flatpak);
        match run_command_with_optional_apt_retry(cmd.program, cmd.args, needs_privilege).await {
            Ok(run) => {
                output.push_str(&String::from_utf8_lossy(&run.bytes));
                last_status = run.status_code;
                if run.apt_lock {
                    return CheckOutcome {
                        output,
                        has_updates: false,
                        error: Some(APT_LOCK_ERROR_MESSAGE.to_string()),
                    };
                }
                if id == ManagerId::Apt && run.status_code != 0 {
                    let err = format!("APT check failed with status {}", run.status_code);
                    return CheckOutcome {
                        output,
                        has_updates: false,
                        error: Some(err),
                    };
                }
            }
            Err(err) => {
                output.push_str(&err);
                if !err.ends_with('\n') {
                    output.push('\n');
                }
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

struct CommandRun {
    bytes: Vec<u8>,
    status_code: i32,
    apt_lock: bool,
}

async fn run_command_with_optional_apt_retry(
    program: &str,
    args: &[&str],
    requires_privilege: bool,
) -> Result<CommandRun, String> {
    if is_apt_program(program) {
        run_command_with_apt_lock_retry(program, args, requires_privilege).await
    } else {
        let (bytes, status_code) = run_command(program, args, requires_privilege).await?;
        Ok(CommandRun {
            bytes,
            status_code,
            apt_lock: false,
        })
    }
}

async fn run_command_with_apt_lock_retry(
    program: &str,
    args: &[&str],
    requires_privilege: bool,
) -> Result<CommandRun, String> {
    let mut combined = Vec::new();
    let mut delay_secs = APT_LOCK_RETRY_INITIAL_DELAY_SECS;
    let mut retries = 0usize;

    loop {
        let (bytes, status_code) = run_command(program, args, requires_privilege).await?;
        combined.extend_from_slice(&bytes);

        let text = String::from_utf8_lossy(&bytes);
        if is_apt_lock_error(&text) {
            if retries < APT_LOCK_RETRY_LIMIT {
                let note = format!(
                    "\n[mystic-update] APT lock detected; retrying in {}s (attempt {}/{})...\n",
                    delay_secs,
                    retries + 1,
                    APT_LOCK_RETRY_LIMIT
                );
                combined.extend_from_slice(note.as_bytes());
                sleep(Duration::from_secs(delay_secs)).await;
                retries += 1;
                delay_secs = std::cmp::min(
                    delay_secs.saturating_mul(2),
                    APT_LOCK_RETRY_MAX_DELAY_SECS,
                );
                continue;
            }

            let total_attempts = retries + 1;
            let note = format!(
                "\n[mystic-update] APT lock still held after {total_attempts} attempts; giving up.\n"
            );
            combined.extend_from_slice(note.as_bytes());
            return Ok(CommandRun {
                bytes: combined,
                status_code,
                apt_lock: true,
            });
        }

        return Ok(CommandRun {
            bytes: combined,
            status_code,
            apt_lock: false,
        });
    }
}

fn is_apt_program(program: &str) -> bool {
    matches!(program, "apt" | "apt-get")
}

fn is_apt_lock_error(output: &str) -> bool {
    let lower = output.to_lowercase();
    if lower.contains("permission denied") {
        return false;
    }

    [
        "could not get lock",
        "unable to acquire the dpkg frontend lock",
        "unable to lock the administration directory",
        "is another process using it",
        "is held by process",
        "waiting for cache lock",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

async fn run_privileged(
    program: &str,
    args: &[&str],
) -> Result<(Vec<u8>, i32), String> {
    let mut last_err: Option<String> = None;

    if !HELPER_DISABLED.load(Ordering::Relaxed) {
        for _attempt in 0..2 {
            let mut helper_guard = HELPER.lock().await;
            if helper_guard.is_none() {
                *helper_guard = Some(PrivilegedHelper::spawn().await?);
            }

            let helper = helper_guard.as_mut().expect("helper just created");
            match helper.run(program, args).await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    *helper_guard = None;
                    last_err = Some(err);
                }
            }
        }

        HELPER_DISABLED.store(true, Ordering::Relaxed);
    }

    let mut shell_guard = SHELL_HELPER.lock().await;
    if shell_guard.is_none() {
        *shell_guard = Some(ShellHelper::spawn().await?);
    }

    let shell = shell_guard.as_mut().expect("shell helper just created");
    match shell.run(program, args).await {
        Ok(result) => Ok(result),
        Err(err) => {
            *shell_guard = None;
            if let Some(helper_err) = last_err {
                Err(format!("{helper_err}\n{err}"))
            } else {
                Err(err)
            }
        }
    }
}

fn has_updates(id: ManagerId, output: &str, status_code: i32) -> bool {
    match id {
        ManagerId::Apt => status_code == 0 && !parse_apt_updates(output).is_empty(),
        ManagerId::Flatpak => status_code == 0 && !parse_flatpak_updates(output).is_empty(),
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

pub fn parse_updates(id: ManagerId, output: &str) -> Vec<UpdateEntry> {
    match id {
        ManagerId::Apt => parse_apt_updates(output),
        ManagerId::Flatpak => parse_flatpak_updates(output),
        ManagerId::Dnf | ManagerId::Yum => parse_dnf_yum_updates(id, output),
        _ => Vec::new(),
    }
}

fn parse_apt_updates(output: &str) -> Vec<UpdateEntry> {
    let mut updates = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('$')
            || line.starts_with("Listing")
            || line.starts_with("WARNING")
        {
            continue;
        }

        let mut parts = line.split_whitespace();
        let package_part = match parts.next() {
            Some(part) => part,
            None => continue,
        };
        let new_version = parts.next().map(|version| version.to_string());

        let name = package_part
            .split('/')
            .next()
            .unwrap_or(package_part)
            .to_string();

        let current_version = line
            .split("[upgradable from:")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map(|version| version.trim().to_string());

        updates.push(UpdateEntry {
            manager: ManagerId::Apt,
            id: name.clone(),
            name,
            current_version,
            new_version,
            scope: None,
        });
    }

    updates
}

fn parse_dnf_yum_updates(id: ManagerId, output: &str) -> Vec<UpdateEntry> {
    let mut updates = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('$')
            || line.starts_with("Last metadata")
            || line.starts_with("Updating and loading repositories")
            || line.starts_with("Repositories loaded")
            || line.starts_with("Loaded plugins")
            || line.starts_with("Loading mirror speeds")
            || line.starts_with("Obsoleting Packages")
            || line.starts_with("Obsoleting packages")
            || line.starts_with("Security:")
            || line.starts_with("Error:")
            || line.starts_with("==")
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let package = parts[0];
        let new_version = parts[1];

        let (display_name, id_value) = match split_rpm_name_arch(package) {
            Some((name, _arch)) => (name.to_string(), package.to_string()),
            None => continue,
        };

        updates.push(UpdateEntry {
            manager: id,
            id: id_value,
            name: display_name,
            current_version: None,
            new_version: Some(new_version.to_string()),
            scope: None,
        });
    }

    updates
}

struct DnfYumProgress {
    download_current: Option<u32>,
    download_total: Option<u32>,
    transaction_current: Option<u32>,
    transaction_total: Option<u32>,
    last_progress: f32,
}

impl DnfYumProgress {
    fn new() -> Self {
        Self {
            download_current: None,
            download_total: None,
            transaction_current: None,
            transaction_total: None,
            last_progress: 0.0,
        }
    }

    fn update_from_line(&mut self, line: &str) -> Option<f32> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let (current, total) = parse_ratio_near_slash(trimmed)?;
        if total == 0 {
            return None;
        }

        let current = current.min(total);
        if is_download_ratio_line(trimmed) {
            self.download_current = Some(current);
            self.download_total = Some(total);
        } else {
            self.transaction_current = Some(current);
            self.transaction_total = Some(total);
        }

        let progress = self.compute_progress()?;
        let progress = progress.max(self.last_progress).clamp(0.0, 100.0);
        if progress > self.last_progress {
            self.last_progress = progress;
            Some(progress)
        } else {
            None
        }
    }

    fn compute_progress(&self) -> Option<f32> {
        if let (Some(current), Some(total)) =
            (self.transaction_current, self.transaction_total)
        {
            let base = if self.download_total.is_some() { 60.0 } else { 0.0 };
            let span = if self.download_total.is_some() { 40.0 } else { 100.0 };
            return Some(base + span * (current as f32 / total as f32));
        }

        if let (Some(current), Some(total)) = (self.download_current, self.download_total) {
            return Some(60.0 * (current as f32 / total as f32));
        }

        None
    }
}

fn parse_ratio_near_slash(text: &str) -> Option<(u32, u32)> {
    let slash = text.find('/')?;
    let left = extract_digits_left(&text[..slash])?;
    let right = extract_digits_right(&text[slash + 1..])?;
    let left_value = left.parse().ok()?;
    let right_value = right.parse().ok()?;
    Some((left_value, right_value))
}

fn extract_digits_left(text: &str) -> Option<String> {
    let mut digits = String::new();
    for ch in text.chars().rev() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    if digits.is_empty() {
        None
    } else {
        Some(digits.chars().rev().collect())
    }
}

fn extract_digits_right(text: &str) -> Option<String> {
    let mut digits = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn is_download_ratio_line(line: &str) -> bool {
    line.trim_start().starts_with('(')
}

fn parse_flatpak_updates(output: &str) -> Vec<UpdateEntry> {
    let mut updates = Vec::new();
    let mut current_versions: HashMap<String, FlatpakInstalledInfo> = HashMap::new();
    let mut mode = FlatpakParseMode::None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Application")
            || line.starts_with("Name")
            || line.starts_with("error:")
            || line.starts_with("Error:")
        {
            continue;
        }

        if line.starts_with('$') {
            if line.contains("flatpak list") {
                mode = FlatpakParseMode::List;
            } else if line.contains("flatpak remote-ls") {
                mode = FlatpakParseMode::Updates;
            } else {
                mode = FlatpakParseMode::None;
            }
            continue;
        }

        let parts: Vec<&str> = if line.contains('\t') {
            line.split('\t').collect()
        } else {
            line.split_whitespace().collect()
        };

        if parts.len() < 3 {
            continue;
        }

        let application = parts[0].to_string();
        let ref_id = parts[1].to_string();
        let version = parts[2].to_string();

        match mode {
            FlatpakParseMode::List => {
                let commit = if parts.len() >= 5 {
                    parts.get(3).and_then(|value| normalize_flatpak_value(value))
                } else {
                    None
                };
                let scope = if parts.len() >= 5 {
                    parts.get(4).and_then(|value| parse_install_scope(value))
                } else {
                    parts.get(3).and_then(|value| parse_install_scope(value))
                };
                let info = FlatpakInstalledInfo {
                    version: version.clone(),
                    commit,
                    scope,
                };
                if let Some(app_id) = flatpak_app_id_from_ref(&ref_id) {
                    current_versions.insert(app_id, info.clone());
                }
                current_versions.insert(ref_id.clone(), info.clone());
                current_versions.insert(application, info);
            }
            FlatpakParseMode::Updates => {
                let new_commit = parts
                    .get(3)
                    .and_then(|value| normalize_flatpak_value(value));
                let app_id = flatpak_app_id_from_ref(&ref_id);
                let current_info = current_versions
                    .get(&ref_id)
                    .or_else(|| current_versions.get(&application))
                    .or_else(|| app_id.as_ref().and_then(|id| current_versions.get(id)));
                let mut current_version = current_info.map(|info| info.version.clone());
                let scope = current_info.and_then(|info| info.scope);
                let mut new_version = Some(version);

                if let (Some(current), Some(new)) = (current_version.clone(), new_version.clone()) {
                    if current == new {
                        if let (Some(current_commit), Some(new_commit)) =
                            (current_info.and_then(|info| info.commit.clone()), new_commit)
                        {
                            if current_commit != new_commit {
                                let current_suffix = short_commit(&current_commit);
                                let new_suffix = short_commit(&new_commit);
                                current_version = Some(format!("{current} ({current_suffix})"));
                                new_version = Some(format!("{new} ({new_suffix})"));
                            }
                        }
                    }
                }

                updates.push(UpdateEntry {
                    manager: ManagerId::Flatpak,
                    id: ref_id,
                    name: application,
                    current_version,
                    new_version,
                    scope,
                });
            }
            FlatpakParseMode::None => {}
        }
    }

    updates
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlatpakParseMode {
    None,
    List,
    Updates,
}

#[derive(Clone, Debug)]
struct FlatpakInstalledInfo {
    version: String,
    commit: Option<String>,
    scope: Option<InstallScope>,
}

fn normalize_flatpak_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn short_commit(commit: &str) -> String {
    let mut short = String::new();
    for (index, ch) in commit.chars().enumerate() {
        if index >= 12 {
            break;
        }
        short.push(ch);
    }
    short
}

fn split_rpm_name_arch(value: &str) -> Option<(&str, &str)> {
    let (name, arch) = value.rsplit_once('.')?;
    if is_rpm_arch(arch) {
        Some((name, arch))
    } else {
        None
    }
}

fn is_rpm_arch(value: &str) -> bool {
    matches!(
        value,
        "noarch"
            | "x86_64"
            | "i686"
            | "i586"
            | "i386"
            | "aarch64"
            | "armv7hl"
            | "armv7hnl"
            | "armv7h"
            | "armv6hl"
            | "ppc64le"
            | "ppc64"
            | "s390x"
            | "riscv64"
    )
}

fn flatpak_app_id_from_ref(ref_id: &str) -> Option<String> {
    let mut parts = ref_id.split('/');
    let kind = parts.next()?;
    if kind != "app" {
        return None;
    }
    let app_id = parts.next()?;
    Some(app_id.to_string())
}

fn parse_install_scope(value: &str) -> Option<InstallScope> {
    match value {
        "user" => Some(InstallScope::User),
        "system" => Some(InstallScope::System),
        _ => None,
    }
}

fn has_non_empty_payload(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !line.trim_start().starts_with('$'))
        .any(|line| !line.contains("No updates") && !line.contains("Nothing to do."))
}

#[cfg(feature = "packagekit")]
async fn run_apt_packagekit(
    entries: Option<Vec<UpdateEntry>>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    let package_names = entries.map(|entries| {
        let mut names: Vec<String> = entries
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        names.sort();
        names.dedup();
        names
    });
    let progress_clone = progress.clone();
    let result = tokio::task::spawn_blocking(move || {
        packagekit_update_blocking(package_names, progress_clone)
    })
    .await;

    match result {
        Ok(outcome) => outcome,
        Err(err) => UpdateOutcome {
            output: format!("PackageKit update task failed: {err}"),
            status: TaskStatus::Failed(err.to_string()),
        },
    }
}

#[cfg(feature = "packagekit")]
fn packagekit_update_blocking(
    package_names: Option<Vec<String>>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    let mut output = String::new();
    output.push_str("$ packagekit update\n");

    let mut client = match PackageKitClient::new() {
        Ok(client) => client,
        Err(err) => {
            output.push_str(&err);
            output.push('\n');
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    };

    let update_ids = match client.collect_update_ids(package_names.as_ref()) {
        Ok(ids) => ids,
        Err(err) => {
            output.push_str(&err);
            output.push('\n');
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    };

    if update_ids.is_empty() {
        output.push_str("No updates available.\n");
        clear_progress(&progress, ManagerId::Apt);
        return UpdateOutcome {
            output,
            status: TaskStatus::Success,
        };
    }

    report_progress(&progress, ManagerId::Apt, 0.0);

    let update_result = client.run_update(&update_ids, |percent| {
        report_progress(&progress, ManagerId::Apt, percent as f32);
    });

    match update_result {
        Ok(()) => {
            report_progress(&progress, ManagerId::Apt, 100.0);
            output.push_str(&format!(
                "{} upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n",
                update_ids.len()
            ));
            UpdateOutcome {
                output,
                status: TaskStatus::Success,
            }
        }
        Err(err) => {
            output.push_str(&err);
            output.push('\n');
            clear_progress(&progress, ManagerId::Apt);
            UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            }
        }
    }
}

#[cfg(feature = "packagekit")]
struct PackageKitClient {
    connection: Connection,
}

#[cfg(feature = "packagekit")]
impl PackageKitClient {
    fn new() -> Result<Self, String> {
        let connection = Connection::system().map_err(|err| err.to_string())?;
        Ok(Self { connection })
    }

    fn transaction(&self) -> Result<TransactionProxyBlocking<'_>, String> {
        let pk = PackageKitProxyBlocking::new(&self.connection)
            .map_err(|err| err.to_string())?;
        let tx_path = pk.create_transaction().map_err(|err| err.to_string())?;
        TransactionProxyBlocking::builder(&self.connection)
            .destination("org.freedesktop.PackageKit")
            .map_err(|err| err.to_string())?
            .path(tx_path)
            .map_err(|err| err.to_string())?
            .build()
            .map_err(|err| err.to_string())
    }

    fn collect_update_ids(
        &mut self,
        filter_names: Option<&Vec<String>>,
    ) -> Result<Vec<String>, String> {
        let tx = self.transaction()?;
        tx.get_updates(FilterKind::None as u64)
            .map_err(|err| err.to_string())?;
        let packages = transaction_collect_packages(tx)?;
        let mut ids = Vec::new();
        let name_filter = filter_names.map(|names| {
            names
                .iter()
                .map(|name| name.as_str())
                .collect::<std::collections::HashSet<_>>()
        });

        for package in packages {
            let name = package.package_id.split(';').next().unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if let Some(filter) = &name_filter {
                if !filter.contains(name) {
                    continue;
                }
            }
            ids.push(package.package_id);
        }

        ids.sort();
        ids.dedup();

        if let Some(filter) = name_filter {
            if ids.is_empty() && !filter.is_empty() {
                return Err("No updates selected.".to_string());
            }
        }

        Ok(ids)
    }

    fn run_update(
        &mut self,
        package_ids: &[String],
        mut on_progress: impl FnMut(u32),
    ) -> Result<(), String> {
        let tx = self.transaction()?;
        tx.set_hints(&["interactive=true"])
            .map_err(|err| err.to_string())?;
        let id_refs: Vec<&str> = package_ids.iter().map(|id| id.as_str()).collect();
        tx.update_packages(TransactionFlag::OnlyTrusted as u64, &id_refs)
            .map_err(|err| err.to_string())?;
        transaction_run_with_progress(tx, |percent| on_progress(percent))?;
        Ok(())
    }
}

#[cfg(feature = "packagekit")]
#[allow(dead_code)]
#[repr(u64)]
enum FilterKind {
    None = 1 << 1,
}

#[cfg(feature = "packagekit")]
#[allow(dead_code)]
#[repr(u64)]
enum TransactionFlag {
    OnlyTrusted = 1 << 1,
}

#[cfg(feature = "packagekit")]
struct TransactionPackage {
    package_id: String,
}

#[cfg(feature = "packagekit")]
fn transaction_collect_packages(
    tx: TransactionProxyBlocking,
) -> Result<Vec<TransactionPackage>, String> {
    let mut packages = Vec::new();
    for signal in tx.receive_all_signals().map_err(|err| err.to_string())? {
        if let Some(member) = signal.member() {
            match member.as_str() {
                "Package" => {
                    let (_info, package_id, _summary) =
                        signal.body::<(u32, String, String)>().map_err(|err| err.to_string())?;
                    packages.push(TransactionPackage { package_id });
                }
                "ErrorCode" => {
                    let (code, details) =
                        signal.body::<(u32, String)>().map_err(|err| err.to_string())?;
                    if code != 48 {
                        return Err(format!("{details} (code {code})"));
                    }
                }
                "Finished" => {
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(packages)
}

#[cfg(feature = "packagekit")]
fn transaction_run_with_progress(
    tx: TransactionProxyBlocking,
    mut on_progress: impl FnMut(u32),
) -> Result<(), String> {
    for signal in tx.receive_all_signals().map_err(|err| err.to_string())? {
        if let Some(member) = signal.member() {
            match member.as_str() {
                "ItemProgress" => {
                    let (_package_id, _status, percentage) =
                        signal.body::<(String, u32, u32)>().map_err(|err| err.to_string())?;
                    let total_percentage = tx.percentage().unwrap_or(percentage);
                    on_progress(total_percentage);
                }
                "ErrorCode" => {
                    let (code, details) =
                        signal.body::<(u32, String)>().map_err(|err| err.to_string())?;
                    if code != 48 {
                        return Err(format!("{details} (code {code})"));
                    }
                }
                "Finished" => {
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(feature = "flatpak")]
async fn run_flatpak_libflatpak(
    entries: Vec<UpdateEntry>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    let progress_clone = progress.clone();
    let result = tokio::task::spawn_blocking(move || {
        flatpak_update_blocking(entries, progress_clone)
    })
    .await;

    match result {
        Ok(outcome) => outcome,
        Err(err) => UpdateOutcome {
            output: format!("Flatpak update task failed: {err}"),
            status: TaskStatus::Failed(err.to_string()),
        },
    }
}

#[cfg(feature = "flatpak")]
fn flatpak_update_blocking(
    entries: Vec<UpdateEntry>,
    progress: Option<ProgressTracker>,
) -> UpdateOutcome {
    let mut output = String::new();
    output.push_str("$ flatpak update (libflatpak)\n");

    let mut user_refs = Vec::new();
    let mut system_refs = Vec::new();
    let mut unknown_refs = Vec::new();

    for entry in entries {
        if entry.manager != ManagerId::Flatpak {
            continue;
        }
        match entry.scope {
            Some(InstallScope::User) => user_refs.push(entry.id),
            Some(InstallScope::System) => system_refs.push(entry.id),
            None => unknown_refs.push(entry.id),
        }
    }

    for refs in [&mut user_refs, &mut system_refs, &mut unknown_refs] {
        refs.sort();
        refs.dedup();
    }

    let mut groups = Vec::new();
    if !user_refs.is_empty() {
        groups.push(FlatpakGroup {
            user: true,
            refs: user_refs,
        });
    }
    if !system_refs.is_empty() {
        groups.push(FlatpakGroup {
            user: false,
            refs: system_refs,
        });
    }
    if !unknown_refs.is_empty() {
        groups.push(FlatpakGroup {
            user: true,
            refs: unknown_refs.clone(),
        });
        groups.push(FlatpakGroup {
            user: false,
            refs: unknown_refs,
        });
    }

    if groups.is_empty() {
        output.push_str("No Flatpak updates selected.\n");
        return UpdateOutcome {
            output,
            status: TaskStatus::Failed("No updates selected".to_string()),
        };
    }

    let total_groups = groups.len() as f32;
    for (index, group) in groups.into_iter().enumerate() {
        let group_span = 100.0 / total_groups;
        let group_start = group_span * index as f32;
        let progress = progress.clone();
        let progress_for_report = progress.clone();
        let report = Rc::new(RefCell::new(Box::new(move |value: f32| {
            let clamped = value.clamp(0.0, 100.0);
            let scaled = group_start + (clamped / 100.0) * group_span;
            report_progress(&progress_for_report, ManagerId::Flatpak, scaled);
        }) as Box<dyn FnMut(f32)>));

        if let Ok(mut callback) = report.try_borrow_mut() {
            callback(0.0);
        }

        let added = match run_flatpak_group(&group, report.clone()) {
            Ok(added) => added,
            Err(err) => {
                output.push_str(&err);
                output.push('\n');
                clear_progress(&progress, ManagerId::Flatpak);
                return UpdateOutcome {
                    output,
                    status: TaskStatus::Failed(err),
                };
            }
        };

        if added > 0 {
            let scope_label = if group.user { "user" } else { "system" };
            output.push_str(&format!(
                "Updated {added} Flatpak refs ({scope_label}).\n"
            ));
        }

        if let Ok(mut callback) = report.try_borrow_mut() {
            callback(100.0);
        }
    }

    report_progress(&progress, ManagerId::Flatpak, 100.0);
    UpdateOutcome {
        output,
        status: TaskStatus::Success,
    }
}

#[cfg(feature = "flatpak")]
struct FlatpakGroup {
    user: bool,
    refs: Vec<String>,
}

#[cfg(feature = "flatpak")]
fn run_flatpak_group(
    group: &FlatpakGroup,
    progress_cb: Rc<RefCell<Box<dyn FnMut(f32)>>>,
) -> Result<usize, String> {
    let inst = if group.user {
        Installation::new_user(Cancellable::NONE)
    } else {
        Installation::new_system(Cancellable::NONE)
    }
    .map_err(|err| err.to_string())?;

    let tx = Transaction::for_installation(&inst, Cancellable::NONE)
        .map_err(|err| err.to_string())?;

    attach_flatpak_progress(&tx, progress_cb);

    let added = add_flatpak_updates(&inst, &tx, &group.refs)?;
    if added == 0 {
        return Ok(0);
    }

    tx.run(Cancellable::NONE).map_err(|err| err.to_string())?;
    Ok(added)
}

#[cfg(feature = "flatpak")]
fn attach_flatpak_progress(
    tx: &Transaction,
    progress_cb: Rc<RefCell<Box<dyn FnMut(f32)>>>,
) {
    let total_ops = Rc::new(Cell::new(0));
    tx.connect_ready({
        let total_ops = total_ops.clone();
        move |tx| {
            total_ops.set(tx.operations().len());
            true
        }
    });
    let started_ops = Rc::new(Cell::new(0));
    tx.connect_new_operation(move |_, _op, progress| {
        let current_op = started_ops.get();
        started_ops.set(current_op + 1);
        let progress_per_op = 100.0 / (total_ops.get().max(started_ops.get()) as f32);
        let progress_cb = progress_cb.clone();
        progress.connect_changed(move |progress| {
            let op_progress = (progress.progress() as f32) / 100.0;
            let total_progress = ((current_op as f32) + op_progress) * progress_per_op;
            let mut progress_cb = progress_cb.borrow_mut();
            progress_cb(total_progress);
        });
    });
}

#[cfg(feature = "flatpak")]
fn add_flatpak_updates(
    inst: &Installation,
    tx: &Transaction,
    refs: &[String],
) -> Result<usize, String> {
    let mut added = 0usize;
    for ref_str in refs {
        let r = match Ref::parse(ref_str) {
            Ok(ok) => ok,
            Err(err) => {
                return Err(format!("failed to parse flatpak ref {ref_str}: {err}"));
            }
        };
        let id = r.name().unwrap_or_default();
        let installed = match inst.installed_ref(
            r.kind(),
            &id,
            r.arch().as_deref(),
            r.branch().as_deref(),
            Cancellable::NONE,
        ) {
            Ok(installed) => installed,
            Err(_) => continue,
        };

        if let Some(eol_rebase) = installed.eol_rebase() {
            let origin = installed.origin().unwrap_or_default();
            unsafe {
                let subpaths = ptr::null_mut();
                let mut previous_ids = vec![id.as_ptr()];
                let mut error: *mut libflatpak::glib::ffi::GError = ptr::null_mut();
                if libflatpak::ffi::flatpak_transaction_add_rebase(
                    tx.as_ptr(),
                    origin.as_ptr(),
                    eol_rebase.as_ptr(),
                    subpaths,
                    previous_ids.as_mut_ptr(),
                    &mut error,
                ) == 0
                {
                    let error_message = if error.is_null() {
                        "unspecified error".to_string()
                    } else {
                        glib::Error::from_glib_ptr_borrow(&error)
                            .message()
                            .to_string()
                    };
                    return Err(format!(
                        "failed to rebase {} to {}: {}",
                        ref_str, eol_rebase, error_message
                    ));
                }
            }

            tx.add_uninstall(ref_str)
                .map_err(|err| err.to_string())?;
            added += 1;
            continue;
        }

        tx.add_update(ref_str, &[], None)
            .map_err(|err| err.to_string())?;
        added += 1;
    }

    Ok(added)
}

async fn run_flatpak_selected(entries: &[UpdateEntry]) -> UpdateOutcome {
    let mut user_refs: Vec<String> = Vec::new();
    let mut system_refs: Vec<String> = Vec::new();
    let mut unknown_refs: Vec<String> = Vec::new();

    for entry in entries {
        if entry.manager != ManagerId::Flatpak {
            continue;
        }
        match entry.scope {
            Some(InstallScope::User) => user_refs.push(entry.id.clone()),
            Some(InstallScope::System) => system_refs.push(entry.id.clone()),
            None => unknown_refs.push(entry.id.clone()),
        }
    }

    for refs in [&mut user_refs, &mut system_refs, &mut unknown_refs] {
        refs.sort();
        refs.dedup();
    }

    if user_refs.is_empty() && system_refs.is_empty() && unknown_refs.is_empty() {
        return UpdateOutcome {
            output: "No Flatpak updates selected.\n".to_string(),
            status: TaskStatus::Failed("No updates selected".to_string()),
        };
    }

    let mut output = String::new();

    if !user_refs.is_empty() {
        let mut args = vec!["update".to_string(), "-y".to_string(), "--user".to_string()];
        args.extend(user_refs);
        if let Err(err) = run_command_logged(&mut output, "flatpak", args, false).await {
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    }

    if !system_refs.is_empty() {
        let mut args = vec!["update".to_string(), "-y".to_string(), "--system".to_string()];
        args.extend(system_refs);
        if let Err(err) = run_command_logged(&mut output, "flatpak", args, true).await {
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    }

    if !unknown_refs.is_empty() {
        let mut args = vec!["update".to_string(), "-y".to_string()];
        args.extend(unknown_refs);
        if let Err(err) = run_command_logged(&mut output, "flatpak", args, false).await {
            return UpdateOutcome {
                output,
                status: TaskStatus::Failed(err),
            };
        }
    }

    UpdateOutcome {
        output,
        status: TaskStatus::Success,
    }
}

async fn run_command_logged(
    output: &mut String,
    program: &str,
    args: Vec<String>,
    requires_privilege: bool,
) -> Result<(), String> {
    let arg_refs: Vec<&str> = args.iter().map(|arg| arg.as_str()).collect();
    let line = if requires_privilege {
        format!("$ pkexec {} {}\n", program, arg_refs.join(" "))
    } else {
        format!("$ {} {}\n", program, arg_refs.join(" "))
    };
    output.push_str(&line);

    let run_result =
        run_command_with_optional_apt_retry(program, &arg_refs, requires_privilege).await?;
    output.push_str(&String::from_utf8_lossy(&run_result.bytes));

    if run_result.apt_lock {
        output.push_str(APT_LOCK_ERROR_MESSAGE);
        output.push('\n');
        return Err(APT_LOCK_ERROR_MESSAGE.to_string());
    }

    if run_result.status_code != 0 {
        let status = format!("Exited with status: {}", run_result.status_code);
        output.push_str(&status);
        output.push('\n');
        Err(status)
    } else {
        Ok(())
    }
}

struct PrivilegedHelper {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

static HELPER: LazyLock<Mutex<Option<PrivilegedHelper>>> =
    LazyLock::new(|| Mutex::new(None));
static SHELL_HELPER: LazyLock<Mutex<Option<ShellHelper>>> =
    LazyLock::new(|| Mutex::new(None));
static HELPER_DISABLED: LazyLock<AtomicBool> =
    LazyLock::new(|| AtomicBool::new(false));

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
        let mut preamble = String::new();
        loop {
            line.clear();
            let bytes_read = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|err| err.to_string())?;
            if bytes_read == 0 {
                if preamble.trim().is_empty() {
                    return Err("privileged helper closed the connection".to_string());
                }
                return Err(preamble.trim().to_string());
            }
            if line.starts_with("OUT ") {
                break;
            }
            if !line.trim().is_empty() {
                if !preamble.is_empty() {
                    preamble.push('\n');
                }
                preamble.push_str(line.trim_end());
            }
            if preamble.len() > 4096 {
                return Err(preamble.trim().to_string());
            }
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

struct ShellHelper {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    counter: u64,
}

impl ShellHelper {
    async fn spawn() -> Result<Self, String> {
        let mut child = Command::new("pkexec")
            .arg("sh")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| err.to_string())?;

        let mut stdin = child.stdin.take().ok_or("missing shell helper stdin")?;
        let stdout = child.stdout.take().ok_or("missing shell helper stdout")?;

        stdin
            .write_all(b"exec 2>&1\n")
            .await
            .map_err(|err| err.to_string())?;
        stdin.flush().await.map_err(|err| err.to_string())?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            counter: 0,
        })
    }

    async fn run(&mut self, program: &str, args: &[&str]) -> Result<(Vec<u8>, i32), String> {
        self.counter = self.counter.wrapping_add(1);
        let token = format!("__MYSTIC_DONE_{}__", self.counter);
        let command = build_shell_command(program, args);
        let line = format!(
            "{command}; status=$?; printf '{token}%d\\n' $status\n"
        );

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        self.stdin.flush().await.map_err(|err| err.to_string())?;

        let mut output = Vec::new();
        let token_bytes = token.as_bytes();
        let mut line_bytes = Vec::new();

        loop {
            line_bytes.clear();
            let bytes_read = self
                .stdout
                .read_until(b'\n', &mut line_bytes)
                .await
                .map_err(|err| err.to_string())?;
            if bytes_read == 0 {
                return Err("privileged shell helper closed the connection".to_string());
            }

            if line_bytes.starts_with(token_bytes) {
                let status_part = &line_bytes[token_bytes.len()..];
                let status_text = String::from_utf8_lossy(status_part);
                let status_code: i32 = status_text
                    .trim()
                    .parse()
                    .map_err(|_| "invalid shell helper status".to_string())?;
                return Ok((output, status_code));
            }

            output.extend_from_slice(&line_bytes);
        }
    }
}

fn build_shell_command(program: &str, args: &[&str]) -> String {
    let mut command = String::new();
    command.push_str(&shell_escape(program));
    for arg in args {
        command.push(' ');
        command.push_str(&shell_escape(arg));
    }
    command
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    let mut escaped = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            escaped.push_str("'\\''");
        } else {
            escaped.push(ch);
        }
    }
    escaped.push('\'');
    escaped
}
