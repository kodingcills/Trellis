//! Tree-sitter backend: deterministic syntax-grounded Python extraction.
//!
//! This backend proves **syntactic facts only** (definitions, signatures,
//! imports, parse status — per unit). Every semantic cross-file query
//! returns [`Answer::Unsupported`]: tree-sitter is a syntax parser, not a
//! semantic resolver (spec §9, M3 node contract). Uncertainty is never
//! rendered as an empty set.

use std::collections::BTreeMap;

use tree_sitter::{Node, Parser, Tree};

use crate::index::{Answer, ProgramIndex, Unsupported};
use crate::model::{
    DefKind, Definition, Import, ModuleId, Param, ParamKind, ParseStatus, Signature, SourceSpan,
    SymbolPath,
};

/// Deterministic syntax-grounded index over a set of Python source units.
#[derive(Debug, Clone)]
pub struct PythonSyntaxIndex {
    units: BTreeMap<ModuleId, IndexedUnit>,
}

#[derive(Debug, Clone)]
struct IndexedUnit {
    tree: Tree,
    source: String,
    status: ParseStatus,
}

impl PythonSyntaxIndex {
    /// Index a tree of source files (canonical relative paths → contents),
    /// as produced by the M1 source layer (`trellis_source::read_tree` /
    /// reconcile output). Only `.py` files are indexed; results depend only
    /// on file contents, never on iteration order or absolute paths.
    #[must_use]
    pub fn index(tree: &BTreeMap<String, String>) -> Self {
        let mut parser = Self::new_parser();
        let mut units = BTreeMap::new();
        for (path, source) in tree {
            let Some(module) = ModuleId::from_path(path) else {
                continue;
            };
            let parsed = parser.parse(source, None);
            let (tree, status) = match parsed {
                Some(t) => {
                    let mut count = 0u32;
                    let mut first: Option<SourceSpan> = None;
                    scan_errors(t.root_node(), &mut count, &mut first);
                    let status = if count == 0 {
                        ParseStatus::Parsed
                    } else {
                        ParseStatus::Failed {
                            error_count: count,
                            first_error: first,
                        }
                    };
                    (t, status)
                }
                None => (
                    // Parser unavailable for this input: record explicit
                    // failure rather than silently dropping the unit.
                    Self::new_parser().parse("", None).expect("empty parse"),
                    ParseStatus::Failed {
                        error_count: 0,
                        first_error: None,
                    },
                ),
            };
            units.insert(
                module,
                IndexedUnit {
                    tree,
                    source: source.clone(),
                    status,
                },
            );
        }
        Self { units }
    }

