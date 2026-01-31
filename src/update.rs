// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

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

pub async fn run_manager_selected(id: ManagerId, entries: Vec<UpdateEntry>) -> UpdateOutcome {
    match id {
        ManagerId::Apt => run_apt_selected(&entries).await,
        ManagerId::Flatpak => run_flatpak_selected(&entries).await,
        _ => run_manager(id).await,
    }
}

pub async fn check_manager(id: ManagerId, force_privileged: bool) -> CheckOutcome {
    let spec = id.spec();
    let mut output = String::new();
    let mut last_status = 0;

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

async fn run_apt_selected(entries: &[UpdateEntry]) -> UpdateOutcome {
    let mut packages: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            if entry.manager == ManagerId::Apt {
                Some(entry.id.clone())
            } else {
                None
            }
        })
        .collect();

    packages.sort();
    packages.dedup();

    if packages.is_empty() {
        return UpdateOutcome {
            output: "No APT updates selected.\n".to_string(),
            status: TaskStatus::Failed("No updates selected".to_string()),
        };
    }

    let mut output = String::new();

    let update_args = vec!["update".to_string()];
    if let Err(err) = run_command_logged(
        &mut output,
        "apt-get",
        update_args,
        true,
    )
    .await
    {
        return UpdateOutcome {
            output,
            status: TaskStatus::Failed(err),
        };
    }

    let mut install_args = vec!["install".to_string(), "-y".to_string()];
    install_args.extend(packages);

    match run_command_logged(&mut output, "apt-get", install_args, true).await {
        Ok(()) => UpdateOutcome {
            output,
            status: TaskStatus::Success,
        },
        Err(err) => UpdateOutcome {
            output,
            status: TaskStatus::Failed(err),
        },
    }
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
