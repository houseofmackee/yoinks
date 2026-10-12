//! The `yoinks` command line. Runs when the binary gets a subcommand; talks
//! to the running app (starting it if needed) over the socket.

use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use crate::ipc::{Request, Response, client, server};
use crate::paths;

const USAGE: &str = "\
Usage: yoinks <command>

Commands:
  add <file>...          Put files on the shelf (several become a stack)
  add --text <text>      Put a text clipping on the shelf
  add --url <url> [--title <title>]
                         Put a link on the shelf
  list [--json]          List what is on the shelf
  show | hide            Show or hide the shelf
  recall                 Bring back the last item dragged out
  clear                  Remove everything (but pinned items) from the shelf
  shelf [<name>]         Switch to a shelf, or list them (* marks the current one)
  help                   Show this help

Yoinks is started in the background if it is not running.";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Send { request: Request, json: bool },
}

/// True if the arguments ask for the command line rather than the app. Any
/// word counts (so typos get an error, not a second app); arguments macOS
/// passes to apps (`-psn_...`, `-NSDocumentRevisionsDebugMode`) start with a
/// dash.
pub fn is_cli(args: &[String]) -> bool {
    args.first().is_some_and(|first| {
        !first.starts_with('-') || ["--help", "-h", "--version"].contains(&first.as_str())
    })
}

