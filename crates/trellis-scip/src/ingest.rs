//! SCIP Index → normalized graph ingest (spec §9.1, §9.3).
//!
//! SCIP knowledge is confined to this crate; this module owns the
//! mapping from the protobuf `Index` into Trellis's graph. Nothing is
//! silently dropped: every document is ingested, skipped (non-Python),
//! or accounted for in the [`IngestReport`].
//!
//! Normalization contract (v0.1, scip-python shape):
//! - File-scoped descriptors use the backtick-escaped path form
//!   (verified against the scip crate grammar: `/` is only legal inside
//!   backtick escapes). The dotted subject of a symbol is
//!   `path→module` (`auth/tokens.py` → `auth.tokens`) followed by the
//!   non-namespace descriptors (`auth.service.AuthService.verify`).
//! - A reference occurrence of symbol S contributes the caller member
//!   `<enclosing def scope>` when its range is enclosed by a definition
//!   of the same document, else the module member. References never
//!   include the definition site of S itself.
//! - `Relationship.is_implementation` / `is_type_definition` edges are
//!   precise (SCIP proof): implementations/subclasses Established.

use std::collections::BTreeMap;

use scip::types::{descriptor, Index};

use crate::ScipGraph;

/// Ingest accounting: what was consumed, skipped, and how. Nothing is
/// silently dropped (spec §8).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IngestReport {
    /// Python documents ingested into the graph.
    pub documents_ingested: usize,
    /// Documents skipped because they are not Python (out of scope for
    /// v0.1, §9.4) — counted explicitly.
    pub documents_non_python: usize,
    /// Reference occurrences consumed into caller/reference sets.
    pub references_consumed: usize,
    /// Implementation/subclass relationships consumed.
    pub relationships_consumed: usize,
    /// The indexer identity derived from SCIP metadata + adapter
    /// version (§8.1 coverage evidence).
    pub identity: String,
}

/// SCIP symbol role bit for "definition" (scip.proto `SymbolRole`).
const ROLE_DEFINITION: i32 = 0b0001;

/// Decode a SCIP symbol string into its Trellis dotted form.
///
/// scip-python symbol shape:
/// `scip-python pip <pkg> <ver> \`auth/service.py\`/AuthService.verify().`
/// → `auth.service.AuthService.verify`. File descriptors (backtick
/// escaped, Namespace suffix, name ends with `.py`) map to the dotted
/// module; Term/Method descriptors append to the scope chain.
fn symbol_to_dotted(raw: &str) -> Option<String> {
    let parsed = scip::symbol::parse_symbol(raw).ok()?;
    let mut module_parts: Vec<String> = Vec::new();
    let mut scope_parts: Vec<String> = Vec::new();
    for descriptor in &parsed.descriptors {
        let Ok(suffix) = descriptor.suffix.enum_value() else {
            return None;
        };
        match suffix {
            descriptor::Suffix::Namespace if descriptor.name.ends_with(".py") => {
                let stem = descriptor.name.strip_suffix(".py")?;
                let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
                module_parts = stem.split('/').map(str::to_string).collect();
                scope_parts.clear();
            }
            descriptor::Suffix::Namespace => {
                module_parts.push(descriptor.name.clone());
            }
            descriptor::Suffix::Term | descriptor::Suffix::Method => {
                scope_parts.push(descriptor.name.clone());
            }
            _ => return None,
        }
    }
    if module_parts.is_empty() {
        return None;
    }
    module_parts.extend(scope_parts);
    Some(module_parts.join("."))
}

