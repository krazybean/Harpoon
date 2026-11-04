//! Read-only preflight of a real Harpoon runtime: runs Harpoon's own Docker and management-channel
//! parsers against the live answers and reports which accept them. **Every call here is a GET or one of
//! three fixed read-only guest commands.** It creates, starts, stops, removes and builds nothing.
//!
//!     cargo run --example live_preflight -- <docker-socket> <management-socket>
//!
//! Both sockets are required arguments; there are no defaults.

use harpoon_deploy_docker::api::DockerApi;
use harpoon_deploy_docker::guest::MgmtDfProbe;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [docker, mgmt] = args.as_slice() else {
        eprintln!("usage: live_preflight <docker-socket> <management-socket>");
        std::process::exit(2);
    };
    let t = Duration::from_secs(10);
    let api = DockerApi::new(docker, t);
    let mut bad = 0;
    let mut say = |what: &str, r: Result<String, String>| match r {
        Ok(s) => println!("PASS {what}: {s}"),
        Err(e) => {
            bad += 1;
            println!("FAIL {what}: {e}")
        }
    };
    say(
        "ping",
        api.ping(t).map(|()| "OK".into()).map_err(|e| e.to_string()),
    );
    say(
        "info",
        api.info(t)
            .map(|i| {
                format!(
                    "name={:?} root={:?} kernel={:?}",
                    i.name, i.root_dir, i.kernel_version
                )
            })
            .map_err(|e| e.to_string()),
    );
    let containers = api.list_containers(t);
    say(
        "container list",
        containers
            .as_ref()
            .map(|c| format!("{} containers parsed", c.len()))
            .map_err(|e| e.to_string()),
    );
    for c in containers.unwrap_or_default() {
        say(
            &format!("container inspect {}", &c.id[..12.min(c.id.len())]),
            api.container_inspect(&c.id, t)
                .map(|i| match i {
                    Some(i) => format!(
                        "running={} labels={} published={:?}",
                        i.running,
                        i.labels.len(),
                        i.published
                            .iter()
                            .map(|p| (p.container_port, p.host_ip.clone(), p.host_port))
                            .collect::<Vec<_>>()
                    ),
                    None => "vanished".into(),
                })
                .map_err(|e| e.to_string()),
        );
        say(
            &format!("configured ports {}", &c.id[..12.min(c.id.len())]),
            api.configured_host_ports(&c.id, t)
                .map(|p| format!("{p:?}"))
                .map_err(|e| e.to_string()),
        );
    }
    say(
        "published ports",
        api.published_ports(t)
            .map(|p| format!("{p:?}"))
            .map_err(|e| e.to_string()),
    );
    let images = api.list_images(t);
    say(
        "image list",
        images
            .as_ref()
            .map(|i| format!("{} rows parsed", i.len()))
            .map_err(|e| e.to_string()),
    );
    for (id, tag, _) in images.unwrap_or_default() {
        say(
            &format!("image inspect {tag}"),
            api.image_inspect(&id, t)
                .map(|i| match i {
                    Some(i) => format!("id_ok={} volumes={:?}", i.id == id, i.volumes),
                    None => "vanished".into(),
                })
                .map_err(|e| e.to_string()),
        );
    }
    say(
        "base image alpine:3.22",
        api.image_inspect("alpine:3.22", t)
            .map(|i| match i {
                Some(i) => format!("present id={} volumes={:?}", i.id, i.volumes),
                None => "ABSENT".into(),
            })
            .map_err(|e| e.to_string()),
    );
    say(
        "volume list",
        api.list_volumes(t)
            .map(|v| format!("{v:?}"))
            .map_err(|e| e.to_string()),
    );
    let probe = MgmtDfProbe::new(docker, mgmt, t);
    say(
        "guest free space (MgmtDfProbe)",
        probe
            .measure()
            .map(|(free, what)| format!("{free} bytes free on {what}")),
    );
    println!(
        "{}",
        if bad == 0 {
            "ALL PARSERS ACCEPTED THE LIVE ANSWERS"
        } else {
            "SOME PARSERS REJECTED LIVE ANSWERS"
        }
    );
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
