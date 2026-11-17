//! Read-only storage inventory: what Docker reports it stores, and what Harpoon can prove about who owns it.
//!
//! Two documents are read, both with `GET` and a fixed path: `/system/df` (sizes of images, container writable layers,
//! volumes and BuildKit cache) and `/containers/json?all=true` (state, mounts and image of every container). Nothing is
//! requested that changes Docker, and this module has no way to name a removal.
//!
//! Accounting rules (the reason the output is shaped as it is):
//! * Every size is an `Option`: Docker's `-1`, a missing field or a mistyped one is "unavailable", never zero.
//! * Image sizes include layers shared with other images, so they are never added up. `LayersSize` is Docker's own
//!   layer accounting and is reported as exactly that.
//! * A category total is given only where its members do not overlap, and says whether every member contributed.
//! * Nothing here is a share of the guest filesystem: the categories are independent, and no "remainder" is computed.
//! * A body over [`MAX_BODY`] is never parsed (it would be truncated); that endpoint is reported as oversized.
//!
//! Ownership is evidence, not a verdict: the journal's recorded ids (confirmed), Harpoon's three labels (a claim until
//! the journal agrees), and the container references Docker reports. Names are never used. Cleanup rules are not here.

use crate::api::{ApiError, Capped, DockerApi};
use crate::json::{n, obj, s, Json};
use harpoon_deploy::ids::ApplicationId;
use harpoon_deploy::ledger::{Ledger, ResourceKind};
use harpoon_deploy::serving::serving_release;
use harpoon_deploy::state::Deployment;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The largest response body read from either endpoint. Raise it only on evidence from a real engine.
pub const MAX_BODY: usize = 4 * 1024 * 1024;
const DF: &str = "/system/df";
const CONTAINERS: &str = "/containers/json?all=true";
/// Items listed per category; the rest are counted and reported as omitted.
const MAX_ITEMS: usize = 200;
const LABEL_MANAGED: &str = "io.harpoon.managed";
const LABEL_APP: &str = "io.harpoon.app";
const LABEL_DEPLOYMENT: &str = "io.harpoon.deployment";

/// What one endpoint gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Document {
    Body(String),
    /// The body exceeded the cap and was not read to the end.
    Oversized(usize),
    Failed(String),
}

/// Read both documents under one deadline: each request gets only the time that is left, and a request that would start
/// after the deadline is not sent. Two `GET`s, in sequence; the caller bounds how many of these run at once.
pub fn fetch(api: &DockerApi, deadline: Instant) -> (Document, Document) {
    let get = |path: &str| {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Document::Failed(
                "the inventory's time limit was reached before this request could start".into(),
            );
        }
        match api.call_capped("GET", path, MAX_BODY, left) {
            Ok(Capped::Ok(r)) if r.status == 200 => Document::Body(r.text()),
            Ok(Capped::Ok(r)) => Document::Failed(format!("HTTP {}", r.status)),
            Ok(Capped::TooLarge) => Document::Oversized(MAX_BODY),
            Err(e) => Document::Failed(match e {
                ApiError::Transport(t) => t.to_string(),
                other => other.to_string(),
            }),
        }
    };
    let df = get(DF);
    (df, get(CONTAINERS))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Container,
    Image,
}

/// One object the journal's ledger recorded an id for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFact {
    pub kind: FactKind,
    pub id: String,
    pub app: String,
    pub deployment: String,
    /// The ledger says Harpoon already released it (Docker may still hold it).
    pub released: bool,
}

/// What the journal knows, read-only. `serving_*` are the ids the applications serve right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalFacts {
    pub entries: Vec<JournalFact>,
    pub serving_containers: BTreeSet<String>,
    pub serving_images: BTreeSet<String>,
    /// `false` when the journal could not be used: nothing is then confirmed, and the output says so.
    pub available: bool,
    /// Why it could not be used (no journal found, or the read failed). `None` when `available`.
    pub note: Option<String>,
}

