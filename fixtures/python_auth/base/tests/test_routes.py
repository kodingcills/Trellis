"""End-to-end register/login through the route handlers."""

import unittest

from api.routes import post_login, post_register
from auth.password import hash_password
from auth.providers.password_provider import PasswordAuthProvider
from auth.service import AuthService
from users.repository import UserRepository
from users.service import UserService


class RoutesTests(unittest.TestCase):
    def test_register_then_login(self) -> None:
        repo = UserRepository()
        provider = PasswordAuthProvider({"u1": hash_password("pw")})
        users = UserService(repo, AuthService(provider))
        registered = post_register(
            users, {"user_id": "u1", "email": "u1@example.io", "password": "ignored"}
        )
        self.assertEqual(registered["status"], 201)
        logged = post_login(users, {"email": "u1@example.io", "password": "pw"})
        self.assertEqual(logged["status"], 200)
        self.assertIn("token", logged["body"])

    def test_bad_login_is_401(self) -> None:
        repo = UserRepository()
        provider = PasswordAuthProvider({})
        users = UserService(repo, AuthService(provider))
        response = post_login(users, {"email": "nobody@example.io", "password": "pw"})
        self.assertEqual(response["status"], 401)


if __name__ == "__main__":
    unittest.main()
