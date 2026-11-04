//! Pure and loopback-only units: Dockerfile checks, preflight helpers, the real HTTP transport
//! against a listener this test owns.

use harpoon_deploy::buildx::{resolve, BuildxError, FileKind, FileProbe};
use harpoon_deploy::health::Observation;
use harpoon_deploy_docker::buildx_run::StdFileProbe;
use harpoon_deploy_docker::dockerfile::{required_images, DockerfileProblem};
use harpoon_deploy_docker::preflight::{context_size, parse_df};
use harpoon_deploy_docker::probe::{BindProbe, HttpTransport, PortProbe, TcpHttp};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn required_images_come_from_from_and_copy_from_only() {
    let r = required_images("FROM --platform=linux/arm64 node:22 AS build\nRUN x\nFROM alpine:3.22\nCOPY --from=build /a /b\nCOPY --from=0 /a /b\nCOPY --from=busybox:1 /x /y\n").unwrap();
    assert_eq!(
        r.into_iter().collect::<Vec<_>>(),
        ["alpine:3.22", "busybox:1", "node:22"]
    );
    assert!(required_images("FROM scratch\n").unwrap().is_empty());
    assert_eq!(
        required_images("FROM a AS x\nFROM X\n").unwrap().len(),
        1,
        "stage names are case-insensitive"
    );
}

#[test]
fn syntax_directives_variables_and_empty_files_are_problems() {
    assert!(matches!(
        required_images("# syntax=docker/dockerfile:1\nFROM a\n"),
        Err(DockerfileProblem::ExternalFrontend(_))
    ));
    assert!(matches!(
        required_images("#syntax = docker/dockerfile:1.7\nFROM a\n"),
        Err(DockerfileProblem::ExternalFrontend(_))
    ));
    assert!(matches!(
        required_images("FROM ${BASE}\n"),
        Err(DockerfileProblem::VariableImage(_))
    ));
    assert!(matches!(
        required_images("FROM a\nCOPY --from=$X /a /b\n"),
        Err(DockerfileProblem::VariableImage(_))
    ));
    assert!(matches!(
        required_images("# only a comment\n"),
        Err(DockerfileProblem::Empty)
    ));
    // an ordinary comment that mentions syntax later in the file is fine
    assert!(required_images("FROM a\n# syntax=whatever is just a comment here\n").is_ok());
}

#[test]
fn context_size_counts_files_stops_at_the_limit_and_ignores_symlinks() {
    let d = std::env::temp_dir().join(format!("hdd-ctx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("a"), vec![0; 1000]).unwrap();
    std::fs::write(d.join("sub/b"), vec![0; 500]).unwrap();
    std::os::unix::fs::symlink("/usr/lib", d.join("link")).unwrap();
    assert_eq!(context_size(&d, 1 << 30).unwrap(), 1500);
    assert!(
        context_size(&d, 900).unwrap() > 900,
        "stops once over the limit"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn df_output_is_parsed_strictly() {
    assert_eq!(parse_df("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 971350180 400 5000 41% /\n").unwrap(), 5000 * 1024);
    assert!(parse_df("").is_err());
    assert!(parse_df("header\n/dev/x 1 2\n").is_err());
    assert!(parse_df("header\n/dev/x 1 2 notanumber 4 /\n").is_err());
}

fn serve_once(response: &'static [u8]) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(response);
        }
    });
    port
}

#[test]
fn the_real_transport_reports_what_it_saw() {
    let t = Duration::from_secs(2);
    let ok = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(TcpHttp.get(ok, "/", t), Observation::Status(200));
    let bad = serve_once(b"HTTP/1.1 503 Service Unavailable\r\n\r\n");
    assert_eq!(TcpHttp.get(bad, "/health", t), Observation::Status(503));
    let junk = serve_once(b"SSH-2.0-OpenSSH_9.0\r\n");
    assert!(
        matches!(TcpHttp.get(junk, "/", t), Observation::ConnectFailed(m) if m.contains("not an HTTP response"))
    );
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    assert!(matches!(
        TcpHttp.get(closed, "/", t),
        Observation::ConnectFailed(_)
    ));
    assert!(!TcpHttp.is_simulated());
}

#[test]
fn the_bind_probe_sees_a_port_that_is_in_use() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(BindProbe.is_listening(port));
    drop(l);
    assert!(!BindProbe.is_listening(port));
}

#[test]
fn the_file_probe_distinguishes_missing_directory_symlink_and_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("hdd-fp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let bin = d.join("harpoon/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let p = bin.join("docker-buildx");
    assert!(matches!(
        resolve(Some(&d), &StdFileProbe),
        Err(BuildxError::Missing(_))
    ));
    std::fs::write(&p, "x").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(StdFileProbe.kind(&p), FileKind::File { executable: false });
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(resolve(Some(&d), &StdFileProbe).is_ok());
    std::fs::remove_file(&p).unwrap();
    std::fs::create_dir(&p).unwrap();
    assert!(matches!(
        resolve(Some(&d), &StdFileProbe),
        Err(BuildxError::NotAFile(_))
    ));
    let _ = std::fs::remove_dir_all(&d);
}
