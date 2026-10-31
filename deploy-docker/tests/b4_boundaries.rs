//! B4: source confinement, `.dockerignore` sizing, port allocation, health-response association
//! and the "nothing unowned is touched" audit.
//!
//! Servers here are disposable in-process listeners on loopback. They exercise Harpoon's own
//! checks; they say **nothing** about Docker's real port forwarder (see the B7 gate in
//! `docs/results/DEPLOY_DOCKER_BACKEND.md`).

mod support;

use harpoon_deploy::health::{
    run_health, Attempt, HealthSpec, Observation, Probe, StatusClass, Verdict,
};
use harpoon_deploy::state::*;
use harpoon_deploy::time::SystemClock;
use harpoon_deploy_docker::backend::{
    preliminary_name, replacement_name, serving_name, DockerBackend, DockerConfig,
};
use harpoon_deploy_docker::confine::{confine, ConfineError};
use harpoon_deploy_docker::ignore::IgnoreRules;
use harpoon_deploy_docker::preflight::{context_size, context_size_with};
use harpoon_deploy_docker::probe::{
    BindProbe, HttpTransport, PortProbe, ResponseMarker, SleepWaiter, TcpHttp,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::fake_docker::Action;
use support::*;

// ------------------------------------------------------------------ helpers

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "hdd-b4-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(std::fs::canonicalize(&p).unwrap())
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn write(&self, rel: &str, body: impl AsRef<[u8]>) -> PathBuf {
        let p = self.p(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        fn open_up(p: &Path) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let m = std::fs::symlink_metadata(e.path()).unwrap();
                    if m.is_dir() {
                        let _ = std::fs::set_permissions(
                            e.path(),
                            std::fs::Permissions::from_mode(0o755),
                        );
                        open_up(&e.path());
                    }
                }
            }
        }
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        open_up(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const DF: &str = "FROM alpine:3.22\nCMD [\"true\"]\n";

/// `<tmp>/proj` is the selected project; `<tmp>/outside` and `<tmp>/proj-evil` are not.
fn project() -> Tmp {
    let t = Tmp::new();
    t.write("proj/Dockerfile", DF);
    t.write("outside/Dockerfile", DF);
    t.write("outside/secret.txt", "secret");
    t.write("proj-evil/Dockerfile", DF);
    t
}

fn can_still_read(p: &Path) -> bool {
    std::fs::File::open(p).is_ok()
}

fn backend_with<T: HttpTransport>(
    h: &Harness,
    t: T,
    f: impl FnOnce(&mut DockerConfig),
) -> DockerBackend<T> {
    let mut cfg = config(&h.ws, &h.docker);
    f(&mut cfg);
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), t)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
}

fn ok200() -> Scripted {
    Scripted::always(Observation::Status(200))
}

/// Run to completion and return the failure (step, message), asserting the deployment failed.
fn fails(h: &mut Harness, b: &mut DockerBackend<impl HttpTransport>) -> (Step, String) {
    let mut j = StampedJournal::new();
    let r = h.run(b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed, "{:?}", h.dep.failure);
    let f = h.dep.failure.clone().unwrap();
    (f.step, f.message)
}

fn buildx_ran(h: &Harness) -> bool {
    !h.ws.buildx_record("args.txt").is_empty()
}

fn any_mutation(h: &Harness) -> bool {
    h.docker.requests().iter().any(|r| r.method != "GET")
}

// ------------------------------------------------------------------ 1. confinement (pure)

#[test]
fn a_dockerfile_inside_the_root_is_accepted_and_resolved_canonically() {
    let t = project();
    t.write("proj/app/Dockerfile.prod", DF);
    let s = confine(&t.p("proj"), &t.p("proj/app/../app"), "Dockerfile.prod").unwrap();
    assert_eq!(s.context, t.p("proj/app"));
    assert_eq!(s.dockerfile, t.p("proj/app/Dockerfile.prod"));
    assert!(s.dockerfile_text.contains("FROM alpine"));
    // a context that is a sub-folder of the root is fine
    assert!(confine(&t.p("proj"), &t.p("proj"), "app/Dockerfile.prod").is_ok());
}

#[test]
fn absolute_dotdot_and_malformed_dockerfile_names_are_refused() {
    let t = project();
    for name in [
        "/etc/passwd",
        t.p("outside/Dockerfile").to_str().unwrap(),
        "../outside/Dockerfile",
        "sub/../../outside/Dockerfile",
        "./Dockerfile",
        "",
        "Docker\0file",
        "..",
        ".",
    ] {
        let e = confine(&t.p("proj"), &t.p("proj"), name).unwrap_err();
        assert!(
            matches!(e, ConfineError::DockerfileName(_)),
            "{name:?} -> {e:?}"
        );
    }
}

#[test]
fn a_context_outside_the_root_is_refused_however_it_is_spelled() {
    let t = project();
    symlink(t.p("outside"), t.p("proj/link-out")).unwrap();
    for ctx in [
        t.p("outside"),
        t.p("proj/../outside"),
        t.p("proj-evil"), // shares a string prefix with the root, not a path prefix
        t.p("proj/link-out"),
        PathBuf::from("/"),
    ] {
        let e = confine(&t.p("proj"), &ctx, "Dockerfile").unwrap_err();
        assert!(
            matches!(e, ConfineError::ContextEscapes { .. }),
            "{} -> {e:?}",
            ctx.display()
        );
    }
}

#[test]
fn a_symlinked_dockerfile_must_still_lead_inside_the_context() {
    let t = project();
    // file symlink out
    symlink(t.p("outside/Dockerfile"), t.p("proj/Dockerfile.out")).unwrap();
    let e = confine(&t.p("proj"), &t.p("proj"), "Dockerfile.out").unwrap_err();
    assert!(matches!(e, ConfineError::DockerfileEscapes { .. }), "{e:?}");
    // directory symlink out
    symlink(t.p("outside"), t.p("proj/dir-out")).unwrap();
    let e = confine(&t.p("proj"), &t.p("proj"), "dir-out/Dockerfile").unwrap_err();
    assert!(matches!(e, ConfineError::DockerfileEscapes { .. }), "{e:?}");
    // a chain of two links ending outside
    symlink(t.p("proj/Dockerfile.out"), t.p("proj/Dockerfile.chain")).unwrap();
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile.chain").unwrap_err(),
        ConfineError::DockerfileEscapes { .. }
    ));
    // a link that stays inside is fine, and the canonical target is what is returned
    symlink(t.p("proj/Dockerfile"), t.p("proj/Dockerfile.in")).unwrap();
    let s = confine(&t.p("proj"), &t.p("proj"), "Dockerfile.in").unwrap();
    assert_eq!(s.dockerfile, t.p("proj/Dockerfile"));
    // a dockerfile inside a sub-context cannot reach a sibling folder of the same root
    t.write("proj/other/Dockerfile", DF);
    symlink(t.p("proj/other/Dockerfile"), t.p("proj/app-ctx-link")).unwrap_or(());
    std::fs::create_dir_all(t.p("proj/app")).unwrap();
    symlink(t.p("proj/other/Dockerfile"), t.p("proj/app/Dockerfile")).unwrap();
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj/app"), "Dockerfile").unwrap_err(),
        ConfineError::DockerfileEscapes { .. }
    ));
}

