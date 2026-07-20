use crate::model::{Binding, Dependency};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Declaration, ExportAllDeclaration, ExportNamedDeclaration,
    Expression, ImportDeclaration, ImportDeclarationSpecifier, ImportExpression, NewExpression,
    Program, Statement, TSModuleReference,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

static STYLE_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"@import\s+(?:url\(\s*)?['\"]([^'\"]+)['\"]\s*\)?"#).expect("regex")
});
static STYLE_COMPOSES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bcomposes\s*:[^;]*?\sfrom\s*['\"]([^'\"]+)['\"]"#).expect("regex")
});
static STYLE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\burl\(\s*['\"]?([^)'\"\s]+)['\"]?\s*\)"#).expect("regex"));

fn wildcard_binding() -> Binding {
    Binding {
        imported: "*".into(),
        ..Binding::default()
    }
}

fn add_dependency(dependencies: &mut Vec<Dependency>, mut dependency: Dependency) {
    if dependency.bindings.is_empty() {
        dependency.bindings.push(wildcard_binding());
    }
    if let Some(existing) = dependencies.iter_mut().find(|existing| {
        existing.kind == dependency.kind
            && existing.specifier == dependency.specifier
            && existing.glob_pattern.is_some() == dependency.glob_pattern.is_some()
    }) {
        let mut seen: HashSet<Binding> = existing.bindings.iter().cloned().collect();
        for binding in dependency.bindings {
            if seen.insert(binding.clone()) {
                existing.bindings.push(binding);
            }
        }
    } else {
        dependencies.push(dependency);
    }
}

fn dependency(kind: &str, specifier: impl Into<String>, bindings: Vec<Binding>) -> Dependency {
    Dependency {
        bindings,
        glob_pattern: None,
        kind: kind.into(),
        reason: None,
        specifier: specifier.into(),
        target: None,
        unresolved: None,
        workspace: None,
    }
}

fn parse_program<'a>(allocator: &'a Allocator, file: &str, source: &'a str) -> Program<'a> {
    let source_type = SourceType::from_path(file).unwrap_or_default();
    Parser::new(allocator, source, source_type).parse().program
}

fn import_bindings(node: &ImportDeclaration<'_>) -> (Vec<Binding>, Vec<Binding>) {
    let declaration_type = node.import_kind.is_type();
    let Some(specifiers) = &node.specifiers else {
        return (vec![wildcard_binding()], vec![]);
    };
    if specifiers.is_empty() {
        return (vec![wildcard_binding()], vec![]);
    }
    let mut runtime = vec![];
    let mut typed = vec![];
    for specifier in specifiers {
        let (imported, type_only) = match specifier {
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => (
                specifier.imported.name().to_string(),
                declaration_type || specifier.import_kind.is_type(),
            ),
            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {
                ("default".into(), declaration_type)
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                ("*".into(), declaration_type)
            }
        };
        let binding = Binding {
            imported,
            ..Binding::default()
        };
        if type_only {
            typed.push(binding);
        } else {
            runtime.push(binding);
        }
    }
    (runtime, typed)
}

fn named_export_bindings(node: &ExportNamedDeclaration<'_>) -> (Vec<Binding>, Vec<Binding>) {
    let declaration_type = node.export_kind.is_type();
    let mut runtime = vec![];
    let mut typed = vec![];
    for specifier in &node.specifiers {
        let binding = Binding {
            exclude_default: false,
            exported: Some(specifier.exported.name().to_string()),
            imported: specifier.local.name().to_string(),
        };
        if declaration_type || specifier.export_kind.is_type() {
            typed.push(binding);
        } else {
            runtime.push(binding);
        }
    }
    if runtime.is_empty() && typed.is_empty() {
        if declaration_type {
            typed.push(wildcard_binding());
        } else {
            runtime.push(wildcard_binding());
        }
    }
    (runtime, typed)
}

fn all_export_binding(node: &ExportAllDeclaration<'_>) -> Binding {
    match &node.exported {
        Some(exported) => Binding {
            exported: Some(exported.name().to_string()),
            imported: "*".into(),
            ..Binding::default()
        },
        None => Binding {
            exclude_default: true,
            exported: Some("*".into()),
            imported: "*".into(),
        },
    }
}

struct DynamicVisitor {
    dependencies: Vec<Dependency>,
}

impl DynamicVisitor {
    fn string_argument(argument: Option<&Argument<'_>>) -> Option<String> {
        match argument? {
            Argument::StringLiteral(value) => Some(value.value.to_string()),
            _ => None,
        }
    }

    fn is_import_meta_member(expression: &Expression<'_>, names: &[&str]) -> bool {
        let Expression::StaticMemberExpression(member) = expression else {
            return false;
        };
        let Expression::MetaProperty(meta) = &member.object else {
            return false;
        };
        meta.meta.name == "import"
            && meta.property.name == "meta"
            && names.contains(&member.property.name.as_str())
    }
}

impl<'a> Visit<'a> for DynamicVisitor {
    fn visit_import_expression(&mut self, expression: &ImportExpression<'a>) {
        if let Expression::StringLiteral(value) = &expression.source {
            add_dependency(
                &mut self.dependencies,
                dependency("dynamic", value.value.to_string(), vec![wildcard_binding()]),
            );
        }
        walk::walk_import_expression(self, expression);
    }

    fn visit_call_expression(&mut self, expression: &CallExpression<'a>) {
        if let Some(specifier) = Self::string_argument(expression.arguments.first()) {
            if matches!(&expression.callee, Expression::Identifier(identifier) if identifier.name == "require")
            {
                add_dependency(
                    &mut self.dependencies,
                    dependency("runtime", specifier, vec![wildcard_binding()]),
                );
            } else if Self::is_import_meta_member(&expression.callee, &["glob", "globEager"]) {
                let mut item = dependency("glob", specifier.clone(), vec![wildcard_binding()]);
                item.glob_pattern = Some(specifier);
                add_dependency(&mut self.dependencies, item);
            }
        }
        walk::walk_call_expression(self, expression);
    }

    fn visit_new_expression(&mut self, expression: &NewExpression<'a>) {
        let is_url = matches!(&expression.callee, Expression::Identifier(identifier) if identifier.name == "URL");
        let has_import_meta_url =
            expression
                .arguments
                .get(1)
                .is_some_and(|argument| match argument {
                    Argument::StaticMemberExpression(member) => {
                        matches!(&member.object, Expression::MetaProperty(meta)
                        if meta.meta.name == "import"
                            && meta.property.name == "meta"
                            && member.property.name == "url")
                    }
                    _ => false,
                });
        if is_url
            && has_import_meta_url
            && let Some(specifier) = Self::string_argument(expression.arguments.first())
        {
            add_dependency(
                &mut self.dependencies,
                dependency("asset", specifier, vec![wildcard_binding()]),
            );
        }
        walk::walk_new_expression(self, expression);
    }
}

fn extract_code_dependencies(program: &Program<'_>) -> Vec<Dependency> {
    let mut dependencies = vec![];
    for statement in &program.body {
        match statement {
            Statement::ImportDeclaration(node) => {
                let (runtime, typed) = import_bindings(node);
                if !runtime.is_empty() {
                    add_dependency(
                        &mut dependencies,
                        dependency("runtime", node.source.value.to_string(), runtime),
                    );
                }
                if !typed.is_empty() {
                    add_dependency(
                        &mut dependencies,
                        dependency("type", node.source.value.to_string(), typed),
                    );
                }
            }
            Statement::ExportNamedDeclaration(node) if node.source.is_some() => {
                let (runtime, typed) = named_export_bindings(node);
                let source = node.source.as_ref().expect("checked").value.to_string();
                if !runtime.is_empty() {
                    add_dependency(
                        &mut dependencies,
                        dependency("reexport", source.clone(), runtime),
                    );
                }
                if !typed.is_empty() {
                    add_dependency(
                        &mut dependencies,
                        dependency("type-reexport", source, typed),
                    );
                }
            }
            Statement::ExportAllDeclaration(node) => {
                let kind = if node.export_kind.is_type() {
                    "type-reexport"
                } else {
                    "reexport"
                };
                add_dependency(
                    &mut dependencies,
                    dependency(
                        kind,
                        node.source.value.to_string(),
                        vec![all_export_binding(node)],
                    ),
                );
            }
            Statement::TSImportEqualsDeclaration(node) => {
                if let TSModuleReference::ExternalModuleReference(reference) =
                    &node.module_reference
                {
                    add_dependency(
                        &mut dependencies,
                        dependency(
                            if node.import_kind.is_type() {
                                "type"
                            } else {
                                "runtime"
                            },
                            reference.expression.value.to_string(),
                            vec![wildcard_binding()],
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    let mut visitor = DynamicVisitor {
        dependencies: vec![],
    };
    visitor.visit_program(program);
    for item in visitor.dependencies {
        add_dependency(&mut dependencies, item);
    }
    dependencies
}

fn extract_style_dependencies(source: &str) -> Vec<Dependency> {
    let mut dependencies = vec![];
    for capture in STYLE_IMPORT.captures_iter(source) {
        add_dependency(
            &mut dependencies,
            dependency("style", capture[1].to_string(), vec![wildcard_binding()]),
        );
    }
    for capture in STYLE_COMPOSES.captures_iter(source) {
        add_dependency(
            &mut dependencies,
            dependency("style", capture[1].to_string(), vec![wildcard_binding()]),
        );
    }
    for capture in STYLE_URL.captures_iter(source) {
        let value = capture[1].to_string();
        if !is_url_like(&value) {
            add_dependency(
                &mut dependencies,
                dependency("asset", value, vec![wildcard_binding()]),
            );
        }
    }
    dependencies
}

fn extract_tsconfig_dependencies(source: &str) -> Vec<Dependency> {
    let Ok(config) = json5::from_str::<Value>(source) else {
        return vec![];
    };
    let mut dependencies = vec![];
    match config.get("extends") {
        Some(Value::String(value)) => dependencies.push(dependency("config", value, vec![])),
        Some(Value::Array(values)) => {
            for value in values.iter().filter_map(Value::as_str) {
                dependencies.push(dependency("config", value, vec![]));
            }
        }
        _ => {}
    }
    if let Some(references) = config.get("references").and_then(Value::as_array) {
        for path in references
            .iter()
            .filter_map(|reference| reference.get("path"))
            .filter_map(Value::as_str)
        {
            dependencies.push(dependency(
                "config",
                format!("{}/tsconfig.json", path.trim_end_matches('/')),
                vec![],
            ));
        }
    }
    dependencies
}

#[derive(Default)]
pub struct SourceAnalysis {
    pub dependencies: Vec<Dependency>,
    pub export_fingerprints: BTreeMap<String, String>,
}

pub fn analyze_source(file: &str, source: &str) -> SourceAnalysis {
    let path = Path::new(file);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if matches!(extension, "css" | "scss" | "less") {
        return SourceAnalysis {
            dependencies: extract_style_dependencies(source),
            ..SourceAnalysis::default()
        };
    }
    if extension == "json"
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("tsconfig"))
    {
        return SourceAnalysis {
            dependencies: extract_tsconfig_dependencies(source),
            ..SourceAnalysis::default()
        };
    }
    if matches!(
        extension,
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs"
    ) {
        let allocator = Allocator::default();
        let program = parse_program(&allocator, file, source);
        return SourceAnalysis {
            dependencies: extract_code_dependencies(&program),
            export_fingerprints: extract_export_fingerprints_from_program(&program, source),
        };
    }
    SourceAnalysis::default()
}

pub fn extract_dependencies(file: &str, source: &str) -> Vec<Dependency> {
    analyze_source(file, source).dependencies
}

fn is_url_like(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with('#')
        || value.starts_with("//")
        || value.starts_with("data:")
        || value.starts_with("var(")
        || value.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        })
}

fn declaration_names(declaration: &Declaration<'_>) -> Vec<String> {
    match declaration {
        Declaration::VariableDeclaration(declaration) => declaration
            .declarations
            .iter()
            .flat_map(|item| item.id.get_binding_identifiers())
            .map(|identifier| identifier.name.to_string())
            .collect(),
        Declaration::FunctionDeclaration(declaration) => declaration
            .id
            .as_ref()
            .map(|identifier| vec![identifier.name.to_string()])
            .unwrap_or_default(),
        Declaration::ClassDeclaration(declaration) => declaration
            .id
            .as_ref()
            .map(|identifier| vec![identifier.name.to_string()])
            .unwrap_or_default(),
        Declaration::TSTypeAliasDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Declaration::TSInterfaceDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Declaration::TSEnumDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Declaration::TSModuleDeclaration(_) | Declaration::TSImportEqualsDeclaration(_) => vec![],
    }
}

fn statement_declaration_names(statement: &Statement<'_>) -> Vec<String> {
    match statement {
        Statement::VariableDeclaration(declaration) => declaration
            .declarations
            .iter()
            .flat_map(|item| item.id.get_binding_identifiers())
            .map(|identifier| identifier.name.to_string())
            .collect(),
        Statement::FunctionDeclaration(declaration) => declaration
            .id
            .as_ref()
            .map(|identifier| vec![identifier.name.to_string()])
            .unwrap_or_default(),
        Statement::ClassDeclaration(declaration) => declaration
            .id
            .as_ref()
            .map(|identifier| vec![identifier.name.to_string()])
            .unwrap_or_default(),
        Statement::TSTypeAliasDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Statement::TSInterfaceDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Statement::TSEnumDeclaration(declaration) => vec![declaration.id.name.to_string()],
        Statement::ExportNamedDeclaration(node) => node
            .declaration
            .as_ref()
            .map(declaration_names)
            .unwrap_or_default(),
        _ => vec![],
    }
}

fn text_at_span(source: &str, span: oxc_span::Span) -> &str {
    source
        .get(span.start as usize..span.end as usize)
        .unwrap_or_default()
}

fn hash_text(value: &str) -> String {
    let mut hash = 2_166_136_261_u32;
    for byte in value.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("{}:{hash:08x}", value.len())
}

fn append_fingerprint(map: &mut HashMap<String, String>, name: String, value: &str) {
    let combined = map
        .get(&name)
        .map(|previous| format!("{previous}:{value}"))
        .unwrap_or_else(|| value.to_string());
    map.insert(name, hash_text(&combined));
}

fn extract_export_fingerprints_from_program(
    program: &Program<'_>,
    source: &str,
) -> BTreeMap<String, String> {
    let mut locals = HashMap::new();
    for statement in &program.body {
        let text = text_at_span(source, statement.span());
        for name in statement_declaration_names(statement) {
            append_fingerprint(&mut locals, name, text);
        }
    }
    let mut exports = HashMap::new();
    for statement in &program.body {
        let text = text_at_span(source, statement.span());
        match statement {
            Statement::ExportDefaultDeclaration(_) => {
                append_fingerprint(&mut exports, "default".into(), text);
            }
            Statement::ExportAllDeclaration(node) => {
                append_fingerprint(
                    &mut exports,
                    node.exported
                        .as_ref()
                        .map(|value| value.name().to_string())
                        .unwrap_or_else(|| "*".into()),
                    text,
                );
            }
            Statement::ExportNamedDeclaration(node) => {
                if let Some(declaration) = &node.declaration {
                    let names = declaration_names(declaration);
                    for name in if names.is_empty() {
                        vec!["*".into()]
                    } else {
                        names
                    } {
                        append_fingerprint(&mut exports, name, text);
                    }
                } else {
                    for specifier in &node.specifiers {
                        let exported = specifier.exported.name().to_string();
                        let local = specifier.local.name().to_string();
                        let local_fingerprint = if node.source.is_none() {
                            locals.get(&local).cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        append_fingerprint(
                            &mut exports,
                            exported,
                            &format!("{text}:{local_fingerprint}"),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    exports.into_iter().collect()
}

pub fn extract_export_fingerprints(file: &str, source: &str) -> BTreeMap<String, String> {
    analyze_source(file, source).export_fingerprints
}

pub fn changed_export_specifiers(
    file: &str,
    previous_source: Option<&str>,
    current_source: Option<&str>,
) -> Vec<String> {
    if previous_source == current_source {
        return vec![];
    }
    let (Some(previous_source), Some(current_source)) = (previous_source, current_source) else {
        return vec!["*".into()];
    };
    let previous = extract_export_fingerprints(file, previous_source);
    let current = extract_export_fingerprints(file, current_source);
    let names: HashSet<String> = previous.keys().chain(current.keys()).cloned().collect();
    let mut changed: Vec<String> = names
        .into_iter()
        .filter(|name| previous.get(name) != current.get(name))
        .collect();
    changed.sort();
    if changed.is_empty() {
        vec!["*".into()]
    } else {
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxc_extracts_static_dynamic_asset_and_glob_dependencies() {
        let dependencies = extract_dependencies(
            "src/index.ts",
            r#"
                import type { Model } from './model';
                import primary, { value as local } from './runtime';
                export { value as publicValue } from './barrel';
                const lazy = import('./lazy');
                const worker = new URL('./worker.ts', import.meta.url);
                const modules = import.meta.glob('./features/*.ts');
            "#,
        );
        let summary: Vec<_> = dependencies
            .iter()
            .map(|item| (item.kind.as_str(), item.specifier.as_str()))
            .collect();
        assert!(summary.contains(&("type", "./model")));
        assert!(summary.contains(&("runtime", "./runtime")));
        assert!(summary.contains(&("reexport", "./barrel")));
        assert!(summary.contains(&("dynamic", "./lazy")));
        assert!(summary.contains(&("asset", "./worker.ts")));
        assert!(summary.contains(&("glob", "./features/*.ts")));
    }

    #[test]
    fn fingerprints_named_exports_independently() {
        let changed = changed_export_specifiers(
            "model.ts",
            Some("export const feature = true; export const other = true;"),
            Some("export const feature = false; export const other = true;"),
        );
        assert_eq!(changed, ["feature"]);
    }
}
