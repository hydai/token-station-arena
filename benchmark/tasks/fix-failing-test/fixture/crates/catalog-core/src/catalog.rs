use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub price_cents: u32,
    pub popularity: u32,
}

pub fn sample_products() -> Vec<Product> {
    vec![
        Product {
            id: String::from("starter-pack"),
            name: String::from("Starter Pack"),
            price_cents: 1_500,
            popularity: 62,
        },
        Product {
            id: String::from("pro-compiler"),
            name: String::from("Pro Compiler"),
            price_cents: 12_000,
            popularity: 98,
        },
        Product {
            id: String::from("runtime-tracer"),
            name: String::from("Runtime Tracer"),
            price_cents: 7_500,
            popularity: 84,
        },
    ]
}