#[test]
fn missing_and_wrong_kind_paths_are_distinct_errors() {
    let t = project();
    assert!(matches!(
        confine(&t.p("nope"), &t.p("proj"), "Dockerfile").unwrap_err(),
        ConfineError::RootMissing(..)
    ));
    t.write("a-file", "x");
    assert!(matches!(
        confine(&t.p("a-file"), &t.p("a-file"), "Dockerfile").unwrap_err(),
        ConfineError::RootNotDirectory(_)
    ));
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj/missing"), "Dockerfile").unwrap_err(),
        ConfineError::ContextMissing(..)
    ));
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj/Dockerfile"), "Dockerfile").unwrap_err(),
        ConfineError::ContextNotDirectory(_)
    ));
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile.missing").unwrap_err(),
        ConfineError::DockerfileMissing(_)
    ));
    std::fs::create_dir_all(t.p("proj/Dockerfile.d")).unwrap();
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile.d").unwrap_err(),
        ConfineError::DockerfileNotFile(_)
    ));
    // a dangling link is "missing", not an escape
    symlink(t.p("gone"), t.p("proj/Dockerfile.dangling")).unwrap();
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile.dangling").unwrap_err(),
        ConfineError::DockerfileMissing(_)
    ));
}

#[test]
fn permission_failures_are_reported_not_mistaken_for_missing_files() {
    let t = project();
    let df = t.p("proj/Dockerfile");
    std::fs::set_permissions(&df, std::fs::Permissions::from_mode(0o000)).unwrap();
    if can_still_read(&df) {
        eprintln!("running with privileges that ignore file modes: permission cases skipped");
        return;
    }
    let e = confine(&t.p("proj"), &t.p("proj"), "Dockerfile").unwrap_err();
    assert!(
        matches!(&e, ConfineError::Unreadable(_, m) if m.contains("permission denied")),
        "{e:?}"
    );
    std::fs::set_permissions(&df, std::fs::Permissions::from_mode(0o644)).unwrap();

    t.write("proj/locked/Dockerfile", DF);
    std::fs::set_permissions(t.p("proj/locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = confine(&t.p("proj"), &t.p("proj"), "locked/Dockerfile").unwrap_err();
    assert!(matches!(e, ConfineError::Unreadable(..)), "{e:?}");
    std::fs::set_permissions(t.p("proj/locked"), std::fs::Permissions::from_mode(0o755)).unwrap();

    // an unreadable ignore file is an error, never "no ignore file"
    let ig = t.write("proj/.dockerignore", "x\n");
    std::fs::set_permissions(&ig, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = confine(&t.p("proj"), &t.p("proj"), "Dockerfile").unwrap_err();
    assert!(matches!(e, ConfineError::Unreadable(..)), "{e:?}");
}

#[test]
fn an_oversized_dockerfile_or_ignore_file_is_refused_unread() {
    let t = project();
    t.write("proj/Dockerfile.big", vec![b'#'; 2 * 1024 * 1024]);
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile.big").unwrap_err(),
        ConfineError::Unreadable(_, m) if m.contains("larger")
    ));
}

#[test]
fn an_ignore_file_that_leads_outside_the_context_is_refused() {
    let t = project();
    t.write("outside/ignore", "node_modules\n");
    symlink(t.p("outside/ignore"), t.p("proj/.dockerignore")).unwrap();
    assert!(matches!(
        confine(&t.p("proj"), &t.p("proj"), "Dockerfile").unwrap_err(),
        ConfineError::IgnoreEscapes { .. }
    ));
}

// ------------------------------------------------------------------ 1. confinement (through the backend)

#[test]
fn a_dockerfile_that_escapes_stops_the_deployment_before_buildx_and_before_docker() {
    for how in ["file-symlink", "dir-symlink", "dotdot", "absolute"] {
        let mut h = Harness::new();
        let out = h.ws.root.join("outside");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("Dockerfile"), DF).unwrap();
        let ctx = h.ws.context();
        let name = match how {
            "file-symlink" => {
                std::fs::remove_file(ctx.join("Dockerfile")).unwrap();
                symlink(out.join("Dockerfile"), ctx.join("Dockerfile")).unwrap();
                "Dockerfile".to_string()
            }
            "dir-symlink" => {
                symlink(&out, ctx.join("d")).unwrap();
                "d/Dockerfile".to_string()
            }
            "dotdot" => "../outside/Dockerfile".to_string(),
            _ => out.join("Dockerfile").to_string_lossy().into_owned(),
        };
        let mut b = backend_with(&h, ok200(), |c| c.dockerfile = name.clone());
        let (step, msg) = fails(&mut h, &mut b);
        assert_eq!(step, Step::Source, "{how}: {msg}");
        assert!(
            msg.contains("outside") || msg.contains("plain relative path"),
            "{how}: {msg}"
        );
        assert!(!buildx_ran(&h), "{how}: buildx must not run");
        assert!(!any_mutation(&h), "{how}: nothing was created or changed");
    }
}

#[test]
fn a_context_outside_the_selected_project_stops_the_deployment() {
    let mut h = Harness::new();
    let other = h.ws.root.join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Dockerfile"), DF).unwrap();
    let project_root = h.ws.context();
    let mut b = backend_with(&h, ok200(), |c| {
        c.context_dir = other.clone();
        c.project_root = Some(project_root.clone());
    });
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::Source);
    assert!(msg.contains("outside the selected project folder"), "{msg}");
    assert!(!buildx_ran(&h) && !any_mutation(&h));
}

#[test]
fn a_context_inside_the_project_builds_with_canonical_paths() {
    let mut h = Harness::new();
    let sub = h.ws.context().join("services/web");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("Dockerfile"), DF).unwrap();
    h.seed_built_image();
    let root = h.ws.context();
    // spelled with a `..` detour; the build runs from a private copy of the resolved folder
    let spelled = h.ws.context().join("services/../services/web");
    let mut b = backend_with(&h, ok200(), |c| {
        c.context_dir = spelled.clone();
        c.project_root = Some(root.clone());
    });
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let args = h.ws.buildx_record("args.txt");
    let canon = std::fs::canonicalize(&sub).unwrap();
    assert!(
        !args.contains(canon.to_str().unwrap()),
        "Buildx is not pointed at the user's folder: {args}"
    );
    assert!(
        args.contains("/ctx-"),
        "it is pointed at the private snapshot: {args}"
    );
    assert!(!args.contains(".."), "{args}");
    let left: Vec<_> = std::fs::read_dir(h.ws.scratch())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        left.iter()
            .all(|n| !n.starts_with("ctx-") && !n.starts_with("iid-")),
        "the snapshot and the ID file are removed: {left:?}"
    );
}

