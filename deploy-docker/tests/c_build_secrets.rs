//! Milestone C at the Docker backend: Dockerfile settings, BuildKit secret mounts, runtime secrets
//! and what is captured from the build. The fake Buildx is a shell script that plays the part of a
//! build step: it records what it was given, reads the secret file the way BuildKit would, and prints
//! planted secrets. Nothing contacts a real Docker or Buildx; layer and `docker history` exposure
//! cannot be observed here and is listed as an acceptance gap in the report.

mod support;

use harpoon_deploy::engine::RunConfig;
use harpoon_deploy::health::Observation;
use harpoon_deploy::logs::Redactor;
use harpoon_deploy::secrets::{SecretBundle, SecretRef, SecretValue};
use harpoon_deploy::state::*;
use harpoon_deploy_docker::backend::{serving_name, DockerBackend, DockerConfig};
use harpoon_deploy_docker::probe::HttpTransport;
use std::time::Duration;
use support::fake_docker::Req;
use support::*;

fn ok200() -> Scripted {
    Scripted::always(Observation::Status(200))
}

fn backend_with<T: HttpTransport>(
    h: &Harness,
    t: T,
    secrets: SecretBundle,
    f: impl FnOnce(&mut DockerConfig),
) -> DockerBackend<T> {
    let mut cfg = config(&h.ws, &h.docker);
    f(&mut cfg);
    DockerBackend::with_transport(cfg, Some(&h.ws.resources()), t)
        .expect("bundled buildx resolves")
        .with_entropy(test_entropy())
        .with_port_probe(NoListeners)
        .with_disk_probe(FakeDisk(10 * 1024 * 1024 * 1024))
        .with_secrets(secrets)
}

fn bundle(items: &[(&SecretRef, &str)]) -> SecretBundle {
    let mut b = SecretBundle::new();
    for (r, v) in items {
        b.insert(
            (*r).clone(),
            SecretValue::new(v.as_bytes().to_vec()).unwrap(),
        );
    }
    b
}

fn created(h: &Harness, name: &str) -> Option<Req> {
    h.docker.requests().into_iter().find(|r| {
        r.method == "POST"
            && r.path.starts_with("/containers/create")
            && r.path.ends_with(&format!("name={name}"))
    })
}

/// The fake build step: what BuildKit's client would do with `--secret id=X,src=FILE`.
const READS_THE_SECRET: &str = r#"
for a in "$@"; do
  case "$a" in
    id=*,src=*)
      src="${a#*src=}"
      echo "$src" > "$D/secretpath.txt"
      cat "$src" > "$D/seen.txt"
      find "$src" -perm 600 | wc -l > "$D/filemode.txt"
      find "$(dirname "$src")" -maxdepth 0 -perm 700 | wc -l > "$D/dirmode.txt"
      if grep -rqF -- "$(cat "$src")" "$PWD" 2>/dev/null; then echo leak > "$D/ctxleak.txt"; else echo clean > "$D/ctxleak.txt"; fi
      ;;
  esac
done
echo built
"#;

