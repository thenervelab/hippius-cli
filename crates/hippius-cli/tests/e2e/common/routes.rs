//! The check that each command sends only to the routes `hipc schema` lists for it, made on
//! every request a watched run sends, so the exported routes cannot fall behind the code.

use std::process::Stdio;
use std::sync::OnceLock;

use serde_json::Value;

use super::{Home, Recorded, command};

/// One command of `hipc schema`: its path, and the method and template of each of its routes.
struct Listed {
    path: Vec<String>,
    routes: Vec<(String, String)>,
}

/// What `hipc schema` says, read once per test binary.
struct Schema {
    /// Every command, the longest paths first.
    commands: Vec<Listed>,
    /// The global flags, which may stand between the words of a command path, each with
    /// whether it takes a value.
    global_flags: Vec<(String, bool)>,
}

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let output = command(&Home::new())
            .arg("schema")
            .stdout(Stdio::piped())
            .output()
            .expect("run `hipc schema`");
        assert!(output.status.success(), "`hipc schema` failed");
        let schema: Value = serde_json::from_slice(&output.stdout).expect("the schema is JSON");
        let text = |value: &Value| value.as_str().expect("a string").to_owned();
        let mut listed: Vec<Listed> = schema["commands"]
            .as_array()
            .expect("the schema lists its commands")
            .iter()
            .map(|entry| Listed {
                path: entry["path"]
                    .as_array()
                    .expect("a path")
                    .iter()
                    .map(text)
                    .collect(),
                routes: entry["routes"]
                    .as_array()
                    .expect("every command lists its routes")
                    .iter()
                    .map(|route| (text(&route["method"]), text(&route["path"])))
                    .collect(),
            })
            .collect();
        listed.sort_by_key(|command| std::cmp::Reverse(command.path.len()));
        let global_flags = schema["global_flags"]
            .as_array()
            .expect("the schema lists the global flags")
            .iter()
            .map(|flag| (text(&flag["name"]), !flag["value_name"].is_null()))
            .collect();
        Schema {
            commands: listed,
            global_flags,
        }
    })
}

/// `args` without the global flags and their values.
fn without_global_flags<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let flags = &schema().global_flags;
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        let name = arg.split_once('=').map_or(arg, |(name, _)| name);
        match flags.iter().find(|(flag, _)| flag == name) {
            // Its value is the next argument, unless it came after an `=`.
            Some((_, true)) if name == arg => {
                args.next();
            }
            Some(_) => {}
            None => words.push(arg),
        }
    }
    words
}

/// The command `args` runs: the longest command path whose words stand together in `args`
/// once the global flags, which may come anywhere, are left out.
fn command_of(args: &[&str]) -> Option<&'static Listed> {
    let args = without_global_flags(args);
    schema().commands.iter().find(|command| {
        args.windows(command.path.len()).any(|words| {
            words
                .iter()
                .zip(&command.path)
                .all(|(word, name)| word == name)
        })
    })
}

/// Fails when `request`, which the run of `args` sent, is not to one of the routes `hipc
/// schema` lists for the command. `hipc api` sends to the route it is given, so its requests
/// are not checked.
pub(super) fn assert_listed(args: &[&str], request: &Recorded) {
    let Some(command) = command_of(args) else {
        panic!(
            "{} {} was sent by no command: {args:?}",
            request.method, request.target
        );
    };
    if command.path == ["api"] {
        return;
    }
    let listed = command.routes.iter().any(|(method, template)| {
        *method == request.method
            && hippius::routes::all()
                .any(|endpoint| endpoint.template() == template && endpoint.matches(request.path()))
    });
    assert!(
        listed,
        "`hipc {}` sent {} {}, which its `routes` in `hipc schema` do not list: {:?}",
        command.path.join(" "),
        request.method,
        request.target,
        command.routes
    );
}