#[test]
fn a_dockerfile_swapped_for_an_escaping_link_after_preflight_is_caught_before_the_build() {
    let mut h = Harness::new();
    let out = h.ws.root.join("outside");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("Dockerfile"), DF).unwrap();
    let df = h.ws.context().join("Dockerfile");
    let target = out.join("Dockerfile");
    // The tag inspect happens after preflight, immediately before the build.
    h.docker.fault(
        Some("GET"),
        "/images/harpoon/",
        1,
        Action::Race(Arc::new(move |_| {
            let _ = std::fs::remove_file(&df);
            symlink(&target, &df).unwrap();
        })),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::Build, "{msg}");
    assert!(msg.contains("outside the build context"), "{msg}");
    assert!(!buildx_ran(&h), "the swap was caught before Buildx started");
}

#[test]
fn an_unreadable_folder_inside_the_context_is_a_visible_preflight_failure() {
    let mut h = Harness::new();
    let locked = h.ws.context().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("f"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("running with privileges that ignore directory modes: skipped");
        return;
    }
    let mut b = backend_with(&h, ok200(), |_| {});
    let (step, msg) = fails(&mut h, &mut b);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(step, Step::Source);
    assert!(msg.contains("cannot measure the build context"), "{msg}");
    assert!(!buildx_ran(&h));
}

#[test]
fn the_size_walk_never_follows_a_symlink_out_of_the_context() {
    let t = project();
    t.write("outside/huge.bin", vec![0u8; 50_000]);
    symlink(t.p("outside"), t.p("proj/out")).unwrap();
    symlink(t.p("proj"), t.p("proj/loop")).unwrap();
    // only Dockerfile counts: neither the link target's files nor the loop are walked
    assert_eq!(
        context_size(&t.p("proj"), 1 << 30).unwrap(),
        DF.len() as u64
    );
}

// ------------------------------------------------------------------ 1. .dockerignore and the size limit

fn s(v: &str) -> Vec<String> {
    v.split('/').map(str::to_string).collect()
}

#[test]
fn dockerignore_patterns_are_anchored_to_the_context_root_like_docker() {
    let r = IgnoreRules::parse(
        "# comment\n\nnode_modules\n/build/\n*.log\ndocs/**/*.md\n**/.cache\nsrc/gen*\n",
    );
    assert!(r.notes.is_empty() && !r.disabled, "{:?}", r.notes);
    assert!(r.excludes(&s("node_modules")));
    assert!(r.excludes(&s("node_modules/a/b.js")), "children of a match");
    assert!(
        !r.excludes(&s("sub/node_modules")),
        "a bare name is root-anchored"
    );
    assert!(r.excludes(&s("build/out.o")));
    assert!(r.excludes(&s("x.log")));
    assert!(!r.excludes(&s("sub/x.log")), "`*` does not cross folders");
    assert!(r.excludes(&s("docs/a/b/c.md")) && r.excludes(&s("docs/c.md")));
    assert!(!r.excludes(&s("docs/c.txt")));
    assert!(r.excludes(&s(".cache")) && r.excludes(&s("a/b/.cache/x")));
    assert!(r.excludes(&s("src/generated.rs")) && !r.excludes(&s("src/main.rs")));
    assert!(!r.excludes(&s("nodemodules")));
}

#[test]
fn an_exception_line_turns_filtering_off_because_it_could_re_include_files() {
    let r = IgnoreRules::parse("big\n!big/keep.txt\n");
    assert!(r.disabled && r.is_empty());
    assert!(
        !r.excludes(&s("big/other")),
        "counted, so the limit is safe"
    );
    assert!(r.notes[0].contains("whole context is counted"));
}

#[test]
fn patterns_whose_meaning_is_uncertain_exclude_nothing() {
    let r = IgnoreRules::parse("a?b\n[ab]c\nx\\y\n../up\n./here\nfoo**bar\n{a,b}\n");
    assert!(r.is_empty(), "nothing uncertain may shrink the measurement");
    assert_eq!(r.notes.len(), 7);
    assert!(!r.excludes(&s("axb")) && !r.excludes(&s("ac")));
}

fn ctx_with_big_ignored_folder() -> Tmp {
    let t = Tmp::new();
    t.write("Dockerfile", DF);
    t.write("app.txt", vec![0u8; 100]);
    t.write("node_modules/pkg/big.bin", vec![0u8; 10_000]);
    t.write("sub/node_modules/small.bin", vec![0u8; 50]);
    t
}

#[test]
fn ignored_files_do_not_count_toward_the_limit_but_everything_else_does() {
    let t = ctx_with_big_ignored_folder();
    let rules = IgnoreRules::parse("node_modules\n");
    let keep = [PathBuf::from("Dockerfile")];
    let all = context_size(&t.0, 1 << 30).unwrap();
    let filtered = context_size_with(&t.0, 1 << 30, &rules, &keep).unwrap();
    assert_eq!(all, DF.len() as u64 + 100 + 10_000 + 50);
    assert_eq!(
        filtered,
        DF.len() as u64 + 100 + 50,
        "root node_modules is ignored; sub/node_modules is not matched"
    );
}

#[test]
fn the_dockerfile_and_ignore_files_are_counted_even_when_ignored() {
    let t = Tmp::new();
    t.write("Dockerfile", DF);
    t.write(".dockerignore", "Dockerfile\n.dockerignore\n");
    t.write("conf/Dockerfile.x", DF);
    t.write("conf/other", vec![0u8; 1000]);
    let rules = IgnoreRules::parse("Dockerfile\n.dockerignore\nconf\n");
    let keep = harpoon_deploy_docker::ignore::always_sent(Path::new("conf/Dockerfile.x"));
    let n = context_size_with(&t.0, 1 << 30, &rules, &keep).unwrap();
    // The Dockerfile in use (conf/Dockerfile.x) and `.dockerignore` are sent whatever the rules
    // say; the *other* file named `Dockerfile` and conf/other are not.
    let ignore_len = "Dockerfile\n.dockerignore\n".len() as u64;
    assert_eq!(n, DF.len() as u64 + ignore_len);
}

fn size_failure_message(ignore: Option<&str>, limit: u64) -> Option<String> {
    let mut h = Harness::new();
    h.ws.write_context(DF);
    std::fs::create_dir_all(h.ws.context().join("node_modules")).unwrap();
    std::fs::write(
        h.ws.context().join("node_modules/big.bin"),
        vec![0u8; 20_000],
    )
    .unwrap();
    if let Some(i) = ignore {
        std::fs::write(h.ws.context().join(".dockerignore"), i).unwrap();
    }
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |c| c.max_context_bytes = limit);
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    if r.status == DeploymentStatus::Succeeded {
        None
    } else {
        Some(h.dep.failure.unwrap().message)
    }
}

