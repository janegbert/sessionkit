pub fn complete(order_id: u64, total: u64) -> u64 {
    crate::billing::persistence::InvoiceRepository::insert(order_id, total)
}