/// The journal's recorded ids, and the ids each application serves now (decided by the same `serving_release` rule the
/// cleanup uses). Read-only: it only looks at a ledger that was already read; nothing is locked or written.
pub fn facts_from(ledger: &Ledger, deployments: &[Deployment]) -> JournalFacts {
    let mut f = JournalFacts {
        available: true,
        ..Default::default()
    };
    for e in ledger.entries() {
        let kind = match e.key.kind {
            ResourceKind::Container => FactKind::Container,
            ResourceKind::Image => FactKind::Image,
            ResourceKind::PortLease => continue,
        };
        if let Some(id) = &e.id {
            f.entries.push(JournalFact {
                kind,
                id: id.clone(),
                app: e.app.to_string(),
                deployment: e.deployment.to_string(),
                released: e.is_released(),
            });
        }
    }
    let apps: BTreeSet<ApplicationId> = deployments
        .iter()
        .map(|d| d.application_id.clone())
        .collect();
    for app in &apps {
        let Some(s) = serving_release(ledger, deployments, app) else {
            continue;
        };
        if let Some(c) = &s.config {
            f.serving_images.insert(c.image_id.clone());
        }
        if let Some(id) = ledger
            .entries()
            .iter()
            .find(|e| {
                e.deployment == s.container_owner
                    && e.key.kind == ResourceKind::Container
                    && e.key.name == s.container_name
            })
            .and_then(|e| e.id.clone())
        {
            f.serving_containers.insert(id);
        }
    }
    f
}

fn norm(id: &str) -> String {
    id.trim_start_matches("sha256:").to_ascii_lowercase()
}

/// Same wording as the desktop's formatter, for the few detail lines the runtime writes itself.
fn human(b: u64) -> String {
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    let unit = ["B", "KiB", "MiB", "GiB", "TiB"][i];
    if i == 0 || v >= 100.0 {
        format!("{} {unit}", v.round())
    } else {
        format!("{v:.1} {unit}")
    }
}

fn short(id: &str) -> String {
    norm(id).chars().take(12).collect()
}

/// A size Docker really reported: a whole, non-negative number. `-1`, absent, null, fractional or mistyped is unknown.
fn size(v: Option<&Json>) -> Option<u64> {
    v.and_then(Json::as_i64)
        .filter(|b| *b >= 0)
        .map(|b| b as u64)
}

fn jb(v: Option<u64>) -> Json {
    v.map_or(Json::Null, |b| Json::Num(b as f64))
}

fn arr(v: Vec<Json>) -> Json {
    Json::Arr(v)
}

fn pairs(v: &[(String, String)]) -> Json {
    arr(v
        .iter()
        .map(|(k, x)| obj(vec![("label", s(k.as_str())), ("value", s(x.as_str()))]))
        .collect())
}

fn labels(j: Option<&Json>) -> Option<Vec<(String, String)>> {
    match j {
        None | Some(Json::Null) => Some(Vec::new()),
        Some(Json::Obj(m)) => m
            .iter()
            .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
            .collect(),
        _ => None,
    }
}

fn label<'a>(l: &'a [(String, String)], k: &str) -> Option<&'a str> {
    l.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

#[derive(Clone)]
struct Ref {
    name: String,
    running: bool,
}

#[derive(Clone)]
struct Item {
    id: String,
    name: String,
    kind: &'static str,
    bytes: Option<u64>,
    bytes_note: String,
    state: String,
    refs: Option<Vec<Ref>>,
    owner: Vec<(String, String)>,
    protection: &'static str,
    protection_note: String,
    details: Vec<(String, String)>,
}

impl Item {
    fn json(&self) -> Json {
        obj(vec![
            ("id", s(self.id.as_str())),
            ("name", s(self.name.as_str())),
            ("kind", s(self.kind)),
            ("bytes", jb(self.bytes)),
            ("bytesNote", s(self.bytes_note.as_str())),
            ("state", s(self.state.as_str())),
            (
                "references",
                match &self.refs {
                    None => Json::Null,
                    Some(r) => arr(r
                        .iter()
                        .map(|x| {
                            obj(vec![
                                ("name", s(x.name.as_str())),
                                ("running", Json::Bool(x.running)),
                            ])
                        })
                        .collect()),
                },
            ),
            (
                "ownership",
                arr(self
                    .owner
                    .iter()
                    .map(|(p, e)| {
                        obj(vec![
                            ("property", s(p.as_str())),
                            ("evidence", s(e.as_str())),
                        ])
                    })
                    .collect()),
            ),
            ("protection", s(self.protection)),
            ("protectionNote", s(self.protection_note.as_str())),
            ("details", pairs(&self.details)),
        ])
    }
    fn has(&self, property: &str) -> bool {
        self.owner.iter().any(|(p, _)| p == property)
    }
}

