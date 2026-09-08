"""High-level authentication service."""

from .interfaces import AuthProvider
from .session import open_session, resume_session


class AuthService:
    """Facade combining a credential provider with stateless sessions."""

    def __init__(self, provider: AuthProvider) -> None:
        self._provider = provider

    def login(self, credentials: dict[str, str]) -> tuple[str, object]:
        """Authenticate credentials and open a stateless session."""
        user_id = self._provider.authenticate(credentials)
        return open_session(user_id)

    def resume(self, token: str) -> object:
        """Validate a token and return the resumed session."""
        return resume_session(token)

    def verify(self, code: str) -> bool:
        """Secondary verification pass delegated to the provider."""
        return self._provider.validate(code)
