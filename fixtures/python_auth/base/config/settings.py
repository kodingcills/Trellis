"""Service configuration: declared inputs with safe fixture defaults.

The env lookups use explicitly named variables (deny-by-default in spirit:
anything not declared here is never consulted) and always carry fixture
defaults so the suite runs without external setup.
"""

import os


class Settings:
    """Process configuration; values are read from declared variables."""

    _TOKEN_SECRET = "fixture-only-secret"  # never a real credential
    _TOKEN_TTL = 3600
    _SESSION_TTL = 86_400

    @staticmethod
    def token_secret() -> str:
        return os.environ.get("ACME_TOKEN_SECRET", Settings._TOKEN_SECRET)

    @staticmethod
    def token_ttl_seconds() -> int:
        return int(os.environ.get("ACME_TOKEN_TTL", Settings._TOKEN_TTL))

    @staticmethod
    def session_ttl_seconds() -> int:
        return int(os.environ.get("ACME_SESSION_TTL", Settings._SESSION_TTL))