struct Category {
    id: &'static str,
    title: &'static str,
    basis: &'static str,
    status: &'static str,
    note: Option<String>,
    items: Vec<Item>,
    /// `Some` only where the members do not overlap.
    additive: bool,
    skipped: usize,
    figures: Vec<(String, Option<u64>, String)>,
    counts: Vec<(String, usize)>,
}

impl Category {
    fn new(id: &'static str, title: &'static str, basis: &'static str, additive: bool) -> Category {
        Category {
            id,
            title,
            basis,
            status: "ok",
            note: None,
            items: Vec::new(),
            additive,
            skipped: 0,
            figures: Vec::new(),
            counts: Vec::new(),
        }
    }
    fn unavailable(mut self, why: impl Into<String>) -> Category {
        self.status = "unavailable";
        self.note = Some(why.into());
        self
    }
    fn json(mut self) -> Json {
        self.items
            .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
        let known: Vec<u64> = self.items.iter().filter_map(|i| i.bytes).collect();
        let (bytes, complete) = if self.additive && self.status != "unavailable" {
            (
                if known.is_empty() && !self.items.is_empty() {
                    None
                } else {
                    Some(known.iter().sum::<u64>())
                },
                known.len() == self.items.len(),
            )
        } else {
            (None, false)
        };
        let count = self.items.len();
        let omitted = count.saturating_sub(MAX_ITEMS);
        if self.skipped > 0 && self.status == "ok" {
            self.status = "partial";
            self.note.get_or_insert_with(|| {
                format!(
                    "{} entries in Docker's answer could not be read and are not listed.",
                    self.skipped
                )
            });
        }
        self.items.truncate(MAX_ITEMS);
        obj(vec![
            ("id", s(self.id)),
            ("title", s(self.title)),
            ("sizeBasis", s(self.basis)),
            ("status", s(self.status)),
            ("note", self.note.as_deref().map_or(Json::Null, s)),
            ("count", n(count as i64)),
            ("bytes", jb(bytes)),
            ("bytesComplete", Json::Bool(complete)),
            ("skipped", n(self.skipped as i64)),
            ("omitted", n(omitted as i64)),
            (
                "figures",
                arr(self
                    .figures
                    .iter()
                    .map(|(l, b, m)| {
                        obj(vec![
                            ("label", s(l.as_str())),
                            ("bytes", jb(*b)),
                            ("meaning", s(m.as_str())),
                        ])
                    })
                    .collect()),
            ),
            (
                "counts",
                arr(self
                    .counts
                    .iter()
                    .map(|(l, c)| obj(vec![("label", s(l.as_str())), ("count", n(*c as i64))]))
                    .collect()),
            ),
            ("items", arr(self.items.iter().map(Item::json).collect())),
        ])
    }
}