#[test]
fn the_256_mb_limit_is_applied_after_a_dockerignore_that_harpoon_understands() {
    assert!(size_failure_message(None, 10_000)
        .unwrap()
        .contains("larger than"));
    assert!(size_failure_message(Some("node_modules\n"), 10_000).is_none());
    // an exception line: counted in full, fails closed
    assert!(size_failure_message(Some("node_modules\n!node_modules/x\n"), 10_000).is_some());
    // a pattern Harpoon cannot interpret does not shrink anything
    assert!(size_failure_message(Some("node_mod?les\n"), 10_000).is_some());
}

#[test]
fn a_dockerfile_specific_ignore_file_takes_precedence_like_buildkit() {
    let t = Tmp::new();
    t.write("Dockerfile", DF);
    t.write(".dockerignore", "a\n");
    t.write("Dockerfile.dockerignore", "b\n");
    let s = confine(&t.0, &t.0, "Dockerfile").unwrap();
    assert_eq!(s.ignore.unwrap().1, "b\n");
}

// ------------------------------------------------------------------ 2. port allocation

static PORTS: Mutex<()> = Mutex::new(());

fn host_port_of(h: &Harness, name: &str) -> Option<u16> {
    let body = h
        .docker
        .requests()
        .into_iter()
        .find(|r| {
            r.method == "POST"
                && r.path.starts_with("/containers/create")
                && r.path.ends_with(&format!("name={name}"))
        })?
        .body;
    let i = body.find("\"HostPort\":\"")? + 12;
    body[i..].split('"').next()?.parse().ok()
}

#[test]
fn the_bind_probe_sees_loopback_only_and_wildcard_listeners() {
    let a = TcpListener::bind("127.0.0.1:0").unwrap();
    let b = TcpListener::bind("0.0.0.0:0").unwrap();
    assert!(BindProbe.is_listening(a.local_addr().unwrap().port()));
    assert!(BindProbe.is_listening(b.local_addr().unwrap().port()));
}

#[test]
fn a_real_listener_a_stopped_container_and_a_running_one_each_steer_allocation() {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    h.seed_built_image();
    // 39000: a real process on the host. 38000: a *stopped* container's mapping. 38001: running.
    let held = TcpListener::bind("127.0.0.1:39000");
    h.docker.with(|s| {
        for (n, port, running) in [("stopped-one", 38000, false), ("running-one", 38001, true)] {
            s.add_container(n, vec![], running);
            s.containers.last_mut().unwrap().host_port = Some(port);
        }
    });
    let mut b = backend_with(&h, ok200(), |_| {}).with_port_probe(BindProbe);
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let pre = host_port_of(&h, &preliminary_name(&h.dep.id)).unwrap();
    let app = host_port_of(&h, &serving_name(&h.dep.id)).unwrap();
    assert_eq!(app, 38002, "both Docker mappings are skipped");
    if held.is_ok() {
        assert_eq!(pre, 39001, "the host listener on 39000 is skipped");
    } else {
        assert_ne!(pre, 39000, "something else already held 39000");
    }
}

#[test]
fn independent_deployments_get_distinct_ports_and_released_ports_are_reusable() {
    let mut h = Harness::new();
    let mut seen_apps = Vec::new();
    // two successes keep their application containers (and so their ports)
    for _ in 0..2 {
        h.seed_built_image();
        let mut b = backend_with(&h, ok200(), |_| {});
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
        assert_eq!(
            host_port_of(&h, &preliminary_name(&h.dep.id)),
            Some(39000),
            "the test port is released before cutover, so every deployment can reuse it"
        );
        seen_apps.push(host_port_of(&h, &serving_name(&h.dep.id)).unwrap());
        assert!(r
            .ledger
            .leased_ports()
            .iter()
            .all(|p| *p >= 38000 && *p < 39000));
        h.dep = new_deployment(&mut h.ids);
    }
    assert_eq!(seen_apps, [38000, 38001]);

    // a deployment that fails its final check releases its port...
    h.seed_built_image();
    let t = Scripted::always(Observation::Status(503));
    *t.answers.borrow_mut() = vec![Observation::Status(200), Observation::Status(200)];
    let mut b = backend_with(&h, t, |_| {});
    let (step, _) = fails(&mut h, &mut b);
    assert_eq!(step, Step::FinalCheck);
    let failed_port = host_port_of(&h, &serving_name(&h.dep.id)).unwrap();
    assert_eq!(failed_port, 38002);
    h.dep = new_deployment(&mut h.ids);

    // ...so the next one is given the same port again, not skipped
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    assert_eq!(h.run(&mut b, &mut j).status, DeploymentStatus::Succeeded);
    assert_eq!(
        host_port_of(&h, &serving_name(&h.dep.id)),
        Some(failed_port)
    );
}

/// Records which ports were probed.
struct Recording {
    ports: Arc<Mutex<Vec<u16>>>,
}
impl HttpTransport for Recording {
    fn get(&self, port: u16, _p: &str, _t: Duration) -> Observation {
        self.ports.lock().unwrap().push(port);
        Observation::Status(200)
    }
    fn is_simulated(&self) -> bool {
        true
    }
}

#[test]
fn a_port_taken_between_the_check_and_the_start_is_never_probed_or_reported_healthy() {
    let mut h = Harness::new();
    h.seed_built_image();
    // Occupancy says 38000 is free; another process takes it before Docker binds.
    h.docker.with(|s| {
        s.taken_ports.insert(38000);
    });
    let ports = Arc::new(Mutex::new(Vec::new()));
    let mut b = backend_with(
        &h,
        Recording {
            ports: ports.clone(),
        },
        |_| {},
    );
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let probed = ports.lock().unwrap().clone();
    assert!(
        !probed.contains(&38000),
        "the contested port was never probed: {probed:?}"
    );
    assert_eq!(host_port_of(&h, &replacement_name(&h.dep.id)), Some(38001));
    assert!(
        probed
            .iter()
            .filter(|p| **p >= 38000 && **p < 39000)
            .all(|p| *p == 38001),
        "{probed:?}"
    );
    assert!(h.docker.container(&serving_name(&h.dep.id)).is_none());
}

#[test]
fn a_stopped_containers_unreadable_port_bindings_fail_allocation_instead_of_being_ignored() {
    let mut h = Harness::new();
    h.seed_built_image();
    let id = h
        .docker
        .with(|s| s.add_container("stopped-one", vec![], false));
    h.docker.fault(
        Some("GET"),
        &format!("/containers/{id}/json"),
        1_000,
        Action::Respond(200, "{\"HostConfig\":{\"PortBindings\":[1]}}".into()),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::PreliminaryCheck);
    assert!(msg.contains("port mappings"), "{msg}");
    assert!(!h
        .docker
        .requests()
        .iter()
        .any(|r| r.path.starts_with("/containers/create")));
}

