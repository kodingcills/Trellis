//! Backend-independent program model (spec §9.3, §20).
//!
//! Trellis owns its normalized representation; tree-sitter (M3) and SCIP
//! (M8) are input backends and never appear in these types. Symbol and
//! module identity are deterministic and derived from canonical relative
//! paths plus Python nesting — never from absolute checkout locations,
//! traversal order, or parser allocation identity.

use std::fmt;

/// Identity of a Python source unit (module), derived from its canonical
/// repo-relative path: directory separators become `.`, `.py` is stripped,
/// and `__init__.py` maps to its package's dotted name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(String);

impl ModuleId {
    /// Derive the module identity from a canonical relative path
    /// (`/`-separated, `.py`-suffixed). Returns `None` for non-Python
    /// paths or non-canonical forms.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let stem = path.strip_suffix(".py")?;
        if path != format!("{stem}.py") || path.starts_with('/') || path.contains("..") {
            return None;
        }
        let dotted = stem.replace('/', ".");
        let module = dotted.strip_suffix(".__init__").unwrap_or(&dotted);
        if module.is_empty() || module.split('.').any(|p| p.is_empty()) {
            return None;
        }
        Some(Self(module.to_string()))
    }

    /// The canonical dotted module name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of a program symbol: `module` + dotted nesting path, e.g.
/// `auth.tokens.refresh_token` or `auth.service.AuthService.login`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolPath(String);

impl SymbolPath {
    /// Construct from a canonical dotted path. Returns `None` for empty or
    /// malformed paths.
    #[must_use]
    pub fn new(path: &str) -> Option<Self> {
        if path.is_empty() || path.starts_with('.') || path.ends_with('.') {
            return None;
        }
        if path.split('.').any(|p| p.is_empty()) {
            return None;
        }
        Some(Self(path.to_string()))
    }

    /// Construct a child symbol path (e.g. a method of a class).
    #[must_use]
    pub fn child(&self, name: &str) -> Option<Self> {
        Self::new(&format!("{}.{}", self.0, name))
    }

    /// The canonical dotted path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SymbolPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Syntactic declaration kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    /// A module-level unit (implicit declaration of the source unit).
    Module,
    /// A class declaration.
    Class,
    /// A plain function (module-level or nested).
    Function,
    /// An `async def` function.
    AsyncFunction,
    /// A function declared directly in a class body.
    Method,
    /// An `async def` declared directly in a class body.
    AsyncMethod,
}

impl DefKind {
    /// Whether the kind denotes an async declaration.
    #[must_use]
    pub const fn is_async(self) -> bool {
        matches!(self, DefKind::AsyncFunction | DefKind::AsyncMethod)
    }
}

/// A source span: 0-based line/column offsets, deterministic, path-relative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceSpan {
    /// 0-based start line.
    pub start_line: u32,
    /// 0-based start column (bytes).
    pub start_column: u32,
    /// 0-based end line (exclusive of the last row's content semantics:
    /// tree-sitter end position).
    pub end_line: u32,
    /// 0-based end column (bytes).
    pub end_column: u32,
}

/// Parse status of a source unit. Parse **success is syntactic evidence
/// only** — never proof of semantic resolvability (spec §8, §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseStatus {
    /// The unit parsed without syntax errors.
    Parsed,
    /// The unit contains syntax errors (tree-sitter ERROR nodes or missing
    /// children). Definitions extracted from such units may be incomplete;
    /// the failure is always explicit, never silently omitted.
    Failed {
        /// Number of ERROR nodes encountered.
        error_count: u32,
        /// First error position, if located.
        first_error: Option<SourceSpan>,
    },
}

impl ParseStatus {
    /// Whether the unit parsed cleanly.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, ParseStatus::Parsed)
    }
}

/// One syntactic definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// Canonical symbol path of the definition.
    pub symbol: SymbolPath,
    /// Syntactic declaration kind.
    pub kind: DefKind,
    /// Source span of the declaration.
    pub span: SourceSpan,
}

/// One normalized parameter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Param {
    /// Parameter name (no star/prefix decorations).
    pub name: String,
    /// Parameter kind (positional, positional-only, keyword-only, vararg, kwarg).
    pub kind: ParamKind,
    /// Whitespace-normalized annotation source, if annotated.
    pub annotation: Option<String>,
    /// Whitespace-normalized default-value source, if present.
    ///
    /// Defaults participate in normalized signature equality (they are
    /// deterministic source content and the frozen oracle labels render
    /// them); comments are stripped and whitespace collapsed.
    pub default: Option<String>,
}

/// Syntactic parameter kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamKind {
    /// Positional-or-keyword.
    Positional,
    /// Before a `/` marker.
    PositionalOnly,
    /// After a `*` (bare or `*args`) marker.
    KeywordOnly,
    /// `*args`.
    VarArgs,
    /// `**kwargs`.
    KwArgs,
}

/// A normalized signature. Equality covers parameter names/kinds/order,
/// annotations, defaults, return annotation, and async-ness. It excludes
/// decorators, docstrings, body, module qualification, and formatting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Signature {
    /// Whether declared `async def`.
    pub is_async: bool,
    /// Parameters in declaration order.
    pub params: Vec<Param>,
    /// Whitespace-normalized return annotation, if present.
    pub ret: Option<String>,
}

impl Signature {
    /// Canonical rendering, e.g. `(self, code: str) -> bool`. Rendered
    /// text is whitespace-normalized; two signatures are equal iff their
    /// canonical renderings are equal.
    #[must_use]
    pub fn canonical(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut pos_only_open = self
            .params
            .iter()
            .any(|p| p.kind == ParamKind::PositionalOnly);
        let mut kw_only_open = false;
        for p in &self.params {
            // Insert "/" before the first non-positional-only param.
            if pos_only_open && p.kind != ParamKind::PositionalOnly {
                parts.push("/".to_string());
                pos_only_open = false;
            }
            if p.kind == ParamKind::KeywordOnly && !kw_only_open {
                parts.push("*".to_string());
                kw_only_open = true;
            }
            let prefix = match p.kind {
                ParamKind::VarArgs => {
                    // A bare or starred * opens keyword-only context.
                    kw_only_open = true;
                    "*"
                }
                ParamKind::KwArgs => "**",
                _ => "",
            };
            let mut s = format!("{prefix}{}", p.name);
            if let Some(a) = &p.annotation {
                s.push_str(": ");
                s.push_str(a);
            }
            if let Some(d) = &p.default {
                s.push_str(" = ");
                s.push_str(d);
            }
            parts.push(s);
        }
        if pos_only_open {
            parts.push("/".to_string());
        }
        let mut out = format!("({})", parts.join(", "));
        if let Some(r) = &self.ret {
            out.push_str(" -> ");
            out.push_str(r);
        }
        out
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// One syntactic import statement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Import {
    /// The module or symbol path as written (dots for module components).
    pub target: String,
    /// The local binding name (alias if present, else the imported name /
    /// module root).
    pub binding: String,
    /// Whether this is a `from X import y` (true) or `import X` (false).
    pub from_import: bool,
    /// 1-based line number of the import statement.
    pub line: u32,
}
