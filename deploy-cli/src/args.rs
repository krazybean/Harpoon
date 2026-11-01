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
        values: &["runtime-dir", "docker-socket"],
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
            "env",
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
            "env",
            "build-arg",
            "project",
            "environment",
            "target",
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
            "env",
            "build-arg",
            "project",
            "environment",
            "target",
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
        command: "config-set",
        values: &["runtime-dir", "scope", "name", "value"],
        switches: &["build", "runtime"],
    },
    Spec {
        command: "secret-set",
        values: &["runtime-dir", "scope", "name"],
        switches: &["build", "runtime"],
    },
    Spec {
        command: "config-unset",
        values: &["runtime-dir", "scope", "name"],
        switches: &[],
    },
    Spec {
        command: "config-defs",
        values: &["runtime-dir", "scope"],
        switches: &[],
    },
    Spec {
        command: "config-show",
        values: &["runtime-dir", "app", "project", "environment", "deployment"],
        switches: &["check-secrets"],
    },
    Spec {
        command: "config-diff",
        values: &["runtime-dir", "from", "to", "project", "environment"],
        switches: &[],
    },
    Spec {
        command: "resume",
        values: &["runtime-dir", "app", "revision"],
        switches: &["dismiss", "newest"],
    },
    Spec {
        command: "events",
        values: &["runtime-dir", "app"],
        switches: &[],
    },
    Spec {
        command: "releases",
        values: &["runtime-dir", "app"],
        switches: &[],
    },
    Spec {
        command: "rollback",
        values: &[
            "runtime-dir",
            "app",
            "to",
            "docker-socket",
            "resources-dir",
            "mgmt-socket",
            "forwarder-pid",
            "forwarder-command",
            "lsof-path",
            AUTH_FLAG,
        ],
        switches: &["simulate", "confirm-rollback"],
    },
    Spec {
        command: "restore",
        values: &[
            "runtime-dir",
            "deployment",
            "docker-socket",
            "resources-dir",
            "mgmt-socket",
            "forwarder-pid",
            "forwarder-command",
            "lsof-path",
            AUTH_FLAG,
        ],
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

/// Value flags that may be given more than once (collected in order). Every other value flag still
/// rejects a repeat.
const REPEATABLE: &[&str] = &["env", "build-arg"];

#[derive(Debug, Default)]
pub struct Parsed {
    pub command: String,
    values: BTreeMap<String, String>,
    repeated: BTreeMap<String, Vec<String>>,
    switches: Vec<String>,
}

impl Parsed {
    pub fn value(&self, k: &str) -> Option<&str> {
        self.values.get(k).map(String::as_str)
    }
    /// Every occurrence of a repeatable flag, in the order given.
    pub fn values(&self, k: &str) -> &[String] {
        self.repeated.get(k).map(Vec::as_slice).unwrap_or(&[])
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
            if REPEATABLE.contains(&name) {
                p.repeated.entry(name.to_string()).or_default().push(v);
            } else if p.values.insert(name.to_string(), v).is_some() {
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
                  (--docker-socket lists Harpoon-labelled containers only if the store is missing)
  doctor          read-only checks: source, bundled Buildx, journal, explicit runtime
  plan            validate source/config and show what a deploy would do (starts nothing)
  recover         mark interrupted deployments (writes the journal); never touches Docker
  reconcile       compare the ledger with the runtime; --apply removes only verified resources
  deploy          first-deployment workflow; live execution is disabled until B7 (--simulate is offline)
  replace         health-gated replacement of a running release: prepares a candidate, shows the cutover plan; live only in the separate acceptance build (closed in this one); offline with --simulate
  releases        list an application's retained releases, the one serving, and why any cannot be rolled back to (reads the journal only)
  resume          record ONE explicit decision for a paused application's held events: --dismiss (clear the pause), --newest or --revision REV
                  (select a held revision: it deploys NOTHING and the application stays blocked until dismissed). Never contacts Docker
  events          read-only: pause records and held/selected/dismissed/superseded events with their decisions
  rollback        explicit, developer-initiated rollback to a named earlier release (--to REL; needs --confirm-rollback); live only in the separate acceptance build (closed in this one); offline with --simulate
  restore         explicit, bounded restoration after an interrupted cutover (dry run by default; --apply); live only in the separate acceptance build (closed in this one); offline with --simulate
  config-set      define a NON-SECRET Config value at a scope (--scope global|project:NAME|app:ID[@ENV] --name N --value V [--build] [--runtime])
  secret-set      save a Secret at a scope; the value is read from standard input only (never argv). Saving again creates the next
                  immutable version (a rotation); the value goes to the macOS Keychain and is never shown again
  config-unset    explicitly unset a name at a scope: it blocks inheritance from less specific scopes
  config-defs     list the current definitions (Config values are readable; a Secret is shown as id@version)
  config-show     the effective configuration with the source scope of every key: --app A [--project P] [--environment E], or
                  --deployment D for that release's recorded, immutable snapshot; --check-secrets tests Keychain availability
  config-diff     names, scopes, flags and secret versions that differ: --from DEP [--to DEP] (default: what would apply now); never values
  export-journal  copy the journal to a new file for investigation (--to FILE)
  teardown        dismantle ONE succeeded deployment's container, image and leases (dry run by default; --apply)
  cleanup-image   remove the retained image of ONE failed or interrupted deployment, by its recorded id (dry run by default; --apply)
  inventory       read-only listing of containers, images, volumes, port mappings and host listeners (no --runtime-dir)

EVERY COMMAND EXCEPT inventory NEEDS --runtime-dir. One runtime directory = one journal (DIR/deploy-journal, mode 0700).
Docker is contacted only with an explicit --docker-socket PATH; the Docker environment variables, Docker contexts and
~/.docker are never read. Changing Docker state additionally needs
  --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE

RUNTIME VARIABLES (plan, deploy, replace)
  --env KEY=VALUE   repeatable. NON-SECRET variables for the release's containers: they are visible in `docker inspect`
                    and are stored in the journal with the release. Do not use for secrets (use `secret-set`). Names: letters, digits, underscores; HARPOON_* is reserved (the per-container
                    HARPOON_DEPLOYMENT_MARKER is generated for you). Values may be empty or contain `=`; control
                    characters are refused. Each release is pinned with its own variables: a restoration or rollback
                    reuses that release's pin, never the current command line. `replace` without --env gives the
                    candidate no variables.

SCOPED CONFIGURATION AND SECRETS (deploy, replace; Milestone C)
  --project NAME --environment ENV   which scopes apply. Resolution, most specific wins: application+env > application >
                    project+env > project > global+env > global. An explicit unset blocks inheritance.
  --build-arg KEY=VALUE   repeatable, NON-SECRET build arguments (also from config flagged --build)
  --target STAGE    the Dockerfile stage to build
  Variables flagged --build reach the build (Config as --build-arg, Secret as a BuildKit --secret mount, never an argument);
  --runtime ones reach the container (visible in `docker inspect`). Each deployment records an immutable snapshot of what it
  resolved and is pinned with the exact secret versions; a restore or rollback uses that release's own versions, never the
  current ones. A secret that cannot be read stops the command before anything is built, stopped or written.

EXIT CODES
  0 ok   2 usage   3 source/configuration invalid   4 journal problem (corrupt, locked, permissions)
  5 runtime unreachable or not configured   6 refused by a safety gate   7 deployment failed or left work to do
";