/// Ownership evidence for one container or image. Returns the evidence plus whether the journal proves it is serving
/// (`active`) or only that it once belonged to a deployment (`historical`).
fn owner_of(
    kind: FactKind,
    id: &str,
    l: &[(String, String)],
    facts: &JournalFacts,
) -> (Vec<(String, String)>, bool, bool) {
    let key = norm(id);
    let mut out: Vec<(String, String)> = Vec::new();
    let matched: Vec<&JournalFact> = facts
        .entries
        .iter()
        .filter(|f| f.kind == kind && norm(&f.id) == key)
        .collect();
    let serving = match kind {
        FactKind::Container => &facts.serving_containers,
        FactKind::Image => &facts.serving_images,
    };
    let active = serving.iter().any(|x| norm(x) == key);
    let historical = !matched.is_empty() && !active;
    let what = if kind == FactKind::Image {
        "image"
    } else {
        "container"
    };
    if active {
        out.push((
            "active-deployment".into(),
            format!(
                "The journal records this {what} id, and it is the one an application serves now."
            ),
        ));
    }
    if let Some(f) = matched.first() {
        if !active {
            out.push(("historical-deployment".into(), format!("The journal records this {what} id for deployment {} of application {}; that release no longer serves it.", f.deployment, f.app)));
        }
        if matched.iter().all(|f| f.released) {
            out.push((
                "journal-released".into(),
                "The journal says Harpoon already released it; Docker still holds it.".into(),
            ));
        }
    }
    let claim = (label(l, LABEL_MANAGED) == Some("true"))
        .then(|| (label(l, LABEL_APP), label(l, LABEL_DEPLOYMENT)));
    match (claim, matched.first()) {
        (Some((Some(a), Some(d))), Some(f)) if a != f.app || d != f.deployment => out.push(("label-journal-mismatch".into(), format!("Its Harpoon labels name application {a}, deployment {d}; the journal records {} and {}.", f.app, f.deployment))),
        (Some((Some(a), Some(d))), None) => out.push(("harpoon-labelled-unconfirmed".into(), format!("It carries Harpoon's labels (application {a}, deployment {d}), but the journal has no record of this id. Labels are a claim, not proof."))),
        (Some(_), None) => out.push(("harpoon-labelled-unconfirmed".into(), "It carries io.harpoon.managed but not the full set of Harpoon labels, and the journal has no record of it.".into())),
        _ => {}
    }
    if out.is_empty() {
        let why = if facts.available {
            "No Harpoon labels and no journal record."
        } else {
            "No Harpoon labels, and the journal could not be read."
        };
        out.push((
            "unknown-ownership".into(),
            format!("{why} Untracked does not mean orphaned."),
        ));
    }
    (out, active, historical)
}

fn reference_evidence(refs: &Option<Vec<Ref>>, owner: &mut Vec<(String, String)>) -> (bool, bool) {
    let Some(r) = refs else { return (false, false) };
    let run: Vec<&str> = r
        .iter()
        .filter(|x| x.running)
        .map(|x| x.name.as_str())
        .collect();
    let stop: Vec<&str> = r
        .iter()
        .filter(|x| !x.running)
        .map(|x| x.name.as_str())
        .collect();
    if !run.is_empty() {
        owner.push((
            "referenced-by-running-container".into(),
            format!("Used by running: {}.", run.join(", ")),
        ));
    }
    if !stop.is_empty() {
        owner.push((
            "referenced-by-stopped-container".into(),
            format!("Used by stopped: {}.", stop.join(", ")),
        ));
    }
    (!run.is_empty(), !stop.is_empty())
}

enum Section<'a> {
    Absent,
    Invalid,
    Items(&'a [Json]),
}

fn section<'a>(df: &'a Json, key: &str) -> Section<'a> {
    match df.get(key) {
        None => Section::Absent,
        Some(Json::Null) => Section::Items(&[]),
        Some(Json::Arr(a)) => Section::Items(a),
        Some(_) => Section::Invalid,
    }
}

fn source(endpoint: &str, d: &Document, parsed: &Option<Json>) -> Json {
    let (status, detail) = match (d, parsed) {
        (Document::Failed(m), _) => ("failed", m.clone()),
        (Document::Oversized(c), _) => ("oversized", format!("The answer is larger than the {c}-byte limit and was not read, so none of it is shown.")),
        (Document::Body(_), Some(_)) => ("ok", String::new()),
        (Document::Body(_), None) => ("invalid", "The answer is not the document this app expects, so none of it is shown.".into()),
    };
    obj(vec![
        ("endpoint", s(endpoint)),
        ("status", s(status)),
        ("detail", s(detail)),
    ])
}

fn parse(d: &Document, want_array: bool) -> Option<Json> {
    let Document::Body(t) = d else { return None };
    let j = Json::parse(t).ok()?;
    let ok = if want_array {
        matches!(j, Json::Arr(_))
    } else {
        matches!(j, Json::Obj(_))
    };
    ok.then_some(j)
}

