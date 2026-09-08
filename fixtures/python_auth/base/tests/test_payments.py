"""Gateway and ledger behavior."""

import unittest

from payments.gateway import FakeGateway
from payments.ledger import Ledger


class PaymentTests(unittest.TestCase):
    def test_fake_gateway_charges(self) -> None:
        gateway = FakeGateway()
        first = gateway.charge(500, "usd")
        second = gateway.charge(300, "usd")
        self.assertTrue(first.startswith("fake-"))
        self.assertNotEqual(first, second)
        self.assertTrue(gateway.refund(first, 100))
        self.assertFalse(gateway.refund("unknown", 100))

    def test_ledger_balance(self) -> None:
        ledger = Ledger()
        ledger.settle("fake-0001", 500)
        ledger.settle("fake-0002", 300)
        self.assertEqual(ledger.balance_cents(), 800)


if __name__ == "__main__":
    unittest.main()