/// Ingest a parsed SCIP [`Index`] into the normalized graph.
#[must_use]
pub fn ingest(index: &Index) -> (ScipGraph, IngestReport) {
    let mut graph = ScipGraph::default();
    let mut report = IngestReport::default();

    if let Some(meta) = index.metadata.as_ref() {
        let tool = meta
            .tool_info
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let version = meta
            .tool_info
            .as_ref()
            .map(|t| t.version.clone())
            .unwrap_or_default();
        report.identity = crate::ScipIdentity::new(tool, version).render();
    } else {
        report.identity = crate::ScipIdentity::new("", "").render();
    }

    // Pass 1: decode every document's symbol inventory plus the index's
    // external symbols (scip-python emits SymbolInformation only for
    // symbols DEFINED in a document; references to symbols defined
    // elsewhere still resolve through external_symbols or through the
    // defining document's inventory).
    let mut dotted_by_symbol: BTreeMap<String, String> = BTreeMap::new();
    for symbol_info in &index.external_symbols {
        if let Some(dotted) = symbol_to_dotted(&symbol_info.symbol) {
            dotted_by_symbol.insert(symbol_info.symbol.clone(), dotted);
        }
    }
    for document in &index.documents {
        if document.language != "python" {
            report.documents_non_python += 1;
            continue;
        }
        report.documents_ingested += 1;
        graph.documents.push(document.relative_path.clone());
        for symbol_info in &document.symbols {
            if let Some(dotted) = symbol_to_dotted(&symbol_info.symbol) {
                dotted_by_symbol.insert(symbol_info.symbol.clone(), dotted);
            }
        }
    }

    // Pass 2: relationships + reference occurrences.
    for document in &index.documents {
        if document.language != "python" {
            continue;
        }
        let stem = document
            .relative_path
            .strip_suffix(".py")
            .unwrap_or(&document.relative_path);
        let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
        let module = stem.replace('/', ".");

        // Definition ranges of this document for enclosing-member
        // resolution: dotted def → its definition range.
        let mut def_ranges: Vec<(String, [i32; 4])> = Vec::new();
        for symbol_info in &document.symbols {
            let Some(dotted) = dotted_by_symbol.get(&symbol_info.symbol) else {
                continue;
            };
            for occurrence in &document.occurrences {
                if occurrence.symbol == symbol_info.symbol
                    && occurrence.symbol_roles & ROLE_DEFINITION != 0
                {
                    def_ranges.push((dotted.clone(), range4(&occurrence.range)));
                }
            }
        }

        for symbol_info in &document.symbols {
            let Some(subject) = dotted_by_symbol.get(&symbol_info.symbol) else {
                continue;
            };
            for relationship in &symbol_info.relationships {
                let Some(target) = symbol_to_dotted(&relationship.symbol) else {
                    continue;
                };
                if relationship.is_implementation {
                    graph
                        .implementations
                        .entry(target.clone())
                        .or_default()
                        .push(subject.clone());
                    report.relationships_consumed += 1;
                }
                if relationship.is_type_definition {
                    graph
                        .subclasses
                        .entry(target)
                        .or_default()
                        .push(subject.clone());
                    report.relationships_consumed += 1;
                }
            }
        }

        // Reference occurrences: every document-level occurrence whose
        // symbol resolves to a known dotted subject becomes a caller /
        // reference of that subject (definitions are skipped, and the
        // subject's own definition site is never a member). This shape
        // matches real indexers: SymbolInformation is emitted for
        // defined symbols; references to foreign symbols appear only as
        // document-level occurrences.
        for occurrence in &document.occurrences {
            if occurrence.symbol_roles & ROLE_DEFINITION != 0 {
                continue;
            }
            let Some(subject) = dotted_by_symbol.get(&occurrence.symbol) else {
                continue;
            };
            if *subject == module {
                continue;
            }
            report.references_consumed += 1;
            let member = enclosing_member(&occurrence.range, &def_ranges, &module);
            // The referencing unit: the document's own module (not a
            // re-derivation from the member — module_of would strip a
            // dotted module name's last component, e.g. module-level
            // code in auth.tokens would land on `auth`).
            graph
                .references
                .entry(subject.clone())
                .or_default()
                .push(module.clone());
            graph
                .callers
                .entry(subject.clone())
                .or_default()
                .push(member);
        }
    }

    // Canonicalize every member set and the document list — identity
    // depends on canonical order (§19).
    for members in graph.callers.values_mut() {
        members.sort();
        members.dedup();
    }
    for members in graph.references.values_mut() {
        members.sort();
        members.dedup();
    }
    for members in graph.implementations.values_mut() {
        members.sort();
        members.dedup();
    }
    for members in graph.subclasses.values_mut() {
        members.sort();
        members.dedup();
    }
    graph.documents.sort();
    graph.documents.dedup();

    (graph, report)
}

/// Normalize a SCIP occurrence range to a comparable `[start_line,
/// start_char, end_line, end_char]` (three-element single-line ranges
/// infer the end line).
fn range4(range: &[i32]) -> [i32; 4] {
    match range {
        [sl, sc, ec] => [*sl, *sc, *sl, *ec],
        [sl, sc, el, ec] => [*sl, *sc, *el, *ec],
        _ => [0, 0, 0, 0],
    }
}

/// The dotted member an occurrence belongs to: the innermost definition
/// whose range encloses the occurrence (`module.def`), falling back to
/// the module member when no definition spans it (module-level code).
fn enclosing_member(
    occurrence_range: &[i32],
    def_ranges: &[(String, [i32; 4])],
    module: &str,
) -> String {
    let [osl, osc, oel, oec] = range4(occurrence_range);
    let mut best: Option<(i32, &String)> = None;
    for (dotted, [dsl, dsc, del, dec]) in def_ranges {
        let encloses = (*dsl < osl || (*dsl == osl && *dsc <= osc))
            && (*del > oel || (*del == oel && *dec >= oec));
        if encloses {
            let span = del - dsl;
            let better = match best {
                None => true,
                Some((best_span, _)) => span < best_span,
            };
            if better {
                best = Some((span, dotted));
            }
        }
    }
    match best {
        Some((_, dotted)) => dotted.clone(),
        None => module.to_string(),
    }
}
