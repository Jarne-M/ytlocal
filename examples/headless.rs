//! Drive the backend without the GUI, for end-to-end testing.
//!
//!   cargo run --example headless -- tools
//!   cargo run --example headless -- install ytdlp|ffmpeg|deno
//!   cargo run --example headless -- update

use ytlocal::backend::{Backend, Event, Request};
use ytlocal::binaries::Package;

fn wait_tool_task(backend: &Backend) -> bool {
    let mut ok = true;
    let mut last_stage = String::new();
    while let Some(ev) = backend.recv() {
        match ev {
            Event::ToolProgress { pkg, progress } => {
                if progress.stage != last_stage {
                    println!("[{}] {}", pkg.label(), progress.stage);
                    last_stage = progress.stage.clone();
                }
            }
            Event::ToolFinished { pkg, result } => {
                println!("[{}] finished: {result:?}", pkg.label());
                ok = result.is_ok();
            }
            Event::Tools(report) => {
                print_report(&report);
                return ok;
            }
        }
    }
    ok
}

fn print_report(report: &ytlocal::binaries::ToolReport) {
    for tool in ytlocal::binaries::Tool::ALL {
        println!("{:8} {:?}", tool.label(), report.get(tool));
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start(rt.handle(), None);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = match args.first().map(String::as_str) {
        Some("tools") | None => {
            backend.send(Request::RefreshTools);
            wait_tool_task(&backend)
        }
        Some("install") => {
            let pkg = match args.get(1).map(String::as_str) {
                Some("ytdlp") => Package::YtDlp,
                Some("ffmpeg") => Package::Ffmpeg,
                Some("deno") => Package::Deno,
                _ => panic!("install ytdlp|ffmpeg|deno"),
            };
            backend.send(Request::Install(pkg));
            wait_tool_task(&backend)
        }
        Some("update") => {
            backend.send(Request::RefreshTools);
            wait_tool_task(&backend);
            backend.send(Request::UpdateYtDlp);
            wait_tool_task(&backend)
        }
        Some(other) => panic!("unknown command {other}"),
    };
    std::process::exit(if ok { 0 } else { 1 });
}
