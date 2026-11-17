//! The storage inventory against hand-written Docker 28 shaped documents (from the Engine API docs, not captured from a
//! live engine) and a fake socket. It asks only two GETs, never parses a truncated body, never turns "unknown" into
//! zero, and says what it can prove about ownership and what it cannot.

use harpoon_deploy_docker::api::DockerApi;
use harpoon_deploy_docker::json::Json;
use harpoon_deploy_docker::storage::{
    build, fetch, Document, FactKind, JournalFact, JournalFacts, MAX_BODY,
};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const A: &str = "sha256:aaaa000000000000000000000000000000000000000000000000000000000001"; // serving release image
const OLD: &str = "sha256:bbbb000000000000000000000000000000000000000000000000000000000002"; // superseded release image
const STOPPED: &str = "sha256:cccc000000000000000000000000000000000000000000000000000000000003"; // used by a stopped container
const TAGGED: &str = "sha256:dddd000000000000000000000000000000000000000000000000000000000004"; // tagged, nothing uses it
const DANGLING: &str = "sha256:eeee000000000000000000000000000000000000000000000000000000000005";
const ADOPT: &str = "sha256:1111000000000000000000000000000000000000000000000000000000000007"; // Harpoon labels, no journal record
const PG: &str = "sha256:ffff000000000000000000000000000000000000000000000000000000000006";
const LBL: &str =
    r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_1","io.harpoon.deployment":"dep_1"}"#;
const LBL9: &str =
    r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_9","io.harpoon.deployment":"dep_9"}"#;
const LBL2: &str =
    r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_1","io.harpoon.deployment":"dep_2"}"#;

fn df() -> String {
    format!(
        r#"{{
 "LayersSize":943718400,
 "Images": [
  {{"Id":"{A}","RepoTags":["web:2"],"Labels":{LBL2},"Size":524288000,"SharedSize":314572800,"Containers":1}},
  {{"Id":"{OLD}","RepoTags":["web:1"],"Labels":{LBL},"Size":503316480,"SharedSize":314572800,"Containers":0}},
  {{"Id":"{STOPPED}","RepoTags":["tools:1"],"Labels":null,"Size":209715200,"SharedSize":-1,"Containers":1}},
  {{"Id":"{TAGGED}","RepoTags":["scratch:1"],"Labels":{{}},"Size":104857600,"SharedSize":0,"Containers":0}},
  {{"Id":"{DANGLING}","RepoTags":["<none>:<none>"],"Size":52428800,"Containers":0}},
  {{"Id":"{ADOPT}","RepoTags":["adopted:1"],"Labels":{LBL9},"Size":31457280,"SharedSize":0,"Containers":0}},
  {{"Id":"{PG}","RepoTags":["postgres:15"],"Labels":null,"Size":419430400,"SharedSize":0,"Containers":1}}
 ],
 "Containers": [
  {{"Id":"c_web","SizeRw":10485760,"SizeRootFs":534773760}},
  {{"Id":"c_old","SizeRw":0,"SizeRootFs":503316480}},
  {{"Id":"c_tool","SizeRootFs":209715200}},
  {{"Id":"c_pg","SizeRw":-1,"SizeRootFs":419430400}}
 ],
 "Volumes": [
  {{"Name":"pgdata","Driver":"local","Labels":null,"Mountpoint":"/var/lib/docker/volumes/pgdata/_data","UsageData":{{"Size":1048576000,"RefCount":1}}}},
  {{"Name":"unmeasured","Driver":"local","Labels":{{}},"UsageData":{{"Size":-1,"RefCount":-1}}}}
 ],
 "BuildCache": [
  {{"ID":"bc1","Type":"regular","InUse":true,"Shared":false,"Size":104857600,"Description":"RUN npm ci"}},
  {{"ID":"bc2","Type":"regular","InUse":false,"Shared":true,"Size":734003200}},
  {{"ID":"bc3","Type":"source.local","InUse":false,"Shared":false,"Size":52428800}}
 ]
}}"#
    )
}

