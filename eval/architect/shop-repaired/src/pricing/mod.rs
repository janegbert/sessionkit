use crate::catalog::Product;
pub fn price_of(id: u64) -> u64 { id * 100 }
pub fn price_product(p: &Product) -> u64 { price_of(p.id) }