fn run_with_stopped_container_reporting(body: &str) -> (Harness, Result<(), (Step, String)>) {
    let mut h = Harness::new();
    h.seed_built_image();
    let id = h
        .docker
        .with(|s| s.add_container("stopped-one", vec![], false));
    h.docker.fault(
        Some("GET"),
        &format!("/containers/{id}/json"),
        1_000,
        Action::Respond(200, body.to_string()),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    let res = if r.status == DeploymentStatus::Succeeded {
        Ok(())
    } else {
        let f = h.dep.failure.clone().unwrap();
        Err((f.step, f.message))
    };
    (h, res)
}

#[test]
fn malformed_port_binding_entries_fail_allocation_closed() {
    for (what, body) in [
        (
            "non-numeric HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"abc"}]}}}"#,
        ),
        (
            "out-of-range HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"70000"}]}}}"#,
        ),
        (
            "negative HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"-1"}]}}}"#,
        ),
        (
            "port range",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"38000-38010"}]}}}"#,
        ),
        (
            "padded HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":" 38000"}]}}}"#,
        ),
        (
            "numeric HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":38000}]}}}"#,
        ),
        (
            "binding is a string",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":["38000"]}}}"#,
        ),
        (
            "binding is a number",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[38000]}}}"#,
        ),
        (
            "binding is null",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[null]}}}"#,
        ),
        (
            "a later binding is bad",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"38000"},{"HostPort":"x"}]}}}"#,
        ),
    ] {
        let (h, res) = run_with_stopped_container_reporting(body);
        let (step, msg) = res.expect_err(what);
        assert_eq!(step, Step::PreliminaryCheck, "{what}");
        assert!(msg.contains("port mappings"), "{what}: {msg}");
        assert!(
            !h.docker
                .requests()
                .iter()
                .any(|r| r.path.starts_with("/containers/create")),
            "{what}: nothing was created on a guess"
        );
    }
}

#[test]
fn unassigned_port_bindings_are_valid_and_hold_no_port() {
    for (what, body) in [
        (
            "empty HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostIp":"","HostPort":""}]}}}"#,
        ),
        (
            "zero HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":"0"}]}}}"#,
        ),
        (
            "absent HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostIp":"127.0.0.1"}]}}}"#,
        ),
        (
            "null HostPort",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":null}]}}}"#,
        ),
        (
            "empty list",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":[]}}}"#,
        ),
        (
            "null list",
            r#"{"HostConfig":{"PortBindings":{"8080/tcp":null}}}"#,
        ),
        ("no bindings", r#"{"HostConfig":{"PortBindings":{}}}"#),
        ("null bindings", r#"{"HostConfig":{"PortBindings":null}}"#),
        ("no HostConfig", r#"{}"#),
    ] {
        let (h, res) = run_with_stopped_container_reporting(body);
        res.unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(
            host_port_of(&h, &preliminary_name(&h.dep.id)),
            Some(39000),
            "{what}"
        );
        assert_eq!(
            host_port_of(&h, &serving_name(&h.dep.id)),
            Some(38000),
            "{what}: nothing was reserved"
        );
    }
}

#[test]
fn valid_bindings_still_reserve_their_ports_next_to_unassigned_ones() {
    let body = r#"{"HostConfig":{"PortBindings":{"8080/tcp":[{"HostPort":""},{"HostPort":"38000"}],"9090/tcp":[{"HostPort":"38001"}]}}}"#;
    let (h, res) = run_with_stopped_container_reporting(body);
    res.unwrap();
    assert_eq!(host_port_of(&h, &serving_name(&h.dep.id)), Some(38002));
}

#[test]
fn two_collisions_fail_the_deployment_instead_of_looping_or_borrowing_a_port() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.docker.with(|s| {
        s.taken_ports.insert(38000);
        s.taken_ports.insert(38001);
    });
    let ports = Arc::new(Mutex::new(Vec::new()));
    let mut b = backend_with(
        &h,
        Recording {
            ports: ports.clone(),
        },
        |_| {},
    );
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::Cutover, "{msg}");
    assert!(
        ports.lock().unwrap().iter().all(|p| *p == 39000),
        "only the test port"
    );
    assert!(h.objects_for_dep().is_empty());
}

// ------------------------------------------------------------------ 3. health probing

/// A disposable HTTP server. Each connection is handled on its own thread.
fn serve(handler: impl Fn(std::net::TcpStream) + Send + Sync + 'static) -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    serve_on(l, handler)
}

fn serve_on(
    l: TcpListener,
    handler: impl Fn(std::net::TcpStream) + Send + Sync + 'static,
) -> (u16, Arc<AtomicUsize>) {
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let (h2, handler) = (hits.clone(), Arc::new(handler));
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            h2.fetch_add(1, Ordering::SeqCst);
            let handler = handler.clone();
            std::thread::spawn(move || handler(c));
        }
    });
    (port, hits)
}

fn read_request(c: &mut std::net::TcpStream) {
    let mut buf = [0u8; 2048];
    let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = c.read(&mut buf);
}

