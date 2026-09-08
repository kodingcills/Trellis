"""OIDC-style provider: verifies pre-exchanged identity-token claims."""

from ..errors import InvalidCredentials
from ..interfaces import AuthProvider
from ..tokens import verify_token


class OidcAuthProvider(AuthProvider):
    """Authenticates users by verifying an upstream identity token."""

    def __init__(self, allowed_subject_prefix: str) -> None:
        self._prefix = allowed_subject_prefix

    def authenticate(self, credentials: dict[str, str]) -> str:
        token = credentials.get("id_token", "")
        try:
            user_id = verify_token(token)
        except Exception as exc:  # boundary conversion to the auth error type
            raise InvalidCredentials("oidc") from exc
        if not user_id.startswith(self._prefix):
            raise InvalidCredentials("oidc-subject")
        return user_id

    def validate(self, code: str) -> bool:
        return len(code) == 6 and code.isdigit()
