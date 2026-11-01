//! The ACCEPTANCE build of the CLI (Cargo feature `acceptance`): live `replace`, `rollback` and
//! `restore` are open, for live acceptance runs under written authorization. It is not the shipping
//! binary and is never part of a release: `harpoon-deploy` (`src/main.rs`) keeps `Gate::shipping()`.
//!
//! Everything else is the library's and unchanged: the exact authorization phrase, an explicit
//! `--docker-socket` and the other explicit paths, ownership verification, the journal and the
//! bounded recovery. There is no fault injection, no crash control and no synthetic health failure.

use harpoon_deploy_cli::{run, Gate};

const REV: &str = match option_env!("HARPOON_ACCEPTANCE_REV") {
    Some(r) => r,
    None => "unknown",
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--acceptance-info") {
        println!("harpoon-deploy-acceptance: ACCEPTANCE BUILD, live replace/rollback/restore OPEN, build revision {REV}");
        return;
    }
    eprintln!("*** ACCEPTANCE BUILD — live replacement gate OPEN — revision {REV} ***");
    let out = run(
        &args,
        &Gate {
            live_deploy_enabled: true,
            live_replacement_enabled: true,
        },
    );
    print!("{}", out.stdout);
    eprint!("{}", out.stderr);
    std::process::exit(out.code);
}
