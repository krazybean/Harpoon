use harpoon_deploy_cli::{run, Gate};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = run(&args, &Gate::shipping());
    print!("{}", out.stdout);
    eprint!("{}", out.stderr);
    std::process::exit(out.code);
}
