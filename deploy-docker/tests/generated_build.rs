//! A repository without a Dockerfile: Harpoon renders one into the private build snapshot only, joins the
//! chosen network, and applies no per-application caps. Fake Docker and fake Buildx, as in B7.

mod support;

use harpoon_deploy::health::Observation;
use harpoon_deploy::state::DeploymentStatus;
use harpoon_deploy_docker::api::Limits;
use harpoon_deploy_docker::backend::DockerBackend;
use harpoon_deploy_docker::json::Json;
use harpoon_deploy_docker::plan::{detect, BuildPlan, Settings};
use support::*;

#[test]
fn a_project_without_a_dockerfile_is_built_from_a_private_generated_one() {
    let mut h = Harness::new();
    h.seed_built_image();
    let ctx = h.ws.context();
    std::fs::remove_file(ctx.join("Dockerfile")).unwrap();
    std::fs::write(
        ctx.join("package.json"),
        r#"{"scripts":{"build":"next build","start":"next start"},"dependencies":{"next":"15.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(ctx.join("package-lock.json"), "{}").unwrap();
    // the fake Buildx runs in the snapshot: keep what the builder was given
    h.ws.install_buildx(&format!(
        "cp .harpoon-generated.Dockerfile \"{d}/seen.Dockerfile\"",
        d = h.ws.resources().join("harpoon/bin").display()
    ));
    let plan = BuildPlan::resolve(
        detect(&ctx).unwrap(),
        &Settings::parse(r#"{"network":"proj_default"}"#).unwrap(),
    )
    .unwrap();
    let mut cfg = config(&h.ws, &h.docker);
    cfg.plan = Some(plan);
    cfg.network = Some("proj_default".into());
    cfg.limits = Limits::UNLIMITED;
    let mut b = DockerBackend::with_transport(
        cfg,
        Some(&h.ws.resources()),
        Scripted::always(Observation::Status(200)),
    )
    .unwrap()
    .with_entropy(test_entropy())
    .with_port_probe(NoListeners)
    .with_disk_probe(FakeDisk(10 << 30));
    let mut j = StampedJournal::new();
    let r = h.run(&mut b, &mut j);
    assert_eq!(r.status, DeploymentStatus::Succeeded, "{:?}", h.dep.failure);

    let seen = h.ws.buildx_record("seen.Dockerfile");
    assert!(seen.starts_with("FROM node:22-slim\n"), "{seen}");
    assert!(
        seen.contains("RUN npm ci\n") && seen.contains("RUN npm run build\n"),
        "{seen}"
    );
    assert!(
        seen.contains("PORT=8080") && seen.contains("exec npm start"),
        "{seen}"
    );
    // nothing was written into the user's workspace, and the snapshot is gone
    assert!(!ctx.join(".harpoon-generated.Dockerfile").exists());
    assert!(std::fs::read_dir(h.ws.scratch()).unwrap().next().is_none());

    let body = h
        .docker
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.starts_with("/containers/create"))
        .map(|r| Json::parse(&r.body).unwrap())
        .next()
        .expect("a container was created");
    let host = body.get("HostConfig").unwrap();
    assert_eq!(
        host.get("NetworkMode").and_then(Json::as_str),
        Some("proj_default")
    );
    assert_eq!(host.get("Memory").and_then(Json::as_i64), Some(0));
    assert_eq!(host.get("PidsLimit").and_then(Json::as_i64), Some(0));
}

#[test]
fn a_file_named_like_the_generated_dockerfile_is_never_shadowed() {
    let h = Harness::new();
    let ctx = h.ws.context();
    std::fs::remove_file(ctx.join("Dockerfile")).unwrap();
    std::fs::write(
        ctx.join("package.json"),
        r#"{"scripts":{"start":"node a.js"}}"#,
    )
    .unwrap();
    std::fs::write(ctx.join(".harpoon-generated.Dockerfile"), "FROM scratch\n").unwrap();
    let err = harpoon_deploy_docker::confine::confine_generated(&ctx, &ctx, "FROM x\n".into())
        .unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}
