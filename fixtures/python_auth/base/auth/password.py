"""Password hashing helpers (standard library only)."""

import hashlib
import hmac
import secrets


def _digest(password: str, salt: bytes) -> str:
    return hashlib.pbkdf2_hmac("sha256", password.encode(), salt, 100_000).hex()


def hash_password(password: str, salt: bytes | None = None) -> str:
    """Return a ``salt$hash`` string for the given password."""
    salt = salt or secrets.token_bytes(16)
    return f"{salt.hex()}${_digest(password, salt)}"


def verify_password(password: str, stored: str) -> bool:
    """Constant-time verification of a stored ``salt$hash`` string."""
    salt_hex, expected = stored.split("$", 1)
    return hmac.compare_digest(_digest(password, bytes.fromhex(salt_hex)), expected)
