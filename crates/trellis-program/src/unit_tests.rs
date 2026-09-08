//! Unit tests for the normalized model and backend-independent contracts.

use crate::prelude::*;

#[test]
fn module_identity_derives_from_canonical_paths() {
    assert_eq!(
        ModuleId::from_path("auth/tokens.py").unwrap().name(),
        "auth.tokens"
    );
    assert_eq!(
        ModuleId::from_path("auth/providers/password_provider.py")
            .unwrap()
            .name(),
        "auth.providers.password_provider"
    );
    // __init__.py maps to its package.
    assert_eq!(
        ModuleId::from_path("users/__init__.py").unwrap().name(),
        "users"
    );
    // Non-Python or non-canonical paths are rejected.
    assert!(ModuleId::from_path("README.md").is_none());
    assert!(ModuleId::from_path("/abs/auth.py").is_none());
    assert!(ModuleId::from_path("a/../b.py").is_none());
}

#[test]
fn symbol_paths_are_canonical_or_rejected() {
    assert!(SymbolPath::new("auth.tokens.refresh_token").is_some());
    assert!(SymbolPath::new("").is_none());
    assert!(SymbolPath::new(".leading").is_none());
    assert!(SymbolPath::new("trailing.").is_none());
    assert!(SymbolPath::new("a..b").is_none());
    let cls = SymbolPath::new("auth.service.AuthService").unwrap();
    assert_eq!(
        cls.child("login").unwrap().as_str(),
        "auth.service.AuthService.login"
    );
}

#[test]
fn signature_equality_ignores_formatting_not_content() {
    let s1 = Signature {
        is_async: false,
        params: vec![Param {
            name: "code".into(),
            kind: ParamKind::Positional,
            annotation: Some("str".into()),
            default: None,
        }],
        ret: Some("bool".into()),
    };
    let mut s2 = s1.clone();
    assert_eq!(s1, s2);
    // Default changes value.
    s2.params[0].default = Some("\"abc\"".into());
    assert_ne!(s1, s2);
    // Async-ness changes value.
    s2 = s1.clone();
    s2.is_async = true;
    assert_ne!(s1, s2);
    // Return annotation changes value.
    s2 = s1.clone();
    s2.ret = Some("int".into());
    assert_ne!(s1, s2);
}

#[test]
fn signature_canonical_renders_markers() {
    let sig = Signature {
        is_async: false,
        params: vec![
            Param {
                name: "a".into(),
                kind: ParamKind::PositionalOnly,
                annotation: None,
                default: None,
            },
            Param {
                name: "b".into(),
                kind: ParamKind::Positional,
                annotation: None,
                default: None,
            },
            Param {
                name: "kw".into(),
                kind: ParamKind::KeywordOnly,
                annotation: Some("int".into()),
                default: None,
            },
            Param {
                name: "rest".into(),
                kind: ParamKind::VarArgs,
                annotation: None,
                default: None,
            },
            Param {
                name: "opts".into(),
                kind: ParamKind::KwArgs,
                annotation: None,
                default: None,
            },
        ],
        ret: None,
    };
    assert_eq!(sig.canonical(), "(a, /, b, *, kw: int, *rest, **opts)");
    // Keyword-only after *rest needs no extra marker.
    let sig2 = Signature {
        is_async: false,
        params: vec![
            Param {
                name: "rest".into(),
                kind: ParamKind::VarArgs,
                annotation: None,
                default: None,
            },
            Param {
                name: "kw".into(),
                kind: ParamKind::KeywordOnly,
                annotation: None,
                default: None,
            },
        ],
        ret: None,
    };
    assert_eq!(sig2.canonical(), "(*rest, kw)");
}