fn list() -> String {
    format!(
        r#"[
 {{"Id":"c_web","Names":["/hp-web"],"ImageID":"{A}","State":"running","Labels":{LBL2},"Mounts":[]}},
 {{"Id":"c_old","Names":["/hp-web-old"],"ImageID":"{OLD}","State":"exited","Labels":{LBL},"Mounts":[]}},
 {{"Id":"c_tool","Names":["/tools"],"ImageID":"{STOPPED}","State":"exited","Labels":null,"Mounts":[]}},
 {{"Id":"c_pg","Names":["/db"],"ImageID":"{PG}","State":"running","Labels":null,"Mounts":[{{"Type":"volume","Name":"pgdata","Destination":"/var/lib/postgresql/data"}}]}}
]"#
    )
}

fn facts() -> JournalFacts {
    let f = |kind, id: &str, dep: &str| JournalFact {
        kind,
        id: id.into(),
        app: "app_1".into(),
        deployment: dep.into(),
        released: false,
    };
    JournalFacts {
        entries: vec![
            f(FactKind::Image, A, "dep_2"),
            f(FactKind::Image, OLD, "dep_1"),
            f(FactKind::Container, "c_web", "dep_2"),
            f(FactKind::Container, "c_old", "dep_1"),
        ],
        serving_containers: ["c_web".to_string()].into(),
        serving_images: [A.to_string()].into(),
        available: true,
        note: None,
    }
}

fn run(df: Document, list: Document, facts: &JournalFacts) -> Result<Json, String> {
    build(&df, &list, facts, 1_000)
}
fn ok() -> Json {
    run(Document::Body(df()), Document::Body(list()), &facts()).unwrap()
}
fn cat<'a>(j: &'a Json, id: &str) -> &'a Json {
    j.get("categories")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("id").unwrap().as_str() == Some(id))
        .unwrap()
}
fn item<'a>(c: &'a Json, name: &str) -> &'a Json {
    c.get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i.get("name").unwrap().as_str() == Some(name))
        .unwrap_or_else(|| panic!("no item {name}"))
}
fn props(i: &Json) -> Vec<String> {
    i.get("ownership")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.get("property").unwrap().as_str().unwrap().to_string())
        .collect()
}
fn bytes(j: &Json) -> Option<i64> {
    j.get("bytes").unwrap().as_i64()
}

#[test]
fn deployment_images_are_split_by_journal_evidence_and_protected_when_serving() {
    let j = ok();
    let d = cat(&j, "deployment-images");
    let serving = item(d, "web:2");
    assert_eq!(
        props(serving),
        ["active-deployment", "referenced-by-running-container"]
    );
    assert_eq!(serving.get("protection").unwrap().as_str(), Some("active"));
    let old = item(d, "web:1");
    assert!(props(old).contains(&"historical-deployment".to_string()));
    assert_eq!(
        old.get("state").unwrap().as_str(),
        Some("referenced-by-stopped")
    );
    // the postgres image is not Harpoon's: it is in "other", with no ownership inferred from its name
    assert!(d.get("items").unwrap().as_array().unwrap().iter().all(|i| i
        .get("name")
        .unwrap()
        .as_str()
        != Some("postgres:15")));
}

#[test]
fn other_images_distinguish_stopped_tagged_dangling_and_running_references() {
    let j = ok();
    let o = cat(&j, "other-images");
    assert_eq!(
        item(o, "tools:1").get("state").unwrap().as_str(),
        Some("referenced-by-stopped")
    );
    assert_eq!(
        item(o, "scratch:1").get("state").unwrap().as_str(),
        Some("tagged-unreferenced")
    );
    assert_eq!(
        item(o, "(untagged) eeee00000000")
            .get("state")
            .unwrap()
            .as_str(),
        Some("dangling")
    );
    assert_eq!(
        item(o, "postgres:15").get("state").unwrap().as_str(),
        Some("referenced-by-running")
    );
    for n in ["scratch:1", "(untagged) eeee00000000"] {
        let i = item(o, n);
        assert_eq!(props(i), ["unknown-ownership"]);
        assert_eq!(i.get("protection").unwrap().as_str(), Some("none"));
        assert!(i.get("ownership").unwrap().as_array().unwrap()[0]
            .get("evidence")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("does not mean orphaned"));
    }
    assert_eq!(
        item(o, "postgres:15").get("protection").unwrap().as_str(),
        Some("in-use")
    );
}

