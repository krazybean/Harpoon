//! A removed registration as the journal stores and replays it: framed with a checksum, applied in order, and read
//! all-or-nothing. Nothing writes this record yet; these tests prove a build can already read it.

use harpoon_deploy::git::*;
use harpoon_deploy::journal_format::{decode_line, replay, Line};

const APP: &str = "app_00000000000000000000000001";

fn registration(at_ms: u64) -> RepoRec {
    RepoRec {
        name: "shop".into(),
        url: "https://example.com/acme/shop.git".into(),
        username: Some("ci".into()),
        cred: CredKind::Token,
        known_host: None,
        webhook: None,
        app: APP.parse().unwrap(),
        project: None,
        environment: None,
        branch: "main".into(),
        filters: vec![],
        container_port: 8080,
        dockerfile: None,
        context: None,
        target: None,
        health_path: None,
        at_ms,
    }
}

fn removal(r: &RepoRec, at_ms: u64) -> GitRecord {
    GitRecord::Removed(RemovedRec {
        name: r.name.clone(),
        app: r.app.clone(),
        registration: r.at_ms,
        url: r.url.clone(),
        cred: r.cred,
        at_ms,
    })
}

fn seen(commit: char, at_ms: u64) -> GitRecord {
    GitRecord::Seen(SeenRec {
        repo: "shop".into(),
        ref_name: "refs/heads/main".into(),
        commit: commit.to_string().repeat(40),
        at_ms,
    })
}

fn lines(records: &[GitRecord]) -> Vec<Line> {
    records
        .iter()
        .map(|r| decode_line(&encode(r)).expect("a framed record decodes"))
        .collect()
}

#[test]
fn a_removal_is_stored_with_a_checksum_and_a_flipped_byte_is_caught() {
    let r = registration(5);
    let rm = removal(&r, 9);
    let line = encode(&rm);
    assert_eq!(decode_line(&line).unwrap(), Line::Git(rm));
    // flip one character of the body: the checksum no longer matches
    let mut bytes = line.into_bytes();
    let last = bytes.len() - 1;
    bytes[last] = if bytes[last] == b'1' { b'2' } else { b'1' };
    let err = decode_line(&String::from_utf8(bytes).unwrap()).unwrap_err();
    assert!(err.0.contains("checksum"), "{err}");
}

#[test]
fn replay_applies_a_removal_in_order_and_keeps_every_other_record() {
    let r = registration(5);
    let state = replay(&lines(&[
        GitRecord::Repo(Box::new(r.clone())),
        seen('a', 6),
        removal(&r, 9),
        // the name is registered again, with a fresh application, after the removal
        GitRecord::Repo(Box::new(RepoRec {
            app: "app_00000000000000000000000002".parse().unwrap(),
            at_ms: 12,
            ..r.clone()
        })),
        seen('b', 13),
    ]))
    .expect("a journal with a removal replays");
    let git = state.git;
    assert_eq!(git.retired.len(), 1);
    assert_eq!(git.repos["shop"].app.to_string(), "app_00000000000000000000000002");
    assert_eq!(
        git.seen.len(),
        1,
        "only the observation made under the new registration"
    );
    assert_eq!(git.seen.values().next().unwrap().commit, "b".repeat(40));
}

#[test]
fn a_record_that_cannot_apply_makes_replay_fail_rather_than_continue_with_a_wrong_state() {
    let r = registration(5);
    // an observation or a source written after the removal: this is what the writers must never append
    let late = replay(&lines(&[
        GitRecord::Repo(Box::new(r.clone())),
        removal(&r, 9),
        seen('c', 10),
    ]));
    assert!(format!("{late:?}").contains("does not apply"), "{late:?}");
    // a removal of something never registered, and a removal of a stale version
    assert!(replay(&lines(&[removal(&r, 9)])).is_err());
    assert!(replay(&lines(&[GitRecord::Repo(Box::new(registration(6))), removal(&r, 9)])).is_err());
    // another name taking the removed application
    let theft = replay(&lines(&[
        GitRecord::Repo(Box::new(r.clone())),
        removal(&r, 9),
        GitRecord::Repo(Box::new(RepoRec {
            name: "other".into(),
            at_ms: 11,
            ..r.clone()
        })),
    ]));
    assert!(format!("{theft:?}").contains("removed repository shop"), "{theft:?}");
}

#[test]
fn a_journal_without_removals_replays_exactly_as_before() {
    let r = registration(5);
    let state = replay(&lines(&[GitRecord::Repo(Box::new(r)), seen('d', 6)])).unwrap();
    assert!(state.git.retired.is_empty());
    assert_eq!((state.git.repos.len(), state.git.seen.len()), (1, 1));
}

#[test]
fn a_record_kind_this_build_does_not_know_is_an_error_which_is_the_boundary_older_builds_hit() {
    // An older build does not know "gitdrop" and fails exactly like this one does for a kind it does not know: the
    // whole journal is refused, never read past. That is why the writer is separate and later.
    let err = decode_line("00000000\tgitdrop99\t1\tx").unwrap_err();
    assert!(!err.0.is_empty());
}