    /// An index with no units.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            units: BTreeMap::new(),
        }
    }

    /// Number of indexed units (including explicit parse failures).
    #[must_use]
    pub fn unit_count(&self) -> usize {
        self.units.len()
    }

    fn new_parser() -> Parser {
        let mut parser = Parser::new();
        let language = tree_sitter_python::LANGUAGE.into();
        parser
            .set_language(&language)
            .expect("python grammar loads");
        parser
    }

    fn unit(&self, module: &ModuleId) -> Answer<&IndexedUnit> {
        match self.units.get(module) {
            Some(u) => Answer::Proven(u),
            None => Answer::Unsupported(Unsupported::UnitNotIndexed(module.name().to_string())),
        }
    }

    /// A unit whose contents may be fully extracted. Parse-failed units
    /// yield Unsupported: extraction from incomplete evidence is never
    /// proven (spec §8; uncertainty is never an empty result).
    fn proven_unit(&self, module: &ModuleId) -> Answer<&IndexedUnit> {
        match self.unit(module) {
            Answer::Proven(u) if u.status.is_ok() => Answer::Proven(u),
            Answer::Proven(_) => {
                Answer::Unsupported(Unsupported::ParseIncomplete(module.name().to_string()))
            }
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }

    /// Resolve a symbol path to its unit: the **longest** dotted prefix
    /// that corresponds to an indexed module (e.g.
    /// `auth.service.AuthService.login` → unit `auth.service`, nesting
    /// path `AuthService.login`). Returns `None` when no prefix is an
    /// indexed module.
    fn resolve_symbol(&self, symbol: &SymbolPath) -> Option<(ModuleId, String)> {
        let segments: Vec<&str> = symbol.as_str().split('.').collect();
        if segments.len() < 2 {
            return None;
        }
        for split in (1..segments.len()).rev() {
            let module_part = segments[..split].join(".");
            let module = match ModuleId::from_path(&format!("{}.py", module_part.replace('.', "/")))
            {
                Some(m) => m,
                None => continue,
            };
            if self.units.contains_key(&module) {
                let nesting = segments[split..].join(".");
                return Some((module, nesting));
            }
        }
        None
    }

    /// Extract all definitions (with signatures where applicable) of a
    /// unit, in source order. Unsupported for failed-parse units —
    /// extraction from incomplete evidence is never proven (spec §8); the
    /// failure itself stays queryable via `parse_status`.
    #[must_use]
    pub fn extract_definitions(
        &self,
        module: &ModuleId,
    ) -> Answer<Vec<(Definition, Option<Signature>)>> {
        match self.proven_unit(module) {
            Answer::Proven(unit) => {
                let mut out = Vec::new();
                walk_defs(
                    unit.tree.root_node(),
                    &unit.source,
                    module,
                    None,
                    false,
                    &mut out,
                );
                out.sort_by(|a, b| {
                    a.0.span
                        .cmp(&b.0.span)
                        .then_with(|| a.0.symbol.cmp(&b.0.symbol))
                });
                Answer::Proven(out)
            }
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }

    /// Map a reconcile changed-file set to syntactically affected symbols.
    /// Proven only when every changed Python unit parsed cleanly; a
    /// failed-parse unit in the set makes the candidate set provably
    /// incomplete and downgrades the answer (spec §2.1, §8).
    #[must_use]
    pub fn changed_symbols_impl(&self, changed_paths: &[String]) -> Answer<Vec<SymbolPath>> {
        let mut symbols = Vec::new();
        for path in changed_paths {
            let Some(module) = ModuleId::from_path(path) else {
                continue; // non-Python paths have no program symbols
            };
            match self.proven_unit(&module) {
                Answer::Proven(unit) => {
                    symbols.extend(
                        extract_defs(unit, &module)
                            .into_iter()
                            .map(|(d, _)| d.symbol),
                    );
                }
                Answer::Unsupported(u) => return Answer::Unsupported(u),
            }
        }
        symbols.sort();
        symbols.dedup();
        Answer::Proven(symbols)
    }

    /// Content digest of a tracked Python file (the `file(path)`
    /// projection). Content-derived and deterministic; digests do not
    /// depend on parsing. `None` only for a canonical `.py` path absent
    /// from the indexed tree — proven absence, since the index was built
    /// from the full reconciled tree. Non-Python or non-canonical paths
    /// are outside the indexed universe and return Unsupported (they are
    /// not proven absences, spec §8).
    #[must_use]
    pub fn file_digest_impl(&self, path: &str) -> Answer<Option<trellis_core::ids::ContentHash>> {
        let Some(module) = ModuleId::from_path(path) else {
            return Answer::Unsupported(Unsupported::UnitNotIndexed(path.to_string()));
        };
        match self.units.get(&module) {
            Some(unit) => Answer::Proven(Some(trellis_core::ids::ContentHash::compute(
                trellis_core::ids::HashAlgo::Blake3,
                unit.source.as_bytes(),
            ))),
            None => Answer::Proven(None),
        }
    }

    /// Extract the syntactic imports of a unit, in source order.
    #[must_use]
    pub fn extract_imports(&self, module: &ModuleId) -> Answer<Vec<Import>> {
        match self.proven_unit(module) {
            Answer::Proven(unit) => {
                let mut out = Vec::new();
                collect_imports(unit.tree.root_node(), &unit.source, &mut out);
                Answer::Proven(out)
            }
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }
}

fn span_of(node: Node<'_>) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    SourceSpan {
        start_line: start.row as u32,
        start_column: start.column as u32,
        end_line: end.row as u32,
        end_column: end.column as u32,
    }
}

fn scan_errors(node: Node<'_>, count: &mut u32, first: &mut Option<SourceSpan>) {
    if node.is_error() || node.is_missing() {
        *count += 1;
        if first.is_none() {
            *first = Some(span_of(node));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scan_errors(child, count, first);
    }
}

fn normalize_ws(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract definitions/signatures from one unit tree.
fn extract_defs(unit: &IndexedUnit, module: &ModuleId) -> Vec<(Definition, Option<Signature>)> {
    let mut out = Vec::new();
    walk_defs(
        unit.tree.root_node(),
        &unit.source,
        module,
        None,
        false,
        &mut out,
    );
    out.sort_by(|a, b| {
        a.0.span
            .cmp(&b.0.span)
            .then_with(|| a.0.symbol.cmp(&b.0.symbol))
    });
    out
}

fn walk_defs(
    node: Node<'_>,
    source: &str,
    module: &ModuleId,
    scope: Option<&SymbolPath>,
    in_class: bool,
    out: &mut Vec<(Definition, Option<Signature>)>,
) {
    // Dispatch on the node itself; compound containers recurse into
    // children. Decorators do not alter normalized identity.
    match node.kind() {
        "function_definition" => {
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            let name = &source[name_node.byte_range()];
            let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");
            let kind = match (in_class, is_async) {
                (true, false) => DefKind::Method,
                (true, true) => DefKind::AsyncMethod,
                (false, false) => DefKind::Function,
                (false, true) => DefKind::AsyncFunction,
            };
            let symbol = match scope {
                Some(parent) => parent.child(name),
                None => SymbolPath::new(&format!("{}.{}", module.name(), name)),
            };
            if let Some(sym) = symbol {
                let sig = signature_of_function(node, source);
                out.push((
                    Definition {
                        symbol: sym.clone(),
                        kind,
                        span: span_of(node),
                    },
                    sig,
                ));
                // Nested functions live outside any class scope.
                if let Some(body) = node.child_by_field_name("body") {
                    walk_defs(body, source, module, Some(&sym), false, out);
                }
            }
        }
        "class_definition" => {
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            let name = &source[name_node.byte_range()];
            let symbol = match scope {
                Some(parent) => parent.child(name),
                None => SymbolPath::new(&format!("{}.{}", module.name(), name)),
            };
            if let Some(sym) = symbol {
                out.push((
                    Definition {
                        symbol: sym.clone(),
                        kind: DefKind::Class,
                        span: span_of(node),
                    },
                    None,
                ));
                if let Some(body) = node.child_by_field_name("body") {
                    walk_defs(body, source, module, Some(&sym), true, out);
                }
            }
        }
        other => {
            // Compound containers (module, block, decorated_definition,
            // if/try bodies, …): recurse into children. Leaf nodes and
            // statements without nested definitions terminate here.
            if matches!(
                other,
                "module"
                    | "block"
                    | "decorated_definition"
                    | "if_statement"
                    | "elif_clause"
                    | "else_clause"
                    | "for_statement"
                    | "while_statement"
                    | "try_statement"
                    | "except_clause"
                    | "finally_clause"
                    | "with_statement"
                    | "match_statement"
                    | "case_clause"
            ) {
                let mut cursor = node.walk();
                let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
                for child in children {
                    walk_defs(child, source, module, scope, in_class, out);
                }
            }
        }
    }
}

/// Normalized signature of a function_definition node.
fn signature_of_function(node: Node<'_>, source: &str) -> Option<Signature> {
    let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");
    let params_node = node.child_by_field_name("parameters")?;
    let mut params: Vec<Param> = Vec::new();
    let mut seen_pos_only_marker = false;
    let mut seen_kw_marker = false;
    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        match child.kind() {
            "positional_separator" => seen_pos_only_marker = true,
            "keyword_separator" => seen_kw_marker = true,
            "comment" => {}
            kind_name => {
                let Some(kind) = param_kind_for(
                    kind_name,
                    child,
                    source,
                    seen_pos_only_marker,
                    seen_kw_marker,
                ) else {
                    continue;
                };
                let (name, annotation, default) = decompose_param(child, source, kind);
                if !name.is_empty() {
                    params.push(Param {
                        name,
                        kind,
                        annotation,
                        default,
                    });
                }
            }
        }
    }
    let ret = node
        .child_by_field_name("return_type")
        .map(|n| normalize_ws(&source[n.byte_range()]));
    Some(Signature {
        is_async,
        params,
        ret,
    })
}

fn param_kind_for(
    kind: &str,
    node: Node<'_>,
    source: &str,
    pos_only_open: bool,
    kw_only_open: bool,
) -> Option<ParamKind> {
    Some(match kind {
        "identifier" | "default_parameter" | "typed_default_parameter" => {
            if kw_only_open {
                ParamKind::KeywordOnly
            } else if pos_only_open {
                ParamKind::PositionalOnly
            } else {
                ParamKind::Positional
            }
        }
        "typed_parameter" => {
            // ``**kw: T`` / ``*args: T`` parse as typed_parameter wrapping a
            // splat pattern; their splat child determines the kind.
            if node_text_starts_splat(node, source, "**") {
                return Some(ParamKind::KwArgs);
            }
            if node_text_starts_splat(node, source, "*") {
                return Some(ParamKind::VarArgs);
            }
            if contains_splat(node, "dictionary_splat_pattern") {
                ParamKind::KwArgs
            } else if contains_splat(node, "list_splat_pattern") {
                ParamKind::VarArgs
            } else if kw_only_open {
                ParamKind::KeywordOnly
            } else if pos_only_open {
                ParamKind::PositionalOnly
            } else {
                ParamKind::Positional
            }
        }
        "list_splat_pattern" => ParamKind::VarArgs,
        "dictionary_splat_pattern" => ParamKind::KwArgs,
        _ => return None,
    })
}

/// Whether a typed parameter's source text begins with a splat marker
/// (``*``/``**``), which the child-node kinds alone do not reveal for
/// annotated splats (``**kw: T`` parses as typed_parameter).
fn node_text_starts_splat(node: Node<'_>, source: &str, star: &str) -> bool {
    source[node.byte_range()].starts_with(star)
}

fn contains_splat(node: Node<'_>, want: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == want {
            return true;
        }
        if contains_splat(child, want) {
            return true;
        }
    }
    false
}