#[test]
fn image_sizes_are_never_added_and_shared_layers_are_reported_as_such() {
    let j = ok();
    for id in ["deployment-images", "other-images", "obsolete-artifacts"] {
        let c = cat(&j, id);
        assert!(
            c.get("bytes").unwrap().is_null(),
            "{id} must not carry a total"
        );
        assert_eq!(c.get("bytesComplete").unwrap().as_bool(), Some(false));
    }
    assert_eq!(
        j.get("imageLayersBytes").unwrap().as_i64(),
        Some(900 * 1048576)
    ); // less than the 1730 MiB the images' own sizes add up to
    let web = item(cat(&j, "deployment-images"), "web:2");
    assert!(web
        .get("details")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.get("value").unwrap().as_str() == Some("300 MiB of its size is shared")));
    // Docker's -1 shared size is "not reported", not zero
    let tools = item(cat(&j, "other-images"), "tools:1");
    assert!(tools
        .get("details")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.get("value").unwrap().as_str() == Some("not reported")));
}

#[test]
fn obsolete_artifacts_are_journal_recorded_non_serving_objects_only() {
    let j = ok();
    let o = cat(&j, "obsolete-artifacts");
    let names: Vec<_> = o
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"web:1".to_string()) && names.contains(&"hp-web-old".to_string()));
    assert!(
        !names
            .iter()
            .any(|n| n == "hp-web" || n == "web:2" || n == "tools"),
        "serving or unknown objects are never listed as obsolete"
    );
}

#[test]
fn the_postgres_volume_is_persistent_with_references_and_no_name_based_ownership() {
    let j = ok();
    let v = item(cat(&j, "volumes"), "pgdata");
    assert_eq!(v.get("protection").unwrap().as_str(), Some("persistent"));
    assert_eq!(
        props(v),
        [
            "persistent-data",
            "referenced-by-running-container",
            "unknown-ownership"
        ]
    );
    let r = v.get("references").unwrap().as_array().unwrap();
    assert_eq!(
        (
            r.len(),
            r[0].get("name").unwrap().as_str(),
            r[0].get("running").unwrap().as_bool()
        ),
        (1, Some("db"), Some(true))
    );
    assert_eq!(bytes(v), Some(1000 * 1048576));
}

#[test]
fn unavailable_sizes_are_unknown_not_zero_and_totals_say_they_are_incomplete() {
    let j = ok();
    let vols = cat(&j, "volumes");
    assert!(item(vols, "unmeasured").get("bytes").unwrap().is_null());
    assert_eq!(
        (bytes(vols), vols.get("bytesComplete").unwrap().as_bool()),
        (Some(1000 * 1048576), Some(false))
    );
    let w = cat(&j, "writable-layers");
    assert!(
        item(w, "tools").get("bytes").unwrap().is_null(),
        "a missing SizeRw is unknown"
    );
    assert_eq!(
        item(w, "hp-web-old").get("bytes").unwrap().as_i64(),
        Some(0),
        "a reported 0 is zero"
    );
    assert!(
        item(w, "db").get("bytes").unwrap().is_null(),
        "-1 is unknown"
    );
    assert_eq!(w.get("bytesComplete").unwrap().as_bool(), Some(false));
    assert!(item(w, "hp-web")
        .get("details")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.get("label").unwrap().as_str() == Some("Log size")
            && d.get("value").unwrap().as_str() == Some("not measured")));
}

