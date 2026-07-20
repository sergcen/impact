use crate::config::{Config, ProjectSelection, matches_any_glob};
use crate::graph::{Snapshot, metadata_projects};
use crate::model::{Binding, Graph, GraphStats, Importer, Predecessor, Reason, Workspaces};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize)]
pub struct DependentResult {
    #[serde(rename = "dependentFiles")]
    pub dependent_files: Vec<String>,
    pub direct: bool,
    pub projects: Vec<String>,
    pub reasons: BTreeMap<String, Vec<Reason>>,
    #[serde(rename = "targetFiles")]
    pub target_files: Vec<String>,
    #[serde(rename = "targetSpecifiers")]
    pub target_specifiers: Vec<String>,
    #[serde(rename = "graphStats")]
    pub graph_stats: GraphStats,
}

#[derive(Serialize)]
pub struct AffectedResult {
    #[serde(rename = "affectedFiles")]
    pub affected_files: Vec<String>,
    pub base: String,
    #[serde(rename = "changedFiles")]
    pub changed_files: Vec<String>,
    #[serde(rename = "changedSpecifiers")]
    pub changed_specifiers: BTreeMap<String, Vec<String>>,
    #[serde(rename = "graphStats")]
    pub graph_stats: GraphStats,
    pub projects: Vec<String>,
    pub reasons: BTreeMap<String, Vec<Reason>>,
}

fn normalize_specifiers(specifiers: &[String]) -> BTreeSet<String> {
    let values: BTreeSet<String> = if specifiers.is_empty() {
        ["*".to_string()].into_iter().collect()
    } else {
        specifiers.iter().cloned().collect()
    };
    if values.contains("*") {
        ["*".to_string()].into_iter().collect()
    } else {
        values
    }
}

fn binding_matches(binding: &Binding, specifiers: &BTreeSet<String>) -> bool {
    if specifiers.contains("*") {
        return true;
    }
    if binding.imported == "*" {
        return specifiers
            .iter()
            .any(|specifier| !(binding.exclude_default && specifier == "default"));
    }
    specifiers.contains(&binding.imported)
}

fn propagate_specifiers(
    dependent: &Importer,
    specifiers: &BTreeSet<String>,
) -> Option<(BTreeSet<String>, Binding)> {
    let fallback = Binding {
        imported: "*".into(),
        ..Binding::default()
    };
    let bindings = if dependent.bindings.is_empty() {
        vec![&fallback]
    } else {
        dependent.bindings.iter().collect()
    };
    let matched: Vec<&Binding> = bindings
        .into_iter()
        .filter(|binding| binding_matches(binding, specifiers))
        .collect();
    if matched.is_empty() {
        return None;
    }
    if dependent.kind != "reexport" && dependent.kind != "type-reexport" {
        return Some((["*".to_string()].into_iter().collect(), matched[0].clone()));
    }
    let mut propagated = BTreeSet::new();
    for binding in &matched {
        match binding.exported.as_deref() {
            Some(exported) if exported != "*" => {
                propagated.insert(exported.to_string());
            }
            _ if specifiers.contains("*") => {
                propagated.insert("*".into());
            }
            _ => {
                propagated.extend(
                    specifiers
                        .iter()
                        .filter(|specifier| {
                            !(binding.exclude_default && specifier.as_str() == "default")
                        })
                        .cloned(),
                );
            }
        }
    }
    (!propagated.is_empty()).then(|| {
        let values: Vec<String> = propagated.into_iter().collect();
        (normalize_specifiers(&values), matched[0].clone())
    })
}

fn merge_file_specifiers(
    affected: &mut HashMap<String, BTreeSet<String>>,
    file: &str,
    incoming: &BTreeSet<String>,
) -> Option<BTreeSet<String>> {
    if affected
        .get(file)
        .is_some_and(|values| values.contains("*"))
    {
        return None;
    }
    if incoming.contains("*") {
        let wildcard: BTreeSet<String> = ["*".to_string()].into_iter().collect();
        affected.insert(file.to_string(), wildcard.clone());
        return Some(wildcard);
    }
    let current = affected.entry(file.to_string()).or_default();
    let added: BTreeSet<String> = incoming.difference(current).cloned().collect();
    if added.is_empty() {
        return None;
    }
    current.extend(added.iter().cloned());
    Some(added)
}

