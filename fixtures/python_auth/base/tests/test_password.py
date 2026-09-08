"""Password hashing behavior."""

import unittest

from auth.password import hash_password, verify_password


class PasswordTests(unittest.TestCase):
    def test_hash_then_verify(self) -> None:
        stored = hash_password("hunter2")
        self.assertTrue(verify_password("hunter2", stored))
        self.assertFalse(verify_password("hunter3", stored))

    def test_salts_differ(self) -> None:
        self.assertNotEqual(hash_password("pw"), hash_password("pw"))


if __name__ == "__main__":
    unittest.main()
