"""Provider selection behavior (validate pass through AuthService)."""

import unittest

from auth.providers.oidc_provider import OidcAuthProvider
from auth.providers.password_provider import PasswordAuthProvider
from auth.service import AuthService


class ProviderTests(unittest.TestCase):
    def test_password_provider_validate(self) -> None:
        self.assertTrue(AuthService(PasswordAuthProvider({})).verify("1234"))
        self.assertFalse(AuthService(PasswordAuthProvider({})).verify("nope"))

    def test_oidc_provider_validate(self) -> None:
        oidc = AuthService(OidcAuthProvider("u"))
        self.assertTrue(oidc.verify("123456"))
        self.assertFalse(oidc.verify("12345"))


if __name__ == "__main__":
    unittest.main()