#[test]
fn build_cache_reports_in_use_unused_shared_and_types_without_promising_recovery() {
    let j = ok();
    let c = cat(&j, "build-cache");
    let fig = |l: &str| {
        c.get("figures")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f.get("label").unwrap().as_str() == Some(l))
            .unwrap()
            .get("bytes")
            .unwrap()
            .as_i64()
    };
    assert_eq!(fig("Reported by Docker as not in use"), Some(750 * 1048576));
    assert_eq!(fig("Sum of record sizes"), Some(850 * 1048576));
    let count = |l: &str| {
        c.get("counts")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f.get("label").unwrap().as_str() == Some(l))
            .unwrap()
            .get("count")
            .unwrap()
            .as_i64()
    };
    assert_eq!(
        (
            count("type regular"),
            count("type source.local"),
            count("in use"),
            count("marked shared")
        ),
        (Some(2), Some(1), Some(1), Some(1))
    );
    assert!(c
        .get("sizeBasis")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("not a promise"));
}

#[test]
fn a_missing_size_makes_the_cache_figure_unknown_not_a_smaller_total() {
    let d = df().replace(r#""Shared":false,"Size":52428800}"#, r#""Shared":false}"#);
    let j = run(Document::Body(d), Document::Body(list()), &facts()).unwrap();
    let c = cat(&j, "build-cache");
    let f = c.get("figures").unwrap().as_array().unwrap();
    let by = |l: &str| {
        f.iter()
            .find(|x| x.get("label").unwrap().as_str() == Some(l))
            .unwrap()
            .get("bytes")
            .unwrap()
            .clone()
    };
    assert!(by("Sum of record sizes").is_null());
    assert!(by("Reported by Docker as not in use").is_null());
}

#[test]
fn partial_inventory_when_the_container_list_fails_references_become_unknown() {
    let j = run(
        Document::Body(df()),
        Document::Failed("HTTP 500".into()),
        &facts(),
    )
    .unwrap();
    let srcs = j.get("sources").unwrap().as_array().unwrap();
    assert_eq!(srcs[1].get("status").unwrap().as_str(), Some("failed"));
    let i = item(cat(&j, "other-images"), "scratch:1");
    assert!(i.get("references").unwrap().is_null());
    assert_eq!(i.get("state").unwrap().as_str(), Some("references-unknown"));
    assert_eq!(
        cat(&j, "writable-layers").get("status").unwrap().as_str(),
        Some("unavailable")
    );
    assert_eq!(
        cat(&j, "volumes").get("status").unwrap().as_str(),
        Some("ok")
    );
}

#[test]
fn a_failed_df_leaves_every_size_category_unavailable_not_empty() {
    let j = run(
        Document::Oversized(MAX_BODY),
        Document::Body(list()),
        &facts(),
    )
    .unwrap();
    assert_eq!(
        j.get("sources").unwrap().as_array().unwrap()[0]
            .get("status")
            .unwrap()
            .as_str(),
        Some("oversized")
    );
    for id in [
        "deployment-images",
        "other-images",
        "volumes",
        "build-cache",
        "obsolete-artifacts",
    ] {
        assert_eq!(
            cat(&j, id).get("status").unwrap().as_str(),
            Some("unavailable"),
            "{id}"
        );
    }
}

#[test]
fn malformed_documents_are_invalid_never_partial() {
    for bad in ["", "{", "[1,2]", r#"{"Images":"x""#, "null"] {
        let r = run(
            Document::Body(bad.into()),
            Document::Body("not json".into()),
            &facts(),
        );
        assert!(r.is_err(), "{bad:?}");
    }
    let j = run(
        Document::Body(df()),
        Document::Body(r#"{"not":"a list"}"#.into()),
        &facts(),
    )
    .unwrap();
    assert_eq!(
        j.get("sources").unwrap().as_array().unwrap()[1]
            .get("status")
            .unwrap()
            .as_str(),
        Some("invalid")
    );
    // a list inside df that is not a list makes only that category unavailable
    let j = run(
        Document::Body(r#"{"Images":{},"Volumes":[],"BuildCache":null,"Containers":null}"#.into()),
        Document::Body("[]".into()),
        &facts(),
    )
    .unwrap();
    assert_eq!(
        cat(&j, "other-images").get("status").unwrap().as_str(),
        Some("unavailable")
    );
    assert_eq!(
        cat(&j, "volumes").get("status").unwrap().as_str(),
        Some("ok")
    );
    // an entry without an id is counted as skipped, and the category says so
    let j = run(Document::Body(r#"{"Images":[{"RepoTags":["x"]},{"Id":"sha256:1","RepoTags":["y"],"Size":1}],"Volumes":[],"BuildCache":[],"Containers":[]}"#.into()), Document::Body("[]".into()), &facts()).unwrap();
    let o = cat(&j, "other-images");
    assert_eq!(
        (
            o.get("status").unwrap().as_str(),
            o.get("count").unwrap().as_i64()
        ),
        (Some("ok"), Some(1))
    );
    assert_eq!(
        cat(&j, "deployment-images").get("status").unwrap().as_str(),
        Some("partial")
    );
}

#[test]
fn a_journal_that_could_not_be_read_confirms_nothing_and_says_so() {
    let j = run(
        Document::Body(df()),
        Document::Body(list()),
        &JournalFacts::default(),
    )
    .unwrap();
    assert_eq!(j.get("journalAvailable").unwrap().as_bool(), Some(false));
    assert!(cat(&j, "deployment-images")
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let w = item(cat(&j, "other-images"), "web:2");
    assert_eq!(
        props(w),
        [
            "harpoon-labelled-unconfirmed",
            "referenced-by-running-container"
        ]
    );
    assert_eq!(w.get("protection").unwrap().as_str(), Some("in-use"));
}

#[test]
fn labels_that_disagree_with_the_journal_are_flagged() {
    let mut f = facts();
    f.entries[0].deployment = "dep_9".into();
    let j = run(Document::Body(df()), Document::Body(list()), &f).unwrap();
    assert!(props(item(cat(&j, "deployment-images"), "web:2"))
        .contains(&"label-journal-mismatch".to_string()));
}

// ---- the wire: two GETs, a cap, no truncated parse

fn fake(df_body: String, list_body: String) -> (DockerApi, Arc<Mutex<Vec<String>>>) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hps-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let _ = std::fs::remove_file(&sock);
    let l = UnixListener::bind(&sock).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap_or(0);
            let line = String::from_utf8_lossy(&buf[..n])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            log.lock().unwrap().push(line.clone());
            let body = if line.contains("/system/df") {
                &df_body
            } else if line.contains("/containers/json") {
                &list_body
            } else {
                "{}"
            };
            let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = c.write_all(body.as_bytes());
        }
    });
    (DockerApi::new(&sock, Duration::from_secs(5)), seen)
}

#[test]
fn the_inventory_asks_exactly_two_get_requests() {
    let (api, seen) = fake(df(), list());
    let (d, l) = fetch(&api, Instant::now() + Duration::from_secs(5));
    assert!(matches!(d, Document::Body(_)) && matches!(l, Document::Body(_)));
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "GET /system/df HTTP/1.1",
            "GET /containers/json?all=true HTTP/1.1"
        ]
    );
}