/// Build the inventory. `Err` only when neither endpoint gave a usable document.
pub fn build(
    df_doc: &Document,
    ctr_doc: &Document,
    facts: &JournalFacts,
    now_ms: u64,
) -> Result<Json, String> {
    let df = parse(df_doc, false);
    let list = parse(ctr_doc, true);
    let sources = arr(vec![
        source(DF, df_doc, &df),
        source(CONTAINERS, ctr_doc, &list),
    ]);
    if df.is_none() && list.is_none() {
        return Err("Docker's storage figures could not be read: neither /system/df nor the container list gave a usable answer".into());
    }

    // Containers: references by image and by volume, and the list's own view of each container.
    let mut by_image: BTreeMap<String, Vec<Ref>> = BTreeMap::new();
    let mut by_volume: BTreeMap<String, Vec<Ref>> = BTreeMap::new();
    struct Ctr {
        id: String,
        name: String,
        state: String,
        image: String,
        labels: Vec<(String, String)>,
    }
    let mut ctrs: Vec<Ctr> = Vec::new();
    let mut ctr_skipped = 0usize;
    if let Some(Json::Arr(a)) = &list {
        for c in a {
            let (Some(id), Some(state)) = (
                c.get("Id").and_then(Json::as_str).filter(|v| !v.is_empty()),
                c.get("State").and_then(Json::as_str),
            ) else {
                ctr_skipped += 1;
                continue;
            };
            let Some(l) = labels(c.get("Labels")) else {
                ctr_skipped += 1;
                continue;
            };
            let name = c
                .get("Names")
                .and_then(Json::as_array)
                .and_then(|x| x.first())
                .and_then(Json::as_str)
                .map(|v| v.trim_start_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| short(id));
            let r = Ref {
                name: name.clone(),
                running: matches!(state, "running" | "restarting" | "paused"),
            };
            let image = c
                .get("ImageID")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            if !image.is_empty() {
                by_image.entry(norm(&image)).or_default().push(r.clone());
            }
            for m in c.get("Mounts").and_then(Json::as_array).unwrap_or(&[]) {
                if m.get("Type").and_then(Json::as_str) == Some("volume") {
                    if let Some(v) = m.get("Name").and_then(Json::as_str) {
                        by_volume.entry(v.to_string()).or_default().push(r.clone());
                    }
                }
            }
            ctrs.push(Ctr {
                id: id.to_string(),
                name,
                state: state.to_string(),
                image,
                labels: l,
            });
        }
    }
    let refs_known = list.is_some();
    let no_refs = "Container references are unknown because the container list could not be read.";
    let no_df = |c: Category| {
        c.unavailable(match df_doc {
            Document::Oversized(_) => "/system/df was too large to read.".to_string(),
            Document::Failed(m) => format!("/system/df failed: {m}"),
            _ => "/system/df gave an unusable answer.".to_string(),
        })
    };

    let mut dep_images = Category::new("deployment-images", "Deployment images", "Each size includes layers shared with other images, so these sizes are not added together.", false);
    let mut other_images = Category::new("other-images", "Other images", "Each size includes layers shared with other images, so these sizes are not added together.", false);
    let mut volumes = Category::new("volumes", "Persistent volumes", "Size Docker measured for each volume's data. Volumes do not overlap, so listed sizes add up; a volume Docker did not measure is not counted.", true);
    let mut writable = Category::new("writable-layers", "Container writable layers", "Data each container wrote on top of its image (SizeRw). Container logs are not measured here: Docker does not report their size.", true);
    let mut cache = Category::new("build-cache", "Build cache", "BuildKit cache records as Docker reports them. 'Not in use' is what Docker says no build is using; it is not a promise of how much disk a cleanup would recover.", true);
    let mut obsolete = Category::new("obsolete-artifacts", "Obsolete deployment artifacts", "Objects the journal records for releases that no longer serve. They also appear in their own rows, so sizes are not added. This is a description, not a removal list.", false);
    let mut image_layers: Option<u64> = None;
    let mut obsolete_items: Vec<Item> = Vec::new();

    match &df {
        None => {
            dep_images = no_df(dep_images);
            other_images = no_df(other_images);
            volumes = no_df(volumes);
            writable = no_df(writable);
            cache = no_df(cache);
            obsolete = no_df(obsolete);
        }
        Some(df) => {
            image_layers = size(df.get("LayersSize"));
            // ---- images
            match section(df, "Images") {
                Section::Items(imgs) => {
                    for i in imgs {
                        let (Some(id), Some(l)) = (
                            i.get("Id").and_then(Json::as_str).filter(|v| !v.is_empty()),
                            labels(i.get("Labels")),
                        ) else {
                            dep_images.skipped += 1;
                            continue;
                        };
                        let tags: Vec<String> = i
                            .get("RepoTags")
                            .and_then(Json::as_array)
                            .unwrap_or(&[])
                            .iter()
                            .filter_map(Json::as_str)
                            .filter(|t| *t != "<none>:<none>")
                            .map(str::to_string)
                            .collect();
                        let refs = refs_known
                            .then(|| by_image.get(&norm(id)).cloned().unwrap_or_default());
                        let (mut owner, active, historical) =
                            owner_of(FactKind::Image, id, &l, facts);
                        let (run, stop) = reference_evidence(&refs, &mut owner);
                        let state = match (&refs, run, stop, tags.is_empty()) {
                            (None, ..) => "references-unknown",
                            (_, true, ..) => "referenced-by-running",
                            (_, false, true, _) => "referenced-by-stopped",
                            (_, false, false, true) => "dangling",
                            _ => "tagged-unreferenced",
                        };
                        let (shared, df_refs) = (
                            size(i.get("SharedSize")),
                            i.get("Containers")
                                .and_then(Json::as_i64)
                                .filter(|c| *c >= 0),
                        );
                        let mut details = vec![("Image id".to_string(), norm(id))];
                        details.push((
                            "Tags".into(),
                            if tags.is_empty() {
                                "none".into()
                            } else {
                                tags.join(", ")
                            },
                        ));
                        details.push((
                            "Shared with other images".into(),
                            shared.map_or("not reported".into(), |b| {
                                format!("{} of its size is shared", human(b))
                            }),
                        ));
                        if let Some(c) = df_refs {
                            details.push(("Containers (Docker's count)".into(), c.to_string()));
                        }
                        let protection = if active {
                            (
                                "active",
                                "The release an application serves now uses this image.",
                            )
                        } else if run {
                            ("in-use", "A running container uses this image.")
                        } else {
                            ("none", "Inspect only: Harpoon removes nothing from here.")
                        };
                        let it = Item {
                            id: norm(id),
                            name: tags
                                .first()
                                .cloned()
                                .unwrap_or_else(|| format!("(untagged) {}", short(id))),
                            kind: "image",
                            bytes: size(i.get("Size")),
                            bytes_note: "Includes layers shared with other images.".into(),
                            state: state.into(),
                            refs: refs.clone(),
                            owner,
                            protection: protection.0,
                            protection_note: protection.1.into(),
                            details,
                        };
                        if historical && !run {
                            obsolete_items.push(it.clone());
                        }
                        if active || historical {
                            dep_images.items.push(it);
                        } else {
                            other_images.items.push(it);
                        }
                    }
                }
                Section::Absent => {
                    dep_images =
                        dep_images.unavailable("This Docker does not report images in /system/df.");
                    other_images = other_images
                        .unavailable("This Docker does not report images in /system/df.");
                }
                Section::Invalid => {
                    dep_images = dep_images.unavailable("Docker's image list is not a list.");
                    other_images = other_images.unavailable("Docker's image list is not a list.");
                }
            }
            if !refs_known {
                dep_images.note.get_or_insert_with(|| no_refs.into());
                other_images.note.get_or_insert_with(|| no_refs.into());
            }
            // ---- container writable layers (sizes from df, state from the list)
            let sizes: BTreeMap<String, (Option<u64>, Option<u64>)> =
                match section(df, "Containers") {
                    Section::Items(a) => a
                        .iter()
                        .filter_map(|c| {
                            Some((
                                c.get("Id")?.as_str()?.to_string(),
                                (size(c.get("SizeRw")), size(c.get("SizeRootFs"))),
                            ))
                        })
                        .collect(),
                    _ => BTreeMap::new(),
                };
            if !refs_known {
                writable = writable.unavailable(
                    "The container list could not be read, so containers are not listed.",
                );
            } else if matches!(
                section(df, "Containers"),
                Section::Absent | Section::Invalid
            ) {
                writable.note =
                    Some("This Docker did not report container sizes in /system/df.".into());
            }
            if refs_known {
                for c in &ctrs {
                    let (rw, root) = sizes.get(&c.id).copied().unwrap_or((None, None));
                    let (owner, active, historical) =
                        owner_of(FactKind::Container, &c.id, &c.labels, facts);
                    let running = matches!(c.state.as_str(), "running" | "restarting" | "paused");
                    let protection = if active {
                        ("active", "An application serves from this container now.")
                    } else if running {
                        ("in-use", "This container is running.")
                    } else {
                        ("none", "Inspect only: Harpoon removes nothing from here.")
                    };
                    let mut details = vec![("Container id".to_string(), c.id.clone())];
                    if !c.image.is_empty() {
                        details.push(("Image".into(), short(&c.image)));
                    }
                    details.push((
                        "Total size with its image".into(),
                        root.map_or("not reported".into(), |b| {
                            format!("{} (includes the image)", human(b))
                        }),
                    ));
                    details.push(("Log size".into(), "not measured".into()));
                    let it = Item {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        kind: "container",
                        bytes: rw,
                        bytes_note: "Writable layer only (SizeRw).".into(),
                        state: c.state.clone(),
                        refs: None,
                        owner,
                        protection: protection.0,
                        protection_note: protection.1.into(),
                        details,
                    };
                    if historical && !running {
                        obsolete_items.push(it.clone());
                    }
                    writable.items.push(it);
                }
                writable.skipped += ctr_skipped;
            }
            // ---- volumes
            match section(df, "Volumes") {
                Section::Items(vs) => {
                    for v in vs {
                        let (Some(name), Some(l)) = (
                            v.get("Name")
                                .and_then(Json::as_str)
                                .filter(|v| !v.is_empty()),
                            labels(v.get("Labels")),
                        ) else {
                            volumes.skipped += 1;
                            continue;
                        };
                        let refs =
                            refs_known.then(|| by_volume.get(name).cloned().unwrap_or_default());
                        let mut owner = vec![("persistent-data".to_string(), "A Docker volume holds data that outlives containers; Harpoon never removes volumes.".to_string())];
                        reference_evidence(&refs, &mut owner);
                        if let (Some("true"), Some(a), Some(d)) = (
                            label(&l, LABEL_MANAGED),
                            label(&l, LABEL_APP),
                            label(&l, LABEL_DEPLOYMENT),
                        ) {
                            owner.push(("harpoon-labelled-unconfirmed".into(), format!("It carries Harpoon's labels (application {a}, deployment {d}). The journal records no volumes, so this is a claim only.")));
                        } else {
                            owner.push(("unknown-ownership".into(), "No Harpoon labels, and the journal records no volumes. Ownership is not inferred from the name.".into()));
                        }
                        let usage = v.get("UsageData");
                        let mut details = vec![(
                            "Driver".to_string(),
                            v.get("Driver")
                                .and_then(Json::as_str)
                                .unwrap_or("not reported")
                                .to_string(),
                        )];
                        if let Some(m) = v.get("Mountpoint").and_then(Json::as_str) {
                            details.push(("Mount point".into(), m.to_string()));
                        }
                        details.push((
                            "Used by".into(),
                            match &refs {
                                None => "unknown".into(),
                                Some(r) if r.is_empty() => "no container".into(),
                                Some(r) => r
                                    .iter()
                                    .map(|x| {
                                        format!(
                                            "{} ({})",
                                            x.name,
                                            if x.running { "running" } else { "stopped" }
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            },
                        ));
                        volumes.items.push(Item {
                            id: name.to_string(),
                            name: name.to_string(),
                            kind: "volume",
                            bytes: usage.and_then(|u| size(u.get("Size"))),
                            bytes_note: "Docker's measurement of the volume's data; absent when Docker did not measure it.".into(),
                            state: if refs.as_ref().is_some_and(|r| r.iter().any(|x| x.running)) { "in-use".into() } else if refs.is_none() { "references-unknown".into() } else { "not-running".into() },
                            refs,
                            owner,
                            protection: "persistent",
                            protection_note: "Persistent data: shown for inspection only.".into(),
                            details,
                        });
                    }
                }
                Section::Absent => {
                    volumes =
                        volumes.unavailable("This Docker does not report volumes in /system/df.")
                }
                Section::Invalid => {
                    volumes = volumes.unavailable("Docker's volume list is not a list.")
                }
            }
            // ---- build cache
            match section(df, "BuildCache") {
                Section::Items(recs) => {
                    let (mut in_use, mut shared_n) = (0usize, 0usize);
                    let mut types: BTreeMap<String, usize> = BTreeMap::new();
                    let (mut total, mut total_ok, mut free, mut free_ok) = (0u64, true, 0u64, true);
                    for r in recs {
                        let Some(id) = r.get("ID").and_then(Json::as_str).filter(|v| !v.is_empty())
                        else {
                            cache.skipped += 1;
                            continue;
                        };
                        let used = r.get("InUse").and_then(Json::as_bool);
                        let shared = r.get("Shared").and_then(Json::as_bool);
                        let bytes = size(r.get("Size"));
                        let ty = r
                            .get("Type")
                            .and_then(Json::as_str)
                            .unwrap_or("unknown")
                            .to_string();
                        *types.entry(ty.clone()).or_default() += 1;
                        in_use += (used == Some(true)) as usize;
                        shared_n += (shared == Some(true)) as usize;
                        match bytes {
                            Some(b) => total += b,
                            None => total_ok = false,
                        }
                        match (bytes, used) {
                            (Some(b), Some(false)) => free += b,
                            (_, Some(true)) => {}
                            _ => free_ok = false,
                        }
                        let desc = r
                            .get("Description")
                            .and_then(Json::as_str)
                            .filter(|d| !d.is_empty());
                        cache.items.push(Item {
                            id: id.to_string(),
                            name: desc.map_or_else(|| format!("{ty} {}", short(id)), |d| d.chars().take(120).collect()),
                            kind: "cache",
                            bytes,
                            bytes_note: if shared == Some(true) { "Shares data with other cache records.".into() } else { "As Docker reports it.".into() },
                            state: match used { Some(true) => "build-in-use", Some(false) => "not-in-use", None => "use-unknown" }.into(),
                            refs: None,
                            owner: vec![("unknown-ownership".into(), "BuildKit cache records carry no Harpoon labels; the journal does not track them.".into())],
                            protection: if used == Some(true) { "in-use" } else { "none" },
                            protection_note: if used == Some(true) { "Docker says a build is using this record.".into() } else { "Inspect only.".into() },
                            details: vec![("Type".into(), ty), ("Record id".into(), id.to_string())],
                        });
                    }
                    cache.figures.push(("Reported by Docker as not in use".into(), free_ok.then_some(free), "Sum of records Docker marks not in use. Not a guarantee of disk space recovered.".into()));
                    cache.figures.push((
                        "Sum of record sizes".into(),
                        total_ok.then_some(total),
                        "Records marked shared may overlap.".into(),
                    ));
                    cache.counts = types
                        .into_iter()
                        .map(|(t, c)| (format!("type {t}"), c))
                        .chain([
                            ("in use".to_string(), in_use),
                            ("marked shared".to_string(), shared_n),
                        ])
                        .collect();
                }
                Section::Absent => {
                    cache =
                        cache.unavailable("This Docker does not report build cache in /system/df.")
                }
                Section::Invalid => {
                    cache = cache.unavailable("Docker's build cache list is not a list.")
                }
            }
        }
    }
    // The cache total is carried by `figures`; the category's own total would repeat it.
    obsolete.items = obsolete_items;

    let cats = [dep_images, other_images, cache, volumes, writable, obsolete];
    let unknown = cats
        .iter()
        .flat_map(|c| c.items.iter())
        .filter(|i| i.has("unknown-ownership"))
        .map(|i| (i.kind, i.id.as_str()))
        .collect::<BTreeSet<_>>()
        .len();
    let mut out = vec![
        ("schema", n(1)),
        ("generatedMs", n(now_ms as i64)),
        ("sources", sources),
        ("journalAvailable", Json::Bool(facts.available)),
        ("journalNote", facts.note.as_deref().map_or(Json::Null, s)),
        ("imageLayersBytes", jb(image_layers)),
        ("imageLayersMeaning", s("Docker's own count of the layers all images use, with shared layers counted once. It is Docker's accounting, not a measurement of disk space.")),
        ("unknownOwnershipItems", n(unknown as i64)),
    ];
    out.push((
        "categories",
        arr(cats.into_iter().map(Category::json).collect()),
    ));
    Ok(obj(out))
}
