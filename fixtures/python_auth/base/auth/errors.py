"""Authentication error types."""


class AuthError(Exception):
    """Base class for authentication failures."""


class InvalidToken(AuthError):
    """A token failed signature or expiry validation."""


class InvalidCredentials(AuthError):
    """A login attempt presented bad credentials."""