fn scratch_has_secret_dirs(h: &Harness) -> Vec<String> {
    std::fs::read_dir(h.ws.scratch())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("buildsecrets-"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn target_build_args_and_secret_mounts_reach_buildx_and_no_secret_is_in_argv_or_its_environment() {
    let mut h = Harness::new();
    h.seed_built_image();
    h.ws.install_buildx(READS_THE_SECRET);
    let token = SecretRef::new("global/NPM_TOKEN", 3);
    let value = "npm-token-ONE\nsecond line of a multi-line value";
    let mut b = backend_with(&h, ok200(), bundle(&[(&token, value)]), |c| {
        c.build_target = Some("release".into());
        c.build_args = vec![("NODE_ENV".into(), "production".into())];
        c.build_secrets = vec![("NPM_TOKEN".into(), token.clone())];
    });
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);

    let args = h.ws.buildx_record("args.txt");
    assert!(args.contains("--target release"), "{args}");
    assert!(args.contains("--build-arg NODE_ENV=production"), "{args}");
    assert!(args.contains("--secret id=NPM_TOKEN,src="), "{args}");
    for sink in [args.as_str(), h.ws.buildx_record("env.txt").as_str()] {
        assert!(
            !sink.contains("npm-token-ONE") && !sink.contains("multi-line"),
            "{sink}"
        );
    }
    // BuildKit read exactly the value, from a 0600 file in a 0700 directory, outside the context
    assert_eq!(h.ws.buildx_record("seen.txt"), value);
    assert_eq!(h.ws.buildx_record("filemode.txt").trim(), "1");
    assert_eq!(h.ws.buildx_record("dirmode.txt").trim(), "1");
    assert_eq!(
        h.ws.buildx_record("ctxleak.txt").trim(),
        "clean",
        "the secret is not in the build context"
    );
    let path = h.ws.buildx_record("secretpath.txt");
    let cwd = h.ws.buildx_record("cwd.txt");
    assert!(!path.trim().starts_with(cwd.trim()), "{path} inside {cwd}");
    assert!(
        path.trim()
            .starts_with(&h.ws.scratch().display().to_string()),
        "{path}"
    );
    // and it is gone again
    assert!(!std::path::Path::new(path.trim()).exists());
    assert_eq!(scratch_has_secret_dirs(&h), Vec::<String>::new());
    // a build-only secret is not a runtime variable
    let c = created(&h, &serving_name(&h.dep.id)).expect("serving container");
    assert!(
        !c.body.contains("npm-token-ONE") && !c.body.contains("NPM_TOKEN"),
        "{}",
        c.body
    );
}

#[test]
fn secret_files_are_removed_after_failure_and_timeout_and_a_crashs_leftovers_are_swept() {
    let token = SecretRef::new("global/NPM_TOKEN", 1);
    // failure
    {
        let mut h = Harness::new();
        h.ws.install_buildx(&format!("{READS_THE_SECRET}\nexit 3"));
        let mut b = backend_with(&h, ok200(), bundle(&[(&token, "tok")]), |c| {
            c.build_secrets = vec![("NPM_TOKEN".into(), token.clone())];
        });
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Failed);
        assert_eq!(
            scratch_has_secret_dirs(&h),
            Vec::<String>::new(),
            "after a failed build"
        );
    }
    // timeout: the whole process group is killed and the files are still removed
    {
        let mut h = Harness::new();
        h.ws.install_buildx(&format!("{READS_THE_SECRET}\nsleep 30 &\nwait"));
        let mut b = backend_with(&h, ok200(), bundle(&[(&token, "tok")]), |c| {
            c.build_secrets = vec![("NPM_TOKEN".into(), token.clone())];
        });
        let mut j = StampedJournal::new();
        let mut cfg = RunConfig::default();
        cfg.budgets.build_ms = 500;
        let t0 = std::time::Instant::now();
        let r = h.run_with(&mut b, &mut j, &cfg);
        assert!(t0.elapsed() < Duration::from_secs(8));
        assert!(h.dep.failure.as_ref().unwrap().timed_out);
        assert_eq!(r.status, DeploymentStatus::Failed);
        assert_eq!(
            scratch_has_secret_dirs(&h),
            Vec::<String>::new(),
            "after a timed-out build"
        );
    }
    // a hard kill leaves a directory behind; the next build removes it before doing anything else
    {
        let mut h = Harness::new();
        h.seed_built_image();
        h.ws.install_buildx(READS_THE_SECRET);
        let stale = h.ws.scratch().join("buildsecrets-0123456789abcdef");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("s0"), "left-by-a-crash").unwrap();
        let mut b = backend_with(&h, ok200(), bundle(&[(&token, "tok")]), |c| {
            c.build_secrets = vec![("NPM_TOKEN".into(), token.clone())];
        });
        let mut j = StampedJournal::new();
        let r = h.run(&mut b, &mut j);
        assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
        assert!(
            !stale.exists(),
            "a leftover from a crashed build is swept at the next build"
        );
    }
}

