"""Password-based authentication provider."""

from ..errors import InvalidCredentials
from ..interfaces import AuthProvider
from ..password import verify_password


class PasswordAuthProvider(AuthProvider):
    """Verifies username/password credentials against stored hashes."""

    def __init__(self, stored_hashes: dict[str, str]) -> None:
        self._stored = stored_hashes

    def authenticate(self, credentials: dict[str, str]) -> str:
        user_id = credentials.get("user_id", "")
        presented = credentials.get("password", "")
        stored = self._stored.get(user_id)
        if stored is None or not verify_password(presented, stored):
            raise InvalidCredentials(user_id)
        return user_id

    def validate(self, code: str) -> bool:
        return bool(code) and code.isdigit()
