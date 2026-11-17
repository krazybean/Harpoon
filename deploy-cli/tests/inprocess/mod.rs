//! The desktop runs deployments in its own process while its screens and router read the same journal. Replacements through
//! the whole CLI path (simulated runtime, real SQLite journal) must all finish and be recorded while readers in this process
//! keep reading; before the in-process guard the writer's lock failed ("disk I/O error") and runs stopped unrecorded.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn replacements_complete_and_are_recorded_while_readers_in_the_same_process_keep_reading() {
    let t = T::new();
    let o = t.sim_deploy();
    assert_exit(&o, 0);
    let app = o
        .stdout
        .split("(application ")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap()
        .to_string();
    let dir = t.journal_dir();
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (dir, stop) = (dir.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = SqliteJournal::read_only(&dir);
                    std::thread::yield_now();
                }
            })
        })
        .collect();
    let src = t.src();
    let mut failures = Vec::new();
    for i in 0..8 {
        let mut a: Vec<&str> = vec!["replace", "--simulate", "--app", &app, "--confirm-cutover"];
        a.extend(src.iter().map(String::as_str));
        let o = t.cli(&a);
        if o.code != 0 {
            failures.push(format!(
                "replacement {i}: exit {} {}",
                o.code,
                o.stderr.lines().next().unwrap_or("")
            ));
        }
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
    assert!(failures.is_empty(), "{failures:?}");
    let status = t.cli(&["status"]).stdout;
    assert!(
        !status.contains(" running "),
        "a deployment was left running:\n{status}"
    );
    assert_eq!(
        status.matches(" succeeded ").count(),
        9,
        "every deployment is recorded as succeeded:\n{status}"
    );
}
