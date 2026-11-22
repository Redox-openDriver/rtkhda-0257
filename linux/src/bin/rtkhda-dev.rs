use std::path::Path;
use std::process::ExitCode;

use drv_i18n::{Catalog, Fail, Msg, as_msg};
use sonus::{Graph, Levels, Mixer, Role, Route};

type BoxError = Box<dyn std::error::Error>;

fn role_key(role: Role) -> &'static str {
    match role {
        Role::Speaker => "dev-role-speaker",
        Role::Headphone => "dev-role-headphone",
        Role::LineOut => "dev-role-line-out",
        Role::InternalMic => "dev-role-internal-mic",
        Role::HeadsetMic => "dev-role-headset-mic",
        Role::LineIn => "dev-role-line-in",
    }
}

fn nodes(path: &[u8]) -> String {
    path.iter().map(|node| format!("{node:#04x}")).collect::<Vec<_>>().join(" → ")
}

fn route_line(catalog: &Catalog, route: &Route) -> String {
    let key = if route.role.output() { "dev-route-out" } else { "dev-route-in" };
    catalog.render(&Msg::new(key).arg("role", catalog.text(role_key(route.role))).arg("path", nodes(&route.path)))
}

fn graphs() -> Result<Vec<(String, u32, Graph)>, BoxError> {
    let dumps = rtkhda_linux::dumps(Path::new(rtkhda_linux::ASOUND)).map_err(|e| Fail::new("dev-no-asound").arg("detail", e))?;
    let mut found = Vec::new();
    for path in dumps {
        let Ok(text) = rtkhda_linux::read(&path) else { continue };
        let Some(graph) = sonus::procfs::parse(&text) else { continue };
        if graph.outputs().is_empty() && graph.inputs().is_empty() {
            continue;
        }
        let name = rtkhda_linux::field(&text, "Codec:").unwrap_or("?").to_string();
        found.push((name, rtkhda_linux::vendor(&text).unwrap_or(0), graph));
    }
    if found.is_empty() {
        return Err(Fail::new("dev-no-codecs").into());
    }
    Ok(found)
}

fn show(catalog: &Catalog) -> Result<(), BoxError> {
    for (name, vendor, graph) in graphs()? {
        println!("{}", catalog.render(&Msg::new("dev-codec").arg("name", &name).arg("vendor", format!("{vendor:#010x}"))));
        for route in graph.outputs().iter().chain(&graph.inputs()) {
            println!("  {}", route_line(catalog, route));
        }
        let levels = Levels::default();
        println!(
            "  {}",
            catalog
                .render(&Msg::new("dev-levels").num("speaker", levels.speaker).num("headphones", levels.headphone).num("safe", sonus::mixer::SAFE_HEADPHONES))
        );
    }
    Ok(())
}

fn verbs(catalog: &Catalog, plugged: &[&str]) -> Result<(), BoxError> {
    let (_, _, graph) = graphs()?.into_iter().next().ok_or_else(|| Fail::new("dev-no-codecs"))?;
    let mut mixer = Mixer::new(0, 1, graph.clone(), Levels::default());
    let mut verbs = mixer.start();
    for what in plugged {
        let role = match *what {
            "headphones" => Role::Headphone,
            "headset" => Role::HeadsetMic,
            other => return Err(Fail::new("dev-bad-option").arg("value", other).into()),
        };
        let pin =
            graph.outputs().iter().chain(&graph.inputs()).find(|route| route.role == role).map(|route| route.pin).ok_or_else(|| Fail::new("dev-no-jack"))?;
        verbs.extend(mixer.sensed(pin, true));
    }
    for verb in verbs {
        println!("{}", catalog.render(&Msg::new("dev-verb").arg("verb", format!("{:08x}", verb.0)).arg("node", format!("{:#04x}", verb.node()))));
    }
    Ok(())
}

fn run(catalog: &Catalog) -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("show") => show(catalog),
        Some("verbs") => verbs(catalog, &args[1..].iter().map(String::as_str).collect::<Vec<_>>()),
        Some("help" | "--help" | "-h") => {
            println!("{}", catalog.text("dev-usage"));
            Ok(())
        }
        Some(other) => Err(Fail::new("dev-bad-option").arg("value", other).into()),
    }
}

fn main() -> ExitCode {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let catalog = rtkhda_linux::catalog();
    match run(&catalog) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", catalog.render(&as_msg(e.as_ref(), "dev-error")));
            ExitCode::FAILURE
        }
    }
}
