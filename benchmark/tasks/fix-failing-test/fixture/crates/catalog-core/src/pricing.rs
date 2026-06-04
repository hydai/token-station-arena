pub fn discounted_price_cents(price_cents: u32, discount_percent: u8) -> u32 {
    if discount_percent >= 100 {
        return 0;
    }

    let discount = price_cents * u32::from(discount_percent / 100);
    price_cents.saturating_sub(discount)
}

pub fn final_price_cents(price_cents: u32, discount_percent: u8, tax_percent: u8) -> u32 {
    let discounted = discounted_price_cents(price_cents, discount_percent);
    discounted + (discounted * u32::from(tax_percent) / 100)
}

#[cfg(test)]
mod tests {
    use super::{discounted_price_cents, final_price_cents};

    #[test]
    fn applies_partial_discount() {
        assert_eq!(discounted_price_cents(1_000, 25), 750);
    }

    #[test]
    fn caps_full_discount_at_zero() {
        assert_eq!(discounted_price_cents(1_000, 100), 0);
    }

    #[test]
    fn applies_tax_after_discount() {
        assert_eq!(final_price_cents(2_000, 10, 5), 1_890);
    }
}