#[test]
fn a_missing_build_secret_stops_before_buildx_runs() {
    let h = Harness::new();
    let mut h = h;
    let token = SecretRef::new("global/NPM_TOKEN", 1);
    let mut b = backend_with(&h, ok200(), SecretBundle::new(), |c| {
        c.build_secrets = vec![("NPM_TOKEN".into(), token.clone())];
    });
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert_eq!(f.step, Step::Build);
    assert!(
        f.message.contains("NPM_TOKEN") && f.message.contains("not available"),
        "{}",
        f.message
    );
    assert_eq!(h.ws.buildx_record("args.txt"), "", "Buildx never started");
    assert!(h.docker.with(|s| s.containers.is_empty()));
}

#[test]
fn captured_build_output_masks_planted_secrets_of_every_shape_and_keeps_its_order() {
    let mut h = Harness::new();
    let one = SecretRef::new("g/ONE", 1);
    let three = SecretRef::new("g/THREE", 1);
    let eight = SecretRef::new("g/EIGHT", 1);
    let forty = SecretRef::new("g/FORTY", 1);
    let multi = SecretRef::new("g/PEM", 1);
    let long = SecretRef::new("g/LONG", 1);
    let inner = SecretRef::new("g/INNER", 1);
    let v1 = "§";
    let v3 = "Q#7";
    let v8 = "Zk4_mW9p";
    let v40 = "0123456789abcdefghijklmnopqrstuvwxyzABCD";
    let vm = "-----BEGIN KEY-----\nAAAA-line-one\nBBBB-line-two\n-----END KEY-----";
    let vlong = "OVERLAP-secret-long-form";
    let vinner = "secret-long";
    h.ws.install_buildx(&format!(
        r#"
echo "first line"
echo "one={v1} three={v3} eight={v8}"
printf 'forty split: 0123456789abcdefghij'
sleep 0.2
printf 'klmnopqrstuvwxyzABCD done\n'
printf '%s\n' '{vm}'
echo "nested {vlong} and {vinner} alone"
echo "last line"
exit 4
"#
    ));
    let items: Vec<(&SecretRef, &str)> = vec![
        (&one, v1),
        (&three, v3),
        (&eight, v8),
        (&forty, v40),
        (&multi, vm),
        (&long, vlong),
        (&inner, vinner),
    ];
    let named: Vec<(&str, &str)> = vec![
        ("ONE", v1),
        ("THREE", v3),
        ("EIGHT", v8),
        ("FORTY", v40),
        ("PEM", vm),
        ("LONG", vlong),
        ("INNER", vinner),
    ];
    let mut b = backend_with(&h, ok200(), bundle(&items), |_| {});
    let mut j = StampedJournal::new();
    let cfg = RunConfig {
        redactor: Redactor::new(&named),
        ..RunConfig::default()
    };
    let r = h.run_with(&mut b, &mut j, &cfg);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let log = &r.logs.iter().find(|(s, _)| *s == Step::Build).unwrap().1;
    for v in [
        v1,
        v3,
        v8,
        v40,
        "AAAA-line-one",
        "BBBB-line-two",
        "BEGIN KEY",
        vlong,
        vinner,
    ] {
        assert!(!log.text.contains(v), "{v:?} leaked:\n{}", log.text);
    }
    for m in [
        "[REDACTED:ONE]",
        "[REDACTED:THREE]",
        "[REDACTED:EIGHT]",
        "[REDACTED:FORTY]",
        "[REDACTED:PEM]",
        "[REDACTED:LONG]",
        "[REDACTED:INNER]",
    ] {
        assert!(log.text.contains(m), "{m} missing:\n{}", log.text);
    }
    // structure and order survive: the first and last lines are where they were
    let lines: Vec<&str> = log.text.lines().collect();
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle}"))
    };
    assert!(
        at("context snapshot") < at("first line"),
        "the backend's own line comes first"
    );
    assert!(
        at("first line") < at("one=")
            && at("one=") < at("forty split")
            && at("forty split") < at("nested")
    );
    assert!(at("nested") < at("last line"));
    assert_eq!(lines.last(), Some(&"last line"));
    let f = h.dep.failure.as_ref().unwrap();
    assert!(
        !f.message.contains(v40) && !f.message.contains(v8),
        "{}",
        f.message
    );
    assert!(!log.summary.withheld);
}