/// Split a parameter node into (name, annotation, default), with
/// whitespace-normalized annotation/default text.
/// Extract a parameter name: the identifier child, descending into splat
/// patterns (``**kwargs: T`` parses as typed_parameter wrapping a
/// dictionary_splat_pattern).
fn extract_param_name(node: Node<'_>, source: &str) -> String {
    if node.kind() == "identifier" {
        return source[node.byte_range()].to_string();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" => return source[child.byte_range()].to_string(),
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                return extract_param_name(child, source);
            }
            _ => {}
        }
    }
    String::new()
}

fn decompose_param(
    node: Node<'_>,
    source: &str,
    kind: ParamKind,
) -> (String, Option<String>, Option<String>) {
    // Structural decomposition over children; field names are unreliable
    // across grammar versions, so use node kinds + byte ranges.
    match kind {
        ParamKind::VarArgs | ParamKind::KwArgs => {
            // Splat nodes may be wrapped by a typed_parameter annotation
            // (``**kw: T``): reuse the marker-slicing extraction.
            let name = extract_param_name(node, source);
            let mut annotation: Option<String> = None;
            let mut default: Option<String> = None;
            let mut c = node.walk();
            let children: Vec<Node<'_>> = node.children(&mut c).collect();
            let mut i = 0usize;
            while i < children.len() {
                if children[i].kind() == ":" {
                    let mut j = i + 1;
                    while j < children.len() && children[j].kind() != "=" {
                        j += 1;
                    }
                    if j > i + 1 {
                        let start = children[i + 1].start_byte();
                        let end = children[j - 1].end_byte();
                        annotation = Some(normalize_ws(&source[start..end]));
                    }
                    i = j;
                    continue;
                }
                if children[i].kind() == "=" {
                    let start = children[i + 1].start_byte();
                    let end = children[children.len() - 1].end_byte();
                    default = Some(normalize_ws(&source[start..end]));
                    break;
                }
                i += 1;
            }
            (name, annotation, default)
        }
        _ => match node.kind() {
            "identifier" => (source[node.byte_range()].to_string(), None, None),
            "typed_parameter" | "typed_default_parameter" | "default_parameter" => {
                let name = extract_param_name(node, source);
                let mut annotation: Option<String> = None;
                let mut default: Option<String> = None;
                let mut c = node.walk();
                let children: Vec<Node> = node.children(&mut c).collect();
                let mut i = 0usize;
                while i < children.len() {
                    let kind_name = children[i].kind();
                    if kind_name == "=" {
                        // default: everything after this "="
                        let start = children[i + 1].start_byte();
                        let end = children[children.len() - 1].end_byte();
                        default = Some(normalize_ws(&source[start..end]));
                        break;
                    }
                    if kind_name == ":" && annotation.is_none() {
                        // annotation: children after ":" until next "="
                        let mut j = i + 1;
                        while j < children.len() && children[j].kind() != "=" {
                            j += 1;
                        }
                        if j > i + 1 {
                            let start = children[i + 1].start_byte();
                            let end = children[j - 1].end_byte();
                            annotation = Some(normalize_ws(&source[start..end]));
                        }
                        i = j;
                        continue;
                    }
                    i += 1;
                }
                (name, annotation, default)
            }
            _ => (String::new(), None, None),
        },
    }
}

