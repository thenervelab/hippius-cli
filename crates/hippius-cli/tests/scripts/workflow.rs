//! The repository's GitHub workflow and action files, parsed as YAML, and the lookups the
//! tests make in them: jobs, steps by name, and the shell a step runs. The tests assert on
//! these values, never on the files' text, so moving or reindenting a step changes nothing.

use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

/// The file at `path`, relative to the repository root, parsed as one YAML document.
pub(crate) fn load(path: &str) -> Yaml {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut documents =
        YamlLoader::load_from_str(&text).unwrap_or_else(|e| panic!("{path} is YAML: {e}"));
    assert_eq!(documents.len(), 1, "{path} holds one document");
    documents.remove(0)
}

/// The release workflow.
pub(crate) fn release() -> Yaml {
    load(".github/workflows/release.yml")
}

/// The workflow that rehearses the release build.
pub(crate) fn release_check() -> Yaml {
    load(".github/workflows/release-check.yml")
}

/// The composite action every release build goes through.
pub(crate) fn release_build_action() -> Yaml {
    load(".github/actions/release-build/action.yml")
}

/// Every workflow file, by name.
pub(crate) fn every_workflow() -> Vec<(&'static str, Yaml)> {
    [
        "ci.yml",
        "release-check.yml",
        "release.yml",
        "scheduled.yml",
    ]
    .into_iter()
    .map(|name| (name, load(&format!(".github/workflows/{name}"))))
    .collect()
}

/// The string at `node`, which the file must hold there.
pub(crate) fn text<'a>(node: &'a Yaml, what: &str) -> &'a str {
    node.as_str()
        .unwrap_or_else(|| panic!("{what} is a string: {node:?}"))
}

/// The jobs of `workflow`, by id, in file order.
pub(crate) fn jobs(workflow: &Yaml) -> Vec<(&str, &Yaml)> {
    workflow["jobs"]
        .as_hash()
        .expect("a jobs mapping")
        .iter()
        .map(|(id, job)| (text(id, "a job id"), job))
        .collect()
}

/// The job `id` of `workflow`.
pub(crate) fn job<'a>(workflow: &'a Yaml, id: &str) -> &'a Yaml {
    let job = &workflow["jobs"][id];
    assert!(!job.is_badvalue(), "no job {id}");
    job
}

/// The jobs that job `job` names in `needs`, which is one id or a list of them.
pub(crate) fn needs(job: &Yaml) -> Vec<&str> {
    match &job["needs"] {
        Yaml::BadValue => Vec::new(),
        Yaml::Array(ids) => ids.iter().map(|id| text(id, "a needed job")).collect(),
        id => vec![text(id, "a needed job")],
    }
}

/// The steps of a job, or of a composite action's `runs`.
pub(crate) fn steps(job: &Yaml) -> &[Yaml] {
    job["steps"].as_vec().map_or(&[], Vec::as_slice)
}

/// The steps of every job of `workflow`.
pub(crate) fn every_step(workflow: &Yaml) -> impl Iterator<Item = &Yaml> {
    jobs(workflow).into_iter().flat_map(|(_, job)| steps(job))
}

/// The one step of `steps` named `name`.
pub(crate) fn named<'a>(steps: impl IntoIterator<Item = &'a Yaml>, name: &str) -> &'a Yaml {
    let found: Vec<&Yaml> = steps
        .into_iter()
        .filter(|step| step["name"].as_str() == Some(name))
        .collect();
    assert_eq!(found.len(), 1, "one step named {name}");
    found[0]
}

/// The shell that `step` runs.
pub(crate) fn run(step: &Yaml) -> &str {
    text(&step["run"], "the step's run")
}

/// The action `step` uses, if any, as written: `owner/repo@sha` or a local `./path`.
pub(crate) fn uses(step: &Yaml) -> Option<&str> {
    step["uses"].as_str()
}