fn reply(c: &mut std::net::TcpStream, status: &str, headers: &str, body: &str) {
    read_request(c);
    let _ = write!(
        c,
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

const T: Duration = Duration::from_secs(2);

#[test]
fn a_response_must_carry_the_deployments_marker_or_it_is_not_a_success() {
    let (port, _) = serve(|mut c| reply(&mut c, "200 OK", "X-Harpoon-Deployment: dep-A\r\n", "hi"));
    let want = |v: &str| ResponseMarker::header("x-harpoon-deployment", v).unwrap();
    assert_eq!(
        TcpHttp.get_expecting(port, "/", T, Some(&want("dep-A"))),
        Observation::Status(200),
        "header names are case-insensitive"
    );
    // a different deployment's marker (an older release still listening)
    let o = TcpHttp.get_expecting(port, "/", T, Some(&want("dep-B")));
    assert!(
        matches!(&o, Observation::ConnectFailed(m) if m.contains("marker") && m.contains("different listener")),
        "{o:?}"
    );
    // a prefix is not a match
    assert!(matches!(
        TcpHttp.get_expecting(port, "/", T, Some(&want("dep-"))),
        Observation::ConnectFailed(_)
    ));
    // no marker required: today's behaviour
    assert_eq!(
        TcpHttp.get_expecting(port, "/", T, None),
        Observation::Status(200)
    );

    let (plain, _) = serve(|mut c| reply(&mut c, "200 OK", "", "hello"));
    assert!(matches!(
        TcpHttp.get_expecting(plain, "/", T, Some(&want("dep-A"))),
        Observation::ConnectFailed(_)
    ));
}

#[test]
fn a_body_marker_is_found_in_the_body_only() {
    let (port, _) = serve(|mut c| {
        reply(
            &mut c,
            "200 OK",
            "X-Note: marker-123\r\n",
            "<p>build marker-123</p>",
        )
    });
    let (hdr_only, _) = serve(|mut c| reply(&mut c, "200 OK", "X-Note: marker-123\r\n", "nothing"));
    let m = ResponseMarker::body_contains("marker-123").unwrap();
    assert_eq!(
        TcpHttp.get_expecting(port, "/", T, Some(&m)),
        Observation::Status(200)
    );
    assert!(
        matches!(
            TcpHttp.get_expecting(hdr_only, "/", T, Some(&m)),
            Observation::ConnectFailed(_)
        ),
        "the text in a header does not satisfy a body marker"
    );
}

#[test]
fn empty_markers_cannot_be_built_and_a_transport_that_cannot_check_one_fails_closed() {
    assert!(ResponseMarker::header("", "v").is_err());
    assert!(ResponseMarker::header("n", "  ").is_err());
    assert!(ResponseMarker::body_contains("").is_err());
    // a hand-built empty marker still matches nothing
    let (port, _) = serve(|mut c| reply(&mut c, "200 OK", "X-A: \r\n", "x"));
    for m in [
        ResponseMarker::Header {
            name: "X-A".into(),
            value: String::new(),
        },
        ResponseMarker::BodyContains(String::new()),
    ] {
        assert!(matches!(
            TcpHttp.get_expecting(port, "/", T, Some(&m)),
            Observation::ConnectFailed(_)
        ));
    }
    // the default implementation (used by test doubles)
    let m = ResponseMarker::header("X-A", "v").unwrap();
    assert!(matches!(
        ok200().get_expecting(1, "/", T, Some(&m)),
        Observation::ConnectFailed(msg) if msg.contains("cannot verify")
    ));
}

struct Direct<'a> {
    port: u16,
    marker: Option<&'a ResponseMarker>,
    running: bool,
}
impl Probe for Direct<'_> {
    fn attempt(&mut self, at_ms: u64) -> Attempt {
        Attempt {
            at_ms,
            observation: TcpHttp.get_expecting(
                self.port,
                "/",
                Duration::from_millis(300),
                self.marker,
            ),
            container_running: self.running,
        }
    }
}

fn health(
    port: u16,
    marker: Option<&ResponseMarker>,
    class: StatusClass,
    running: bool,
) -> harpoon_deploy::health::CheckEvidence {
    let spec = HealthSpec {
        path: "/".into(),
        expect: class,
        interval_ms: 5,
        consecutive_ok: 2,
        startup_deadline_ms: 400,
    };
    run_health(
        &spec,
        &format!("http://127.0.0.1:{port}/"),
        &mut Direct {
            port,
            marker,
            running,
        },
        &SystemClock,
        &SleepWaiter,
    )
}

#[test]
fn a_redirect_is_not_healthy_and_is_never_followed() {
    let (target, target_hits) = serve(|mut c| reply(&mut c, "200 OK", "", "elsewhere"));
    let loc = format!("Location: http://127.0.0.1:{target}/\r\n");
    let (port, _) = serve(move |mut c| reply(&mut c, "302 Found", &loc, ""));
    let ev = health(port, None, StatusClass::Success2xx, true);
    assert_eq!(ev.verdict, Verdict::NotHealthy);
    assert!(ev.summary().contains("HTTP 302"), "{}", ev.summary());
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        0,
        "the redirect target was never contacted"
    );
    // even the permissive class (anything below 500) does not make a redirect *to another
    // service* look like proof; it is only accepted because the operator chose that class, and the
    // marker requirement still applies on top of it
    let m = ResponseMarker::header("X-Harpoon-Deployment", "dep-A").unwrap();
    assert_eq!(
        health(port, Some(&m), StatusClass::NotServerError, true).verdict,
        Verdict::NotHealthy
    );
}

#[test]
fn a_stale_listener_with_an_old_marker_never_passes() {
    let (port, _) =
        serve(|mut c| reply(&mut c, "200 OK", "X-Harpoon-Deployment: dep-OLD\r\n", "ok"));
    let want = ResponseMarker::header("X-Harpoon-Deployment", "dep-NEW").unwrap();
    let ev = health(port, Some(&want), StatusClass::Success2xx, true);
    assert_eq!(ev.verdict, Verdict::NotHealthy);
    assert!(ev
        .attempts
        .iter()
        .all(|a| matches!(a.observation, Observation::ConnectFailed(_))));
    assert!(ev.validate().is_err());
    // the control: the right marker passes
    let right = ResponseMarker::header("X-Harpoon-Deployment", "dep-OLD").unwrap();
    let ev = health(port, Some(&right), StatusClass::Success2xx, true);
    assert_eq!(ev.verdict, Verdict::Healthy);
    ev.validate().unwrap();
}

#[test]
fn a_listener_that_accepts_and_hangs_up_is_a_failed_attempt() {
    let (port, hits) = serve(drop);
    let ev = health(port, None, StatusClass::Success2xx, true);
    assert_eq!(ev.verdict, Verdict::NotHealthy);
    assert!(hits.load(Ordering::SeqCst) >= 1);
}

