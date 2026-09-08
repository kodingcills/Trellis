"""Order pricing rules."""

TAX_RATE = 0.0
PROMO_CODE = "NONE"


def compute_total(subtotal_cents: int, tax_rate: float | None = None) -> int:
    """Return the total in cents, tax applied on top of the subtotal."""
    rate = TAX_RATE if tax_rate is None else tax_rate
    return subtotal_cents + round(subtotal_cents * rate)
