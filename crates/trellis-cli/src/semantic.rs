//! The CLI's reevaluation source: syntactic kinds from trellis-program
//! (proven), semantic kinds from a clearly-labeled approximate textual
//! resolver (backend = "cli-textual-approx"). Spec §8 honesty: the
//! approximate resolver never claims provenance it does not have; the
//! label travels with every query answer and artifact.

use std::collections::{BTreeMap, BTreeSet};

use trellis_core::projection::{Projection, ProjectionKind};
use trellis_engine::prelude::*;
use trellis_engine::redgreen::{Evaluated, ReevaluationSource, TransitionError};
use trellis_program::prelude::PythonSyntaxIndex;

use crate::state::module_name;

pub const APPROX_BACKEND: &str = "cli-textual-approx";
pub const SYNTAX_BACKEND: &str = "trellis-program-syntax";

pub struct CliSemanticSource<'a> {
    syntactic: SyntacticSource<'a>,
    tree: &'a BTreeMap<String, String>,
}

impl<'a> CliSemanticSource<'a> {
    pub fn new(index: &'a PythonSyntaxIndex, tree: &'a BTreeMap<String, String>) -> Self {
        Self {
            syntactic: SyntacticSource::new(index),
            tree,
        }
    }

    /// Approximate callers over the current tree: import-binding-aware
    /// textual call scan (same algorithm family as the v0.1 engine test
    /// harness; labeled approximate, never authoritative-proven).
    pub fn approx_callers(&self, subject: &str) -> String {
        let def_module = subject.rsplit_once('.').map(|(m, _)| m).unwrap_or("");
        let def_fn = subject.rsplit_once('.').map(|(_, f)| f).unwrap_or(subject);
        let mut members: BTreeSet<String> = BTreeSet::new();
        for path in self.tree.keys() {
            let module = module_name(path);
            let bindings = bindings_for(self.tree, path, &module);
            for (scope, calls) in call_sites(self.tree, path) {
                let Some(scope) = scope else { continue };
                if module == def_module && scope == def_fn {
                    continue;
                }
                for call in calls {
                    let resolved: Option<String> = match &call {
                        CallExpr::Name(name) => bindings
                            .get(name)
                            .filter(|t| *t == subject)
                            .map(|_| format!("{module}.{scope}")),
                        CallExpr::Attribute(alias, tail) => bindings
                            .get(alias)
                            .map(|t| format!("{t}.{tail}"))
                            .filter(|t| t == subject)
                            .map(|_| format!("{module}.{scope}")),
                    };
                    if let Some(member) = resolved {
                        members.insert(member);
                    }
                }
            }
        }
        members.into_iter().collect::<Vec<_>>().join("; ")
    }
}

pub enum CallExpr {
    Name(String),
    Attribute(String, String),
}

/// Import bindings of one file: local name → dotted target.
pub fn bindings_for(
    tree: &BTreeMap<String, String>,
    path: &str,
    module: &str,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(content) = tree.get(path) else {
        return out;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("from ") {
            let Some((base, names)) = rest.split_once(" import ") else {
                continue;
            };
            let absolute = resolve_from_import(module, base.trim());
            for item in names.split(',') {
                let item = item.trim();
                let (name, binding) = match item.split_once(" as ") {
                    Some((n, a)) => (n.trim(), a.trim()),
                    None => (item, item),
                };
                out.insert(binding.to_string(), format!("{absolute}.{name}"));
            }
        } else if let Some(rest) = line.strip_prefix("import ") {
            for item in rest.split(',') {
                let item = item.trim();
                let (target, binding) = match item.split_once(" as ") {
                    Some((t, a)) => (t.trim(), a.trim()),
                    None => (item, item),
                };
                out.insert(binding.to_string(), target.to_string());
            }
        }
    }
    out
}

/// (enclosing dotted scope, call candidates) per non-import line.
fn call_sites(tree: &BTreeMap<String, String>, path: &str) -> Vec<(Option<String>, Vec<CallExpr>)> {
    let mut sites = Vec::new();
    let Some(content) = tree.get(path) else {
        return sites;
    };
    let mut scopes: Vec<(usize, String)> = Vec::new();
    for line in content.lines() {
        let indent = line.len() - line.trim_start().len();
        while scopes.last().is_some_and(|(i, _)| *i >= indent) {
            scopes.pop();
        }
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("def ") {
            let name: String = rest
                .split('(')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            scopes.push((indent, name));
            continue;
        }
        if trimmed.starts_with("class ") {
            let name: String = trimmed
                .strip_prefix("class ")
                .and_then(|r| r.split('(').next())
                .unwrap_or_default()
                .trim()
                .to_string();
            scopes.push((indent, name));
            continue;
        }
        if trimmed.starts_with("from ") || trimmed.starts_with("import ") {
            continue;
        }
        let scope = (!scopes.is_empty()).then(|| {
            scopes
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(".")
        });
        sites.push((scope, scan_calls(line)));
    }
    sites
}

pub fn scan_calls(line: &str) -> Vec<CallExpr> {
    let mut calls = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &line[start..i];
            let mut j = i;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' {
                calls.push(CallExpr::Name(ident.to_string()));
            } else if j < bytes.len() && bytes[j] == b'.' {
                let mut k = j + 1;
                let attr_start = k;
                while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
                    k += 1;
                }
                let attr = &line[attr_start..k];
                let mut m = k;
                while m < bytes.len() && bytes[m] == b' ' {
                    m += 1;
                }
                if !attr.is_empty() && m < bytes.len() && bytes[m] == b'(' {
                    calls.push(CallExpr::Attribute(ident.to_string(), attr.to_string()));
                }
            }
        } else {
            i += 1;
        }
    }
    calls
}

/// `from X import n` base resolution including relative imports.
pub fn resolve_from_import(current_module: &str, base: &str) -> String {
    let dots = base.chars().take_while(|c| *c == '.').count();
    if dots == 0 {
        return base.to_string();
    }
    let rest = &base[dots..];
    let components: Vec<&str> = current_module.split('.').collect();
    let package_len = components.len().saturating_sub(1);
    let up = dots - 1;
    let keep = package_len.saturating_sub(up);
    let mut resolved: Vec<&str> = components[..keep].to_vec();
    if !rest.is_empty() {
        resolved.extend(rest.split('.'));
    }
    resolved.join(".")
}

impl ReevaluationSource for CliSemanticSource<'_> {
    fn evaluate(&self, projection: &Projection) -> Result<Option<Evaluated>, TransitionError> {
        match projection.kind() {
            ProjectionKind::Callers => Ok(Some(Evaluated::new(
                self.approx_callers(projection.subject().canonical()),
                false, // approximate: provisional authority, never Authoritative
            ))),
            _ => self.syntactic.evaluate(projection),
        }
    }
}
