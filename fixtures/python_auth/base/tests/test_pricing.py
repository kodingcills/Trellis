"""Pricing behavior."""

import unittest

from payments import pricing


class PricingTests(unittest.TestCase):
    def test_explicit_tax_rate(self) -> None:
        self.assertEqual(pricing.compute_total(1000, tax_rate=0.1), 1100)

    def test_default_rate_applies(self) -> None:
        self.assertEqual(
            pricing.compute_total(1000),
            1000 + round(1000 * pricing.TAX_RATE),
        )


if __name__ == "__main__":
    unittest.main()