#[test]
fn a_body_over_the_cap_is_oversized_and_never_parsed() {
    let big = format!(r#"{{"Images":[],"Pad":"{}"}}"#, "x".repeat(MAX_BODY + 10));
    let (api, _) = fake(big, list());
    let (d, l) = fetch(&api, Instant::now() + Duration::from_secs(5));
    assert_eq!(d, Document::Oversized(MAX_BODY));
    assert!(matches!(l, Document::Body(_)));
    let j = run(d, l, &facts()).unwrap();
    assert_eq!(
        j.get("sources").unwrap().as_array().unwrap()[0]
            .get("status")
            .unwrap()
            .as_str(),
        Some("oversized")
    );
}

#[test]
fn an_unreachable_socket_is_a_failure_not_an_empty_inventory() {
    let api = DockerApi::new("/nonexistent/harpoon-test.sock", Duration::from_secs(1));
    let (d, l) = fetch(&api, Instant::now() + Duration::from_secs(1));
    assert!(matches!(d, Document::Failed(_)) && matches!(l, Document::Failed(_)));
    assert!(run(d, l, &facts()).is_err());
}

// ---- the journal reader: ids and the serving release come from the same rules cleanup uses

#[test]
fn facts_come_from_the_ledger_and_the_serving_release() {
    use harpoon_deploy::ids::{IdGenerator, ReleaseId};
    use harpoon_deploy::ledger::{Disposition, Ledger, LedgerRecord, ResourceKey, ResourceKind};
    use harpoon_deploy::release::ReleaseConfig;
    use harpoon_deploy::state::{Deployment, DeploymentStatus, Trigger};
    use harpoon_deploy::time::{ManualClock, SeqEntropy};
    use harpoon_deploy_docker::storage::facts_from;
    let clock = ManualClock::new(2_000_000);
    let mut gen = IdGenerator::new(clock, SeqEntropy::new(7));
    let (app, dep) = (gen.application().unwrap(), gen.deployment().unwrap());
    let mut d = Deployment::new(dep.clone(), app.clone(), Trigger::Cli, 1_000);
    d.transition(DeploymentStatus::Queued, 1_001).unwrap();
    d.transition(DeploymentStatus::Running, 1_002).unwrap();
    d.release = Some(ReleaseId::from_deployment(&dep));
    d.pinned = Some(ReleaseConfig::simulated(38123));
    d.transition(DeploymentStatus::Succeeded, 1_003).unwrap();
    let (cname, iname) = (format!("hp-{dep}"), format!("img-{dep}"));
    let mut l = Ledger::new();
    for (kind, name, id) in [
        (ResourceKind::Container, &cname, "cid-1"),
        (ResourceKind::Image, &iname, "sha256:img1"),
    ] {
        let key = ResourceKey::new(kind, name);
        l.apply(&LedgerRecord::Intent {
            key: key.clone(),
            app: app.clone(),
            deployment: dep.clone(),
            disposition: Disposition::RemoveOnFailure,
            at_ms: 1_000,
        })
        .unwrap();
        l.apply(&LedgerRecord::Receipt {
            key,
            deployment: dep.clone(),
            id: id.into(),
            at_ms: 1_001,
        })
        .unwrap();
    }
    let f = facts_from(&l, &[d]);
    assert!(f.available);
    assert_eq!(f.entries.len(), 2);
    assert!(f.entries.iter().any(|e| e.kind == FactKind::Image
        && e.id == "sha256:img1"
        && e.deployment == dep.to_string()
        && !e.released));
    assert_eq!(f.serving_containers.iter().collect::<Vec<_>>(), ["cid-1"]);
    assert_eq!(
        f.serving_images.iter().collect::<Vec<_>>(),
        [&ReleaseConfig::simulated(1).image_id]
    );
}

// ---- the contract with the desktop: the committed fixture is exactly what this module writes for the fixtures above

#[test]
fn the_desktop_fixture_is_what_the_runtime_writes() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ui/harpoon-desktop/src/lib/fixtures/storage-inventory.json");
    let now = ok().to_json();
    if std::env::var_os("WRITE_STORAGE_FIXTURE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &now).unwrap();
    }
    let committed =
        std::fs::read_to_string(&path).expect("fixture missing: run with WRITE_STORAGE_FIXTURE=1");
    assert_eq!(committed, now, "the desktop's fixture is stale: regenerate with WRITE_STORAGE_FIXTURE=1 cargo test --test storage_inventory");
}

