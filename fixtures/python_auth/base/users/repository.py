"""In-memory user repository."""

from users.models import User


class UserRepository:
    """Process-local store, keyed by user id and by email."""

    def __init__(self) -> None:
        self._by_id: dict[str, User] = {}
        self._by_email: dict[str, User] = {}

    def save(self, user: User) -> None:
        self._by_id[user.user_id] = user
        self._by_email[user.email] = user

    def get(self, user_id: str) -> User | None:
        return self._by_id.get(user_id)

    def find_by_email(self, email: str) -> User | None:
        return self._by_email.get(email)
