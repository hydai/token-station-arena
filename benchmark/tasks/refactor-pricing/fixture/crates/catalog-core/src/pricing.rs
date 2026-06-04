pub fn discounted_price_cents(price_cents: u32, discount_percent: u8) -> u32 {
    let discount = price_cents * u32::from(discount_percent.min(100)) / 100;
    price_cents.saturating_sub(discount)
}

pub fn invoice_total_cents(subtotal_cents: u32, discount_percent: u8, tax_percent: u8) -> u32 {
    let discount = subtotal_cents * u32::from(discount_percent.min(100)) / 100;
    let discounted = subtotal_cents.saturating_sub(discount);
    discounted + (discounted * u32::from(tax_percent) / 100)
}

#[cfg(test)]
mod tests {
    use super::{discounted_price_cents, invoice_total_cents};

    #[test]
    fn applies_discount_to_single_price() {
        assert_eq!(discounted_price_cents(1_000, 25), 750);
    }

    #[test]
    fn caps_discount_at_full_price() {
        assert_eq!(discounted_price_cents(1_000, 250), 0);
    }

    #[test]
    fn applies_invoice_discount_before_tax() {
        assert_eq!(invoice_total_cents(10_000, 10, 8), 9_720);
    }
}
