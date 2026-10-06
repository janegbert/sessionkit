use crate::pricing::price_of;
pub struct Product { pub id: u64, pub name: String }
pub fn label(p: &Product) -> String { format!("{} ({})", p.name, price_of(p.id)) }