#[test]
fn when_redaction_fails_the_raw_build_output_is_withheld_and_the_record_says_so() {
    let mut h = Harness::new();
    h.ws.install_buildx("echo 'registry password is hunter2-planted'\necho second\nexit 2");
    let mut b = backend_with(&h, ok200(), SecretBundle::new(), |_| {});
    let mut j = StampedJournal::new();
    let cfg = RunConfig {
        redactor: Redactor::failing(),
        ..RunConfig::default()
    };
    let r = h.run_with(&mut b, &mut j, &cfg);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let log = &r.logs.iter().find(|(s, _)| *s == Step::Build).unwrap().1;
    assert!(log.summary.withheld, "{:?}", log.summary);
    assert!(
        log.text.contains("withheld")
            && !log.text.contains("hunter2")
            && !log.text.contains("second"),
        "{}",
        log.text
    );
}

#[test]
fn runtime_secrets_go_only_to_the_container_and_the_pin_holds_references_not_values() {
    let mut h = Harness::new();
    h.seed_built_image();
    let db = SecretRef::new("app:a@prod/DB_PASSWORD", 2);
    let value = "pw-v2-planted-Zx81";
    let mut b = backend_with(&h, ok200(), bundle(&[(&db, value)]), |c| {
        c.env = vec!["REGION=eu".into()];
        c.runtime_secrets = vec![("DB_PASSWORD".into(), db.clone())];
        c.snapshot_id = Some("dep_00000000000000000000000001".into());
        c.build_identity = Some("dockerfile=Dockerfile;target=-;args=;secrets=".into());
    });
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);
    let c = created(&h, &serving_name(&h.dep.id)).expect("serving container");
    assert!(
        c.body.contains(&format!("DB_PASSWORD={value}")) && c.body.contains("REGION=eu"),
        "{}",
        c.body
    );
    let pin = h.dep.pinned.as_ref().expect("pinned");
    assert_eq!(pin.secrets, vec![("DB_PASSWORD".to_string(), db.clone())]);
    assert!(
        !pin.encode().contains(value) && pin.encode().starts_with("v3;"),
        "{}",
        pin.encode()
    );
    assert!(!format!("{pin:?}").contains(value) && !format!("{:?}", h.dep).contains(value));
    // nothing Harpoon wrote down holds it
    for (_, rec) in &j.stamps {
        assert!(!format!("{rec:?}").contains(value));
    }
}

#[test]
fn a_runtime_secret_that_cannot_be_resolved_creates_no_container() {
    let mut h = Harness::new();
    h.seed_built_image();
    let db = SecretRef::new("app:a@prod/DB_PASSWORD", 2);
    let mut b = backend_with(&h, ok200(), SecretBundle::new(), |c| {
        c.runtime_secrets = vec![("DB_PASSWORD".into(), db.clone())];
    });
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Failed);
    let f = h.dep.failure.as_ref().unwrap();
    assert!(
        f.message.contains("DB_PASSWORD") && f.message.contains("not available"),
        "{}",
        f.message
    );
    assert!(
        h.docker.with(|s| s.containers.is_empty()),
        "no container was created"
    );
}
