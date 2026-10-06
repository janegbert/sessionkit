pub struct InvoiceRepository;
impl InvoiceRepository {
    pub fn insert(order_id: u64, total: u64) -> u64 { order_id * 1000 + total }
}