fn reconstruct_reason(
    file: &str,
    predecessors: &HashMap<String, Predecessor>,
    root_kind: &str,
) -> Vec<Reason> {
    let mut chain = vec![];
    let mut current = file.to_string();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        if let Some(predecessor) = predecessors.get(&current) {
            chain.push(Reason {
                exported_specifier: predecessor.exported_specifier.clone(),
                file: current,
                imported_specifier: Some(predecessor.imported_specifier.clone()),
                kind: predecessor.kind.clone(),
                via: Some(predecessor.specifier.clone()),
            });
            current = predecessor.from.clone();
        } else {
            chain.push(Reason {
                exported_specifier: None,
                file: current,
                imported_specifier: None,
                kind: root_kind.into(),
                via: None,
            });
            break;
        }
    }
    chain.reverse();
    chain
}

fn query_dependents_with(
    target_files: Vec<String>,
    target_specifiers: &[String],
    direct: bool,
    projects: &Workspaces,
    graph_stats: GraphStats,
    mut importers: impl FnMut(&str) -> Result<Vec<Importer>, String>,
) -> Result<DependentResult, String> {
    let target_specifiers = normalize_specifiers(target_specifiers);
    let targets: HashSet<String> = target_files.iter().cloned().collect();
    let mut affected = HashMap::new();
    let mut predecessors = HashMap::new();
    let mut queue = VecDeque::new();
    for target in &target_files {
        merge_file_specifiers(&mut affected, target, &target_specifiers);
        queue.push_back((target.clone(), target_specifiers.clone()));
    }
    while let Some((current, current_specifiers)) = queue.pop_front() {
        for dependent in importers(&current)? {
            let Some((propagated, binding)) = propagate_specifiers(&dependent, &current_specifiers)
            else {
                continue;
            };
            let Some(added) =
                merge_file_specifiers(&mut affected, &dependent.importer, &propagated)
            else {
                continue;
            };
            predecessors
                .entry(dependent.importer.clone())
                .or_insert(Predecessor {
                    exported_specifier: binding.exported,
                    from: current.clone(),
                    imported_specifier: binding.imported,
                    kind: dependent.kind.clone(),
                    specifier: dependent.specifier.clone(),
                });
            if !direct {
                queue.push_back((dependent.importer, added));
            }
        }
    }
    let mut dependent_files: Vec<String> = affected
        .into_keys()
        .filter(|file| !targets.contains(file))
        .collect();
    dependent_files.sort();
    let project_names: Vec<String> = dependent_files
        .iter()
        .filter_map(|file| projects.project_for_file(file))
        .map(|project| project.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let reasons = dependent_files
        .iter()
        .map(|file| {
            (
                file.clone(),
                reconstruct_reason(file, &predecessors, "target"),
            )
        })
        .collect();
    Ok(DependentResult {
        dependent_files,
        direct,
        projects: project_names,
        reasons,
        target_files,
        target_specifiers: target_specifiers.into_iter().collect(),
        graph_stats,
    })
}

pub fn normalize_target(cwd: &Path, file: &str) -> Result<String, String> {
    let candidate = Path::new(file);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    let relative = normalized
        .strip_prefix(cwd)
        .map_err(|_| format!("Module must be inside the repository: {file}"))?;
    if relative.as_os_str().is_empty() {
        return Err(format!("Module must be inside the repository: {file}"));
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub fn query_graph(
    cwd: &Path,
    graph: &Graph,
    workspaces: &Workspaces,
    files: &[String],
    specifiers: &[String],
    direct: bool,
) -> Result<DependentResult, String> {
    let mut targets: Vec<String> = files
        .iter()
        .map(|file| normalize_target(cwd, file))
        .collect::<Result<_, _>>()?;
    targets.sort();
    targets.dedup();
    let indexed: HashSet<&String> = graph.files.iter().collect();
    let missing: Vec<&String> = targets
        .iter()
        .filter(|file| !indexed.contains(file))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Modules not found or excluded from the graph: {}",
            missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    query_dependents_with(
        targets,
        specifiers,
        direct,
        workspaces,
        graph.stats.clone(),
        |file| Ok(graph.reverse.get(file).cloned().unwrap_or_default()),
    )
}

pub fn query_snapshot(
    cwd: &Path,
    snapshot: &mut Snapshot,
    files: &[String],
    specifiers: &[String],
    direct: bool,
) -> Result<DependentResult, String> {
    let mut targets: Vec<String> = files
        .iter()
        .map(|file| normalize_target(cwd, file))
        .collect::<Result<_, _>>()?;
    targets.sort();
    targets.dedup();
    for target in &targets {
        if !snapshot.contains(target)? {
            return Err(format!("module is absent from snapshot: {target}"));
        }
    }
    let projects = Workspaces {
        projects: metadata_projects(&snapshot.metadata),
    };
    let stats = GraphStats::cached(snapshot.metadata.file_count, snapshot.validation());
    query_dependents_with(targets, specifiers, direct, &projects, stats, |file| {
        snapshot.importers(file)
    })
}

pub fn analyze_affected(
    graph: &Graph,
    workspaces: &Workspaces,
    config: &Config,
    base: String,
    changed_files: Vec<String>,
    changed_specifiers: BTreeMap<String, Vec<String>>,
) -> AffectedResult {
    let mut affected_files = HashSet::new();
    let mut affected_specifiers = HashMap::new();
    let mut affected_projects: HashMap<String, Option<String>> = HashMap::new();
    let mut direct_reasons: HashMap<String, Reason> = HashMap::new();
    let mut predecessors = HashMap::new();
    let mut queue = VecDeque::new();
    let mut activated_fallbacks = HashSet::new();
    let enqueue = |file: String,
                   specifiers: BTreeSet<String>,
                   predecessor: Option<Predecessor>,
                   affected_files: &mut HashSet<String>,
                   affected_specifiers: &mut HashMap<String, BTreeSet<String>>,
                   predecessors: &mut HashMap<String, Predecessor>,
                   queue: &mut VecDeque<(String, BTreeSet<String>)>| {
        let Some(added) = merge_file_specifiers(affected_specifiers, &file, &specifiers) else {
            return;
        };
        affected_files.insert(file.clone());
        if let Some(predecessor) = predecessor {
            predecessors.entry(file.clone()).or_insert(predecessor);
        }
        queue.push_back((file, added));
    };
    for changed_file in &changed_files {
        let root_inputs: Vec<_> = config
            .root_inputs
            .iter()
            .filter(|input| matches_any_glob(changed_file, &input.patterns))
            .collect();
        if root_inputs.is_empty() {
            enqueue(
                changed_file.clone(),
                normalize_specifiers(
                    changed_specifiers
                        .get(changed_file)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                ),
                None,
                &mut affected_files,
                &mut affected_specifiers,
                &mut predecessors,
                &mut queue,
            );
            if let Some(project) = workspaces.project_for_file(changed_file)
                && changed_file == &project.manifest_path
            {
                for file in graph.files.iter().filter(|file| {
                    file.starts_with(&format!("{}/", project.dir)) && *file != changed_file
                }) {
                    enqueue(
                        file.clone(),
                        normalize_specifiers(&["*".into()]),
                        Some(Predecessor {
                            exported_specifier: None,
                            from: changed_file.clone(),
                            imported_specifier: "*".into(),
                            kind: "package-manifest".into(),
                            specifier: "package.json".into(),
                        }),
                        &mut affected_files,
                        &mut affected_specifiers,
                        &mut predecessors,
                        &mut queue,
                    );
                }
            }
            continue;
        }
        for root_input in root_inputs {
            match &root_input.projects {
                ProjectSelection::Keyword(keyword) if keyword == "dependents" => enqueue(
                    changed_file.clone(),
                    normalize_specifiers(&["*".into()]),
                    None,
                    &mut affected_files,
                    &mut affected_specifiers,
                    &mut predecessors,
                    &mut queue,
                ),
                ProjectSelection::Keyword(keyword) if keyword == "all" => {
                    for project in &workspaces.projects {
                        direct_reasons.insert(
                            project.name.clone(),
                            Reason {
                                exported_specifier: None,
                                file: changed_file.clone(),
                                imported_specifier: None,
                                kind: "root-input".into(),
                                via: None,
                            },
                        );
                    }
                }
                ProjectSelection::Names(names) => {
                    for name in names {
                        if workspaces.project_by_name(name).is_some() {
                            direct_reasons.insert(
                                name.clone(),
                                Reason {
                                    exported_specifier: None,
                                    file: changed_file.clone(),
                                    imported_specifier: None,
                                    kind: "root-input".into(),
                                    via: None,
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    while let Some((current, current_specifiers)) = queue.pop_front() {
        if let Some(project) = workspaces.project_for_file(&current) {
            affected_projects
                .entry(project.name.clone())
                .or_insert_with(|| Some(current.clone()));
            if config.package_fallback == "unresolved"
                && activated_fallbacks.insert(project.name.clone())
            {
                for unresolved in graph
                    .unresolved_by_workspace
                    .get(&project.name)
                    .into_iter()
                    .flatten()
                {
                    enqueue(
                        unresolved.importer.clone(),
                        normalize_specifiers(&["*".into()]),
                        Some(Predecessor {
                            exported_specifier: None,
                            from: current.clone(),
                            imported_specifier: "*".into(),
                            kind: "unresolved-workspace-fallback".into(),
                            specifier: unresolved.specifier.clone(),
                        }),
                        &mut affected_files,
                        &mut affected_specifiers,
                        &mut predecessors,
                        &mut queue,
                    );
                }
            }
        }
        for dependent in graph.reverse.get(&current).into_iter().flatten() {
            let Some((propagated, binding)) = propagate_specifiers(dependent, &current_specifiers)
            else {
                continue;
            };
            enqueue(
                dependent.importer.clone(),
                propagated,
                Some(Predecessor {
                    exported_specifier: binding.exported,
                    from: current.clone(),
                    imported_specifier: binding.imported,
                    kind: dependent.kind.clone(),
                    specifier: dependent.specifier.clone(),
                }),
                &mut affected_files,
                &mut affected_specifiers,
                &mut predecessors,
                &mut queue,
            );
        }
    }
    for project in direct_reasons.keys() {
        affected_projects.insert(project.clone(), None);
    }
    let mut projects: Vec<String> = affected_projects.keys().cloned().collect();
    projects.sort();
    let reasons = projects
        .iter()
        .map(|project| {
            let reasons = if let Some(reason) = direct_reasons.get(project) {
                vec![reason.clone()]
            } else {
                reconstruct_reason(
                    affected_projects
                        .get(project)
                        .and_then(Option::as_deref)
                        .unwrap_or_default(),
                    &predecessors,
                    "changed",
                )
            };
            (project.clone(), reasons)
        })
        .collect();
    let mut affected_files: Vec<String> = affected_files.into_iter().collect();
    affected_files.sort();
    AffectedResult {
        affected_files,
        base,
        changed_files,
        changed_specifiers,
        graph_stats: graph.stats.clone(),
        projects,
        reasons,
    }
}

pub fn target_exists(cwd: &Path, file: &str) -> bool {
    fs::metadata(cwd.join(file)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliased_reexport_propagates_only_the_matching_binding() {
        let importer = Importer {
            bindings: vec![
                Binding {
                    exported: Some("publicFeature".into()),
                    imported: "feature".into(),
                    ..Binding::default()
                },
                Binding {
                    exported: Some("publicOther".into()),
                    imported: "other".into(),
                    ..Binding::default()
                },
            ],
            importer: "index.ts".into(),
            kind: "reexport".into(),
            specifier: "./model".into(),
        };
        let (propagated, binding) =
            propagate_specifiers(&importer, &["feature".to_string()].into_iter().collect())
                .expect("matching binding");
        assert_eq!(
            propagated,
            ["publicFeature".to_string()].into_iter().collect()
        );
        assert_eq!(binding.imported, "feature");
    }
}
