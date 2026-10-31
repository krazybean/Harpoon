//! Hand-written argument handling: `harpoon-deploy <command> [--flag value | --switch]…`.
//! Unknown flags are errors (exit 2); nothing is read from the environment.

use std::collections::BTreeMap;

pub const AUTH_FLAG: &str = "authorize-live-docker-changes";
pub const AUTH_PHRASE: &str = "I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE";

struct Spec {
    command: &'static str,
    values: &'static [&'static str],
    switches: &'static [&'static str],
}

const SPECS: &[Spec] = &[
    Spec {
        command: "status",
        values: &["runtime-dir"],
        switches: &[],
    },
    Spec {
        command: "doctor",
        values: &[
            "runtime-dir",
            "project-dir",
            "context",
            "dockerfile",
            "container-port",
            "health-path",
            "resources-dir",
            "docker-socket",
            "mgmt-socket",
        ],
        switches: &[],
    },
    Spec {
        command: "plan",
        values: &[
            "runtime-dir",
            "project-dir",
            "context",
            "dockerfile",
            "container-port",
            "health-path",
            "resources-dir",
            "docker-socket",
            "app",
        ],
        switches: &[],
    },
    Spec {
        command: "recover",
        values: &["runtime-dir"],
        switches: &[],
    },
    Spec {
        command: "reconcile",
        values: &["runtime-dir", "docker-socket", "resources-dir", AUTH_FLAG],
        switches: &["apply"],
    },
    Spec {
        command: "deploy",
        values: &[
            "runtime-dir",
            "project-dir",
            "context",
            "dockerfile",
            "container-port",
            "health-path",
            "resources-dir",
            "docker-socket",
            "app",
            "health-timeout-secs",
            "mgmt-socket",
            "forwarder-pid",
            "forwarder-command",
            "lsof-path",
            AUTH_FLAG,
        ],
        switches: &["simulate"],
    },
    Spec {
        command: "replace",
        values: &[
            "runtime-dir",
            "project-dir",
            "context",
            "dockerfile",
            "container-port",
            "health-path",
            "resources-dir",
            "docker-socket",
            "app",
            "health-timeout-secs",
            "mgmt-socket",
            "forwarder-pid",
            "forwarder-command",
            "lsof-path",
            AUTH_FLAG,
        ],
        switches: &["simulate", "confirm-cutover"],
    },
    Spec {
        command: "restore",
        values: &["runtime-dir", "deployment"],
        switches: &["simulate", "apply"],
    },
    Spec {
        command: "export-journal",
        values: &["runtime-dir", "to"],
        switches: &[],
    },
    Spec {
        command: "teardown",
        values: &[
            "runtime-dir",
            "deployment",
            "docker-socket",
            "resources-dir",
            AUTH_FLAG,
        ],
        switches: &["apply"],
    },
    Spec {
        command: "cleanup-image",
        values: &[
            "runtime-dir",
            "deployment",
            "image",
            "docker-socket",
            "resources-dir",
            AUTH_FLAG,
        ],
        switches: &["apply"],
    },
    Spec {
        command: "inventory",
        values: &["docker-socket", "lsof-path", "to"],
        switches: &[],
    },
];

#[derive(Debug, Default)]
pub struct Parsed {
    pub command: String,
    values: BTreeMap<String, String>,
    switches: Vec<String>,
}

impl Parsed {
    pub fn value(&self, k: &str) -> Option<&str> {
        self.values.get(k).map(String::as_str)
    }
    pub fn switch(&self, k: &str) -> bool {
        self.switches.iter().any(|s| s == k)
    }
}

pub fn command_names() -> Vec<&'static str> {
    SPECS.iter().map(|s| s.command).collect()
}

pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let Some(cmd) = args.first() else {
        return Err("no command given".into());
    };
    let Some(spec) = SPECS.iter().find(|s| s.command == cmd) else {
        return Err(format!("unknown command {cmd:?}"));
    };
    let mut p = Parsed {
        command: cmd.clone(),
        ..Parsed::default()
    };
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let Some(flag) = a.strip_prefix("--") else {
            return Err(format!(
                "unexpected argument {a:?} (flags look like --name value)"
            ));
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (flag, None),
        };
        if spec.switches.contains(&name) {
            if inline.is_some() {
                return Err(format!("--{name} takes no value"));
            }
            p.switches.push(name.to_string());
        } else if spec.values.contains(&name) {
            let v = match inline {
                Some(v) => v,
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("--{name} needs a value"))?,
            };
            if p.values.insert(name.to_string(), v).is_some() {
                return Err(format!("--{name} given twice"));
            }
        } else {
            return Err(format!("{cmd} does not take --{name}"));
        }
    }
    Ok(p)
}

pub const HELP: &str = "\
harpoon-deploy — gated developer CLI for the Harpoon deploy engine (first slice)

USAGE: harpoon-deploy <command> --runtime-dir DIR [options]

COMMANDS
  status          read-only: deployments, resource ownership, unresolved cleanup, leases
  doctor          read-only checks: source, bundled Buildx, journal, explicit runtime
  plan            validate source/config and show what a deploy would do (starts nothing)
  recover         mark interrupted deployments (writes the journal); never touches Docker
  reconcile       compare the ledger with the runtime; --apply removes only verified resources
  deploy          first-deployment workflow; live execution is disabled until B7 (--simulate is offline)
  replace         health-gated replacement of a running release: prepares a candidate, shows the cutover plan; --simulate only (live replacement is not enabled)
  restore         explicit, bounded restoration after an interrupted cutover (dry run by default; --apply); --simulate only (live restoration is not enabled)
  export-journal  copy the journal to a new file for investigation (--to FILE)
  teardown        dismantle ONE succeeded deployment's container, image and leases (dry run by default; --apply)
  cleanup-image   remove the retained image of ONE failed or interrupted deployment, by its recorded id (dry run by default; --apply)
  inventory       read-only listing of containers, images, volumes, port mappings and host listeners (no --runtime-dir)

EVERY COMMAND EXCEPT inventory NEEDS --runtime-dir. One runtime directory = one journal (DIR/deploy-journal, mode 0700).
Docker is contacted only with an explicit --docker-socket PATH; the Docker environment variables, Docker contexts and
~/.docker are never read. Changing Docker state additionally needs
  --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE

EXIT CODES
  0 ok   2 usage   3 source/configuration invalid   4 journal problem (corrupt, locked, permissions)
  5 runtime unreachable or not configured   6 refused by a safety gate   7 deployment failed or left work to do
";
