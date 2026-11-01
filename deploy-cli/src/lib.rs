//! Gated developer CLI for the Harpoon deploy engine (Milestone B6).
//!
//! It wires the existing pieces — engine, `DockerBackend`, `FileJournal` — into commands and adds
//! no new policy of its own: every safety rule below is the engine's, the backend's or the
//! journal's, surfaced with an exit code and a diagnostic.
//!
//! * Nothing is read from the environment. Docker is contacted only through `--docker-socket`.
//! * Read-only commands (`status`, `doctor`, `plan`, `reconcile` without `--apply`,
//!   `export-journal`) never write the journal, mark anything interrupted, or create directories.
//! * Live execution is a build-time gate ([`Gate`]) that this crate's binary keeps closed until B7.

mod args;
mod commands;
mod config_cmds;
mod next_steps;
mod store;

pub use args::{command_names, AUTH_FLAG, AUTH_PHRASE, HELP};

/// Exit codes (also in `--help`).
pub mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 2;
    pub const CONFIG: i32 = 3;
    pub const JOURNAL: i32 = 4;
    pub const RUNTIME: i32 = 5;
    pub const REFUSED: i32 = 6;
    pub const FAILED: i32 = 7;
}

/// What the build allows. The shipping binary uses [`Gate::shipping`]: live deployment closed.
/// Tests open it to prove the wiring against a fake Docker; there is no flag, environment
/// variable or file that opens it at run time.
#[derive(Debug, Clone, Copy)]
pub struct Gate {
    pub live_deploy_enabled: bool,
    /// Live `replace`, `rollback` and `restore`. Closed in the shipping binary. Only the
    /// `harpoon-deploy-acceptance` binary, which exists only when the `acceptance` Cargo feature is
    /// asked for by name, opens it; nothing at run time can.
    pub live_replacement_enabled: bool,
}

impl Gate {
    pub const fn shipping() -> Gate {
        Gate {
            live_deploy_enabled: false,
            live_replacement_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A command that did not succeed: an exit code and the message for stderr.
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub message: String,
}

impl Fail {
    pub fn usage(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::USAGE,
            message: m.into(),
        }
    }
    pub fn config(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::CONFIG,
            message: m.into(),
        }
    }
    pub fn journal(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::JOURNAL,
            message: m.into(),
        }
    }
    pub fn runtime(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::RUNTIME,
            message: m.into(),
        }
    }
    pub fn refused(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::REFUSED,
            message: m.into(),
        }
    }
    pub fn failed(m: impl Into<String>) -> Fail {
        Fail {
            code: exit::FAILED,
            message: m.into(),
        }
    }
}

/// What a command may touch besides the runtime directory and Docker: the secret store (the macOS
/// Keychain in the product, a fake in tests) and standard input (the only way a secret value enters).
pub struct Services<'a> {
    pub secrets: &'a dyn harpoon_deploy::secrets::SecretStore,
    pub stdin: &'a mut dyn std::io::Read,
    /// An interactive terminal would echo a secret; `secret-set` refuses one.
    pub stdin_is_terminal: bool,
}

/// Run one command against the real Keychain and standard input. `args` excludes the program name.
pub fn run(args: &[String], gate: &Gate) -> Outcome {
    use std::io::IsTerminal;
    let keychain = harpoon_deploy_keychain::Keychain::new();
    let mut stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    run_with(
        args,
        gate,
        &mut Services {
            secrets: &keychain,
            stdin: &mut stdin,
            stdin_is_terminal: terminal,
        },
    )
}

/// [`run`] with explicit services, for tests.
pub fn run_with(args: &[String], gate: &Gate, svc: &mut Services) -> Outcome {
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        return Outcome {
            code: if args.is_empty() {
                exit::USAGE
            } else {
                exit::OK
            },
            stdout: if args.is_empty() {
                String::new()
            } else {
                HELP.to_string()
            },
            stderr: if args.is_empty() {
                HELP.to_string()
            } else {
                String::new()
            },
        };
    }
    let parsed = match args::parse(args) {
        Ok(p) => p,
        Err(m) => {
            return Outcome {
                code: exit::USAGE,
                stdout: String::new(),
                stderr: format!(
                    "usage error: {m}\n(commands: {}; try `help`)\n",
                    args::command_names().join(", ")
                ),
            }
        }
    };
    let mut out = String::new();
    let res = commands::dispatch(&parsed, gate, svc, &mut out);
    match res {
        Ok(code) => Outcome {
            code,
            stdout: out,
            stderr: String::new(),
        },
        Err(f) => Outcome {
            code: f.code,
            stdout: out,
            stderr: format!("{}\n", f.message),
        },
    }
}