#[test]
fn a_server_that_never_answers_or_drips_bytes_cannot_outlast_the_attempt_timeout() {
    let (silent, _) = serve(|mut c| {
        read_request(&mut c);
        std::thread::sleep(Duration::from_secs(5));
    });
    let t0 = Instant::now();
    let o = TcpHttp.get(silent, "/", Duration::from_millis(300));
    assert!(
        matches!(&o, Observation::ConnectFailed(m) if m.contains("timed out")),
        "{o:?}"
    );
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());

    // one byte every 100 ms would keep a per-read timeout alive forever
    let (drip, _) = serve(|mut c| {
        read_request(&mut c);
        for b in b"HTTP/1.1 200 OK\r\nX-Harpoon-Deployment: dep-A\r\n\r\nbody"
            .iter()
            .cycle()
            .take(500)
        {
            if c.write_all(&[*b]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let m = ResponseMarker::header("X-Harpoon-Deployment", "dep-A").unwrap();
    let t0 = Instant::now();
    let o = TcpHttp.get_expecting(drip, "/", Duration::from_millis(400), Some(&m));
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    // Whether the marker arrived inside the deadline or not, the attempt ended on time.
    assert!(
        matches!(o, Observation::Status(200) | Observation::ConnectFailed(_)),
        "{o:?}"
    );
    let o = TcpHttp.get(drip, "/", Duration::from_millis(250));
    let _ = o;
}

#[test]
fn oversized_response_headers_are_refused() {
    let (port, _) = serve(|mut c| {
        read_request(&mut c);
        let _ = c.write_all(b"HTTP/1.1 200 OK\r\n");
        let junk = format!("X-Pad: {}\r\n", "a".repeat(1000));
        for _ in 0..40 {
            if c.write_all(junk.as_bytes()).is_err() {
                return;
            }
        }
    });
    let m = ResponseMarker::header("X-Pad", "b").unwrap();
    let o = TcpHttp.get_expecting(port, "/", T, Some(&m));
    assert!(
        matches!(&o, Observation::ConnectFailed(m) if m.contains("too large")),
        "{o:?}"
    );

    // a connection that ends before the header block is complete is not a response
    let (cut, _) = serve(|mut c| {
        read_request(&mut c);
        let _ = c.write_all(b"HTTP/1.1 200 OK\r\nX-Harpoon-Deployment: dep-A\r\n");
    });
    let m = ResponseMarker::header("X-Harpoon-Deployment", "dep-A").unwrap();
    let o = TcpHttp.get_expecting(cut, "/", T, Some(&m));
    assert!(
        matches!(&o, Observation::ConnectFailed(m) if m.contains("headers")),
        "{o:?}"
    );
}

#[test]
fn a_container_that_exits_is_never_healthy_even_while_something_still_answers() {
    let (port, _) = serve(|mut c| reply(&mut c, "200 OK", "", "ok"));
    let ev = health(port, None, StatusClass::Success2xx, false);
    assert_ne!(ev.verdict, Verdict::Healthy, "{}", ev.summary());
    assert!(ev.validate().is_err());
}

#[test]
fn a_container_that_stops_mid_check_fails_the_check_though_the_port_keeps_answering() {
    let mut h = Harness::new();
    h.seed_built_image();
    let pre = preliminary_name(&h.dep.id);
    // from the second inspect on, Docker says the container stopped
    h.docker.fault_after(
        Some("GET"),
        "/json",
        1,
        1_000,
        Action::Race(Arc::new(move |s| {
            for c in s.containers.iter_mut().filter(|c| c.name == pre) {
                c.running = false;
                c.exit_code = 1;
            }
        })),
    );
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let (_, ev) = r
        .evidence
        .iter()
        .find(|(s, _)| *s == Step::PreliminaryCheck)
        .unwrap();
    assert_eq!(ev.verdict, Verdict::Exited, "{}", ev.summary());
}

// -- the same questions at deployment level, with the real transport and an unrelated listener

fn real_backend(h: &Harness, marker: Option<ResponseMarker>) -> DockerBackend<TcpHttp> {
    let mut cfg = config(&h.ws, &h.docker);
    cfg.health_marker = marker;
    cfg.health.startup_deadline_ms = 600;
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), TcpHttp)
        .unwrap()
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 << 30))
}

/// Something unrelated holds the first test port (39000). Docker, in the fake, never binds
/// anything, so the *only* listener on that port is the unrelated one — exactly the situation a
/// wildcard-versus-loopback bind race could create on a real host.
fn with_unrelated_listener(headers: &'static str, f: impl FnOnce(&mut Harness)) {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(l) = TcpListener::bind("127.0.0.1:39000") else {
        eprintln!("port 39000 is in use on this machine; unrelated-listener case skipped");
        return;
    };
    serve_on(l, move |mut c| {
        reply(&mut c, "200 OK", headers, "I am not your app")
    });
    let mut h = Harness::new();
    h.seed_built_image();
    f(&mut h);
}

#[test]
fn without_a_marker_an_unrelated_listener_is_indistinguishable_from_the_app() {
    // This documents the hazard the marker exists for. It is a statement about Harpoon's probe,
    // not about Docker.
    with_unrelated_listener("", |h| {
        let mut b = real_backend(h, None);
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        let (_, ev) = r
            .evidence
            .iter()
            .find(|(s, _)| *s == Step::PreliminaryCheck)
            .unwrap();
        assert_eq!(
            ev.verdict,
            Verdict::Healthy,
            "an HTTP 200 alone proves nothing about who answered"
        );
    });
}

#[test]
fn with_a_marker_an_unrelated_listener_fails_the_preliminary_check_and_nothing_goes_live() {
    with_unrelated_listener("", |h| {
        let m = ResponseMarker::header("X-Harpoon-Deployment", &h.dep.id.to_string()).unwrap();
        let mut b = real_backend(h, Some(m));
        let (step, msg) = fails(h, &mut b);
        assert_eq!(step, Step::PreliminaryCheck, "{msg}");
        assert!(msg.contains("marker"), "{msg}");
        assert!(
            h.docker.container(&serving_name(&h.dep.id)).is_none()
                && host_port_of(h, &serving_name(&h.dep.id)).is_none(),
            "no application container was created"
        );
        assert!(h.objects_for_dep().is_empty());
    });
}

#[test]
fn with_a_marker_a_listener_that_does_present_it_passes() {
    // the positive control; its listener plays the application
    with_unrelated_listener("X-Harpoon-Deployment: PLACEHOLDER\r\n", |_| {});
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(l) = TcpListener::bind("127.0.0.1:39000") else {
        return;
    };
    let mut h = Harness::new();
    h.seed_built_image();
    let hdr = format!("X-Harpoon-Deployment: {}\r\n", h.dep.id);
    serve_on(l, move |mut c| reply(&mut c, "200 OK", &hdr, "app"));
    let m = ResponseMarker::header("X-Harpoon-Deployment", &h.dep.id.to_string()).unwrap();
    let mut b = real_backend(&h, Some(m));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    let (_, ev) = r
        .evidence
        .iter()
        .find(|(s, _)| *s == Step::PreliminaryCheck)
        .unwrap();
    assert_eq!(ev.verdict, Verdict::Healthy, "{}", ev.summary());
    assert!(!ev.simulated);
}

#[test]
fn a_marker_with_a_transport_that_cannot_verify_it_fails_the_deployment() {
    let mut h = Harness::new();
    h.seed_built_image();
    let m = ResponseMarker::header("X-Harpoon-Deployment", "x").unwrap();
    let mut b = backend_with(&h, ok200(), |c| c.health_marker = Some(m.clone()));
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::PreliminaryCheck);
    assert!(msg.contains("cannot verify"), "{msg}");
}

// ------------------------------------------------------------------ 4. isolation audit

