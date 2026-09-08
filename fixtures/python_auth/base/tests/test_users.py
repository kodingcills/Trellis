"""Registration and login flow through the user service."""

import unittest

from auth.password import hash_password
from auth.providers.password_provider import PasswordAuthProvider
from auth.service import AuthService
from users.repository import UserRepository
from users.service import UserService


def make_service() -> UserService:
    repo = UserRepository()
    provider = PasswordAuthProvider({"u1": hash_password("pw")})
    return UserService(repo, AuthService(provider))


class UserServiceTests(unittest.TestCase):
    def test_register_then_login(self) -> None:
        users = make_service()
        users.register("u1", "u1@example.io", "ignored-at-registration")
        users._password_hashes["u1"] = hash_password("pw")
        token = users.login("u1@example.io", "pw")
        self.assertIsInstance(token, str)

    def test_duplicate_email_rejected(self) -> None:
        users = make_service()
        users.register("u1", "u1@example.io", "x")
        with self.assertRaises(ValueError):
            users.register("u2", "u1@example.io", "y")

    def test_unknown_email_login_fails(self) -> None:
        users = make_service()
        with self.assertRaises(LookupError):
            users.login("nobody@example.io", "pw")


if __name__ == "__main__":
    unittest.main()
