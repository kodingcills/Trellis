"""User registration and login use-cases."""

from auth.errors import AuthError
from auth.password import hash_password
from auth.service import AuthService
from users.models import Status, User
from users.repository import UserRepository


class UserService:
    """Registration and login orchestration for local accounts."""

    def __init__(self, users: UserRepository, auth: AuthService) -> None:
        self._users = users
        self._auth = auth
        self._password_hashes: dict[str, str] = {}

    def password_hashes(self) -> dict[str, str]:
        """Stored password hashes, suitable for wiring an auth provider."""
        return dict(self._password_hashes)

    def register(self, user_id: str, email: str, password: str) -> User:
        """Register a local account with a hashed password."""
        if self._users.find_by_email(email) is not None:
            raise ValueError("email already registered")
        user = User(user_id=user_id, email=email, status=Status.ACTIVE)
        self._users.save(user)
        self._password_hashes[user_id] = hash_password(password)
        return user

    def login(self, email: str, password: str) -> str:
        """Return a session token for valid credentials."""
        user = self._users.find_by_email(email)
        if user is None or not user.is_active():
            raise LookupError("no active user for email")
        try:
            token, _session = self._auth.login(
                {"user_id": user.user_id, "password": password}
            )
        except AuthError:
            user.failed_logins += 1
            raise
        return token
