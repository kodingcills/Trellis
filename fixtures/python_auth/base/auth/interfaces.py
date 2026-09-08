"""Provider interfaces for credential checking."""

from abc import ABC, abstractmethod


class AuthProvider(ABC):
    """A strategy for verifying presented credentials."""

    @abstractmethod
    def authenticate(self, credentials: dict[str, str]) -> str:
        """Return the user id on success; raise InvalidCredentials on failure."""

    @abstractmethod
    def validate(self, code: str) -> bool:
        """Validate a secondary verification code (e.g. an OTP)."""