/// Extract import statements (syntactic only), in source order.
fn collect_imports(node: Node<'_>, source: &str, out: &mut Vec<Import>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                let line = (child.start_position().row + 1) as u32;
                let mut c = child.walk();
                for clause in child.children(&mut c) {
                    match clause.kind() {
                        "dotted_name" => {
                            let module = source[clause.byte_range()].to_string();
                            let binding = module.split('.').next().unwrap_or(&module).to_string();
                            out.push(Import {
                                target: module,
                                binding,
                                from_import: false,
                                line,
                            });
                        }
                        "aliased_import" => {
                            let mut inner = clause.walk();
                            let mut target = String::new();
                            let mut alias: Option<String> = None;
                            for part in clause.children(&mut inner) {
                                if part.kind() == "dotted_name" && target.is_empty() {
                                    target = source[part.byte_range()].to_string();
                                } else if part.kind() == "identifier" {
                                    alias = Some(source[part.byte_range()].to_string());
                                }
                            }
                            let binding = alias.unwrap_or_else(|| {
                                target.split('.').next().unwrap_or(&target).to_string()
                            });
                            out.push(Import {
                                target,
                                binding,
                                from_import: false,
                                line,
                            });
                        }
                        _ => {}
                    }
                }
            }
            "import_from_statement" => {
                let module_node = child.child_by_field_name("module_name");
                let module = module_node
                    .map(|n| source[n.byte_range()].to_string())
                    .unwrap_or_default();
                let line = (child.start_position().row + 1) as u32;
                let mut c = child.walk();
                for clause in child.children(&mut c) {
                    // The module itself (and the `import` keyword) are not
                    // imported names.
                    if clause.id() == module_node.map(|n| n.id()).unwrap_or(0) {
                        continue;
                    }
                    match clause.kind() {
                        "identifier" | "dotted_name" => {
                            let name = source[clause.byte_range()].to_string();
                            out.push(Import {
                                target: format!("{module}.{name}"),
                                binding: name.rsplit('.').next().unwrap_or(&name).to_string(),
                                from_import: true,
                                line,
                            });
                        }
                        "aliased_import" => {
                            // `from m import name as alias` — the imported
                            // name is the dotted_name; the trailing
                            // identifier is the alias.
                            let mut inner = clause.walk();
                            let mut name: Option<String> = None;
                            let mut alias: Option<String> = None;
                            for part in clause.children(&mut inner) {
                                match part.kind() {
                                    "dotted_name" if name.is_none() => {
                                        name = Some(source[part.byte_range()].to_string());
                                    }
                                    "identifier" => {
                                        alias = Some(source[part.byte_range()].to_string());
                                    }
                                    _ => {}
                                }
                            }
                            let target = format!("{module}.{}", name.clone().unwrap_or_default());
                            let binding = alias.or(name).unwrap_or_else(|| "*".to_string());
                            out.push(Import {
                                target,
                                binding,
                                from_import: true,
                                line,
                            });
                        }
                        "wildcard_import" => {
                            out.push(Import {
                                target: format!("{module}.*"),
                                binding: "*".to_string(),
                                from_import: true,
                                line,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                let mut c = child.walk();
                for grand in child.children(&mut c) {
                    collect_imports(grand, source, out);
                }
            }
        }
    }
}

impl ProgramIndex for PythonSyntaxIndex {
    fn parse_status(&self, module: &ModuleId) -> Answer<ParseStatus> {
        match self.unit(module) {
            Answer::Proven(u) => Answer::Proven(u.status.clone()),
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }

    fn definition(&self, symbol: &SymbolPath) -> Answer<Option<Definition>> {
        match self.resolve_symbol(symbol) {
            None => Answer::Unsupported(Unsupported::UnitNotIndexed(symbol.as_str().to_string())),
            Some((module, _nesting)) => match self.proven_unit(&module) {
                Answer::Proven(unit) => {
                    let defs = extract_defs(unit, &module);
                    Answer::Proven(
                        defs.into_iter()
                            .find(|(d, _)| d.symbol == *symbol)
                            .map(|(d, _)| d),
                    )
                }
                Answer::Unsupported(u) => Answer::Unsupported(u),
            },
        }
    }

    fn signature(&self, symbol: &SymbolPath) -> Answer<Option<Signature>> {
        match self.resolve_symbol(symbol) {
            None => Answer::Unsupported(Unsupported::UnitNotIndexed(symbol.as_str().to_string())),
            Some((module, _nesting)) => match self.proven_unit(&module) {
                Answer::Proven(unit) => {
                    let defs = extract_defs(unit, &module);
                    Answer::Proven(
                        defs.into_iter()
                            .find(|(d, _)| d.symbol == *symbol)
                            .and_then(|(_, s)| s),
                    )
                }
                Answer::Unsupported(u) => Answer::Unsupported(u),
            },
        }
    }

    fn definitions_in(&self, module: &ModuleId) -> Answer<Vec<Definition>> {
        match self.proven_unit(module) {
            Answer::Proven(u) => Answer::Proven(
                extract_defs(u, module)
                    .into_iter()
                    .map(|(d, _)| d)
                    .collect(),
            ),
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }

    fn imports(&self, module: &ModuleId) -> Answer<Vec<Import>> {
        match self.proven_unit(module) {
            Answer::Proven(u) => {
                let mut out = Vec::new();
                collect_imports(u.tree.root_node(), &u.source, &mut out);
                Answer::Proven(out)
            }
            Answer::Unsupported(u) => Answer::Unsupported(u),
        }
    }

    fn changed_symbols(&self, changed_paths: &[String]) -> Answer<Vec<SymbolPath>> {
        self.changed_symbols_impl(changed_paths)
    }

    fn file_digest(&self, path: &str) -> Answer<Option<trellis_core::ids::ContentHash>> {
        self.file_digest_impl(path)
    }

    fn references(&self, _symbol: &SymbolPath) -> Answer<Vec<SymbolPath>> {
        Answer::Unsupported(Unsupported::RequiresSemanticResolution)
    }

    fn callers(&self, _symbol: &SymbolPath) -> Answer<Vec<SymbolPath>> {
        Answer::Unsupported(Unsupported::RequiresSemanticResolution)
    }

    fn implementations(&self, _symbol: &SymbolPath) -> Answer<Vec<SymbolPath>> {
        Answer::Unsupported(Unsupported::RequiresSemanticResolution)
    }

    fn subclasses(&self, _symbol: &SymbolPath) -> Answer<Vec<SymbolPath>> {
        Answer::Unsupported(Unsupported::RequiresSemanticResolution)
    }
}