fn foreign_world(h: &Harness) -> (Vec<(String, String)>, Vec<String>) {
    // containers (running and stopped, unlabelled and Harpoon-labelled for *another* deployment),
    // and images, none of them this deployment's
    let mut other_ids = IdsLike::new();
    let other = other_ids.labels(h);
    h.docker.with(|s| {
        s.add_container("bystander-running", vec![], true);
        s.add_container("bystander-stopped", vec![], false);
        s.add_container("harpoon-of-another-deployment", other.clone(), true);
        s.add_image("redis:7", vec![]);
        s.add_image("harpoon/other-app:dep-zzz", other.clone());
    });
    let ctrs: Vec<(String, String)> = h.docker.with(|s| {
        s.containers
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect()
    });
    let imgs: Vec<String> = h.docker.with(|s| {
        s.images
            .iter()
            .filter(|i| {
                i.tags
                    .iter()
                    .any(|t| t == "redis:7" || t.contains("other-app"))
            })
            .map(|i| i.id.clone())
            .collect()
    });
    (ctrs, imgs)
}

struct IdsLike;
impl IdsLike {
    fn new() -> IdsLike {
        IdsLike
    }
    fn labels(&mut self, h: &Harness) -> Vec<(String, String)> {
        // labels for an application/deployment that is not the one under test
        let mut l = labels(&h.dep.application_id, &h.dep.id);
        for (_, v) in l.iter_mut().filter(|(k, _)| k == "io.harpoon.deployment") {
            *v = "dep-someone-else".into();
        }
        l
    }
}

fn assert_nothing_foreign_was_touched(h: &Harness, ctrs: &[(String, String)], imgs: &[String]) {
    let reqs = h.docker.requests();
    for (id, name) in ctrs {
        for r in reqs.iter().filter(|r| r.method != "GET") {
            assert!(
                !r.path.contains(id.as_str()) && !r.path.contains(&id[..12]),
                "{} {} touches foreign container {name}",
                r.method,
                r.path
            );
            assert!(
                !r.path.contains(&format!("/{name}/")) && !r.path.ends_with(&format!("/{name}")),
                "{} {} touches foreign container {name}",
                r.method,
                r.path
            );
        }
        let c = h
            .docker
            .with(|s| s.containers.iter().find(|c| c.id == *id).cloned());
        let c = c.unwrap_or_else(|| panic!("foreign container {name} disappeared"));
        let started = name != "bystander-stopped";
        assert_eq!(c.running, started, "{name} changed state");
    }
    for id in imgs {
        assert!(
            h.docker.with(|s| s.images.iter().any(|i| i.id == *id)),
            "foreign image {id} was removed"
        );
        for r in reqs.iter().filter(|r| r.method != "GET") {
            assert!(!r.path.contains(id.as_str()), "{} {}", r.method, r.path);
        }
    }
    // every mutating request is one of the supported first-slice operations, and every removal
    // is by full container id (never by a name or tag someone else could reuse)
    for r in reqs.iter().filter(|r| r.method != "GET") {
        let p = r.path.as_str();
        let ok = p.starts_with("/containers/create?")
            || (r.method == "POST"
                && p.starts_with("/containers/")
                && (p.contains("/start") || p.contains("/stop")))
            || (r.method == "DELETE" && p.starts_with("/containers/"))
            || (r.method == "DELETE" && p.starts_with("/images/"));
        assert!(ok, "unsupported mutation: {} {p}", r.method);
        if r.method == "DELETE" && p.starts_with("/containers/") {
            let id = p["/containers/".len()..].split('?').next().unwrap();
            assert!(
                id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "{p}"
            );
            assert!(p.contains("force=false") && p.contains("v=false"), "{p}");
        }
        if r.method == "DELETE" && p.starts_with("/images/") {
            assert!(
                p.contains("force=false") && p.contains("noprune=true"),
                "{p}"
            );
        }
    }
}

#[test]
fn a_successful_deployment_beside_foreign_resources_changes_none_of_them() {
    let mut h = Harness::new();
    h.seed_built_image();
    let (ctrs, imgs) = foreign_world(&h);
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    assert_nothing_foreign_was_touched(&h, &ctrs, &imgs);
}

#[test]
fn failing_deployments_beside_foreign_resources_clean_up_only_their_own() {
    type Setup = fn(&mut Harness) -> Scripted;
    let scenarios: [(&str, Setup); 4] = [
        ("final check fails", |_| {
            let t = Scripted::always(Observation::Status(503));
            *t.answers.borrow_mut() = vec![Observation::Status(200), Observation::Status(200)];
            t
        }),
        ("preliminary check fails", |_| {
            Scripted::always(Observation::Status(500))
        }),
        ("port collision twice", |h| {
            h.docker.with(|s| {
                s.taken_ports.extend([38000, 38001]);
            });
            ok200()
        }),
        ("a foreign container holds our container name", |h| {
            let pre = preliminary_name(&h.dep.id);
            h.docker.with(|s| s.add_container(&pre, vec![], true));
            ok200()
        }),
    ];
    for (what, setup) in scenarios {
        let mut h = Harness::new();
        h.seed_built_image();
        let (mut ctrs, imgs) = foreign_world(&h);
        let t = setup(&mut h);
        // a same-named foreign container is part of the foreign world too
        h.docker.with(|s| {
            for c in &s.containers {
                if !ctrs.iter().any(|(id, _)| *id == c.id) {
                    ctrs.push((c.id.clone(), c.name.clone()));
                }
            }
        });
        let mut b = backend_with(&h, t, |_| {});
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Failed, "{what}");
        assert_nothing_foreign_was_touched(&h, &ctrs, &imgs);
    }
}

#[test]
fn a_foreign_image_on_our_tag_is_neither_overwritten_nor_removed() {
    let mut h = Harness::new();
    let foreign = h.preexisting_image(vec![]);
    let mut b = backend_with(&h, ok200(), |_| {});
    let (step, msg) = fails(&mut h, &mut b);
    assert_eq!(step, Step::Build, "{msg}");
    assert!(!buildx_ran(&h));
    assert!(h.docker.with(|s| s.images.iter().any(|i| i.id == foreign)));
    assert!(!h.docker.requests().iter().any(|r| r.method == "DELETE"));
    let _ = tag_for(&h.dep.application_id, &h.dep.id);
}

#[test]
fn created_containers_never_carry_mounts_binds_or_inherited_volumes() {
    let mut h = Harness::new();
    h.seed_built_image();
    let mut b = backend_with(&h, ok200(), |_| {});
    let mut j = StampedJournal::new();
    assert_eq!(h.run(&mut b, &mut j).status, DeploymentStatus::Succeeded);
    let creates: Vec<_> = h
        .docker
        .requests()
        .into_iter()
        .filter(|r| r.path.starts_with("/containers/create"))
        .collect();
    assert_eq!(creates.len(), 2);
    for r in creates {
        for forbidden in [
            "\"Binds\"",
            "\"Mounts\"",
            "\"Volumes\"",
            "\"VolumesFrom\"",
            "\"Privileged\":true",
            "\"NetworkMode\":\"host\"",
            "\"PidMode\"",
            "\"Devices\"",
        ] {
            assert!(!r.body.contains(forbidden), "{forbidden} in {}", r.body);
        }
    }
}