/// Parses arguments (without the program name). Relative paths are resolved
/// against `cwd`.
pub fn parse(args: &[String], cwd: &Path) -> Result<Command, String> {
    let (first, rest) = args.split_first().ok_or("no command")?;
    let simple = |request: Request| {
        if rest.is_empty() {
            Ok(Command::Send {
                request,
                json: false,
            })
        } else {
            Err(format!("{first} takes no arguments"))
        }
    };
    match first.as_str() {
        "help" | "--help" | "-h" | "--version" => Ok(Command::Help),
        "show" => simple(Request::Show),
        "hide" => simple(Request::Hide),
        "recall" => simple(Request::Recall),
        "clear" => simple(Request::Clear),
        "list" => match rest {
            [] => Ok(Command::Send {
                request: Request::List,
                json: false,
            }),
            [flag] if flag == "--json" => Ok(Command::Send {
                request: Request::List,
                json: true,
            }),
            _ => Err("usage: yoinks list [--json]".to_owned()),
        },
        "shelf" => Ok(Command::Send {
            request: Request::Shelf {
                name: (!rest.is_empty()).then(|| rest.join(" ")),
            },
            json: false,
        }),
        "add" => parse_add(rest, cwd),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn parse_add(args: &[String], cwd: &Path) -> Result<Command, String> {
    let (mut paths, mut text, mut url, mut title) = (Vec::new(), None, None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--text" => text = Some(value("--text")?),
            "--url" => url = Some(value("--url")?),
            "--title" => title = Some(value("--title")?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path => paths.push(absolute(Path::new(path), cwd)),
        }
    }
    if paths.is_empty() && text.is_none() && url.is_none() {
        return Err("add needs files, --text or --url".to_owned());
    }
    if !paths.is_empty() && (text.is_some() || url.is_some()) {
        return Err("add takes files, or --text, or --url, not a mix".to_owned());
    }
    Ok(Command::Send {
        request: Request::Add {
            paths,
            text,
            url,
            title,
        },
        json: false,
    })
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let command = match parse(args, &cwd) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("yoinks: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let Command::Send { request, json } = command else {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    let socket = server::socket_path(&paths::data_dir());
    if !client::is_running(&socket)
        && let Err(error) = launch_app(&socket)
    {
        eprintln!("yoinks: {error}");
        return ExitCode::FAILURE;
    }
    match client::send(&socket, &request) {
        Ok(response) => print(&request, &response, json),
        Err(error) => {
            eprintln!("yoinks: could not reach Yoinks: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print(request: &Request, response: &Response, json: bool) -> ExitCode {
    if !response.ok {
        eprintln!("yoinks: {}", response.error.as_deref().unwrap_or("failed"));
        return ExitCode::FAILURE;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&response.entries).unwrap_or_default()
        );
    } else if let Request::Shelf { name: None } = request {
        for name in &response.shelves {
            let mark = if response.shelf.as_ref() == Some(name) {
                "*"
            } else {
                " "
            };
            println!("{mark} {name}");
        }
    } else if *request == Request::List {
        for entry in &response.entries {
            let paths: Vec<String> = entry
                .items
                .iter()
                .map(|i| i.path.display().to_string())
                .collect();
            println!("{}\t{}", entry.title, paths.join("\t"));
        }
    }
    ExitCode::SUCCESS
}

/// Starts the app bundle this binary lives in, in the background, and waits
/// for its socket.
fn launch_app(socket: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = exe
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Yoinks is not running (start the app first)")?;
    let mut open = Process::new("/usr/bin/open");
    open.arg("-g").arg("-a").arg(bundle);
    if let Some(dir) = std::env::var_os("YOINKS_DATA_DIR") {
        let mut env = std::ffi::OsString::from("YOINKS_DATA_DIR=");
        env.push(dir);
        open.arg("--env").arg(env);
    }
    let status = open
        .status()
        .map_err(|e| format!("could not start Yoinks: {e}"))?;
    if !status.success() {
        return Err("could not start Yoinks".to_owned());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if client::is_running(socket) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("Yoinks did not start in time".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn send(request: Request) -> Command {
        Command::Send {
            request,
            json: false,
        }
    }

    #[test]
    fn detects_cli_invocations() {
        assert!(is_cli(&args(&["add", "x"])));
        assert!(is_cli(&args(&["--help"])));
        assert!(is_cli(&args(&["lsit"])), "typos are command line errors");
        assert!(!is_cli(&args(&[])), "no arguments: run the app");
        assert!(!is_cli(&args(&["-psn_0_12345"])), "Finder launch argument");
        assert!(!is_cli(&args(&["-NSDocumentRevisionsDebugMode", "YES"])));
    }

    #[test]
    fn parses_add() {
        let cwd = Path::new("/work");
        assert_eq!(
            parse(&args(&["add", "a.txt", "/abs/b.pdf"]), cwd).unwrap(),
            send(Request::add_paths(vec![
                "/work/a.txt".into(),
                "/abs/b.pdf".into()
            ]))
        );
        assert_eq!(
            parse(
                &args(&["add", "--url", "https://x.dev", "--title", "X"]),
                cwd
            )
            .unwrap(),
            send(Request::Add {
                paths: vec![],
                text: None,
                url: Some("https://x.dev".into()),
                title: Some("X".into())
            })
        );
        assert!(parse(&args(&["add"]), cwd).is_err());
        assert!(parse(&args(&["add", "--text"]), cwd).is_err());
        assert!(parse(&args(&["add", "a.txt", "--text", "hi"]), cwd).is_err());
        assert!(parse(&args(&["add", "--bogus"]), cwd).is_err());
    }

    #[test]
    fn parses_other_commands() {
        let cwd = Path::new("/");
        assert_eq!(
            parse(&args(&["list", "--json"]), cwd).unwrap(),
            Command::Send {
                request: Request::List,
                json: true
            }
        );
        assert_eq!(parse(&args(&["clear"]), cwd).unwrap(), send(Request::Clear));
        assert_eq!(
            parse(&args(&["shelf"]), cwd).unwrap(),
            send(Request::Shelf { name: None })
        );
        assert_eq!(
            parse(&args(&["shelf", "Design", "assets"]), cwd).unwrap(),
            send(Request::Shelf {
                name: Some("Design assets".to_owned())
            })
        );
        assert_eq!(parse(&args(&["help"]), cwd).unwrap(), Command::Help);
        assert!(parse(&args(&["show", "extra"]), cwd).is_err());
        assert!(parse(&args(&["list", "--yaml"]), cwd).is_err());
        assert!(parse(&args(&["explode"]), cwd).is_err());
    }
}