#[test]
fn a_label_claim_is_kept_apart_from_journal_confirmation_and_from_unknown() {
    let j = ok();
    let claim = item(cat(&j, "other-images"), "adopted:1");
    assert_eq!(props(claim), ["harpoon-labelled-unconfirmed"]);
    assert_eq!(claim.get("protection").unwrap().as_str(), Some("none"));
    assert!(!props(claim)
        .iter()
        .any(|p| p == "active-deployment" || p == "historical-deployment"));
    // confirmed, claimed and unknown never share a property
    let confirmed = props(item(cat(&j, "deployment-images"), "web:1"));
    let unknown = props(item(cat(&j, "other-images"), "scratch:1"));
    assert!(
        confirmed.contains(&"historical-deployment".to_string())
            && !confirmed.contains(&"unknown-ownership".to_string())
    );
    assert_eq!(unknown, ["unknown-ownership"]);
    // running and stopped references are separate properties
    let both = |name: &str| props(item(cat(&j, "other-images"), name));
    assert!(both("postgres:15").contains(&"referenced-by-running-container".to_string()));
    assert!(
        both("tools:1").contains(&"referenced-by-stopped-container".to_string())
            && !both("tools:1").contains(&"referenced-by-running-container".to_string())
    );
}

#[test]
fn a_measured_zero_is_zero_and_every_other_cache_size_is_unknown() {
    let cache = |records: &str| {
        let d = format!(r#"{{"Images":[],"Volumes":[],"Containers":[],"BuildCache":{records}}}"#);
        run(Document::Body(d), Document::Body("[]".into()), &facts()).unwrap()
    };
    let figure = |j: &Json, l: &str| {
        cat(j, "build-cache")
            .get("figures")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f.get("label").unwrap().as_str() == Some(l))
            .unwrap()
            .get("bytes")
            .unwrap()
            .clone()
    };
    // really zero: Docker said 0
    let zero = cache(r#"[{"ID":"a","Type":"regular","InUse":false,"Size":0}]"#);
    assert_eq!(figure(&zero, "Sum of record sizes").as_i64(), Some(0));
    assert_eq!(
        figure(&zero, "Reported by Docker as not in use").as_i64(),
        Some(0)
    );
    let c = cat(&zero, "build-cache");
    assert_eq!(
        (
            c.get("bytes").unwrap().as_i64(),
            c.get("bytesComplete").unwrap().as_bool()
        ),
        (Some(0), Some(true))
    );
    // no records at all is an empty cache Docker listed, also zero
    assert_eq!(
        cat(&cache("[]"), "build-cache")
            .get("bytes")
            .unwrap()
            .as_i64(),
        Some(0)
    );
    // missing, null, negative, fractional and mistyped sizes are unknown, never zero
    for bad in [
        r#""Size":-1"#,
        r#""Size":null"#,
        r#""Size":"12""#,
        r#""Size":1.5"#,
        r#""Foo":1"#,
    ] {
        let j = cache(&format!(
            r#"[{{"ID":"a","Type":"regular","InUse":false,{bad}}}]"#
        ));
        assert!(figure(&j, "Sum of record sizes").is_null(), "{bad}");
        assert!(
            figure(&j, "Reported by Docker as not in use").is_null(),
            "{bad}"
        );
        let c = cat(&j, "build-cache");
        assert!(
            c.get("bytes").unwrap().is_null()
                && c.get("items").unwrap().as_array().unwrap()[0]
                    .get("bytes")
                    .unwrap()
                    .is_null(),
            "{bad}"
        );
    }
    // a record whose use is not reported cannot be counted as unused
    let j = cache(r#"[{"ID":"a","Type":"regular","Size":10}]"#);
    assert_eq!(figure(&j, "Sum of record sizes").as_i64(), Some(10));
    assert!(figure(&j, "Reported by Docker as not in use").is_null());
    // a record without an id is skipped and the category is partial, not a smaller total
    let j = cache(
        r#"[{"Type":"regular","Size":10},{"ID":"b","Type":"regular","InUse":false,"Size":5}]"#,
    );
    assert_eq!(
        cat(&j, "build-cache").get("status").unwrap().as_str(),
        Some("partial")
    );
    // a malformed cache section is unavailable
    let d = r#"{"Images":[],"Volumes":[],"Containers":[],"BuildCache":{"x":1}}"#;
    let j = run(
        Document::Body(d.into()),
        Document::Body("[]".into()),
        &facts(),
    )
    .unwrap();
    assert_eq!(
        cat(&j, "build-cache").get("status").unwrap().as_str(),
        Some("unavailable")
    );
}

// ---- one deadline for both requests

/// A fake engine that answers each path after its own delay (a thread per connection, so a late answer never blocks the next one).
fn slow_fake(df_delay: Duration, list_delay: Duration) -> (DockerApi, Arc<Mutex<Vec<String>>>) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1000);
    let dir = std::env::temp_dir().join(format!(
        "hps-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let _ = std::fs::remove_file(&sock);
    let l = UnixListener::bind(&sock).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { return };
            let log = log.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = c.read(&mut buf).unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                log.lock().unwrap().push(line.clone());
                let (body, delay) = if line.contains("/system/df") {
                    (df(), df_delay)
                } else {
                    (list(), list_delay)
                };
                std::thread::sleep(delay);
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    (DockerApi::new(&sock, Duration::from_secs(30)), seen)
}

#[test]
fn a_slow_first_request_leaves_the_second_only_what_is_left_and_never_overruns_the_deadline() {
    let (api, seen) = slow_fake(Duration::from_millis(250), Duration::ZERO);
    let start = Instant::now();
    let (d, l) = fetch(&api, start + Duration::from_millis(150));
    assert!(matches!(d, Document::Failed(_)), "{d:?}");
    assert!(
        matches!(&l, Document::Failed(m) if m.contains("time limit")),
        "the second request is not even sent: {l:?}"
    );
    assert_eq!(seen.lock().unwrap().len(), 1, "one request reached Docker");
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "bounded by the deadline, not by the slow answer: {:?}",
        start.elapsed()
    );
}

#[test]
fn a_slow_second_request_is_cut_at_the_shared_deadline_and_the_first_still_counts() {
    let (api, _) = slow_fake(Duration::from_millis(50), Duration::from_millis(600));
    let start = Instant::now();
    let (d, l) = fetch(&api, start + Duration::from_millis(300));
    assert!(matches!(d, Document::Body(_)));
    assert!(matches!(l, Document::Failed(_)), "{l:?}");
    assert!(
        start.elapsed() < Duration::from_millis(550),
        "{:?}",
        start.elapsed()
    );
    // the first document alone still makes a partial inventory, with references unknown
    let j = run(d, l, &facts()).unwrap();
    assert_eq!(
        j.get("sources").unwrap().as_array().unwrap()[1]
            .get("status")
            .unwrap()
            .as_str(),
        Some("failed")
    );
    assert!(item(cat(&j, "other-images"), "scratch:1")
        .get("references")
        .unwrap()
        .is_null());
}

#[test]
fn an_expired_deadline_sends_nothing() {
    let (api, seen) = slow_fake(Duration::ZERO, Duration::ZERO);
    let (d, l) = fetch(&api, Instant::now());
    assert!(matches!(d, Document::Failed(_)) && matches!(l, Document::Failed(_)));
    assert!(seen.lock().unwrap().is_empty());
    assert!(
        run(d, l, &facts()).is_err(),
        "nothing readable is an error, not an empty inventory"
    );
}

#[test]
fn both_requests_within_the_deadline_succeed() {
    let (api, seen) = slow_fake(Duration::from_millis(40), Duration::from_millis(40));
    let (d, l) = fetch(&api, Instant::now() + Duration::from_secs(5));
    assert!(matches!(d, Document::Body(_)) && matches!(l, Document::Body(_)));
    assert_eq!(seen.lock().unwrap().len(), 2);
}
